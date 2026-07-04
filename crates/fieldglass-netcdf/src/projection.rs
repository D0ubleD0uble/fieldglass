//! Resolving a *projected* NetCDF grid into the projection parameters the warp
//! needs (decision 0004, issue #168).
//!
//! Decision 0002 renders only regular 1-D lat/lon grids, synthesising a
//! `"latlon"` geometry from the coordinate arrays. Two 0.2.0 corpus models —
//! **WRF `wrfout`** and **GOES** ABI imagery — are instead *regular grids in a
//! projected CRS* (Model A): they ride the existing analytic-inverse warp, but
//! their projection lives in metadata the lat/lon path never reads.
//!
//! Two metadata paths, both terminating in a parameter struct this module
//! returns and the napi layer maps onto a `MessageMeta`:
//!
//! 1. **CF `grid_mapping`** ([`resolve_cf_geostationary`]) — the standard path.
//!    A data variable names a `grid_mapping` variable whose `grid_mapping_name`
//!    + parameters define the CRS. GOES uses `grid_mapping_name =
//!    "geostationary"` with 1-D `x`/`y` radian scan-angle coordinate variables.
//! 2. **WRF global attributes** ([`resolve_wrf_lambert`],
//!    [`resolve_wrf_polar_stereo`], [`resolve_wrf_mercator`]) — WRF output is
//!    not CF-compliant; `MAP_PROJ` + `TRUELAT1/2` / `STAND_LON` /
//!    `MOAD_CEN_LAT` / `DX` / `DY` sit at the file level, with the grid corners
//!    taken from the `XLAT`/`XLONG` arrays.
//!
//! Both are intentionally pure (attribute slices + decoded coordinate arrays in,
//! parameters out) so they unit-test without a reader or the napi boundary. An
//! unrecognised mapping or a missing required parameter returns `None`, leaving
//! the caller to fall back to the regular lat/lon path or source projection.

/// Look up an attribute's display value by name.
fn attr<'a>(attrs: &'a [(String, String)], name: &str) -> Option<&'a str> {
    attrs
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, v)| v.as_str())
}

/// Look up a scalar numeric attribute. NetCDF attribute values reach this layer
/// as display strings (a comma-separated list for arrays); take the first token,
/// which is the scalar these projection parameters always are.
fn attr_f64(attrs: &[(String, String)], name: &str) -> Option<f64> {
    let raw = attr(attrs, name)?;
    raw.split(',').next()?.trim().parse::<f64>().ok()
}

/// First value and the uniform step of a coordinate axis, as `(first, step)`.
/// The step is `(last - first) / (n - 1)`, so the scan direction is baked into
/// its sign exactly like the planar grids' metre spacings. `None` for an axis
/// with fewer than two points (no spacing to interpolate).
fn axis_first_step(coord: &[f64]) -> Option<(f64, f64)> {
    if coord.len() < 2 {
        return None;
    }
    let first = *coord.first()?;
    let last = *coord.last()?;
    Some((first, (last - first) / (coord.len() as f64 - 1.0)))
}

/// A geostationary (space-view) grid resolved from a CF `geostationary`
/// `grid_mapping` plus its 1-D `x`/`y` scan-angle coordinate variables. Mirrors
/// `fieldglass_core::GeostationaryParams` (which the napi warp reconstructs),
/// minus the dependence on that crate so this stays a leaf module.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GeostationaryGrid {
    pub ni: u32,
    pub nj: u32,
    /// Earth-centre → satellite distance, metres (`perspective_point_height`
    /// is height above the surface, so `+ semi_major_axis`).
    pub h_metres: f64,
    pub r_eq: f64,
    pub r_pol: f64,
    pub sub_lon_deg: f64,
    pub sweep_x: bool,
    pub x0: f64,
    pub dx_rad: f64,
    pub y0: f64,
    pub dy_rad: f64,
}

/// Resolve a CF `geostationary` grid mapping. `gm_attrs` are the attributes of
/// the `grid_mapping` variable a data variable points at; `x` / `y` are its
/// decoded scan-angle coordinate arrays **in radians** (the caller applies any
/// CF `scale_factor` / `add_offset` first — real GOES stores them as scaled
/// `int16`). Returns `None` when the mapping is not geostationary or a required
/// parameter is missing.
pub fn resolve_cf_geostationary(
    gm_attrs: &[(String, String)],
    x: &[f64],
    y: &[f64],
) -> Option<GeostationaryGrid> {
    if attr(gm_attrs, "grid_mapping_name")?.trim() != "geostationary" {
        return None;
    }
    let pph = attr_f64(gm_attrs, "perspective_point_height")?;
    let r_eq = attr_f64(gm_attrs, "semi_major_axis")?;
    let r_pol = attr_f64(gm_attrs, "semi_minor_axis")?;
    let sub_lon_deg = attr_f64(gm_attrs, "longitude_of_projection_origin")?;
    // CF's sweep_angle_axis is "x" for GOES-R, "y" for Meteosat; default to the
    // GOES-R convention when absent (the only geostationary corpus model).
    let sweep_x = attr(gm_attrs, "sweep_angle_axis")
        .map(|s| s.trim() != "y")
        .unwrap_or(true);
    let (x0, dx_rad) = axis_first_step(x)?;
    let (y0, dy_rad) = axis_first_step(y)?;
    Some(GeostationaryGrid {
        ni: x.len() as u32,
        nj: y.len() as u32,
        h_metres: pph + r_eq,
        r_eq,
        r_pol,
        sub_lon_deg,
        sweep_x,
        x0,
        dx_rad,
        y0,
        dy_rad,
    })
}

/// A Lambert Conformal grid resolved from WRF global attributes. Mirrors the
/// `lambert_*` + corner fields of a `MessageMeta`; the napi layer copies these
/// straight across and reuses the existing Lambert projector.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WrfLambertGrid {
    pub ni: u32,
    pub nj: u32,
    /// Geographic coordinates of the first scanned point (`XLAT`/`XLONG` at
    /// `south_north = west_east = 0`) — the Lambert grid origin.
    pub lat_first: f64,
    pub lon_first: f64,
    pub lad: f64,
    pub lov: f64,
    pub dx_metres: f64,
    pub dy_metres: f64,
    pub latin1: f64,
    pub latin2: f64,
}

/// WRF's `MAP_PROJ` code for Lambert Conformal Conic.
const WRF_MAP_PROJ_LAMBERT: f64 = 1.0;
/// WRF's `MAP_PROJ` code for polar stereographic.
const WRF_MAP_PROJ_POLAR_STEREO: f64 = 2.0;
/// WRF's `MAP_PROJ` code for Mercator.
pub const WRF_MAP_PROJ_MERCATOR: f64 = 3.0;

/// The file's WRF `MAP_PROJ` code, when the global attribute is present and
/// numeric. Lets the caller decide which grid corners it must read before
/// invoking a resolver (Mercator is the only one that needs the far corner).
pub fn wrf_map_proj(global: &[(String, String)]) -> Option<f64> {
    attr_f64(global, "MAP_PROJ")
}

/// Resolve a WRF Lambert grid from the file's global attributes plus the grid
/// origin read from the `XLAT`/`XLONG` corner. Returns `None` unless
/// `MAP_PROJ == 1` (Lambert) and the standard parallels / orientation / spacing
/// attributes are all present. The polar stereographic and Mercator variants
/// have their own resolvers below; `MAP_PROJ == 6` (lat-lon, possibly rotated)
/// stays unresolved and falls back to source projection.
pub fn resolve_wrf_lambert(
    global: &[(String, String)],
    lat_first: f64,
    lon_first: f64,
    ni: u32,
    nj: u32,
) -> Option<WrfLambertGrid> {
    if attr_f64(global, "MAP_PROJ")? != WRF_MAP_PROJ_LAMBERT {
        return None;
    }
    let latin1 = attr_f64(global, "TRUELAT1")?;
    // A one-parallel (tangent-cone) Lambert omits TRUELAT2; fall back to TRUELAT1.
    let latin2 = attr_f64(global, "TRUELAT2").unwrap_or(latin1);
    let lov = attr_f64(global, "STAND_LON")?;
    // The latitude of true scale only enters `rho0`, which cancels in the
    // origin-relative inverse the projector uses; MOAD_CEN_LAT is the natural
    // choice, with TRUELAT1 as a fallback.
    let lad = attr_f64(global, "MOAD_CEN_LAT").unwrap_or(latin1);
    let dx_metres = attr_f64(global, "DX")?;
    let dy_metres = attr_f64(global, "DY")?;
    Some(WrfLambertGrid {
        ni,
        nj,
        lat_first,
        lon_first,
        lad,
        lov,
        dx_metres,
        dy_metres,
        latin1,
        latin2,
    })
}

/// A polar stereographic grid resolved from WRF global attributes. Mirrors the
/// `polar_stereo_*` + corner fields of a `MessageMeta`; the napi layer copies
/// these straight across and reuses the existing polar stereographic projector.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WrfPolarStereoGrid {
    pub ni: u32,
    pub nj: u32,
    /// Geographic coordinates of the first scanned point (`XLAT`/`XLONG` at
    /// `south_north = west_east = 0`) — the grid origin.
    pub lat_first: f64,
    pub lon_first: f64,
    /// Latitude of true scale — WRF specifies `DX`/`DY` at `TRUELAT1`.
    pub lad: f64,
    /// Orientation longitude (meridian parallel to the y-axis) — `STAND_LON`.
    pub lov: f64,
    pub dx_metres: f64,
    pub dy_metres: f64,
    /// `true` ⇒ south-pole projection. WRF has no projection-centre flag; the
    /// hemisphere follows the sign of `TRUELAT1` (the WPS convention).
    pub south_pole: bool,
}

/// Resolve a WRF polar stereographic grid from the file's global attributes
/// plus the grid origin read from the `XLAT`/`XLONG` corner. Returns `None`
/// unless `MAP_PROJ == 2` and the true-scale / orientation / spacing attributes
/// are all present. WRF's spherical polar stereographic is algebraically the
/// Snyder form the core projector implements, with the pole scale factor
/// `k₀ = (1 + sin|TRUELAT1|)/2`, so `lad = TRUELAT1` is exact.
pub fn resolve_wrf_polar_stereo(
    global: &[(String, String)],
    lat_first: f64,
    lon_first: f64,
    ni: u32,
    nj: u32,
) -> Option<WrfPolarStereoGrid> {
    if attr_f64(global, "MAP_PROJ")? != WRF_MAP_PROJ_POLAR_STEREO {
        return None;
    }
    let lad = attr_f64(global, "TRUELAT1")?;
    let lov = attr_f64(global, "STAND_LON")?;
    let dx_metres = attr_f64(global, "DX")?;
    let dy_metres = attr_f64(global, "DY")?;
    Some(WrfPolarStereoGrid {
        ni,
        nj,
        lat_first,
        lon_first,
        lad,
        lov,
        dx_metres,
        dy_metres,
        south_pole: lad < 0.0,
    })
}

/// A Mercator grid resolved from WRF global attributes. Like the GRIB Mercator
/// grids, it is fully pinned by its corner coordinates: a WRF Mercator domain
/// walks uniform projected metres, which is uniform in longitude along i and in
/// the Mercator ordinate along j — exactly the core `MercatorParams` contract.
/// `DX`/`DY`/`TRUELAT1` therefore never enter the geolocation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WrfMercatorGrid {
    pub ni: u32,
    pub nj: u32,
    /// Geographic coordinates of the first scanned point (`XLAT`/`XLONG` at
    /// `south_north = west_east = 0`).
    pub lat_first: f64,
    pub lon_first: f64,
    /// Geographic coordinates of the last scanned point (`XLAT`/`XLONG` at the
    /// far corner of the first time step).
    pub lat_last: f64,
    pub lon_last: f64,
}

/// Resolve a WRF Mercator grid from the file's global attributes plus **both**
/// grid corners read from `XLAT`/`XLONG` (the caller checks
/// [`wrf_map_proj`]` == `[`WRF_MAP_PROJ_MERCATOR`] before paying for the far
/// corner). Returns `None` unless `MAP_PROJ == 3`.
pub fn resolve_wrf_mercator(
    global: &[(String, String)],
    lat_first: f64,
    lon_first: f64,
    lat_last: f64,
    lon_last: f64,
    ni: u32,
    nj: u32,
) -> Option<WrfMercatorGrid> {
    if attr_f64(global, "MAP_PROJ")? != WRF_MAP_PROJ_MERCATOR {
        return None;
    }
    Some(WrfMercatorGrid {
        ni,
        nj,
        lat_first,
        lon_first,
        lat_last,
        lon_last,
    })
}

/// CF `scale_factor` / `add_offset` of a coordinate (or data) variable, as
/// `(scale, offset)`, defaulting to the identity `(1, 0)` when absent. Packed
/// integer coordinates (real GOES stores `x`/`y` as scaled `int16`) decode to
/// physical units via `physical = packed · scale + offset`; see
/// [`apply_scale_offset`].
pub fn cf_scale_offset(attrs: &[(String, String)]) -> (f64, f64) {
    let scale = attr_f64(attrs, "scale_factor").unwrap_or(1.0);
    let offset = attr_f64(attrs, "add_offset").unwrap_or(0.0);
    (scale, offset)
}

/// Apply CF `scale_factor` / `add_offset` to raw decoded values. A no-op when
/// the attributes are absent (`scale = 1`, `offset = 0`). Used for coordinate
/// arrays, which are never masked; data planes go through [`unpack_cf_data`].
pub fn apply_scale_offset(raw: &[f64], attrs: &[(String, String)]) -> Vec<f64> {
    let (scale, offset) = cf_scale_offset(attrs);
    if scale == 1.0 && offset == 0.0 {
        return raw.to_vec();
    }
    raw.iter().map(|v| v * scale + offset).collect()
}

/// CF valid-range bounds of a variable as inclusive **packed-domain** bounds
/// `(min, max)`, either side `None` when unspecified. A *well-formed*
/// two-element `valid_range` takes precedence over the scalar `valid_min` /
/// `valid_max` pair (CF Conventions §2.5.1); a reversed `valid_range` is
/// normalised. A malformed `valid_range` (not two parseable numbers — a file
/// libnetcdf would itself reject) is ignored, falling back to
/// `valid_min`/`valid_max` rather than failing the render. The bounds describe
/// the *stored* (packed) values, so [`unpack_cf_data`] compares them before
/// applying `scale_factor` / `add_offset`.
fn cf_valid_bounds(attrs: &[(String, String)]) -> (Option<f64>, Option<f64>) {
    if let Some(s) = attr(attrs, "valid_range") {
        let mut it = s.split(',').filter_map(|t| t.trim().parse::<f64>().ok());
        if let (Some(a), Some(b)) = (it.next(), it.next()) {
            return (Some(a.min(b)), Some(a.max(b)));
        }
    }
    (attr_f64(attrs, "valid_min"), attr_f64(attrs, "valid_max"))
}

/// Unpack a decoded **data** plane to physical units per the CF conventions:
/// drop values outside `valid_range` / `valid_min` / `valid_max` (compared in
/// packed units, inclusive), then map the survivors through `scale_factor` /
/// `add_offset`. Points already masked at decode (`_FillValue`) stay masked.
///
/// Mirrors libnetcdf's auto mask+scale and how [`apply_scale_offset`] unpacks
/// coordinate arrays, but over `Option<f64>` so missing data is preserved. The
/// common unscaled, unbounded case returns the plane untouched. A `NaN` packed
/// value survives the range test (both comparisons are false for `NaN`) and
/// stays `NaN`, matching libnetcdf, which masks only `_FillValue` /
/// `missing_value`.
pub fn unpack_cf_data(plane: &[Option<f64>], attrs: &[(String, String)]) -> Vec<Option<f64>> {
    let (scale, offset) = cf_scale_offset(attrs);
    let (lo, hi) = cf_valid_bounds(attrs);
    if scale == 1.0 && offset == 0.0 && lo.is_none() && hi.is_none() {
        return plane.to_vec();
    }
    plane
        .iter()
        .map(|&packed| {
            let v = packed?;
            if lo.is_some_and(|lo| v < lo) || hi.is_some_and(|hi| v > hi) {
                return None;
            }
            Some(v * scale + offset)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attrs(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn geostationary_resolves_goes_grid_mapping() {
        let gm = attrs(&[
            ("grid_mapping_name", "geostationary"),
            ("perspective_point_height", "35786023.0"),
            ("semi_major_axis", "6378137.0"),
            ("semi_minor_axis", "6356752.31414"),
            ("longitude_of_projection_origin", "-75.0"),
            ("sweep_angle_axis", "x"),
        ]);
        let x = [-0.02, -0.01, 0.0, 0.01];
        let y = [0.02, 0.01, 0.0, -0.01]; // descends north→south
        let g = resolve_cf_geostationary(&gm, &x, &y).expect("geostationary resolves");
        assert_eq!((g.ni, g.nj), (4, 4));
        assert_eq!(g.sub_lon_deg, -75.0);
        assert!(g.sweep_x);
        // h is centre→satellite: height above surface + equatorial radius.
        assert!((g.h_metres - (35_786_023.0 + 6_378_137.0)).abs() < 1e-6);
        assert!((g.x0 - -0.02).abs() < 1e-12);
        assert!((g.dx_rad - 0.01).abs() < 1e-12);
        assert!(g.dy_rad < 0.0, "y descends, so the step is negative");
    }

    #[test]
    fn geostationary_rejects_non_geostationary_and_missing_params() {
        let x = [0.0, 0.01];
        let y = [0.0, -0.01];
        // Wrong grid_mapping_name.
        let lcc = attrs(&[("grid_mapping_name", "lambert_conformal_conic")]);
        assert!(resolve_cf_geostationary(&lcc, &x, &y).is_none());
        // Right name, missing the ellipsoid.
        let partial = attrs(&[
            ("grid_mapping_name", "geostationary"),
            ("perspective_point_height", "35786023.0"),
            ("longitude_of_projection_origin", "-75.0"),
        ]);
        assert!(resolve_cf_geostationary(&partial, &x, &y).is_none());
    }

    #[test]
    fn geostationary_meteosat_sweep_y() {
        let gm = attrs(&[
            ("grid_mapping_name", "geostationary"),
            ("perspective_point_height", "35785831.0"),
            ("semi_major_axis", "6378169.0"),
            ("semi_minor_axis", "6356583.8"),
            ("longitude_of_projection_origin", "0.0"),
            ("sweep_angle_axis", "y"),
        ]);
        let g = resolve_cf_geostationary(&gm, &[0.0, 0.01], &[0.0, -0.01]).unwrap();
        assert!(!g.sweep_x, "Meteosat sweeps about y");
    }

    #[test]
    fn wrf_resolves_lambert_global_attrs() {
        let global = attrs(&[
            ("MAP_PROJ", "1"),
            ("TRUELAT1", "30.0"),
            ("TRUELAT2", "60.0"),
            ("STAND_LON", "-97.5"),
            ("MOAD_CEN_LAT", "38.5"),
            ("DX", "3000.0"),
            ("DY", "3000.0"),
        ]);
        let g = resolve_wrf_lambert(&global, 32.0, -100.0, 6, 5).expect("wrf lambert resolves");
        assert_eq!((g.ni, g.nj), (6, 5));
        assert_eq!((g.lat_first, g.lon_first), (32.0, -100.0));
        assert_eq!((g.latin1, g.latin2), (30.0, 60.0));
        assert_eq!(g.lov, -97.5);
        assert_eq!(g.lad, 38.5);
        assert_eq!((g.dx_metres, g.dy_metres), (3000.0, 3000.0));
    }

    #[test]
    fn wrf_resolvers_gate_on_their_own_map_proj() {
        // Each resolver only fires for its own MAP_PROJ code, so exactly one
        // resolves for a given file and MAP_PROJ 6 (lat-lon) resolves nothing —
        // it falls back to source projection.
        let polar = attrs(&[
            ("MAP_PROJ", "2"),
            ("TRUELAT1", "60.0"),
            ("STAND_LON", "-100.0"),
            ("DX", "10000.0"),
            ("DY", "10000.0"),
        ]);
        assert!(resolve_wrf_lambert(&polar, 0.0, 0.0, 4, 4).is_none());
        assert!(resolve_wrf_mercator(&polar, 0.0, 0.0, 1.0, 1.0, 4, 4).is_none());
        let latlon = attrs(&[("MAP_PROJ", "6"), ("TRUELAT1", "0.0")]);
        assert!(resolve_wrf_lambert(&latlon, 0.0, 0.0, 4, 4).is_none());
        assert!(resolve_wrf_polar_stereo(&latlon, 0.0, 0.0, 4, 4).is_none());
        assert!(resolve_wrf_mercator(&latlon, 0.0, 0.0, 1.0, 1.0, 4, 4).is_none());
        assert_eq!(wrf_map_proj(&latlon), Some(6.0));
        assert_eq!(wrf_map_proj(&attrs(&[("TITLE", "not wrf")])), None);
    }

    #[test]
    fn wrf_resolves_polar_stereo_global_attrs() {
        let global = attrs(&[
            ("MAP_PROJ", "2"),
            ("TRUELAT1", "60.0"),
            ("STAND_LON", "-100.0"),
            ("DX", "10000.0"),
            ("DY", "10000.0"),
        ]);
        let g = resolve_wrf_polar_stereo(&global, 55.0, -120.0, 6, 5).expect("polar resolves");
        assert_eq!((g.ni, g.nj), (6, 5));
        assert_eq!((g.lat_first, g.lon_first), (55.0, -120.0));
        assert_eq!(g.lad, 60.0, "DX/DY are true at TRUELAT1");
        assert_eq!(g.lov, -100.0);
        assert_eq!((g.dx_metres, g.dy_metres), (10000.0, 10000.0));
        assert!(!g.south_pole, "positive TRUELAT1 = north-pole projection");
    }

    #[test]
    fn wrf_polar_stereo_southern_hemisphere_from_truelat1_sign() {
        let global = attrs(&[
            ("MAP_PROJ", "2"),
            ("TRUELAT1", "-60.0"),
            ("STAND_LON", "170.0"),
            ("DX", "20000.0"),
            ("DY", "20000.0"),
        ]);
        let g = resolve_wrf_polar_stereo(&global, -65.0, 160.0, 4, 4).unwrap();
        assert!(g.south_pole, "negative TRUELAT1 = south-pole projection");
        assert_eq!(g.lad, -60.0);
    }

    #[test]
    fn wrf_polar_stereo_rejects_missing_params() {
        // No STAND_LON: the orientation is unknowable, so nothing resolves.
        let global = attrs(&[
            ("MAP_PROJ", "2"),
            ("TRUELAT1", "60.0"),
            ("DX", "10000.0"),
            ("DY", "10000.0"),
        ]);
        assert!(resolve_wrf_polar_stereo(&global, 0.0, 0.0, 4, 4).is_none());
    }

    #[test]
    fn wrf_resolves_mercator_from_corners_only() {
        // Mercator geolocation is corner-pinned; TRUELAT1/DX/DY are not needed
        // (and their absence must not block the resolve).
        let global = attrs(&[("MAP_PROJ", "3")]);
        let g = resolve_wrf_mercator(&global, 10.0, -60.0, 14.0, -55.0, 6, 5)
            .expect("mercator resolves");
        assert_eq!((g.ni, g.nj), (6, 5));
        assert_eq!((g.lat_first, g.lon_first), (10.0, -60.0));
        assert_eq!((g.lat_last, g.lon_last), (14.0, -55.0));
    }

    #[test]
    fn wrf_one_parallel_falls_back_to_truelat1() {
        let global = attrs(&[
            ("MAP_PROJ", "1"),
            ("TRUELAT1", "25.0"),
            ("STAND_LON", "-100.0"),
            ("DX", "12000.0"),
            ("DY", "12000.0"),
        ]);
        let g = resolve_wrf_lambert(&global, 20.0, -110.0, 3, 3).unwrap();
        assert_eq!(g.latin2, 25.0, "missing TRUELAT2 mirrors TRUELAT1");
        assert_eq!(g.lad, 25.0, "missing MOAD_CEN_LAT mirrors TRUELAT1");
    }

    #[test]
    fn scale_offset_decodes_packed_then_identity_when_absent() {
        let packed = [0.0, 100.0, -50.0];
        let with = attrs(&[("scale_factor", "0.5"), ("add_offset", "10.0")]);
        assert_eq!(apply_scale_offset(&packed, &with), vec![10.0, 60.0, -15.0]);
        let none = attrs(&[("units", "rad")]);
        assert_eq!(apply_scale_offset(&packed, &none), packed.to_vec());
    }

    #[test]
    fn unpack_masks_valid_range_then_scales() {
        // Inclusive packed bounds [0, 10000]; -50 and 15000 fall out, the bounds
        // themselves stay. scale = 1/16 (exact in binary), offset = 250.
        let plane = [
            Some(-50.0),
            Some(0.0),
            Some(10000.0),
            Some(15000.0),
            None, // already masked at decode (_FillValue)
            Some(5000.0),
        ];
        let a = attrs(&[
            ("scale_factor", "0.0625"),
            ("add_offset", "250.0"),
            ("valid_range", "0, 10000"),
        ]);
        assert_eq!(
            unpack_cf_data(&plane, &a),
            vec![None, Some(250.0), Some(875.0), None, None, Some(562.5),]
        );
    }

    #[test]
    fn unpack_valid_min_max_are_one_sided_and_inclusive() {
        let plane = [Some(5.0), Some(10.0), Some(100.0), Some(200.0)];
        // Only valid_min: drops below 10, keeps the bound.
        let lo_only = attrs(&[("valid_min", "10")]);
        assert_eq!(
            unpack_cf_data(&plane, &lo_only),
            vec![None, Some(10.0), Some(100.0), Some(200.0)]
        );
        // Only valid_max: drops above 100, keeps the bound.
        let hi_only = attrs(&[("valid_max", "100")]);
        assert_eq!(
            unpack_cf_data(&plane, &hi_only),
            vec![Some(5.0), Some(10.0), Some(100.0), None]
        );
    }

    #[test]
    fn unpack_valid_range_takes_precedence_over_min_max() {
        let plane = [Some(0.0), Some(50.0), Some(100.0)];
        let a = attrs(&[
            ("valid_range", "40, 60"),
            ("valid_min", "0"),
            ("valid_max", "100"),
        ]);
        assert_eq!(
            unpack_cf_data(&plane, &a),
            vec![None, Some(50.0), None],
            "valid_range [40,60] wins over the wider valid_min/valid_max"
        );
    }

    #[test]
    fn unpack_normalises_reversed_valid_range() {
        let plane = [Some(5.0), Some(50.0), Some(95.0)];
        let a = attrs(&[("valid_range", "90, 10")]); // stored high-then-low
        assert_eq!(
            unpack_cf_data(&plane, &a),
            vec![None, Some(50.0), None],
            "a reversed valid_range still bounds [10, 90]"
        );
    }

    #[test]
    fn unpack_malformed_valid_range_falls_back_to_min_max() {
        let plane = [Some(5.0), Some(50.0), Some(150.0)];
        // A single-element `valid_range` is malformed (libnetcdf rejects it); we
        // ignore it and honour the scalar valid_min/valid_max instead.
        let a = attrs(&[
            ("valid_range", "10"),
            ("valid_min", "10"),
            ("valid_max", "100"),
        ]);
        assert_eq!(
            unpack_cf_data(&plane, &a),
            vec![None, Some(50.0), None],
            "malformed valid_range ignored; valid_min/valid_max [10,100] apply"
        );
        // A non-numeric `valid_range` with no min/max leaves everything present.
        let b = attrs(&[("valid_range", "n/a")]);
        assert_eq!(unpack_cf_data(&plane, &b), plane.to_vec());
    }

    #[test]
    fn unpack_identity_when_no_scale_or_bounds() {
        let plane = [Some(1.0), None, Some(3.0)];
        let none = attrs(&[("units", "K")]);
        assert_eq!(unpack_cf_data(&plane, &none), plane.to_vec());
    }

    #[test]
    fn unpack_scale_only_leaves_all_present() {
        let plane = [Some(0.0), Some(2.0), None];
        let a = attrs(&[("scale_factor", "0.5"), ("add_offset", "1.0")]);
        assert_eq!(unpack_cf_data(&plane, &a), vec![Some(1.0), Some(2.0), None]);
    }
}
