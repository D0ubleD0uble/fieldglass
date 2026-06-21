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
//! 2. **WRF global attributes** ([`resolve_wrf_lambert`]) — WRF output is not
//!    CF-compliant; `MAP_PROJ` + `TRUELAT1/2` / `STAND_LON` / `MOAD_CEN_LAT` /
//!    `DX` / `DY` sit at the file level, with the grid origin taken from the
//!    `XLAT`/`XLONG` corner.
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

/// Resolve a WRF Lambert grid from the file's global attributes plus the grid
/// origin read from the `XLAT`/`XLONG` corner. Returns `None` unless
/// `MAP_PROJ == 1` (Lambert) and the standard parallels / orientation / spacing
/// attributes are all present. The non-Lambert `MAP_PROJ` variants (polar
/// stereo / Mercator / lat-lon) are a cheap follow-up (decision 0004).
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
/// the attributes are absent (`scale = 1`, `offset = 0`).
pub fn apply_scale_offset(raw: &[f64], attrs: &[(String, String)]) -> Vec<f64> {
    let (scale, offset) = cf_scale_offset(attrs);
    if scale == 1.0 && offset == 0.0 {
        return raw.to_vec();
    }
    raw.iter().map(|v| v * scale + offset).collect()
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
    fn wrf_rejects_non_lambert_map_proj() {
        // MAP_PROJ 2 is polar stereographic — out of scope here.
        let global = attrs(&[("MAP_PROJ", "2"), ("TRUELAT1", "60.0")]);
        assert!(resolve_wrf_lambert(&global, 0.0, 0.0, 4, 4).is_none());
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
}
