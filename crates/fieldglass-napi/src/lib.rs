#![deny(clippy::all)]

use fieldglass_core::{
    CombineOp, EqualEarth, Format, GaussianParams, GaussianProjector, GeostationaryParams,
    GeostationaryProjector, LambertParams, LambertProjector, LatLonParams, MercatorParams,
    Mollweide, Orthographic, PlanarGridProjector, PolarStereoParams, PolarStereoProjector,
    PolarStereographic, ProjectedPolylines, Resampling, Robinson, RotatedLatLonParams,
    RotatedLatLonProjector, SourceGrid, SourceOverlayTarget, TargetRaster, WebMercator,
    colormap::{Colormap, ScaleMode, min_max_ignoring_mask, paint_grid_rgba},
    combine_fields,
    contour::{contour_segments, nice_levels},
    detect_from_bytes, eastward_lon_span, latlon_inverse, latlon_point, lon_grid_is_global,
    mercator_inverse, mercator_point, normalise_lon, project_polylines,
    projection::GridIndex,
    rotated_latlon_point,
    warp::{PreparedTarget, TargetProjection, WarpedRaster, warp},
};
use fieldglass_grib1::{
    Grib1Reader,
    tables::{lookup_centre, lookup_parameter},
};
use fieldglass_grib2::{
    Grib2Reader, HorizontalProductCommon, ProductDefinitionSection,
    lookup_centre as lookup_grib2_centre, lookup_discipline, lookup_fixed_surface,
    lookup_parameter as lookup_grib2_parameter, lookup_production_status, lookup_time_range_unit,
};
use fieldglass_netcdf::{
    DatasetView, Hdf5Attribute, Hdf5Metadata, NetcdfBacking, NetcdfReader, RenderableVariable,
    WRF_EARTH_RADIUS_M, WrfLambertGrid, WrfLatLonGrid, WrfMapProj, WrfMercatorGrid,
    WrfPolarStereoGrid, apply_scale_offset, cf_scale_offset, extract_plane,
    resolve_cf_geostationary, resolve_wrf_lambert, resolve_wrf_latlon, resolve_wrf_mercator,
    resolve_wrf_polar_stereo, synthesize_geometry, unpack_cf_data, wrf_map_proj,
};
use napi_derive::napi;
use std::sync::Mutex;

/// A single message's metadata, exposed to Node.js.
#[napi(object)]
pub struct MessageMeta {
    pub message_index: i32,
    pub offset_bytes: i32,
    pub parameter_name: String,
    pub parameter_units: String,
    pub parameter_abbreviation: String,
    pub level: String,
    pub level_type: String,
    pub reference_time: String,
    pub forecast_hours: i32,
    pub forecast_display: String,
    pub originating_centre: String,
    pub grid_type: Option<String>,
    pub grid_ni: Option<i32>,
    pub grid_nj: Option<i32>,
    pub lat_first: Option<f64>,
    pub lon_first: Option<f64>,
    pub lat_last: Option<f64>,
    pub lon_last: Option<f64>,
    pub format: String,
    /// GRIB edition number (1 or 2). Optional so older callers reading
    /// historical fields stay source-compatible.
    pub edition: Option<i32>,
    /// GRIB2 discipline name (WMO Code Table 0.0). `None` for non-GRIB2.
    pub discipline: Option<String>,
    /// Total length of the message in bytes, surfaced for GRIB2 where the
    /// 64-bit length is part of the IS metadata.
    pub total_length_bytes: Option<f64>,
    /// Human-readable production status (WMO Code Table 1.3). `None` for
    /// formats that don't carry the field.
    pub production_status: Option<String>,
    /// Human-readable processed-data type (WMO Code Table 1.4). `None` for
    /// formats that don't carry the field.
    pub data_type: Option<String>,
    // -------------------------------------------------------------------
    // Lambert Conformal projection parameters (only populated for Lambert
    // grids; `None` for every other grid type). The renderer uses these to
    // run an inverse-projection warp from output (lat, lon) → source grid
    // sample. Naming matches WMO §3.30 / GRIB1 GDS conventions.
    // -------------------------------------------------------------------
    /// Latitude at which Dx and Dy are specified, in degrees. GRIB2 §3.30
    /// carries this explicitly; for GRIB1 it is mirrored from `latin1` (the
    /// historical convention).
    /// Radius of the sphere the grid is projected on, in metres, as the message
    /// declares it (GRIB1's earth-shape flag; GRIB2's `shapeOfTheEarth`; WRF's
    /// own 6 370 000 m sphere). `None` falls back to the projection default.
    pub earth_radius_metres: Option<f64>,
    pub lambert_lad: Option<f64>,
    /// Orientation longitude (the meridian parallel to the y-axis), degrees.
    pub lambert_lov: Option<f64>,
    /// Grid spacing in metres along x at the latitude of true scale.
    pub lambert_dx_metres: Option<f64>,
    /// Grid spacing in metres along y at the latitude of true scale.
    pub lambert_dy_metres: Option<f64>,
    /// First standard parallel, degrees.
    pub lambert_latin1: Option<f64>,
    /// Second standard parallel, degrees.
    pub lambert_latin2: Option<f64>,
    // -------------------------------------------------------------------
    // Gaussian projection parameter (only populated for Gaussian grids).
    // -------------------------------------------------------------------
    /// Number of parallels between a pole and the equator (the "N" in the
    /// Gaussian grid spec). Needed to reconstruct row latitudes via the
    /// Gauss–Legendre quadrature nodes during reprojection.
    pub gaussian_n_parallels: Option<i32>,
    // -------------------------------------------------------------------
    // Polar stereographic projection parameters (only populated for polar
    // stereographic grids; `None` for every other grid type). GRIB1 fixes
    // the latitude of true scale at 60° implicitly — see `PolarStereoParams`
    // — so the only on-the-wire fields are `lov`, the grid spacing in metres
    // along x and y, and the pole-orientation flag.
    // -------------------------------------------------------------------
    /// Orientation longitude (`LoV`) — meridian parallel to the y-axis,
    /// degrees.
    pub polar_stereo_lov: Option<f64>,
    /// Latitude of true scale (`LaD`), degrees — the parallel at which the
    /// grid spacings are specified. GRIB1 fixes this at ±60°; GRIB2 §3.20
    /// carries it explicitly.
    pub polar_stereo_lad: Option<f64>,
    /// Grid spacing in metres along x at the latitude of true scale.
    pub polar_stereo_dx_metres: Option<f64>,
    /// Grid spacing in metres along y at the latitude of true scale.
    pub polar_stereo_dy_metres: Option<f64>,
    /// `true` ⇒ south-pole projection, `false` ⇒ north-pole.
    pub polar_stereo_south_pole: Option<bool>,
    // -------------------------------------------------------------------
    // Rotated latitude/longitude projection parameters (only populated for
    // GRIB2 §3.1 rotated lat/lon grids; `None` for every other grid type).
    // The grid's corner coordinates (lat/lon first/last) are in the rotated
    // frame; these three fields define the rotation back to geographic.
    // -------------------------------------------------------------------
    /// Geographic latitude of the projection's southern pole (degrees).
    pub rotated_south_pole_lat: Option<f64>,
    /// Geographic longitude of the projection's southern pole (degrees).
    pub rotated_south_pole_lon: Option<f64>,
    /// Angle of rotation about the new polar axis (degrees).
    pub rotated_angle_of_rotation: Option<f64>,
    // -------------------------------------------------------------------
    // Geostationary / space-view projection parameters (only populated for
    // GRIB2 §3.90 space-view grids; `None` for every other grid type). The
    // grid is described in scan-angle space, so the warp reconstructs a
    // `GeostationaryProjector` whose inverse maps (lat, lon) → scan angle →
    // grid index. See `fieldglass_core::GeostationaryParams`.
    // -------------------------------------------------------------------
    /// Sub-satellite longitude (`longitude_of_projection_origin`), degrees.
    pub geos_sub_lon: Option<f64>,
    /// Distance from the Earth's centre to the satellite, metres.
    pub geos_height: Option<f64>,
    /// Ellipsoid semi-major axis (equatorial radius), metres.
    pub geos_r_eq: Option<f64>,
    /// Ellipsoid semi-minor axis (polar radius), metres.
    pub geos_r_pol: Option<f64>,
    /// `true` ⇒ sweep angle about the `x` axis (GOES-R; GRIB2 §3.90);
    /// `false` ⇒ about the `y` axis (Meteosat).
    pub geos_sweep_x: Option<bool>,
    /// Scan angle (radians) at column `i = 0`.
    pub geos_x0: Option<f64>,
    /// Signed scan-angle increment per column (radians).
    pub geos_dx_rad: Option<f64>,
    /// Scan angle (radians) at row `j = 0`.
    pub geos_y0: Option<f64>,
    /// Signed scan-angle increment per row (radians).
    pub geos_dy_rad: Option<f64>,
    /// Human-readable data-packing method for this message — the GRIB1 BDS
    /// packing or GRIB2 §5 data-representation template, mapped to a friendly
    /// label (e.g. "Second-order (SPD-2)", "Simple grid-point"). `None` when
    /// the section can't be parsed.
    pub packing: Option<String>,
    /// Whether this message's grid can be reprojected (the render panel's
    /// non-source projection targets). Mirrors [`warp_setup_for`]: only
    /// lat/lon, rotated lat/lon, Gaussian, Mercator, Lambert, and
    /// polar-stereographic source grids reproject, and the two planar
    /// projections additionally need a non-zero grid spacing (some sample files
    /// carry Dx = Dy = 0). The webview hides the reprojection options when this
    /// is `false`.
    pub reprojectable: bool,
    /// Whether the grid's rows scan south→north (GRIB `jScansPositively`).
    /// The source projection paints grid row 0 at the top of the canvas, so a
    /// south→north grid renders upside-down unless flipped; the source render
    /// uses this to orient the raster by default (#286). `None` for grids with
    /// no scan flag (predefined GRIB1 grids, NetCDF), treated as `false`.
    pub j_scans_positive: Option<bool>,
}

/// Whether a grid can feed the warp pipeline's non-source targets. Mirrors the
/// dispatch in [`warp_setup_for`] — lat/lon, rotated lat/lon, Gaussian, and
/// Mercator qualify when they scan west-to-east (their inverse maps are pinned
/// by the corner coordinates — plus the rotated pole — alone, and those maps
/// read a descending-longitude corner pair as an antimeridian wrap, so the
/// vanishingly rare −i scan stays in the source projection rather than
/// mis-rendering); the planar projections (Lambert, polar stereographic) also
/// require a non-zero grid spacing, since a degenerate Dx/Dy collapses every
/// grid point onto one location and the reprojection would render blank. (The
/// planar grids handle −i themselves: the scan sign is baked into Dx.)
fn grid_is_reprojectable(
    grid_type: Option<&str>,
    planar_dx: Option<f64>,
    planar_dy: Option<f64>,
    i_scan_negative: bool,
) -> bool {
    match grid_type {
        // Reduced grids are widened to a regular Ni·Nj raster at decode time, so
        // they reproject through the same lat/lon and Gaussian inverse maps.
        Some("latlon")
        | Some("gaussian")
        | Some("mercator")
        | Some("rotated_latlon")
        | Some("reduced_latlon")
        | Some("reduced_gaussian") => !i_scan_negative,
        // Space view carries its scan-angle increments through the same
        // planar spacing slots; an orthographic view (no camera altitude)
        // leaves them `None` and so does not reproject.
        Some("lambert") | Some("polar_stereo") | Some("space_view") => {
            matches!((planar_dx, planar_dy), (Some(dx), Some(dy)) if dx != 0.0 && dy != 0.0)
        }
        _ => false,
    }
}

/// Map an eccodes-style `packingType` (GRIB1) or §5 template name (GRIB2) to a
/// friendly label for the message table. Falls back to the raw identifier for
/// anything unmapped so a new variant still shows *something* meaningful.
fn friendly_packing(label: &str) -> String {
    let mapped = match label {
        "grid_simple" | "simple" => "Simple grid-point",
        "grid_complex" | "complex" => "Complex packing",
        "grid_complex_spatial_differencing" | "complex_spatial_diff" => {
            "Complex packing + spatial differencing"
        }
        "grid_ieee" | "ieee" => "IEEE float",
        "grid_jpeg" | "jpeg" => "JPEG 2000",
        "grid_png" | "png" => "PNG",
        "grid_ccsds" | "ccsds" => "CCSDS",
        "grid_run_length" | "run_length" => "Run-length",
        "grid_simple_log_preprocessing" | "simple_log_preprocessing" => "Simple packing (log)",
        "grid_simple_matrix" => "Matrix of values",
        "grid_second_order" => "Second-order (SPD-2)",
        "grid_second_order_no_SPD" => "Second-order (no SPD)",
        "grid_second_order_SPD1" => "Second-order (SPD-1)",
        "grid_second_order_SPD3" => "Second-order (SPD-3)",
        "grid_second_order_row_by_row" => "Second-order (row-by-row)",
        "grid_second_order_constant_width" => "Second-order (constant width)",
        "grid_second_order_general_grib1" => "Second-order (general)",
        "grid_second_order_unknown" => "Second-order",
        "spectral" => "Spherical harmonic",
        // GRIB2 unsupported(5.N) templates: name the scheme behind the number.
        other => {
            if let Some(n) = other
                .strip_prefix("unsupported(5.")
                .and_then(|s| s.strip_suffix(')'))
            {
                return match n {
                    "2" | "3" => format!("Complex packing (5.{n})"),
                    "40" => "JPEG 2000 (5.40)".to_string(),
                    "41" => "PNG (5.41)".to_string(),
                    "42" => "CCSDS (5.42)".to_string(),
                    _ => format!("Unsupported (5.{n})"),
                };
            }
            return other.to_string();
        }
    };
    mapped.to_string()
}

/// Detect the format of a file from its raw bytes.
/// Returns "grib1" | "grib2" | "netcdf" | "unknown".
#[napi]
pub fn detect_bytes(bytes: napi::bindgen_prelude::Buffer) -> String {
    match detect_from_bytes(&bytes) {
        Format::Grib1 => "grib1".to_string(),
        Format::Grib2 => "grib2".to_string(),
        Format::NetCdf => "netcdf".to_string(),
        Format::Unknown => "unknown".to_string(),
    }
}

/// Build the `MessageMeta` payload for a single GRIB1 message. Used by
/// [`Grib1Handle::messages`].
fn build_grib1_message_meta(
    msg: &fieldglass_grib1::Grib1Message,
    packing: Option<String>,
) -> MessageMeta {
    let param = lookup_parameter(
        msg.pds.parameter_id,
        msg.pds.table_version,
        msg.pds.originating_centre,
    );
    let (grid_type, grid_ni, grid_nj, lat_first, lon_first, lat_last, lon_last) = match &msg.gds {
        Some(gds) => {
            let dims = gds.dimensions();
            let bounds = gds.bounds();
            (
                Some(gds.grid_type_name().to_string()),
                dims.map(|(ni, _)| ni as i32),
                dims.map(|(_, nj)| nj as i32),
                bounds.map(|(la1, _, _, _)| la1),
                bounds.map(|(_, lo1, _, _)| lo1),
                bounds.map(|(_, _, la2, _)| la2),
                bounds.map(|(_, _, _, lo2)| lo2),
            )
        }
        None => (None, None, None, None, None, None, None),
    };
    let lambert = match &msg.gds {
        Some(fieldglass_grib1::GridDescription::LambertConformal(g)) => Some(g),
        _ => None,
    };
    let lambert_lad = lambert.map(|g| g.latin1);
    let lambert_lov = lambert.map(|g| g.lov);
    // GRIB1 stores Dx/Dy as unsigned magnitudes; bake the scan sign in so the
    // Lambert warp walks the grid's actual scan (see `signed_grid_increments`).
    let lambert_inc = lambert.map(|g| {
        signed_grid_increments(
            g.dx_m as f64,
            g.dy_m as f64,
            g.scanning_mode.i_negative,
            g.scanning_mode.j_positive,
        )
    });
    let lambert_dx_metres = lambert_inc.map(|(dx, _)| dx);
    let lambert_dy_metres = lambert_inc.map(|(_, dy)| dy);
    let lambert_latin1 = lambert.map(|g| g.latin1);
    let lambert_latin2 = lambert.map(|g| g.latin2);
    let gaussian_n_parallels = match &msg.gds {
        Some(fieldglass_grib1::GridDescription::Gaussian(g)) => Some(g.n_gaussians as i32),
        Some(fieldglass_grib1::GridDescription::ReducedGaussian(g)) => Some(g.n_gaussians as i32),
        _ => None,
    };
    let polar_stereo = match &msg.gds {
        Some(fieldglass_grib1::GridDescription::PolarStereographic(g)) => Some(g),
        _ => None,
    };
    // The earth shape is declared per-message (GDS octet 17). Only the planar
    // projections consume it; a lat/lon or Gaussian grid needs no radius.
    let earth_radius_metres = lambert
        .map(|g| g.resolution_flags.earth_radius_m())
        .or_else(|| polar_stereo.map(|g| g.resolution_flags.earth_radius_m()));
    let polar_stereo_lov = polar_stereo.map(|g| g.lov);
    // GRIB1 has no LaD field — its latitude of true scale is fixed at ±60°.
    let polar_stereo_lad = polar_stereo.map(|_| 60.0);
    // GRIB1 stores Dx/Dy as unsigned magnitudes (`read_u24`); bake the
    // scanning-mode sign in so the warp walks the grid's actual scan.
    let polar_stereo_inc = polar_stereo.map(|g| {
        signed_grid_increments(
            g.dx_m as f64,
            g.dy_m as f64,
            g.scanning_mode.i_negative,
            g.scanning_mode.j_positive,
        )
    });
    let polar_stereo_dx_metres = polar_stereo_inc.map(|(dx, _)| dx);
    let polar_stereo_dy_metres = polar_stereo_inc.map(|(_, dy)| dy);
    let polar_stereo_south_pole = polar_stereo.map(|g| g.south_pole);
    let rotated = match &msg.gds {
        Some(fieldglass_grib1::GridDescription::RotatedLatLon(g)) => Some(g),
        _ => None,
    };
    let rotated_south_pole_lat = rotated.map(|g| g.south_pole_lat);
    let rotated_south_pole_lon = rotated.map(|g| g.south_pole_lon);
    let rotated_angle_of_rotation = rotated.map(|g| g.angle_of_rotation);
    // Corner-pinned grids assume a west-to-east scan; surface the −i flag so
    // `grid_is_reprojectable` keeps descending grids in the source projection.
    // (Predefined no-GDS grids have no flag and all scan west-to-east.)
    let i_scan_negative = match &msg.gds {
        Some(fieldglass_grib1::GridDescription::LatLon(g)) => g.scanning_mode.i_negative,
        Some(fieldglass_grib1::GridDescription::RotatedLatLon(g)) => g.scanning_mode.i_negative,
        Some(fieldglass_grib1::GridDescription::ReducedLatLon(g)) => g.scanning_mode.i_negative,
        Some(fieldglass_grib1::GridDescription::Gaussian(g)) => g.scanning_mode.i_negative,
        Some(fieldglass_grib1::GridDescription::ReducedGaussian(g)) => g.scanning_mode.i_negative,
        _ => false,
    };
    // South→north row scan, so the source render can orient the raster (#286).
    let j_scans_positive = match &msg.gds {
        Some(fieldglass_grib1::GridDescription::LatLon(g)) => Some(g.scanning_mode.j_positive),
        Some(fieldglass_grib1::GridDescription::RotatedLatLon(g)) => {
            Some(g.scanning_mode.j_positive)
        }
        Some(fieldglass_grib1::GridDescription::ReducedLatLon(g)) => {
            Some(g.scanning_mode.j_positive)
        }
        Some(fieldglass_grib1::GridDescription::Gaussian(g)) => Some(g.scanning_mode.j_positive),
        Some(fieldglass_grib1::GridDescription::ReducedGaussian(g)) => {
            Some(g.scanning_mode.j_positive)
        }
        Some(fieldglass_grib1::GridDescription::LambertConformal(g)) => {
            Some(g.scanning_mode.j_positive)
        }
        Some(fieldglass_grib1::GridDescription::PolarStereographic(g)) => {
            Some(g.scanning_mode.j_positive)
        }
        _ => None,
    };
    let reprojectable = grid_is_reprojectable(
        grid_type.as_deref(),
        polar_stereo_dx_metres.or(lambert_dx_metres),
        polar_stereo_dy_metres.or(lambert_dy_metres),
        i_scan_negative,
    );
    MessageMeta {
        message_index: msg.message_index as i32,
        offset_bytes: msg.byte_offset as i32,
        parameter_name: param.name.to_string(),
        parameter_units: param.units.to_string(),
        parameter_abbreviation: param.abbreviation.to_string(),
        level: fieldglass_grib1::level_value_str(&msg.pds),
        level_type: fieldglass_grib1::level_type_str(&msg.pds),
        reference_time: fieldglass_grib1::reference_time(&msg.pds),
        forecast_hours: fieldglass_grib1::forecast_hours(&msg.pds),
        forecast_display: fieldglass_grib1::forecast_display(&msg.pds),
        originating_centre: lookup_centre(msg.pds.originating_centre).to_string(),
        grid_type,
        grid_ni,
        grid_nj,
        lat_first,
        lon_first,
        lat_last,
        lon_last,
        format: "grib1".to_string(),
        edition: Some(1),
        discipline: None,
        total_length_bytes: Some(msg.is.total_length as f64),
        production_status: None,
        data_type: None,
        earth_radius_metres,
        lambert_lad,
        lambert_lov,
        lambert_dx_metres,
        lambert_dy_metres,
        lambert_latin1,
        lambert_latin2,
        gaussian_n_parallels,
        polar_stereo_lov,
        polar_stereo_lad,
        polar_stereo_dx_metres,
        polar_stereo_dy_metres,
        polar_stereo_south_pole,
        rotated_south_pole_lat,
        rotated_south_pole_lon,
        rotated_angle_of_rotation,
        geos_sub_lon: None,
        geos_height: None,
        geos_r_eq: None,
        geos_r_pol: None,
        geos_sweep_x: None,
        geos_x0: None,
        geos_dx_rad: None,
        geos_y0: None,
        geos_dy_rad: None,
        packing,
        reprojectable,
        j_scans_positive,
    }
}

/// Render the §4 product fields into the flat (`parameter_*`, `level`,
/// `forecast_*`) shape the JS layer consumes. Splitting the rendering out
/// of [`open_grib2`] keeps the loop body short and lets future templates
/// reuse the same projection.
fn grib2_product_fields(discipline: u8, pds: &ProductDefinitionSection) -> Grib2ProductFields {
    let Some(common) = pds.common() else {
        return Grib2ProductFields::placeholder();
    };
    let (abbreviation, name, units) = match lookup_grib2_parameter(
        discipline,
        common.parameter_category,
        common.parameter_number,
    ) {
        Some((abbr, long, units)) => (abbr.to_string(), long.to_string(), units.to_string()),
        None => (
            String::new(),
            format!(
                "Parameter {}/{}/{}",
                discipline, common.parameter_category, common.parameter_number
            ),
            String::new(),
        ),
    };

    let level_type = lookup_fixed_surface(common.first_surface.surface_type).to_string();
    let level = render_level(common);

    let (forecast_hours, forecast_display) = render_forecast(common);

    Grib2ProductFields {
        parameter_name: name,
        parameter_units: units,
        parameter_abbreviation: abbreviation,
        level,
        level_type,
        forecast_hours,
        forecast_display,
    }
}

struct Grib2ProductFields {
    parameter_name: String,
    parameter_units: String,
    parameter_abbreviation: String,
    level: String,
    level_type: String,
    forecast_hours: i32,
    forecast_display: String,
}

impl Grib2ProductFields {
    fn placeholder() -> Self {
        Self {
            parameter_name: String::new(),
            parameter_units: String::new(),
            parameter_abbreviation: String::new(),
            level: "—".to_string(),
            level_type: "—".to_string(),
            forecast_hours: 0,
            forecast_display: "—".to_string(),
        }
    }
}

/// Render the first fixed surface as a human-readable level string. Falls
/// back to `"—"` when the surface is the WMO "missing" sentinel; otherwise
/// shows the decoded float with the surface label as a unit hint.
fn render_level(common: &HorizontalProductCommon) -> String {
    let surface = &common.first_surface;
    if surface.is_missing() {
        return "—".to_string();
    }
    match surface.value() {
        Some(v) => format!("{v}"),
        None => lookup_fixed_surface(surface.surface_type).to_string(),
    }
}

/// Render forecast time as `(hours_as_i32, display_string)`. Hours are
/// normalised for the common units (minute / hour / day) so the existing
/// `forecast_hours` column stays meaningful for sorting; the display string
/// preserves the original unit when the value can't be coerced.
fn render_forecast(common: &HorizontalProductCommon) -> (i32, String) {
    let unit_label = lookup_time_range_unit(common.forecast_time_unit);
    let raw = common.forecast_time;
    let hours = match common.forecast_time_unit {
        0 => Some(raw / 60), // minute
        1 => Some(raw),      // hour
        2 => Some(raw * 24), // day
        10 => Some(raw * 3),
        11 => Some(raw * 6),
        12 => Some(raw * 12),
        13 => Some(raw / 3600), // second
        _ => None,
    };
    let display = match (hours, common.forecast_time_unit) {
        (Some(h), 1) => format!("+{h}h"),
        (Some(_), _) => format!("+{raw} {unit_label}"),
        (None, _) => format!("+{raw} {unit_label}"),
    };
    let hours_i32 = hours.and_then(|h| i32::try_from(h).ok()).unwrap_or(0);
    (hours_i32, display)
}

/// Apply the GRIB scanning-mode sign to a planar projection's grid spacings.
///
/// Both GRIB1 and GRIB2 store Dx/Dy as unsigned magnitudes and carry the scan
/// direction in separate flags. The planar projectors (`PolarStereoProjector`,
/// `LambertProjector`) map a point to a grid index by `i = (x - origin_x) / dx`,
/// `j = (y - origin_y) / dy` in the LoV-oriented projection plane, so the
/// increment sign *is* the scan direction: `i` runs −x when it scans negatively,
/// and `j` runs −y (north→south) unless it scans positively. Default-scan grids
/// keep positive values.
fn signed_grid_increments(
    dx: f64,
    dy: f64,
    i_scans_negatively: bool,
    j_scans_positively: bool,
) -> (f64, f64) {
    let sdx = if i_scans_negatively {
        -dx.abs()
    } else {
        dx.abs()
    };
    let sdy = if j_scans_positively {
        dy.abs()
    } else {
        -dy.abs()
    };
    (sdx, sdy)
}

/// The geostationary scan-angle grid derived from a GRIB2 §3.90 space-view
/// template, ready to populate the `geos_*` `MessageMeta` fields and rebuild a
/// `GeostationaryProjector`.
#[derive(Debug, Clone, Copy)]
struct GeosScanGrid {
    sub_lon: f64,
    height: f64,
    r_eq: f64,
    r_pol: f64,
    sweep_x: bool,
    x0: f64,
    dx_rad: f64,
    y0: f64,
    dy_rad: f64,
}

/// Reconstruct the scan-angle grid from a §3.90 template, following the CGMS
/// LRIT/HRIT geometry that eccodes' space-view iterator decodes:
/// `angular_size = 2·asin(1/Nr)`, the satellite distance `H = Nr·r_eq`, and
/// per-grid-length scan increments `rx = angular_size/dx`,
/// `ry = (r_pol/r_eq)·angular_size/dy`. The sub-satellite point's grid offset
/// (`Xp`/`Yp` relative to the sector origin `Xo`/`Yo`, adjusted for scan
/// direction) fixes the scan angle at index 0. Returns `None` for an
/// orthographic view (`Nr` missing) or a degenerate apparent diameter, which
/// then can't reproject. GRIB2 §3.90 is the GOES-R sweep-`x` convention.
fn space_view_scan_grid(t: &fieldglass_grib2::SpaceViewTemplate) -> Option<GeosScanGrid> {
    // `Nr` is the camera altitude in units of the Earth's radius × 10⁶.
    let nr = t.nr? as f64 * 1.0e-6;
    // Nr ≤ 1 would put the camera at or below the Earth's surface (asin domain);
    // a zero apparent diameter has no scan increment.
    if nr <= 1.0 || t.dx == 0 || t.dy == 0 {
        return None;
    }
    let angular_size = 2.0 * (1.0 / nr).asin();
    let rx = angular_size / t.dx as f64;
    let ry = (t.r_pol / t.r_eq) * angular_size / t.dy as f64;

    // Scan-direction adjustment of the sub-satellite grid offset, matching the
    // eccodes space-view iterator so index 0 lands at the correct scan angle.
    let i_scans_neg = t.scanning_mode & 0x80 != 0;
    let j_scans_pos = t.scanning_mode & 0x40 != 0;
    let xp_off = t.xp - t.xo as f64;
    let yp_off = t.yp - t.yo as f64;
    let xp_adj = if i_scans_neg {
        (t.nx as f64 - 1.0) - xp_off
    } else {
        xp_off
    };
    let yp_adj = if j_scans_pos {
        yp_off
    } else {
        (t.ny as f64 - 1.0) - yp_off
    };

    // eccodes emits scan rows in reverse (`for iy = ny-1 .. 0`), so stored data
    // row `k` is geometric row `iy = (ny-1) - k`: the scan angle of stored row
    // `k` is `((ny-1) - yp_adj - k)·ry`, i.e. y0 at k=0 with a negative per-row
    // step. The column loop is not reversed, so x keeps a positive step. Index
    // (i, j) must address the stored raster (`raw[j·ni + i]`), so this matches
    // eccodes' data order — otherwise the reprojected image is flipped in y.
    Some(GeosScanGrid {
        sub_lon: t.lop,
        height: nr * t.r_eq,
        r_eq: t.r_eq,
        r_pol: t.r_pol,
        sweep_x: true,
        x0: -xp_adj * rx,
        dx_rad: rx,
        y0: (t.ny as f64 - 1.0 - yp_adj) * ry,
        dy_rad: -ry,
    })
}

/// Parse a GRIB2 file from raw bytes and return per-message metadata.
/// Surfaces §0 + §1 + §3 + §4 fields (edition, discipline, centre, ref-time,
/// parameter triple, level + level type, forecast time, production status,
/// data type, grid template, dimensions, corner coords); §5+ columns remain
/// placeholders until the data-representation / data parsers land.
///
/// Shared by [`open_grib2`] and the new [`Grib2Handle::messages`] method.
fn build_grib2_message_meta(msg: &fieldglass_grib2::Grib2Message) -> MessageMeta {
    let centre = lookup_grib2_centre(msg.ids.centre)
        .map(str::to_string)
        .unwrap_or_else(|| format!("Centre {}", msg.ids.centre));
    let dims = msg.gds.dimensions();
    let bounds = msg.gds.bounds();
    let product = grib2_product_fields(msg.is.discipline, &msg.pds);

    let lambert = match &msg.gds.template {
        fieldglass_grib2::GridTemplate::Lambert(t) => Some(t),
        _ => None,
    };
    let lambert_lad = lambert.map(|t| t.lad);
    let lambert_lov = lambert.map(|t| t.lov);
    // GRIB2 §3.30 stores Dx/Dy as unsigned magnitudes; bake the scan sign in
    // so the Lambert warp walks the grid's actual scan.
    let lambert_inc = lambert.map(|t| {
        signed_grid_increments(
            t.dx_metres,
            t.dy_metres,
            t.scanning_mode & 0x80 != 0,
            t.scanning_mode & 0x40 != 0,
        )
    });
    let lambert_dx_metres = lambert_inc.map(|(dx, _)| dx);
    let lambert_dy_metres = lambert_inc.map(|(_, dy)| dy);
    let lambert_latin1 = lambert.map(|t| t.latin1);
    let lambert_latin2 = lambert.map(|t| t.latin2);
    let gaussian_n_parallels = match &msg.gds.template {
        fieldglass_grib2::GridTemplate::Gaussian(t) => Some(t.n_parallels as i32),
        _ => None,
    };
    let polar_stereo = match &msg.gds.template {
        fieldglass_grib2::GridTemplate::PolarStereographic(t) => Some(t),
        _ => None,
    };
    // GRIB2 §3.20 stores Dx/Dy as unsigned magnitudes; the scan direction
    // lives in the scanning-mode flags. Bake the sign in so the projector's
    // origin-relative index advances along the grid's actual scan.
    let polar_stereo_inc = polar_stereo.map(|t| {
        signed_grid_increments(
            t.dx_metres,
            t.dy_metres,
            t.scanning_mode & 0x80 != 0,
            t.scanning_mode & 0x40 != 0,
        )
    });

    // §3.1 carries the rotated-pole position and rotation angle alongside a
    // §3.0-style lat/lon layout; the warp uses them to rotate a geographic
    // query into the grid's rotated frame.
    let rotated = match &msg.gds.template {
        fieldglass_grib2::GridTemplate::RotatedLatLon(t) => Some(t),
        _ => None,
    };
    let rotated_south_pole_lat = rotated.map(|t| t.south_pole_lat);
    let rotated_south_pole_lon = rotated.map(|t| t.south_pole_lon);
    let rotated_angle_of_rotation = rotated.map(|t| t.angle_of_rotation);

    // §3.90 space view: reconstruct the scan-angle grid for the geostationary
    // warp (sub-satellite point, ellipsoid, camera height, scan increments).
    let space_view = match &msg.gds.template {
        fieldglass_grib2::GridTemplate::SpaceView(t) => space_view_scan_grid(t),
        _ => None,
    };

    let grid_type = msg.gds.template_name();
    // Corner-pinned grids assume a west-to-east scan; surface the −i flag
    // (scanning-mode bit 0x80) so `grid_is_reprojectable` keeps descending
    // grids in the source projection.
    // Earth shape, declared per-message in §3 (`shapeOfTheEarth`). Only the
    // planar projections consume it.
    let earth_radius_metres = match &msg.gds.template {
        fieldglass_grib2::GridTemplate::Lambert(t) => Some(t.earth_radius_m),
        fieldglass_grib2::GridTemplate::PolarStereographic(t) => Some(t.earth_radius_m),
        _ => None,
    };
    let i_scan_negative = match &msg.gds.template {
        fieldglass_grib2::GridTemplate::LatLon(t) => t.scanning_mode & 0x80 != 0,
        fieldglass_grib2::GridTemplate::RotatedLatLon(t) => t.scanning_mode & 0x80 != 0,
        fieldglass_grib2::GridTemplate::Mercator(t) => t.scanning_mode & 0x80 != 0,
        fieldglass_grib2::GridTemplate::Gaussian(t) => t.scanning_mode & 0x80 != 0,
        _ => false,
    };
    let reprojectable = grid_is_reprojectable(
        Some(grid_type.as_str()),
        polar_stereo_inc
            .map(|(dx, _)| dx)
            .or(lambert_dx_metres)
            .or(space_view.map(|g| g.dx_rad)),
        polar_stereo_inc
            .map(|(_, dy)| dy)
            .or(lambert_dy_metres)
            .or(space_view.map(|g| g.dy_rad)),
        i_scan_negative,
    );

    MessageMeta {
        message_index: msg.message_index as i32,
        offset_bytes: msg.byte_offset as i32,
        parameter_name: product.parameter_name,
        parameter_units: product.parameter_units,
        parameter_abbreviation: product.parameter_abbreviation,
        level: product.level,
        level_type: product.level_type,
        reference_time: msg.ids.reference_time_iso8601(),
        forecast_hours: product.forecast_hours,
        forecast_display: product.forecast_display,
        originating_centre: centre,
        grid_type: Some(grid_type),
        grid_ni: dims.map(|(ni, _)| ni as i32),
        grid_nj: dims.map(|(_, nj)| nj as i32),
        lat_first: bounds.map(|(la1, _, _, _)| la1),
        lon_first: bounds.map(|(_, lo1, _, _)| lo1),
        lat_last: bounds.map(|(_, _, la2, _)| la2),
        lon_last: bounds.map(|(_, _, _, lo2)| lo2),
        format: "grib2".to_string(),
        edition: Some(i32::from(msg.is.edition)),
        discipline: Some(lookup_discipline(msg.is.discipline).to_string()),
        total_length_bytes: Some(msg.is.total_length as f64),
        production_status: Some(lookup_production_status(msg.ids.production_status).to_string()),
        data_type: Some(fieldglass_grib2::lookup_data_type(msg.ids.data_type).to_string()),
        earth_radius_metres,
        lambert_lad,
        lambert_lov,
        lambert_dx_metres,
        lambert_dy_metres,
        lambert_latin1,
        lambert_latin2,
        gaussian_n_parallels,
        polar_stereo_lov: polar_stereo.map(|t| t.lov),
        polar_stereo_lad: polar_stereo.map(|t| t.lad),
        polar_stereo_dx_metres: polar_stereo_inc.map(|(dx, _)| dx),
        polar_stereo_dy_metres: polar_stereo_inc.map(|(_, dy)| dy),
        polar_stereo_south_pole: polar_stereo.map(|t| t.south_pole),
        rotated_south_pole_lat,
        rotated_south_pole_lon,
        rotated_angle_of_rotation,
        geos_sub_lon: space_view.map(|g| g.sub_lon),
        geos_height: space_view.map(|g| g.height),
        geos_r_eq: space_view.map(|g| g.r_eq),
        geos_r_pol: space_view.map(|g| g.r_pol),
        geos_sweep_x: space_view.map(|g| g.sweep_x),
        geos_x0: space_view.map(|g| g.x0),
        geos_dx_rad: space_view.map(|g| g.dx_rad),
        geos_y0: space_view.map(|g| g.y0),
        geos_dy_rad: space_view.map(|g| g.dy_rad),
        packing: Some(friendly_packing(&msg.drs.template_name())),
        reprojectable,
        // GRIB2 §3 Flag Table 3.4 bit 2 (0x40): rows scan south→north (#286).
        j_scans_positive: msg.gds.scanning_mode().map(|sm| sm & 0x40 != 0),
    }
}

// ---------------------------------------------------------------------------
// NetCDF
// ---------------------------------------------------------------------------

/// One NetCDF dimension, flattened for the JS boundary.
#[napi(object)]
pub struct DimensionMeta {
    pub name: String,
    /// Length of the dimension. `0` is a valid display value for the
    /// unlimited / record dimension (with `is_record == true`).
    pub length: f64,
    pub is_record: bool,
}

/// One NetCDF attribute (global or per-variable). `value` is already a
/// human-readable string — UTF-8 for Char attributes, comma-separated decimal
/// for numeric ones — so the provider can render it as-is.
#[napi(object)]
pub struct AttributeMeta {
    pub name: String,
    pub nc_type: String,
    pub value: String,
}

/// One NetCDF variable, flattened for the JS boundary. `dimensions` lists
/// resolved dimension names (in declared order) so the provider doesn't need
/// to cross-reference dim ids itself.
#[napi(object)]
pub struct VariableMeta {
    pub name: String,
    pub nc_type: String,
    pub dimensions: Vec<String>,
    pub attributes: Vec<AttributeMeta>,
}

/// Top-level NetCDF dataset metadata. Covers what's exposable from the
/// header alone; per-variable values are a separate decode step (out of
/// scope for issue #29).
#[napi(object)]
pub struct DatasetMeta {
    /// `"classic"` (CDF-1/2/5) or `"hdf5"` (NetCDF-4).
    pub backing: String,
    /// Human-readable label, e.g. `"NetCDF classic (CDF-1)"` or
    /// `"NetCDF-4 / HDF5"`.
    pub backing_label: String,
    /// `true` if dimensions / variables / attributes are populated. Always
    /// `true` for classic; `true` for HDF5 once its dimension-scale metadata
    /// resolves, and `false` (with `note` set) only when an HDF5 file uses a
    /// layout outside the decoded subset.
    pub fully_parsed: bool,
    /// Free-form note for the provider to surface when `fully_parsed` is
    /// false — e.g. why an HDF5 file's metadata could not be fully resolved.
    pub note: Option<String>,
    pub dimensions: Vec<DimensionMeta>,
    pub global_attributes: Vec<AttributeMeta>,
    pub variables: Vec<VariableMeta>,
    /// HDF5 superblock version, when applicable. `None` for classic files.
    pub hdf5_superblock_version: Option<i32>,
}

/// Parse a NetCDF file from raw bytes and return the top-level dataset
/// metadata. Errors only for files that are neither classic CDF nor HDF5;
/// HDF5 files succeed with `fully_parsed = false`.
#[napi]
pub fn open_netcdf(bytes: napi::bindgen_prelude::Buffer) -> napi::Result<DatasetMeta> {
    let reader = NetcdfReader::from_bytes(bytes.to_vec())
        .map_err(|e| napi::Error::from_reason(e.to_string()))?;
    Ok(dataset_meta_from(reader))
}

fn dataset_meta_from(reader: NetcdfReader) -> DatasetMeta {
    let label = reader.backing.label().to_string();
    // HDF5 / NetCDF-4: resolve the dimension-scale convention (decision 0003) to
    // the same dimensions / variables / attributes the classic backing exposes.
    // Resolution is fallible (a layout outside the decoded subset); on failure we
    // fall back to the format-only view with the reason in `note`, so the file
    // still opens and the superblock still renders.
    if let NetcdfBacking::Hdf5(probe) = &reader.backing {
        let superblock = Some(probe.superblock_version as i32);
        return match reader.hdf5_metadata() {
            Ok(meta) => hdf5_dataset_meta(label, superblock, meta),
            Err(e) => hdf5_unparsed_meta(label, superblock, &e.to_string()),
        };
    }
    match reader.backing {
        NetcdfBacking::Classic(h) => {
            let dim_names: Vec<String> = h.dimensions.iter().map(|d| d.name.clone()).collect();
            let dimensions = h
                .dimensions
                .iter()
                .map(|d| DimensionMeta {
                    name: d.name.clone(),
                    length: d.length as f64,
                    is_record: d.is_record,
                })
                .collect();
            let global_attributes = h
                .global_attributes
                .iter()
                .map(|a| AttributeMeta {
                    name: a.name.clone(),
                    nc_type: a.nc_type.name().to_string(),
                    value: a.value.clone(),
                })
                .collect();
            let variables = h
                .variables
                .iter()
                .map(|v| VariableMeta {
                    name: v.name.clone(),
                    nc_type: v.nc_type.name().to_string(),
                    dimensions: v
                        .dim_ids
                        .iter()
                        .map(|&id| {
                            dim_names
                                .get(id as usize)
                                .cloned()
                                .unwrap_or_else(|| format!("dim#{id}"))
                        })
                        .collect(),
                    attributes: v
                        .attributes
                        .iter()
                        .map(|a| AttributeMeta {
                            name: a.name.clone(),
                            nc_type: a.nc_type.name().to_string(),
                            value: a.value.clone(),
                        })
                        .collect(),
                })
                .collect();
            DatasetMeta {
                backing: "classic".to_string(),
                backing_label: label,
                fully_parsed: true,
                note: None,
                dimensions,
                global_attributes,
                variables,
                hdf5_superblock_version: None,
            }
        }
        // HDF5 is handled above, before this match consumes `reader.backing`.
        NetcdfBacking::Hdf5(_) => unreachable!("HDF5 backing resolved before the match"),
    }
}

/// Map a resolved NetCDF-4 / HDF5 root group to the JS dataset metadata.
fn hdf5_dataset_meta(label: String, superblock: Option<i32>, meta: Hdf5Metadata) -> DatasetMeta {
    let dimensions = meta
        .dimensions
        .into_iter()
        .map(|d| DimensionMeta {
            name: d.name,
            length: d.length as f64,
            is_record: d.is_unlimited,
        })
        .collect();
    let variables = meta
        .variables
        .into_iter()
        .map(|v| VariableMeta {
            name: v.name,
            nc_type: v.nc_type.name().to_string(),
            dimensions: v.dimensions,
            attributes: v.attributes.iter().map(hdf5_attribute_meta).collect(),
        })
        .collect();
    DatasetMeta {
        backing: "hdf5".to_string(),
        backing_label: label,
        fully_parsed: true,
        note: None,
        dimensions,
        global_attributes: meta
            .global_attributes
            .iter()
            .map(hdf5_attribute_meta)
            .collect(),
        variables,
        hdf5_superblock_version: superblock,
    }
}

/// The format-only fallback when HDF5 metadata can't be fully resolved — the
/// file still opens and reports its superblock, with the reason in `note`.
fn hdf5_unparsed_meta(label: String, superblock: Option<i32>, reason: &str) -> DatasetMeta {
    DatasetMeta {
        backing: "hdf5".to_string(),
        backing_label: label,
        fully_parsed: false,
        note: Some(format!(
            "NetCDF-4 / HDF5 metadata could not be fully resolved ({reason}); \
             only the superblock has been validated."
        )),
        dimensions: Vec::new(),
        global_attributes: Vec::new(),
        variables: Vec::new(),
        hdf5_superblock_version: superblock,
    }
}

fn hdf5_attribute_meta(a: &Hdf5Attribute) -> AttributeMeta {
    AttributeMeta {
        name: a.name.clone(),
        nc_type: a.datatype.nc_type.name().to_string(),
        value: a.value.clone(),
    }
}

/// Decoded values for one NetCDF variable, flattened for the JS boundary.
/// Mirrors [`DecodedGrid`] but carries the full N-D `shape` (a NetCDF variable
/// may be 1-D, 3-D, 4-D, …) instead of a single width/height.
#[napi(object)]
pub struct DecodedVariable {
    /// Row-major (C / on-disk order) values; `f64::NAN` at masked / fill
    /// positions (read `mask` to distinguish a real value from a fill).
    pub values: napi::bindgen_prelude::Float64Array,
    /// Parallel to `values`: `1` present, `0` equal to the variable's
    /// `_FillValue`.
    pub mask: napi::bindgen_prelude::Buffer,
    /// Dimension lengths in declared (C) order; the unlimited dimension
    /// contributes the file's record count.
    pub shape: Vec<u32>,
}

/// Decode one NetCDF variable's values by index. Works for classic (CDF-1/2/5)
/// and NetCDF-4 / HDF5 backings; for HDF5 a "variable" is a root-group dataset
/// in name-sorted order. Errors for `char` / string variables (text, not
/// numbers), datasets stored with a layout not yet decoded (e.g. a version-4
/// chunk index), and out-of-range indices. Mirrors the GRIB `decode_grid`
/// surface.
#[napi]
pub fn decode_netcdf_variable(
    bytes: napi::bindgen_prelude::Buffer,
    variable_index: u32,
) -> napi::Result<DecodedVariable> {
    let reader = NetcdfReader::from_bytes(bytes.to_vec())
        .map_err(|e| napi::Error::from_reason(e.to_string()))?;
    let raw = reader
        .decode_variable_values(variable_index as usize)
        .map_err(|e| napi::Error::from_reason(e.to_string()))?;
    let shape = reader
        .variable_shape(variable_index as usize)
        .map_err(|e| napi::Error::from_reason(e.to_string()))?
        .into_iter()
        .map(|d| {
            u32::try_from(d)
                .map_err(|_| napi::Error::from_reason(format!("dimension length {d} exceeds u32")))
        })
        .collect::<napi::Result<Vec<u32>>>()?;

    let mut values = vec![0.0f64; raw.len()];
    let mut mask = vec![0u8; raw.len()];
    for (i, v) in raw.iter().enumerate() {
        match v {
            Some(x) => {
                values[i] = *x;
                mask[i] = 1;
            }
            None => values[i] = f64::NAN,
        }
    }
    Ok(DecodedVariable {
        values: napi::bindgen_prelude::Float64Array::new(values),
        mask: mask.into(),
        shape,
    })
}

// ---------------------------------------------------------------------------
// Reader handles + render entry  (closes #41 + the Rust-side render rewrite
// from #45)
// ---------------------------------------------------------------------------

/// Picker-driven render options posted from the webview. `projection` is
/// either `"source"` (paint the source grid unchanged) or
/// `"equirectangular"` (inverse-warp into a north-up lat/lon canvas).
#[napi(object)]
pub struct RenderOptions {
    pub projection: String,
    /// Preset selector for the parameterised targets. `"orthographic"` reads
    /// a centre preset (`"atlantic"` (0°N 0°E, default), `"indian"` (0°N 90°E),
    /// `"pacific"` (0°N 180°E), `"americas"` (0°N 270°E), `"north_pole"`,
    /// `"south_pole"`); `"polar_stereographic"` reads a
    /// hemisphere preset (`"north"` (default), `"south"`). Ignored by the
    /// lat/lon-box targets. `None`/unknown falls back to the default.
    ///
    /// Superseded by [`center_lat`]/[`center_lon`] when those are supplied: the
    /// free-form centre wins, the preset is only the fallback. Retained so
    /// callers that still send a preset keep working.
    ///
    /// [`center_lat`]: RenderOptions::center_lat
    /// [`center_lon`]: RenderOptions::center_lon
    pub projection_preset: Option<String>,
    /// Free-form projection centre for the azimuthal targets (degrees).
    /// `"orthographic"` reads both: `center_lat` is the centre latitude
    /// (clamped to ±90° downstream) and `center_lon` the centre longitude.
    /// `"polar_stereographic"` reads only `center_lon` as the central meridian
    /// (the meridian oriented toward the bottom edge); its pole is the
    /// hemisphere from [`projection_preset`]. The world targets
    /// (`"mollweide"`, `"robinson"`, `"equal_earth"`) likewise read only
    /// `center_lon`, as their central meridian. Either field `None` falls back
    /// to the preset/default for that component. Ignored by the lat/lon-box
    /// targets.
    ///
    /// [`projection_preset`]: RenderOptions::projection_preset
    pub center_lat: Option<f64>,
    /// Free-form projection-centre longitude — see [`center_lat`].
    ///
    /// [`center_lat`]: RenderOptions::center_lat
    pub center_lon: Option<f64>,
    pub resampling: String,
    pub flip_y: bool,
    /// Manual range override. When either is `None` the renderer uses the
    /// computed min/max over the present cells.
    pub range_min: Option<f64>,
    pub range_max: Option<f64>,
    /// Manual equirectangular extent override (degrees). Only consulted for
    /// the `"equirectangular"` target. When all four are `Some` and form a
    /// valid box (`lat_max > lat_min`, `lon_max > lon_min`) the warp renders
    /// that window; otherwise it falls back to the computed source-grid bounds
    /// (echoed back in [`RenderedGrid`] so a UI can pre-fill these inputs).
    /// `lon_min`/`lon_max` may lie outside [-180, 180] to describe a window
    /// that crosses the antimeridian.
    pub bounds_lat_min: Option<f64>,
    pub bounds_lat_max: Option<f64>,
    pub bounds_lon_min: Option<f64>,
    pub bounds_lon_max: Option<f64>,
    /// Name of the colormap to paint with — one of the names [`colormaps`]
    /// reports (`"viridis"`, `"plasma"`, `"turbo"`, `"rdbu"`, …). `None` uses
    /// the default (`"viridis"`), so a caller that never sets it renders
    /// exactly as before. An unknown name is an error rather than a silent
    /// fallback, so a typo can't quietly paint the wrong colours.
    pub colormap: Option<String>,
    /// Flip the colormap end-for-end (blue↔red on a diverging map, dark↔light
    /// on a sequential one). `None` is `false`.
    pub reverse_colormap: Option<bool>,
    /// Value→colour scaling: `"linear"` (default) or `"log10"`. Under `"log10"`
    /// the colour position is `log10(value)`, so quantities spanning orders of
    /// magnitude (precipitation, AOD, chlorophyll) resolve across their whole
    /// range; non-positive values have no logarithm and render as missing
    /// (transparent). `None`/unknown is an error rather than a silent fallback,
    /// matching the colormap field. Log10 needs a positive lower bound: with an
    /// auto range whose minimum is ≤ 0 the render errors, directing the caller
    /// to set a positive manual minimum (the panel disables the toggle in that
    /// case). `used_min`/`used_max` are still echoed back in true (unlogged)
    /// data units so the colorbar labels read correctly.
    pub scale_mode: Option<String>,
}

/// One entry of the colormap registry, as the picker needs it.
#[napi(object)]
pub struct ColormapInfo {
    /// Stable wire name — what [`RenderOptions::colormap`] takes.
    pub name: String,
    /// Human-readable label for the picker.
    pub label: String,
    /// `"sequential"` or `"diverging"`.
    pub kind: String,
    /// Evenly spaced `#rrggbb` stops across the ramp, low → high, for the
    /// legend gradient. Sampled from the same lookup table that paints the
    /// grid, so the legend cannot drift from the image.
    pub stops: Vec<String>,
}

/// Number of gradient stops handed to the webview per colormap. Enough for a
/// smooth CSS gradient without shipping all 256 entries.
const COLORMAP_STOPS: usize = 33;

/// Every colormap the renderer offers, in picker order; the first is the
/// default. The panel builds its dropdown and its legend gradient from this,
/// so Rust stays the single source of truth for the colours.
#[napi]
pub fn colormaps() -> Vec<ColormapInfo> {
    fieldglass_core::colormap::colormaps()
        .iter()
        .map(|c| ColormapInfo {
            name: c.name().to_string(),
            label: c.label().to_string(),
            kind: c.kind().as_str().to_string(),
            stops: c.css_stops(COLORMAP_STOPS, false),
        })
        .collect()
}

/// Output of [`Grib1Handle::render_grid`] / [`Grib2Handle::render_grid`].
/// `rgba` is a paint-ready buffer the webview blits straight to canvas
/// via `putImageData`.
#[napi(object)]
pub struct RenderedGrid {
    /// RGBA bytes, `width * height * 4` long.
    pub rgba: napi::bindgen_prelude::Buffer,
    pub width: i32,
    pub height: i32,
    /// The min/max range actually used to paint, echoed back so the
    /// webview can pre-fill the manual-range inputs when the user
    /// switches to manual mode.
    pub used_min: f64,
    pub used_max: f64,
    /// Equirectangular extent actually rendered (degrees), echoed back so the
    /// webview can pre-fill the manual-bounds inputs. `None` for the
    /// source-projection target, which has no geographic extent. `lonMin`/
    /// `lonMax` may fall outside [-180, 180] for an antimeridian-spanning
    /// window — pass them back verbatim to reproduce the same view.
    pub used_lat_min: Option<f64>,
    pub used_lat_max: Option<f64>,
    pub used_lon_min: Option<f64>,
    pub used_lon_max: Option<f64>,
    /// Human-readable summary of the source→target projection chain,
    /// e.g. `"lambert → equirectangular (nearest)"`.
    pub projection_summary: String,
}

/// Projected overlay geometry returned by `project_overlay` — coastline /
/// graticule / user-shape polylines mapped into the warped raster's pixel
/// space, ready for the webview to stroke on its overlay canvas.
///
/// `xy` is flat `[x0, y0, x1, y1, …]` in output pixel coordinates (post-flipY,
/// identical to the rendered raster); `seg_lengths` gives the vertex count of
/// each visible run after clipping/splitting, so
/// `seg_lengths.iter().sum::<u32>() * 2 == xy.len()`. May be empty when no run
/// survives clipping (every vertex projects off the visible domain).
#[napi(object)]
pub struct ProjectedOverlay {
    pub xy: napi::bindgen_prelude::Float64Array,
    pub seg_lengths: napi::bindgen_prelude::Uint32Array,
}

impl ProjectedOverlay {
    fn from_polylines(p: ProjectedPolylines) -> Self {
        Self {
            xy: p.xy.into(),
            seg_lengths: p.seg_lengths.into(),
        }
    }
}

/// Decoded grid values + presence mask returned by the handle-based
/// `decode_grid`. `values[k]` is the decoded value at scan-order index
/// `k`; `mask[k]` is `1` when present and `0` when bitmap-masked.
///
/// Not consumed by the render panel today — `render_grid` returns
/// paint-ready RGBA instead. Kept on the napi surface for future
/// "export raw values" consumers (e.g. a notebook-style data probe);
/// the JS code path is dormant.
#[napi(object)]
pub struct DecodedGrid {
    pub values: napi::bindgen_prelude::Float64Array,
    pub mask: napi::bindgen_prelude::Buffer,
    pub width: i32,
    pub height: i32,
}

/// Persistent GRIB1 reader handle held across napi calls. Replaces the
/// `open_grib1` → buffer-clone → re-parse round-trip per call.
///
/// `bytes` is kept around because [`Self::set_p1`] hands a freshly-edited
/// buffer back to the JS caller; [`Grib2Handle`] doesn't need this because
/// GRIB2 has no edit path yet (general PDS-field editing is the metadata-
/// editing track, separate from this PR). The asymmetry is intentional —
/// not an oversight.
///
/// `decoded` caches per-message decode output (raw `Vec<Option<f64>>`)
/// so repeat `render_grid` calls with different `RenderOptions` skip the
/// bit-unpack + bitmap-merge step every time the user wiggles a picker.
///
/// The cache is wrapped in a `Mutex` not because we expect contention
/// — napi-rs runs class methods on the Node main thread — but because
/// `#[napi]` requires the class to be `Send`. `RefCell` would be the
/// natural single-threaded fit but its `!Send` rules it out. Lock /
/// unlock overhead at zero contention is negligible (single atomic
/// CAS per render).
#[napi]
pub struct Grib1Handle {
    bytes: Vec<u8>,
    reader: Grib1Reader,
    decoded: Mutex<std::collections::HashMap<u32, std::sync::Arc<Vec<Option<f64>>>>>,
}

#[napi]
impl Grib1Handle {
    /// Parse the supplied buffer once; the handle keeps the parsed
    /// reader alive for the lifetime of the JS object.
    #[napi(factory)]
    pub fn from_bytes(bytes: napi::bindgen_prelude::Buffer) -> napi::Result<Self> {
        let owned = bytes.to_vec();
        let reader = Grib1Reader::from_bytes(owned.clone())
            .map_err(|e| napi::Error::from_reason(e.to_string()))?;
        Ok(Self {
            bytes: owned,
            reader,
            decoded: Mutex::new(std::collections::HashMap::new()),
        })
    }

    #[napi]
    pub fn messages(&self) -> Vec<MessageMeta> {
        self.reader
            .messages
            .iter()
            .map(|msg| {
                let packing = self
                    .reader
                    .packing_label(msg.message_index)
                    .map(friendly_packing);
                build_grib1_message_meta(msg, packing)
            })
            .collect()
    }

    /// Decode one message into a `(values, mask)` typed-array pair. NaN
    /// is reserved for masked cells in `values`; callers should consult
    /// `mask[k] == 0` rather than checking for NaN.
    #[napi]
    pub fn decode_grid(&self, message_index: u32) -> napi::Result<DecodedGrid> {
        let raw = self.cached_decode(message_index)?;
        let (width, height) = grib1_dimensions(&self.reader, message_index as usize)?;
        Ok(decoded_grid_from(&raw, width, height))
    }

    /// Patch the PDS `p1` (forecast period) octet of one message and
    /// return a fresh byte buffer. Callers reconstruct a new
    /// [`Grib1Handle`] from those bytes — handle state is immutable
    /// across edits.
    #[napi]
    pub fn set_p1(
        &self,
        message_index: u32,
        value: u32,
    ) -> napi::Result<napi::bindgen_prelude::Buffer> {
        if value > u8::MAX as u32 {
            return Err(napi::Error::from_reason(format!(
                "p1 must fit in a u8 (0..=255), got {value}"
            )));
        }
        let msg = self
            .reader
            .messages
            .get(message_index as usize)
            .ok_or_else(|| {
                napi::Error::from_reason(format!(
                    "message index {message_index} out of range (have {})",
                    self.reader.messages.len()
                ))
            })?;
        let mut out = self.bytes.clone();
        let off = msg.pds_p1_offset();
        out[off] = value as u8;
        Ok(out.into())
    }

    /// Compose decode + reprojection warp + viridis colormap into a
    /// paint-ready RGBA buffer.
    #[napi]
    pub fn render_grid(
        &self,
        message_index: u32,
        options: RenderOptions,
    ) -> napi::Result<RenderedGrid> {
        let meta = self.message_meta(message_index)?;
        let raw = self.cached_decode(message_index)?;
        render_with_options(&meta, raw.as_ref(), &options)
    }

    /// Render `message_index_a` combined element-wise with `message_index_b`
    /// under `op` (see [`CombineOp`]) — the difference-map workflow. Both
    /// messages must sit on the same grid; the result renders through the
    /// normal pipeline (projection, overlays, palette, scaling, bounds) against
    /// the primary message's geometry.
    #[napi]
    pub fn render_grid_combined(
        &self,
        message_index_a: u32,
        message_index_b: u32,
        op: String,
        options: RenderOptions,
    ) -> napi::Result<RenderedGrid> {
        let op = parse_combine_op(&op)?;
        let meta_a = self.message_meta(message_index_a)?;
        let meta_b = self.message_meta(message_index_b)?;
        let raw_a = self.cached_decode(message_index_a)?;
        let raw_b = self.cached_decode(message_index_b)?;
        render_combined(
            &meta_a,
            raw_a.as_ref(),
            &meta_b,
            raw_b.as_ref(),
            op,
            &options,
        )
    }

    /// Project geographic polylines (coastline / graticule / user shapes)
    /// onto the same raster `render_grid` produces for these `options`,
    /// returning pixel-space runs for the overlay layer (#72). Geometry-only:
    /// it never decodes values, so toggling the overlay never re-decodes.
    #[napi]
    pub fn project_overlay(
        &self,
        message_index: u32,
        options: RenderOptions,
        latlon: napi::bindgen_prelude::Float64Array,
        ring_lengths: napi::bindgen_prelude::Uint32Array,
    ) -> napi::Result<ProjectedOverlay> {
        let meta = self
            .reader
            .messages
            .get(message_index as usize)
            .map(|msg| {
                let packing = self
                    .reader
                    .packing_label(message_index as usize)
                    .map(friendly_packing);
                build_grib1_message_meta(msg, packing)
            })
            .ok_or_else(|| {
                napi::Error::from_reason(format!("message index {message_index} out of range"))
            })?;
        project_overlay_impl(&meta, &options, latlon.as_ref(), ring_lengths.as_ref())
            .map(ProjectedOverlay::from_polylines)
    }

    /// Extract contour isolines from this message and project them onto the same
    /// raster `render_grid` produces, as pixel-space runs for the overlay canvas
    /// (#238). `interval` sets a manual level spacing; `None` picks ~8 nice
    /// levels over the used range. Errors for grid types whose forward
    /// geolocation isn't wired yet (projected + reduced grids).
    #[napi]
    pub fn project_contours(
        &self,
        message_index: u32,
        options: RenderOptions,
        interval: Option<f64>,
    ) -> napi::Result<ProjectedOverlay> {
        let meta = self.message_meta(message_index)?;
        let raw = self.cached_decode(message_index)?;
        project_contours_impl(&meta, raw.as_ref(), &options, interval)
            .map(ProjectedOverlay::from_polylines)
    }

    /// Read the field under a rendered pixel (#172): the point-probe readout.
    /// `(px, py)` are output-raster pixels (post-flip). `None` when the pixel is
    /// off the raster or off the globe.
    #[napi]
    pub fn probe(
        &self,
        message_index: u32,
        options: RenderOptions,
        px: u32,
        py: u32,
    ) -> napi::Result<Option<ProbeResult>> {
        let meta = self.message_meta(message_index)?;
        let raw = self.cached_decode(message_index)?;
        probe_impl(&meta, raw.as_ref(), &options, px, py)
    }
}

impl Grib1Handle {
    /// Build one message's [`MessageMeta`], or an out-of-range error. The one
    /// place `render_grid` / `render_grid_combined` / `project_overlay` get a
    /// message's geometry, so they can't disagree.
    fn message_meta(&self, message_index: u32) -> napi::Result<MessageMeta> {
        self.reader
            .messages
            .get(message_index as usize)
            .map(|msg| {
                let packing = self
                    .reader
                    .packing_label(message_index as usize)
                    .map(friendly_packing);
                build_grib1_message_meta(msg, packing)
            })
            .ok_or_else(|| {
                napi::Error::from_reason(format!("message index {message_index} out of range"))
            })
    }

    /// Get-or-build the decoded values for `message_index`. The cache is
    /// invalidated implicitly when the handle is dropped (which happens
    /// in `provider.ts` as soon as the document bytes change via setP1
    /// or the document closes), so we never have to worry about it
    /// going stale.
    fn cached_decode(&self, message_index: u32) -> napi::Result<std::sync::Arc<Vec<Option<f64>>>> {
        if let Some(hit) = self
            .decoded
            .lock()
            .expect("decode cache mutex poisoned")
            .get(&message_index)
        {
            return Ok(std::sync::Arc::clone(hit));
        }
        let raw = self
            .reader
            .decode_message_values(message_index as usize)
            .map_err(|e| napi::Error::from_reason(e.to_string()))?;
        // Reduced grids decode to sum(PL) points laid out row by row. Expand
        // each row to the widest-row width here, at the single decode boundary,
        // so every downstream path (decode_grid, render, overlay) sees a
        // regular Ni·Nj field and the `width·height == len` invariant holds.
        let raw = match self
            .reader
            .messages
            .get(message_index as usize)
            .and_then(|m| m.gds.as_ref())
        {
            Some(gds) => match (gds.points_per_row(), gds.dimensions()) {
                (Some(pl), Some((width, _))) => {
                    fieldglass_grib1::expand_reduced_to_regular(&raw, pl, width as usize)
                }
                _ => raw,
            },
            None => raw,
        };
        let arc = std::sync::Arc::new(raw);
        self.decoded
            .lock()
            .expect("decode cache mutex poisoned")
            .insert(message_index, std::sync::Arc::clone(&arc));
        Ok(arc)
    }
}

/// Persistent GRIB2 reader handle, sibling to [`Grib1Handle`].
///
/// Unlike `Grib1Handle`, this struct doesn't store the original bytes —
/// GRIB2 has no in-place edit path yet (the metadata-editing track is a
/// separate workstream). The `decoded` cache mirrors `Grib1Handle`'s
/// rationale: subsequent `render_grid` calls with different
/// `RenderOptions` re-paint without re-running the bit-unpack +
/// bitmap-merge step.
#[napi]
pub struct Grib2Handle {
    reader: Grib2Reader,
    decoded: Mutex<std::collections::HashMap<u32, std::sync::Arc<Vec<Option<f64>>>>>,
}

#[napi]
impl Grib2Handle {
    #[napi(factory)]
    pub fn from_bytes(bytes: napi::bindgen_prelude::Buffer) -> napi::Result<Self> {
        let reader = Grib2Reader::from_bytes(bytes.to_vec())
            .map_err(|e| napi::Error::from_reason(e.to_string()))?;
        Ok(Self {
            reader,
            decoded: Mutex::new(std::collections::HashMap::new()),
        })
    }

    #[napi]
    pub fn messages(&self) -> Vec<MessageMeta> {
        self.reader
            .messages
            .iter()
            .map(build_grib2_message_meta)
            .collect()
    }

    #[napi]
    pub fn decode_grid(&self, message_index: u32) -> napi::Result<DecodedGrid> {
        let raw = self.cached_decode(message_index)?;
        let msg = self
            .reader
            .messages
            .get(message_index as usize)
            .ok_or_else(|| napi::Error::from_reason("message index out of range".to_string()))?;
        let (ni, nj) = msg.gds.dimensions().ok_or_else(|| {
            napi::Error::from_reason("grid has no declared dimensions".to_string())
        })?;
        Ok(decoded_grid_from(&raw, ni, nj))
    }

    #[napi]
    pub fn render_grid(
        &self,
        message_index: u32,
        options: RenderOptions,
    ) -> napi::Result<RenderedGrid> {
        let meta = self.message_meta(message_index)?;
        let raw = self.cached_decode(message_index)?;
        render_with_options(&meta, raw.as_ref(), &options)
    }

    /// Render `message_index_a` combined element-wise with `message_index_b`
    /// under `op` (see [`CombineOp`]) — the difference-map workflow. Sibling to
    /// [`Grib1Handle::render_grid_combined`]; both messages must sit on the same
    /// grid, and the result renders through the normal pipeline against the
    /// primary message's geometry.
    #[napi]
    pub fn render_grid_combined(
        &self,
        message_index_a: u32,
        message_index_b: u32,
        op: String,
        options: RenderOptions,
    ) -> napi::Result<RenderedGrid> {
        let op = parse_combine_op(&op)?;
        let meta_a = self.message_meta(message_index_a)?;
        let meta_b = self.message_meta(message_index_b)?;
        let raw_a = self.cached_decode(message_index_a)?;
        let raw_b = self.cached_decode(message_index_b)?;
        render_combined(
            &meta_a,
            raw_a.as_ref(),
            &meta_b,
            raw_b.as_ref(),
            op,
            &options,
        )
    }

    /// Project geographic polylines onto the same raster `render_grid`
    /// produces for these `options` (#72). Sibling to [`Grib1Handle::project_overlay`].
    #[napi]
    pub fn project_overlay(
        &self,
        message_index: u32,
        options: RenderOptions,
        latlon: napi::bindgen_prelude::Float64Array,
        ring_lengths: napi::bindgen_prelude::Uint32Array,
    ) -> napi::Result<ProjectedOverlay> {
        let meta = self
            .reader
            .messages
            .get(message_index as usize)
            .map(build_grib2_message_meta)
            .ok_or_else(|| {
                napi::Error::from_reason(format!("message index {message_index} out of range"))
            })?;
        project_overlay_impl(&meta, &options, latlon.as_ref(), ring_lengths.as_ref())
            .map(ProjectedOverlay::from_polylines)
    }

    /// Contour isolines for this message, projected onto the render raster.
    /// Sibling to [`Grib1Handle::project_contours`].
    #[napi]
    pub fn project_contours(
        &self,
        message_index: u32,
        options: RenderOptions,
        interval: Option<f64>,
    ) -> napi::Result<ProjectedOverlay> {
        let meta = self.message_meta(message_index)?;
        let raw = self.cached_decode(message_index)?;
        project_contours_impl(&meta, raw.as_ref(), &options, interval)
            .map(ProjectedOverlay::from_polylines)
    }

    /// Point-probe readout for this message. Sibling to
    /// [`Grib1Handle::probe`].
    #[napi]
    pub fn probe(
        &self,
        message_index: u32,
        options: RenderOptions,
        px: u32,
        py: u32,
    ) -> napi::Result<Option<ProbeResult>> {
        let meta = self.message_meta(message_index)?;
        let raw = self.cached_decode(message_index)?;
        probe_impl(&meta, raw.as_ref(), &options, px, py)
    }
}

impl Grib2Handle {
    /// Build one message's [`MessageMeta`], or an out-of-range error — the
    /// single geometry source for `render_grid` / `render_grid_combined`.
    fn message_meta(&self, message_index: u32) -> napi::Result<MessageMeta> {
        self.reader
            .messages
            .get(message_index as usize)
            .map(build_grib2_message_meta)
            .ok_or_else(|| {
                napi::Error::from_reason(format!("message index {message_index} out of range"))
            })
    }

    fn cached_decode(&self, message_index: u32) -> napi::Result<std::sync::Arc<Vec<Option<f64>>>> {
        if let Some(hit) = self
            .decoded
            .lock()
            .expect("decode cache mutex poisoned")
            .get(&message_index)
        {
            return Ok(std::sync::Arc::clone(hit));
        }
        let mut raw = self
            .reader
            .decode_message_values(message_index as usize)
            .map_err(|e| napi::Error::from_reason(e.to_string()))?;
        // Undo alternate-row (boustrophedon) scanning here, at the single decode
        // boundary, so every downstream path (decode_grid, render, overlay) sees
        // a regular Ni·Nj raster the projector can address as `raw[j·ni + i]`.
        // GRIB2 Flag Table 3.4 bit 4 (0x10) marks it; NBM's Lambert 2 m field is
        // the real-world case, and the decode is otherwise correct — the scan
        // sign flags fold into the projection, but the alternate-row flip cannot.
        // Only the common i-consecutive layout (bit 3 0x20 clear) is handled.
        if let Some(gds) = self
            .reader
            .messages
            .get(message_index as usize)
            .map(|m| &m.gds)
            && let (Some(sm), Some((ni, _nj))) = (gds.scanning_mode(), gds.dimensions())
            && sm & fieldglass_grib2::SCAN_ALTERNATE_ROWS != 0
            && sm & fieldglass_grib2::SCAN_J_CONSECUTIVE == 0
        {
            fieldglass_grib2::undo_alternate_rows(&mut raw, ni as usize);
        }
        let arc = std::sync::Arc::new(raw);
        self.decoded
            .lock()
            .expect("decode cache mutex poisoned")
            .insert(message_index, std::sync::Arc::clone(&arc));
        Ok(arc)
    }
}

// ---------------------------------------------------------------------------
// NetCDF 2-D slice rendering  (#122; decision 0002)
// ---------------------------------------------------------------------------

/// One axis (dimension) of a renderable NetCDF variable, for the slice picker's
/// index controls.
#[napi(object)]
pub struct NetcdfAxis {
    pub name: String,
    /// Runtime length (the unlimited/record dimension resolves to its count).
    pub length: f64,
}

/// A NetCDF variable the render panel can draw, with its dimensions and the
/// CF-detected horizontal-axis positions. `detectedYDim` / `detectedXDim` are
/// the axis indices (into `dims`) the picker pre-fills the Y / X selectors with;
/// `null` means detection found no matching coordinate variable and the user
/// must assign that axis by hand.
#[napi(object)]
pub struct NetcdfVariableMeta {
    /// Index into the reader's decode order — pass back as `variableIndex`.
    pub variable_index: i32,
    pub name: String,
    pub nc_type: String,
    pub dims: Vec<NetcdfAxis>,
    pub detected_y_dim: Option<i32>,
    pub detected_x_dim: Option<i32>,
}

/// Persistent NetCDF reader handle, sibling to [`Grib1Handle`]/[`Grib2Handle`].
///
/// A NetCDF variable is N-D, so rendering needs a slice picker (which 2-D plane)
/// and synthesised geometry (NetCDF carries no GRIB-style GDS). The handle parses
/// once, caches each decoded variable (including the lat/lon coordinate
/// variables it reads for the corners), and feeds a synthesised `"latlon"`
/// [`MessageMeta`] through the same [`render_with_options`] / [`project_overlay_impl`]
/// the GRIB handles use — honouring the decode-decoupled rule.
///
/// Covers both backings — classic (CDF-1/2/5) and NetCDF-4 / HDF5 (#169) — for
/// regular 1-D lat/lon grids (decision 0002). Curvilinear / projected grids
/// (#168) are a tracked follow-up; for an HDF5 layout whose metadata can't be
/// resolved, `variables()` is empty and the panel falls back to the metadata view.
#[napi]
pub struct NetcdfHandle {
    reader: NetcdfReader,
    view: DatasetView,
    decoded: Mutex<std::collections::HashMap<usize, std::sync::Arc<Vec<Option<f64>>>>>,
}

#[napi]
impl NetcdfHandle {
    #[napi(factory)]
    pub fn from_bytes(bytes: napi::bindgen_prelude::Buffer) -> napi::Result<Self> {
        let reader = NetcdfReader::from_bytes(bytes.to_vec())
            .map_err(|e| napi::Error::from_reason(e.to_string()))?;
        // Both backings build the neutral view that drives the slice picker. The
        // HDF5 metadata is resolved lazily and can fail for a layout outside the
        // decoded subset (decision 0003); on failure the view is empty and the
        // panel falls back to the metadata dump, mirroring `open_netcdf`.
        let view = match &reader.backing {
            NetcdfBacking::Classic(h) => DatasetView::from_classic(h),
            NetcdfBacking::Hdf5(_) => reader
                .hdf5_metadata()
                .map(|meta| DatasetView::from_hdf5(&meta))
                .unwrap_or_else(|_| DatasetView {
                    dims: Vec::new(),
                    vars: Vec::new(),
                    global_attrs: Vec::new(),
                }),
        };
        Ok(Self {
            reader,
            view,
            decoded: Mutex::new(std::collections::HashMap::new()),
        })
    }

    /// The variables the picker can draw (numeric, ≥ 2-D, not coordinate
    /// variables), each with its dimensions and detected horizontal axes.
    #[napi]
    pub fn variables(&self) -> Vec<NetcdfVariableMeta> {
        self.view
            .renderable_variables()
            .into_iter()
            .map(|v| NetcdfVariableMeta {
                variable_index: v.decode_index as i32,
                name: v.name,
                nc_type: v.nc_type.name().to_string(),
                dims: v
                    .dims
                    .iter()
                    .map(|d| NetcdfAxis {
                        name: d.name.clone(),
                        length: d.length as f64,
                    })
                    .collect(),
                detected_y_dim: v.detected_y_dim.map(|p| p as i32),
                detected_x_dim: v.detected_x_dim.map(|p| p as i32),
            })
            .collect()
    }

    /// Render one 2-D slice of a variable. `yDim` / `xDim` are axis indices into
    /// the variable's dimensions (the image's vertical / horizontal axes);
    /// `sliceIndices` holds the fixed index for every dimension (its entries for
    /// `yDim` / `xDim` are ignored). The picked plane is fed through the shared
    /// warp as a synthesised `"latlon"` grid.
    #[napi]
    pub fn render_slice(
        &self,
        variable_index: u32,
        y_dim: u32,
        x_dim: u32,
        slice_indices: Vec<u32>,
        options: RenderOptions,
    ) -> napi::Result<RenderedGrid> {
        let var = self.renderable(variable_index)?;
        let (y, x) = (y_dim as usize, x_dim as usize);
        let plane = self.slice_plane(&var, y, x, &slice_indices)?;
        let meta = self.slice_meta(&var, y, x)?;
        render_with_options(&meta, &plane, &options)
    }

    /// Render one slice combined element-wise with a second slice under `op`
    /// (see [`CombineOp`]) — the difference-map workflow (#239). Field B is a
    /// slice of `variableIndexB` at `sliceIndicesB`, sharing the same image axes
    /// (`yDim` / `xDim`) as field A; the common case is two time steps of one
    /// variable (`variableIndexB == variableIndexA`, different indices). Both
    /// slices must resolve to the same grid; the combined field renders through
    /// the normal pipeline against field A's geometry.
    #[napi]
    #[allow(clippy::too_many_arguments)]
    pub fn render_slice_combined(
        &self,
        variable_index_a: u32,
        y_dim: u32,
        x_dim: u32,
        slice_indices_a: Vec<u32>,
        variable_index_b: u32,
        slice_indices_b: Vec<u32>,
        op: String,
        options: RenderOptions,
    ) -> napi::Result<RenderedGrid> {
        let op = parse_combine_op(&op)?;
        let (y, x) = (y_dim as usize, x_dim as usize);
        let var_a = self.renderable(variable_index_a)?;
        let var_b = self.renderable(variable_index_b)?;
        let plane_a = self.slice_plane(&var_a, y, x, &slice_indices_a)?;
        let plane_b = self.slice_plane(&var_b, y, x, &slice_indices_b)?;
        let meta_a = self.slice_meta(&var_a, y, x)?;
        let meta_b = self.slice_meta(&var_b, y, x)?;
        render_combined(&meta_a, &plane_a, &meta_b, &plane_b, op, &options)
    }

    /// Project geographic polylines onto the same raster `render_slice` produces
    /// for these `options`. Geometry-only — independent of the slice indices, so
    /// it takes only the axis assignment. Sibling to [`Grib2Handle::project_overlay`].
    #[napi]
    pub fn project_overlay(
        &self,
        variable_index: u32,
        y_dim: u32,
        x_dim: u32,
        options: RenderOptions,
        latlon: napi::bindgen_prelude::Float64Array,
        ring_lengths: napi::bindgen_prelude::Uint32Array,
    ) -> napi::Result<ProjectedOverlay> {
        let var = self.renderable(variable_index)?;
        let meta = self.slice_meta(&var, y_dim as usize, x_dim as usize)?;
        project_overlay_impl(&meta, &options, latlon.as_ref(), ring_lengths.as_ref())
            .map(ProjectedOverlay::from_polylines)
    }

    /// Contour isolines for one slice, projected onto the render raster (#238).
    /// Sibling to [`Grib1Handle::project_contours`]; the slice's synthesised
    /// `latlon` geometry means NetCDF grids are always contourable.
    #[napi]
    pub fn project_contours(
        &self,
        variable_index: u32,
        y_dim: u32,
        x_dim: u32,
        slice_indices: Vec<u32>,
        options: RenderOptions,
        interval: Option<f64>,
    ) -> napi::Result<ProjectedOverlay> {
        let var = self.renderable(variable_index)?;
        let (y, x) = (y_dim as usize, x_dim as usize);
        let plane = self.slice_plane(&var, y, x, &slice_indices)?;
        let meta = self.slice_meta(&var, y, x)?;
        project_contours_impl(&meta, &plane, &options, interval)
            .map(ProjectedOverlay::from_polylines)
    }

    /// Point-probe readout for one slice (#172). Sibling to
    /// [`Grib1Handle::probe`].
    #[napi]
    #[allow(clippy::too_many_arguments)]
    pub fn probe(
        &self,
        variable_index: u32,
        y_dim: u32,
        x_dim: u32,
        slice_indices: Vec<u32>,
        options: RenderOptions,
        px: u32,
        py: u32,
    ) -> napi::Result<Option<ProbeResult>> {
        let var = self.renderable(variable_index)?;
        let (y, x) = (y_dim as usize, x_dim as usize);
        let plane = self.slice_plane(&var, y, x, &slice_indices)?;
        let meta = self.slice_meta(&var, y, x)?;
        probe_impl(&meta, &plane, &options, px, py)
    }
}

impl NetcdfHandle {
    /// Look up a renderable variable by its decode index.
    fn renderable(&self, variable_index: u32) -> napi::Result<RenderableVariable> {
        self.view
            .renderable_variables()
            .into_iter()
            .find(|v| v.decode_index == variable_index as usize)
            .ok_or_else(|| {
                napi::Error::from_reason(format!(
                    "variable index {variable_index} is not a renderable NetCDF variable"
                ))
            })
    }

    /// Decode-and-cache one variable's values by decode index.
    fn cached_decode(&self, index: usize) -> napi::Result<std::sync::Arc<Vec<Option<f64>>>> {
        if let Some(hit) = self
            .decoded
            .lock()
            .expect("decode cache mutex poisoned")
            .get(&index)
        {
            return Ok(std::sync::Arc::clone(hit));
        }
        let raw = self
            .reader
            .decode_variable_values(index)
            .map_err(|e| napi::Error::from_reason(e.to_string()))?;
        let arc = std::sync::Arc::new(raw);
        self.decoded
            .lock()
            .expect("decode cache mutex poisoned")
            .insert(index, std::sync::Arc::clone(&arc));
        Ok(arc)
    }

    /// Extract the chosen 2-D plane from the (cached) decoded variable and
    /// unpack it to physical units. The decode returns raw on-disk codes with
    /// only `_FillValue` masked; [`unpack_cf_data`] then applies the CF
    /// `valid_range` mask and `scale_factor` / `add_offset`, so a packed CF
    /// field (scaled `int16`, as GOES / MERRA-2 / ERA5 store it) renders and
    /// labels in real units rather than integer codes (#184). Decode stays
    /// decoupled from rendering; the same unpacking serves both backings.
    fn slice_plane(
        &self,
        var: &RenderableVariable,
        y_dim: usize,
        x_dim: usize,
        slice_indices: &[u32],
    ) -> napi::Result<Vec<Option<f64>>> {
        let values = self.cached_decode(var.decode_index)?;
        let shape = self
            .reader
            .variable_shape(var.decode_index)
            .map_err(|e| napi::Error::from_reason(e.to_string()))?;
        if slice_indices.len() != shape.len() {
            return Err(napi::Error::from_reason(format!(
                "sliceIndices length {} does not match variable rank {}",
                slice_indices.len(),
                shape.len()
            )));
        }
        let fixed: Vec<usize> = slice_indices.iter().map(|&i| i as usize).collect();
        let plane = extract_plane(values.as_ref(), &shape, y_dim, x_dim, &fixed)
            .map_err(|e| napi::Error::from_reason(e.to_string()))?;
        let attrs = self
            .view
            .vars
            .iter()
            .find(|v| v.decode_index == var.decode_index)
            .map(|v| v.attrs.clone())
            .unwrap_or_default();
        Ok(unpack_cf_data(&plane, &attrs))
    }

    /// Synthesise the `"latlon"` [`MessageMeta`] for a slice from the variable's
    /// coordinate arrays. With both lat and lon coordinate variables present the
    /// grid reprojects; without them the grid is "assumed" and only the source
    /// projection is offered (no geographic corners).
    fn slice_meta(
        &self,
        var: &RenderableVariable,
        y_dim: usize,
        x_dim: usize,
    ) -> napi::Result<MessageMeta> {
        let y_axis = var.dims.get(y_dim).ok_or_else(|| {
            napi::Error::from_reason(format!("yDim {y_dim} out of range for {}", var.name))
        })?;
        let x_axis = var.dims.get(x_dim).ok_or_else(|| {
            napi::Error::from_reason(format!("xDim {x_dim} out of range for {}", var.name))
        })?;
        if y_dim == x_dim {
            return Err(napi::Error::from_reason(
                "the X and Y axes must be different dimensions".to_string(),
            ));
        }

        let units = self
            .view
            .vars
            .iter()
            .find(|v| v.decode_index == var.decode_index)
            .and_then(|v| {
                v.attrs
                    .iter()
                    .find(|(n, _)| n == "units")
                    .map(|(_, val)| val.clone())
            })
            .unwrap_or_default();
        let (ni, nj) = (x_axis.length as u32, y_axis.length as u32);

        // A projected grid (WRF / GOES geostationary, decision 0004) takes
        // precedence over the regular lat/lon path: its `x`/`y` coordinate
        // variables are scan angles or projected metres, not degrees, so the
        // lat/lon synthesis would mis-georeference them.
        if let Some(meta) =
            self.try_wrf_projected(&var.name, &units, &y_axis.name, &x_axis.name, ni, nj)?
        {
            return Ok(meta);
        }

        // CF `grid_mapping`: a data variable in a projected CRS names a
        // grid_mapping variable. We resolve `geostationary`; a plain
        // `latitude_longitude` mapping falls through to the lat/lon path. Any
        // other (projected) mapping we don't handle yet must fall back to
        // **source projection only** — never let the lat/lon synthesis treat its
        // projected `x`/`y` as degrees and mis-georeference (decision 0004
        // guardrail).
        let source_only = || synth_latlon_meta(&var.name, &units, ni as i32, nj as i32, None);
        if let Some(gm_attrs) = self.data_grid_mapping_attrs(var) {
            let mapping_name = gm_attrs
                .iter()
                .find(|(n, _)| n == "grid_mapping_name")
                .map(|(_, v)| v.trim());
            match classify_cf_mapping(mapping_name) {
                CfMapping::Geostationary => {
                    let x = self.coordinate_values_for_dim(&x_axis.name)?;
                    let y = self.coordinate_values_for_dim(&y_axis.name)?;
                    return Ok(match (x, y) {
                        (Some(x), Some(y)) => resolve_cf_geostationary(&gm_attrs, &x, &y)
                            .map(|g| synth_geostationary_meta(&var.name, &units, &g))
                            .unwrap_or_else(source_only),
                        _ => source_only(),
                    });
                }
                CfMapping::LatLon => {} // fall through to the lat/lon path
                CfMapping::Unsupported => return Ok(source_only()),
            }
        }

        // Regular 1-D lat/lon grid (decision 0002): corners from the coordinate
        // arrays, or an assumed source-only grid when they are absent. We reach
        // here only when no projection resolved — either no `grid_mapping` (CF
        // mandates one for a projected CRS, so its absence implies geographic
        // coordinates) or an explicit `latitude_longitude` mapping.
        let lat_idx = self.view.coordinate_index(&y_axis.name);
        let lon_idx = self.view.coordinate_index(&x_axis.name);
        let geometry = match (lat_idx, lon_idx) {
            (Some(lat_i), Some(lon_i)) => {
                let lat = self.coordinate_values(lat_i)?;
                let lon = self.coordinate_values(lon_i)?;
                Some(
                    synthesize_geometry(&lat, &lon)
                        .map_err(|e| napi::Error::from_reason(e.to_string()))?,
                )
            }
            _ => None,
        };
        Ok(synth_latlon_meta(
            &var.name, &units, ni as i32, nj as i32, geometry,
        ))
    }

    /// Resolve a WRF projected grid (decision 0004, #220) when the file carries
    /// WRF's `MAP_PROJ` global attributes and the 2-D `XLAT`/`XLONG` arrays
    /// whose `(0, 0)` corner fixes the grid origin: Lambert (`MAP_PROJ = 1`),
    /// polar stereographic (`2`), Mercator (`3`), or unrotated lat-lon (`6`,
    /// `POLE_LAT = 90`). `None` for any non-WRF file (no `XLAT`/`XLONG`) or an
    /// unresolved projection (e.g. a *rotated* `MAP_PROJ = 6` domain, whose
    /// WRF → GRIB2 §3.1 pole mapping is not cleanly documented, so it stays
    /// source-only).
    ///
    /// `XLAT`/`XLONG` must span the selected horizontal axes (their trailing two
    /// dimensions = `y_name`, `x_name`), so a file that merely *names* variables
    /// `XLAT`/`XLONG`, or a WRF field rendered on an unexpected axis pick, doesn't
    /// resolve a grid whose `(ni, nj)` mismatch the coordinate arrays.
    fn try_wrf_projected(
        &self,
        name: &str,
        units: &str,
        y_name: &str,
        x_name: &str,
        ni: u32,
        nj: u32,
    ) -> napi::Result<Option<MessageMeta>> {
        let (Some(xlat), Some(xlong)) = (self.var_named("XLAT"), self.var_named("XLONG")) else {
            return Ok(None);
        };
        if !dims_end_with(&xlat.dim_names, y_name, x_name)
            || !dims_end_with(&xlong.dim_names, y_name, x_name)
        {
            return Ok(None);
        }
        // Match MAP_PROJ before touching XLAT/XLONG values: a file whose
        // projection we don't resolve keeps its source-only fallback even when
        // a corner cell is masked, and pays no coordinate decode. A masked
        // corner on a projection we *do* resolve stays a hard error rather
        // than silently mis-georeferencing (decision 0004 guardrail).
        let global = &self.view.global_attrs;
        let Some(map_proj) = wrf_map_proj(global) else {
            return Ok(None);
        };
        let lat_first = self.grid_corner_value(xlat.decode_index, "XLAT", 0, 0, ni)?;
        let lon_first = self.grid_corner_value(xlong.decode_index, "XLONG", 0, 0, ni)?;
        Ok(match map_proj {
            WrfMapProj::Lambert => resolve_wrf_lambert(global, lat_first, lon_first, ni, nj)
                .map(|g| synth_lambert_meta(name, units, &g)),
            WrfMapProj::PolarStereo => {
                resolve_wrf_polar_stereo(global, lat_first, lon_first, ni, nj)
                    .map(|g| synth_polar_stereo_meta(name, units, &g))
            }
            // Mercator and (unrotated) lat-lon are corner-pinned, so they alone
            // also need the far corner.
            WrfMapProj::Mercator => {
                let (lat_last, lon_last) = self.grid_far_corner(xlat, xlong, ni, nj)?;
                resolve_wrf_mercator(global, lat_first, lon_first, lat_last, lon_last, ni, nj)
                    .map(|g| synth_mercator_meta(name, units, &g))
            }
            WrfMapProj::LatLon => {
                let (lat_last, lon_last) = self.grid_far_corner(xlat, xlong, ni, nj)?;
                resolve_wrf_latlon(global, lat_first, lon_first, lat_last, lon_last, ni, nj)
                    .map(|g| synth_wrf_latlon_meta(name, units, &g))
            }
        })
    }

    /// The attributes of the `grid_mapping` variable a data variable points at,
    /// or `None` when the data variable declares no `grid_mapping` (or it names a
    /// variable that isn't present).
    fn data_grid_mapping_attrs(&self, var: &RenderableVariable) -> Option<Vec<(String, String)>> {
        let gm_name = self.var_attr(var.decode_index, "grid_mapping")?;
        self.view
            .vars
            .iter()
            .find(|v| v.name == gm_name)
            .map(|gm| gm.attrs.clone())
    }

    /// The CF-scaled coordinate values of a dimension's coordinate variable, or
    /// `None` when the dimension has no coordinate variable.
    fn coordinate_values_for_dim(&self, dim_name: &str) -> napi::Result<Option<Vec<f64>>> {
        match self.view.coordinate_index(dim_name) {
            Some(i) => Ok(Some(self.coordinate_values(i)?)),
            None => Ok(None),
        }
    }

    /// A variable looked up by name (any variable, including the 2-D
    /// `XLAT`/`XLONG` and the scalar `grid_mapping` carriers — not just
    /// renderable ones).
    fn var_named(&self, name: &str) -> Option<&fieldglass_netcdf::VarView> {
        self.view.vars.iter().find(|v| v.name == name)
    }

    /// A variable's attribute value by name, looked up by decode index.
    fn var_attr(&self, decode_index: usize, attr: &str) -> Option<String> {
        self.view
            .vars
            .iter()
            .find(|v| v.decode_index == decode_index)?
            .attrs
            .iter()
            .find(|(n, _)| n == attr)
            .map(|(_, v)| v.clone())
    }

    /// The geographic `(lat, lon)` of the far grid corner (`XLAT`/`XLONG` at the
    /// last scanned point, `[nj-1, ni-1]`) — the second corner the corner-pinned
    /// WRF grids (Mercator, unrotated lat-lon) need beyond the origin.
    fn grid_far_corner(
        &self,
        xlat: &fieldglass_netcdf::VarView,
        xlong: &fieldglass_netcdf::VarView,
        ni: u32,
        nj: u32,
    ) -> napi::Result<(f64, f64)> {
        let (j, i) = (nj.saturating_sub(1), ni.saturating_sub(1));
        let lat_last = self.grid_corner_value(xlat.decode_index, "XLAT", j, i, ni)?;
        let lon_last = self.grid_corner_value(xlong.decode_index, "XLONG", j, i, ni)?;
        Ok((lat_last, lon_last))
    }

    /// The value of a 2-D coordinate field at `[j, i]` of the first time step
    /// (flat index `j·ni + i` in C order; the caller has already checked the
    /// variable's trailing two dimensions are the horizontal axes), with the
    /// variable's CF `scale_factor` / `add_offset` applied — a packed
    /// coordinate decodes like the 1-D GOES axes do. A masked corner is a hard
    /// error rather than silently shifting the corner to the next present cell
    /// and mis-georeferencing the whole grid.
    fn grid_corner_value(
        &self,
        index: usize,
        name: &str,
        j: u32,
        i: u32,
        ni: u32,
    ) -> napi::Result<f64> {
        let raw = self
            .cached_decode(index)?
            .get(j as usize * ni as usize + i as usize)
            .copied()
            .flatten()
            .ok_or_else(|| {
                napi::Error::from_reason(format!("{name}[{j},{i}] is missing or masked"))
            })?;
        let (scale, offset) = self
            .view
            .vars
            .iter()
            .find(|v| v.decode_index == index)
            .map(|v| cf_scale_offset(&v.attrs))
            .unwrap_or((1.0, 0.0));
        Ok(raw * scale + offset)
    }

    /// Decode a coordinate variable to a dense `Vec<f64>`, applying CF
    /// `scale_factor` / `add_offset` (GOES stores `x`/`y` as scaled `int16`).
    /// Coordinate axes are never masked, so a fill value there is a hard error.
    fn coordinate_values(&self, index: usize) -> napi::Result<Vec<f64>> {
        let raw: Vec<f64> = self
            .cached_decode(index)?
            .iter()
            .map(|v| {
                v.ok_or_else(|| {
                    napi::Error::from_reason(
                        "coordinate variable contains a fill value".to_string(),
                    )
                })
            })
            .collect::<napi::Result<_>>()?;
        let attrs = self
            .view
            .vars
            .iter()
            .find(|v| v.decode_index == index)
            .map(|v| v.attrs.clone())
            .unwrap_or_default();
        Ok(apply_scale_offset(&raw, &attrs))
    }
}

/// Whether a variable's ordered dimension names end with `y` then `x` — i.e. its
/// trailing two axes are the selected horizontal ones (the WRF `XLAT`/`XLONG`
/// arrays span `(…, south_north, west_east)`).
fn dims_end_with(dims: &[String], y: &str, x: &str) -> bool {
    matches!(dims, [.., dy, dx] if dy == y && dx == x)
}

/// How a data variable's CF `grid_mapping_name` routes through slice synthesis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CfMapping {
    /// `geostationary` — resolve through the geostationary projector.
    Geostationary,
    /// `latitude_longitude` — a plain lat/lon CRS; use the coordinate-array path.
    LatLon,
    /// Any other (projected) mapping we don't handle yet. Falls back to source
    /// projection only; the projected `x`/`y` must never be read as degrees
    /// (decision 0004 guardrail).
    Unsupported,
}

/// Classify a CF `grid_mapping_name`. A missing name (`None`) is treated as
/// `latitude_longitude`: a data variable can carry a `grid_mapping` attribute
/// pointing at a variable that omits the name, and the safe default for an
/// unprojected file is the lat/lon path.
fn classify_cf_mapping(name: Option<&str>) -> CfMapping {
    match name {
        Some("geostationary") => CfMapping::Geostationary,
        Some("latitude_longitude") | None => CfMapping::LatLon,
        Some(_) => CfMapping::Unsupported,
    }
}

/// A blank NetCDF [`MessageMeta`] carrying only the painted plane's identity and
/// dimensions; every projection-specific field is `None` and the grid is not yet
/// reprojectable. The per-grid-type builders below fill in the geometry the warp
/// reads. `ni` = x-axis length, `nj` = y-axis length; these stay decoupled from
/// the coordinate-derived corners so a coordinate array whose length disagrees
/// with its dimension can't desync the raster from its declared size.
fn base_netcdf_meta(name: &str, units: &str, ni: i32, nj: i32) -> MessageMeta {
    MessageMeta {
        earth_radius_metres: None,
        message_index: 0,
        offset_bytes: 0,
        parameter_name: name.to_string(),
        parameter_units: units.to_string(),
        parameter_abbreviation: name.to_string(),
        level: String::new(),
        level_type: String::new(),
        reference_time: String::new(),
        forecast_hours: 0,
        forecast_display: String::new(),
        originating_centre: String::new(),
        grid_type: None,
        grid_ni: Some(ni),
        grid_nj: Some(nj),
        lat_first: None,
        lon_first: None,
        lat_last: None,
        lon_last: None,
        format: "netcdf".to_string(),
        edition: None,
        discipline: None,
        total_length_bytes: None,
        production_status: None,
        data_type: None,
        lambert_lad: None,
        lambert_lov: None,
        lambert_dx_metres: None,
        lambert_dy_metres: None,
        lambert_latin1: None,
        lambert_latin2: None,
        gaussian_n_parallels: None,
        polar_stereo_lov: None,
        polar_stereo_lad: None,
        polar_stereo_dx_metres: None,
        polar_stereo_dy_metres: None,
        polar_stereo_south_pole: None,
        rotated_south_pole_lat: None,
        rotated_south_pole_lon: None,
        rotated_angle_of_rotation: None,
        geos_sub_lon: None,
        geos_height: None,
        geos_r_eq: None,
        geos_r_pol: None,
        geos_sweep_x: None,
        geos_x0: None,
        geos_dx_rad: None,
        geos_y0: None,
        geos_dy_rad: None,
        packing: None,
        reprojectable: false,
        j_scans_positive: None,
    }
}

/// Build a synthesised `"latlon"` [`MessageMeta`] for a NetCDF slice. Only the
/// geometry the warp reads (`grid_type`, corner coordinates) is populated. When
/// `geometry` is `None` (no coordinate variables) the corners are absent and the
/// grid is not reprojectable, so only the source projection is offered.
fn synth_latlon_meta(
    name: &str,
    units: &str,
    ni: i32,
    nj: i32,
    geometry: Option<fieldglass_netcdf::SliceGeometry>,
) -> MessageMeta {
    MessageMeta {
        grid_type: Some("latlon".to_string()),
        lat_first: geometry.map(|g| g.lat_first),
        lon_first: geometry.map(|g| g.lon_first),
        lat_last: geometry.map(|g| g.lat_last),
        lon_last: geometry.map(|g| g.lon_last),
        // A descending (east-to-west) longitude axis would be misread by the
        // west-to-east inverse map as an antimeridian wrap — mirror the GRIB
        // scanning-mode gate and offer only the source projection.
        reprojectable: geometry.is_some_and(|g| !g.lon_descending),
        ..base_netcdf_meta(name, units, ni, nj)
    }
}

/// Build a `"lambert"` [`MessageMeta`] from a WRF-resolved Lambert grid
/// (decision 0004). The corner is the grid origin (first scanned point) and the
/// `lambert_*` fields feed the same projector the GRIB2 §3.30 path uses.
fn synth_lambert_meta(name: &str, units: &str, g: &WrfLambertGrid) -> MessageMeta {
    MessageMeta {
        // WRF projects on its own 6 370 000 m sphere, not a WMO default.
        earth_radius_metres: Some(WRF_EARTH_RADIUS_M),
        grid_type: Some("lambert".to_string()),
        lat_first: Some(g.lat_first),
        lon_first: Some(g.lon_first),
        lambert_lad: Some(g.lad),
        lambert_lov: Some(g.lov),
        lambert_dx_metres: Some(g.dx_metres),
        lambert_dy_metres: Some(g.dy_metres),
        lambert_latin1: Some(g.latin1),
        lambert_latin2: Some(g.latin2),
        reprojectable: true,
        ..base_netcdf_meta(name, units, g.ni as i32, g.nj as i32)
    }
}

/// Build a `"polar_stereo"` [`MessageMeta`] from a WRF-resolved polar
/// stereographic grid (#220). `"polar_stereo"` is the *source-grid* string the
/// GRIB paths emit (routing into `polar_stereo_warp_setup`), distinct from the
/// `"polar_stereographic"` *target*-projection picker option.
fn synth_polar_stereo_meta(name: &str, units: &str, g: &WrfPolarStereoGrid) -> MessageMeta {
    MessageMeta {
        // WRF projects on its own 6 370 000 m sphere, not a WMO default.
        earth_radius_metres: Some(WRF_EARTH_RADIUS_M),
        grid_type: Some("polar_stereo".to_string()),
        lat_first: Some(g.lat_first),
        lon_first: Some(g.lon_first),
        polar_stereo_lov: Some(g.lov),
        polar_stereo_lad: Some(g.lad),
        polar_stereo_dx_metres: Some(g.dx_metres),
        polar_stereo_dy_metres: Some(g.dy_metres),
        polar_stereo_south_pole: Some(g.south_pole),
        reprojectable: true,
        ..base_netcdf_meta(name, units, g.ni as i32, g.nj as i32)
    }
}

/// Build a `"mercator"` [`MessageMeta`] from a WRF-resolved Mercator grid
/// (#220). Like the GRIB Mercator source, the grid is pinned entirely by its
/// corner coordinates — no spacing or true-scale fields exist to copy.
fn synth_mercator_meta(name: &str, units: &str, g: &WrfMercatorGrid) -> MessageMeta {
    MessageMeta {
        grid_type: Some("mercator".to_string()),
        lat_first: Some(g.lat_first),
        lon_first: Some(g.lon_first),
        lat_last: Some(g.lat_last),
        lon_last: Some(g.lon_last),
        reprojectable: true,
        ..base_netcdf_meta(name, units, g.ni as i32, g.nj as i32)
    }
}

/// Build a `"latlon"` [`MessageMeta`] from a WRF-resolved unrotated lat-lon grid
/// (#226). An unrotated WRF lat-lon domain is a plain regular geographic grid,
/// so — like the corner-pinned Mercator — its four corners feed the same lat/lon
/// projector the regular 1-D coordinate path uses. It is always reprojectable,
/// as the Mercator sibling is: WRF scans west-to-east (`+DX`), so the corner
/// longitudes ascend, and a domain straddling the antimeridian
/// (`lon_last < lon_first`) is an eastward wrap the lat/lon inverse map already
/// handles — not the descending axis the regular 1-D path guards against.
fn synth_wrf_latlon_meta(name: &str, units: &str, g: &WrfLatLonGrid) -> MessageMeta {
    MessageMeta {
        grid_type: Some("latlon".to_string()),
        lat_first: Some(g.lat_first),
        lon_first: Some(g.lon_first),
        lat_last: Some(g.lat_last),
        lon_last: Some(g.lon_last),
        reprojectable: true,
        ..base_netcdf_meta(name, units, g.ni as i32, g.nj as i32)
    }
}

/// Build a `"space_view"` (geostationary) [`MessageMeta`] from a CF-resolved
/// grid mapping (decision 0004), feeding the same geostationary projector the
/// GRIB2 §3.90 path uses. Off-disk pixels invert to `None` (transparent limb).
fn synth_geostationary_meta(
    name: &str,
    units: &str,
    g: &fieldglass_netcdf::GeostationaryGrid,
) -> MessageMeta {
    MessageMeta {
        grid_type: Some("space_view".to_string()),
        geos_sub_lon: Some(g.sub_lon_deg),
        geos_height: Some(g.h_metres),
        geos_r_eq: Some(g.r_eq),
        geos_r_pol: Some(g.r_pol),
        geos_sweep_x: Some(g.sweep_x),
        geos_x0: Some(g.x0),
        geos_dx_rad: Some(g.dx_rad),
        geos_y0: Some(g.y0),
        geos_dy_rad: Some(g.dy_rad),
        reprojectable: true,
        ..base_netcdf_meta(name, units, g.ni as i32, g.nj as i32)
    }
}

fn grib1_dimensions(reader: &Grib1Reader, message_index: usize) -> napi::Result<(u32, u32)> {
    let msg = reader
        .messages
        .get(message_index)
        .ok_or_else(|| napi::Error::from_reason("message index out of range".to_string()))?;
    let gds = msg.gds.as_ref().ok_or_else(|| {
        napi::Error::from_reason(
            "message has no GDS and its grid number is not a known predefined grid".to_string(),
        )
    })?;
    gds.dimensions()
        .ok_or_else(|| napi::Error::from_reason("grid type has no declared dimensions".to_string()))
}

/// Repack the decoder's `Vec<Option<f64>>` into the typed-array pair
/// the JS side expects. NaN encodes missing in `values`; the explicit
/// `mask` byte is the source of truth for "present vs absent".
fn decoded_grid_from(raw: &[Option<f64>], width: u32, height: u32) -> DecodedGrid {
    let n = raw.len();
    let mut values = vec![0.0f64; n];
    let mut mask = vec![0u8; n];
    for (i, v) in raw.iter().enumerate() {
        match v {
            Some(x) => {
                values[i] = *x;
                mask[i] = 1;
            }
            None => {
                values[i] = f64::NAN;
                mask[i] = 0;
            }
        }
    }
    DecodedGrid {
        values: napi::bindgen_prelude::Float64Array::new(values),
        mask: mask.into(),
        width: width as i32,
        height: height as i32,
    }
}

/// Drive the warp + colormap pipeline for a single message. Returns the
/// paint-ready RGBA buffer plus the (min, max) actually used + a
/// human-readable summary of the source→target projection chain.
/// Validated picker state. Lifts `RenderOptions`'s loose strings into a
/// closed enum so the rest of the pipeline can pattern-match without
/// silently falling to defaults on a typo.
#[cfg_attr(test, derive(Debug))]
struct ResolvedOptions {
    projection: TargetKind,
    resampling: Resampling,
    flip_y: bool,
    range_min: Option<f64>,
    range_max: Option<f64>,
    bounds: Option<RenderBounds>,
    colormap: &'static Colormap,
    reverse_colormap: bool,
    scale: ScaleMode,
}

/// What the picker's `projection` string resolved to. Named `TargetKind`
/// rather than `TargetProjection` to avoid colliding with the core
/// `fieldglass_core::TargetProjection` *trait* — this is the napi-side
/// dispatch enum, not the per-target math.
#[cfg_attr(test, derive(Debug))]
enum TargetKind {
    /// Paint the source grid unchanged (no warp).
    Source,
    /// Inverse-warp into one of the geographic targets.
    Warp(WarpTarget),
}

/// A validated equirectangular render window. Only constructed when the
/// caller supplied all four edges and they form a non-degenerate box.
#[derive(Clone, Copy)]
#[cfg_attr(test, derive(Debug, PartialEq))]
struct RenderBounds {
    lat_min: f64,
    lat_max: f64,
    lon_min: f64,
    lon_max: f64,
}

impl RenderBounds {
    /// Build from the four optional `RenderOptions` fields. Returns `None`
    /// unless every edge is present and the box is non-degenerate — a
    /// partially-filled or inverted box silently falls back to the computed
    /// bounds, mirroring the manual-range behaviour.
    fn from_options(o: &RenderOptions) -> Option<Self> {
        let (lat_min, lat_max, lon_min, lon_max) = (
            o.bounds_lat_min?,
            o.bounds_lat_max?,
            o.bounds_lon_min?,
            o.bounds_lon_max?,
        );
        if lat_max > lat_min && lon_max > lon_min {
            Some(Self {
                lat_min,
                lat_max,
                lon_min,
                lon_max,
            })
        } else {
            None
        }
    }
}

impl ResolvedOptions {
    fn parse(options: &RenderOptions) -> napi::Result<Self> {
        let preset = options.projection_preset.as_deref();
        let projection = match options.projection.as_str() {
            "source" => TargetKind::Source,
            "equirectangular" => TargetKind::Warp(WarpTarget::Equirectangular),
            "web_mercator" => TargetKind::Warp(WarpTarget::WebMercator),
            "orthographic" => TargetKind::Warp(orthographic_from_options(options, preset)),
            "polar_stereographic" => {
                TargetKind::Warp(polar_stereographic_from_options(options, preset))
            }
            "mollweide" => TargetKind::Warp(WarpTarget::Mollweide {
                lon0: world_central_meridian(options),
            }),
            "robinson" => TargetKind::Warp(WarpTarget::Robinson {
                lon0: world_central_meridian(options),
            }),
            "equal_earth" => TargetKind::Warp(WarpTarget::EqualEarth {
                lon0: world_central_meridian(options),
            }),
            other => {
                return Err(napi::Error::from_reason(format!(
                    "unknown projection {other:?} (expected \"source\", \"equirectangular\", \
                     \"web_mercator\", \"orthographic\", \"polar_stereographic\", \"mollweide\", \
                     \"robinson\", or \"equal_earth\")"
                )));
            }
        };
        let resampling = match options.resampling.as_str() {
            "nearest" => Resampling::Nearest,
            "bilinear" => Resampling::Bilinear,
            other => {
                return Err(napi::Error::from_reason(format!(
                    "unknown resampling {other:?} (expected \"nearest\" or \"bilinear\")"
                )));
            }
        };
        // An unknown colormap is an error, not a silent fallback to viridis: a
        // typo'd name should say so rather than paint the wrong colours.
        let colormap = match options.colormap.as_deref() {
            None => fieldglass_core::colormap::default_colormap(),
            Some(name) => Colormap::by_name(name).ok_or_else(|| {
                let known: Vec<&str> = fieldglass_core::colormap::colormaps()
                    .iter()
                    .map(|c| c.name())
                    .collect();
                napi::Error::from_reason(format!(
                    "unknown colormap {name:?} (expected one of {})",
                    known.join(", ")
                ))
            })?,
        };
        // An unknown scale mode is an error for the same reason an unknown
        // colormap is: a typo should say so, not silently paint linearly.
        let scale = match options.scale_mode.as_deref() {
            None | Some("linear") => ScaleMode::Linear,
            Some("log10") => ScaleMode::Log10,
            Some(other) => {
                return Err(napi::Error::from_reason(format!(
                    "unknown scale mode {other:?} (expected \"linear\" or \"log10\")"
                )));
            }
        };
        Ok(Self {
            projection,
            resampling,
            flip_y: options.flip_y,
            range_min: options.range_min,
            range_max: options.range_max,
            bounds: RenderBounds::from_options(options),
            colormap,
            reverse_colormap: options.reverse_colormap.unwrap_or(false),
            scale,
        })
    }
}

/// Resolve the orthographic centre. A free-form `center_lat`/`center_lon`
/// (degrees) wins per component; otherwise the named preset supplies it; with
/// neither the centre is the Atlantic view (0°N 0°E). (#71 shipped presets
/// only; #113 added the free-form centre, of which the presets are now named
/// shortcuts.)
fn orthographic_from_options(o: &RenderOptions, preset: Option<&str>) -> WarpTarget {
    let (preset_lat, preset_lon) = orthographic_preset_centre(preset);
    WarpTarget::Orthographic {
        lat0: o.center_lat.unwrap_or(preset_lat),
        lon0: o.center_lon.unwrap_or(preset_lon),
    }
}

/// The `(lat0, lon0)` of an orthographic centre preset. Unknown/`None`
/// defaults to the Atlantic view (0°N 0°E).
fn orthographic_preset_centre(preset: Option<&str>) -> (f64, f64) {
    match preset {
        Some("indian") => (0.0, 90.0),
        Some("pacific") => (0.0, 180.0),
        Some("americas") => (0.0, 270.0),
        Some("north_pole") => (90.0, 0.0),
        Some("south_pole") => (-90.0, 0.0),
        // "atlantic" / None / unknown
        _ => (0.0, 0.0),
    }
}

/// Resolve the polar-stereographic target. The pole is the hemisphere preset
/// (`"south"` ⇒ south aspect; otherwise north). `lon0` — the central meridian
/// oriented toward the bottom edge — is the free-form `center_lon` when given,
/// else 0°. (#113 added the free-form central meridian; #71 fixed it at 0°.)
fn polar_stereographic_from_options(o: &RenderOptions, preset: Option<&str>) -> WarpTarget {
    WarpTarget::PolarStereographic {
        south_pole: matches!(preset, Some("south")),
        lon0: o.center_lon.unwrap_or(0.0),
    }
}

/// The central meridian of a whole-world target (Mollweide, Robinson, Equal
/// Earth): the free-form `center_lon` when given, else 0° (Greenwich-centred).
/// These take no preset — they always show the whole globe, and recentring is
/// the only knob.
fn world_central_meridian(o: &RenderOptions) -> f64 {
    o.center_lon.unwrap_or(0.0)
}

/// Output of a projection stage (`paint_source` / `warp_message`):
/// `(values, mask, width, height, geographic_bounds, summary)`. The bounds
/// are `(lat_min, lat_max, lon_min, lon_max)` for the equirectangular target,
/// or `None` for source projection (no geographic extent).
type ProjectionStage = (
    Vec<f64>,
    Vec<u8>,
    u32,
    u32,
    Option<(f64, f64, f64, f64)>,
    String,
);

/// Effective vertical flip for the source projection.
///
/// The source projection paints grid row 0 at the top of the canvas, so a grid
/// that scans south→north (`jScansPositively`) renders upside-down. The source
/// view therefore flips such a grid by default, with the user's Flip Y toggle
/// riding on top as an override. Reprojected targets orient from geometry, so
/// they use `flip_y` verbatim and never call this. (#286)
fn source_flip_y(meta: &MessageMeta, flip_y: bool) -> bool {
    flip_y ^ meta.j_scans_positive.unwrap_or(false)
}

/// Validate a combine-op wire tag (see [`CombineOp`]) or return a napi error
/// listing the valid ones, matching the colormap / scale-mode clamps.
fn parse_combine_op(tag: &str) -> napi::Result<CombineOp> {
    CombineOp::from_wire(tag).ok_or_else(|| {
        napi::Error::from_reason(format!(
            "unknown combine op {tag:?} (expected \"a_minus_b\", \"b_minus_a\", \
             \"a_plus_b\", \"mean\", or \"ratio\")"
        ))
    })
}

/// Whether two messages sit on the same grid — identical dimensions and grid
/// definition — so their decoded fields align cell-for-cell and combining them
/// is meaningful. Compares every geometry-defining field of [`MessageMeta`];
/// parameter, level, time, and packing metadata are deliberately ignored (two
/// fields differing only in those are exactly what a difference map compares).
///
/// NB: when a new geometry field is added to `MessageMeta`, add it here too, or
/// two grids differing only in that field would be wrongly treated as aligned.
fn grids_match(a: &MessageMeta, b: &MessageMeta) -> bool {
    a.grid_type == b.grid_type
        && a.grid_ni == b.grid_ni
        && a.grid_nj == b.grid_nj
        && a.lat_first == b.lat_first
        && a.lon_first == b.lon_first
        && a.lat_last == b.lat_last
        && a.lon_last == b.lon_last
        && a.earth_radius_metres == b.earth_radius_metres
        && a.lambert_lad == b.lambert_lad
        && a.lambert_lov == b.lambert_lov
        && a.lambert_dx_metres == b.lambert_dx_metres
        && a.lambert_dy_metres == b.lambert_dy_metres
        && a.lambert_latin1 == b.lambert_latin1
        && a.lambert_latin2 == b.lambert_latin2
        && a.gaussian_n_parallels == b.gaussian_n_parallels
        && a.polar_stereo_lov == b.polar_stereo_lov
        && a.polar_stereo_lad == b.polar_stereo_lad
        && a.polar_stereo_dx_metres == b.polar_stereo_dx_metres
        && a.polar_stereo_dy_metres == b.polar_stereo_dy_metres
        && a.polar_stereo_south_pole == b.polar_stereo_south_pole
        && a.rotated_south_pole_lat == b.rotated_south_pole_lat
        && a.rotated_south_pole_lon == b.rotated_south_pole_lon
        && a.rotated_angle_of_rotation == b.rotated_angle_of_rotation
        && a.geos_sub_lon == b.geos_sub_lon
        && a.geos_height == b.geos_height
        && a.geos_r_eq == b.geos_r_eq
        && a.geos_r_pol == b.geos_r_pol
        && a.geos_sweep_x == b.geos_sweep_x
        && a.geos_x0 == b.geos_x0
        && a.geos_dx_rad == b.geos_dx_rad
        && a.geos_y0 == b.geos_y0
        && a.geos_dy_rad == b.geos_dy_rad
        && a.j_scans_positive == b.j_scans_positive
}

/// Decode-domain core of the combined render: require identical grids, combine
/// the two aligned fields under `op`, then run the result through the ordinary
/// render pipeline against the **primary** field's geometry (`meta_a`). Because
/// the combined field is just another `Vec<Option<f64>>`, projection, overlays,
/// palette, scaling, and manual bounds all apply unchanged. Shared by the GRIB
/// and NetCDF combined-render entry points.
fn render_combined(
    meta_a: &MessageMeta,
    raw_a: &[Option<f64>],
    meta_b: &MessageMeta,
    raw_b: &[Option<f64>],
    op: CombineOp,
    options: &RenderOptions,
) -> napi::Result<RenderedGrid> {
    if !grids_match(meta_a, meta_b) {
        return Err(napi::Error::from_reason(
            "the two fields are on different grids; combining needs identical grid \
             dimensions and definition"
                .to_string(),
        ));
    }
    let combined = combine_fields(raw_a, raw_b, op);
    render_with_options(meta_a, &combined, options)
}

fn render_with_options(
    meta: &MessageMeta,
    raw: &[Option<f64>],
    options: &RenderOptions,
) -> napi::Result<RenderedGrid> {
    let resolved = ResolvedOptions::parse(options)?;
    let is_source = matches!(resolved.projection, TargetKind::Source);
    let (values, mask, width, height, used_bounds, summary) = match resolved.projection {
        TargetKind::Source => paint_source(meta, raw)?,
        TargetKind::Warp(target) => {
            warp_message(meta, raw, target, resolved.resampling, resolved.bounds)?
        }
    };
    let flip_y = if is_source {
        source_flip_y(meta, resolved.flip_y)
    } else {
        resolved.flip_y
    };

    let (used_min, used_max) = match (resolved.range_min, resolved.range_max) {
        (Some(min), Some(max)) if max > min => (min, max),
        _ => min_max_ignoring_mask(values.iter().enumerate().map(|(i, &v)| {
            if mask.get(i).copied().unwrap_or(0) == 0 {
                None
            } else {
                Some(v)
            }
        }))
        .unwrap_or((0.0, 1.0)),
    };

    // Log10 has no logarithm for a non-positive lower bound. Rather than paint
    // garbage, refuse with an actionable message: the caller (the panel) keeps
    // the log toggle disabled in this case, but a direct API call gets told
    // exactly what to do. A field with a positive floor — or a positive manual
    // minimum over data that dips to/below zero — renders fine; the sub-zero
    // cells simply drop out as missing in the painter.
    if resolved.scale == ScaleMode::Log10 && used_min <= 0.0 {
        return Err(napi::Error::from_reason(format!(
            "log10 scaling needs a positive minimum, but the range starts at \
             {used_min}; set a manual minimum > 0 or pick a field with \
             positive values"
        )));
    }

    let rgba = paint_grid_rgba(
        &values,
        Some(&mask),
        width,
        height,
        used_min,
        used_max,
        flip_y,
        resolved.colormap,
        resolved.reverse_colormap,
        resolved.scale,
    );

    let (used_lat_min, used_lat_max, used_lon_min, used_lon_max) = match used_bounds {
        Some((la_min, la_max, lo_min, lo_max)) => {
            (Some(la_min), Some(la_max), Some(lo_min), Some(lo_max))
        }
        None => (None, None, None, None),
    };

    Ok(RenderedGrid {
        rgba: rgba.into(),
        width: width as i32,
        height: height as i32,
        used_min,
        used_max,
        used_lat_min,
        used_lat_max,
        used_lon_min,
        used_lon_max,
        projection_summary: summary,
    })
}

fn source_projection_summary(meta: &MessageMeta) -> String {
    let kind = meta.grid_type.as_deref().unwrap_or("unknown");
    let dims = match (meta.grid_ni, meta.grid_nj) {
        (Some(ni), Some(nj)) => format!("{ni}×{nj}"),
        _ => "?".to_string(),
    };
    format!("source: {kind} {dims}")
}

/// Source-projection paint: paint the decoded values into a buffer the
/// same shape as the source grid. NaN encodes masked cells.
#[allow(clippy::type_complexity)]
fn paint_source(meta: &MessageMeta, raw: &[Option<f64>]) -> napi::Result<ProjectionStage> {
    let ni = meta
        .grid_ni
        .ok_or_else(|| napi::Error::from_reason("grid has no Ni".to_string()))? as u32;
    let nj = meta
        .grid_nj
        .ok_or_else(|| napi::Error::from_reason("grid has no Nj".to_string()))? as u32;
    let n = (ni as usize) * (nj as usize);
    let mut values = vec![0.0f64; n];
    let mut mask = vec![0u8; n];
    for (i, v) in raw.iter().enumerate().take(n) {
        if let Some(x) = v {
            values[i] = *x;
            mask[i] = 1;
        } else {
            values[i] = f64::NAN;
        }
    }
    // Right-hand side names the actual source projection (e.g. "latlon",
    // "lambert", "polar_stereo") so the picker caption reads
    // `source: latlon 240×121 → latlon (no reprojection)`, mirroring the
    // equirectangular shape but making it explicit *what* the source
    // projection is rather than just labelling it "source projection".
    let kind = meta.grid_type.as_deref().unwrap_or("unknown");
    let summary = format!(
        "{} → {kind} (no reprojection)",
        source_projection_summary(meta),
    );
    // No geographic extent for the source-projection target.
    Ok((values, mask, ni, nj, None, summary))
}

/// Dispatch warp setup to the right per-template helper. Each helper
/// builds the inverse closure (using a projector with precomputed state)
/// and the equirectangular target bounds; the shared finisher then runs
/// the warp itself.
///
/// `bounds_override`, when present, replaces the computed source-grid extent
/// so the caller can render an arbitrary equirectangular window. The bounds
/// actually used are returned (last tuple element) for echo-back.
///
/// LIMITATION (warp output resolution): every setup sizes the output raster to
/// the source dims (`width = ni`, `height = nj`) and the warp samples it
/// uniformly in degrees across the lat/lon bbox. A *projected* source grid's
/// degrees-per-cell varies across the tile — meridians converge poleward — so
/// no single uniform output resolution matches it everywhere: the equator side
/// of a polar grid is downsampled (nearest aliases; bilinear blurs) while the
/// pole side is upsampled (one source cell magnified across several pixels).
/// Aspect isn't preserved either, so degrees-per-pixel differs between axes.
/// This is a deliberate fidelity/perf tradeoff — it keeps the RGBA buffer at a
/// predictable size and spends roughly one output pixel per source cell. The
/// manual-bounds window doubles as a zoom (the fixed pixel budget over a
/// smaller extent recovers detail down to the source resolution), which covers
/// the common "I need detail in this region" case without another knob.
/// Revisit — derive output dims from the extent and a target degrees-per-pixel
/// (capped to bound memory) — only if a real fixture shows aliasing or stretch
/// that's objectionable at pixel scale.
/// Which lat/lon → pixel target projection the warp paints into. Both
/// targets share the same source inverse map and computed bbox; they
/// differ only in how output rows are distributed (linear in latitude vs
/// linear in Mercator Y).
#[derive(Clone, Copy)]
#[cfg_attr(test, derive(Debug, PartialEq))]
enum WarpTarget {
    /// Lat/lon-box targets: rows linear in latitude (equirectangular) or in
    /// Mercator Y (web mercator). These honour a manual lat/lon window and
    /// echo their geographic extent back.
    Equirectangular,
    WebMercator,
    /// Azimuthal targets parameterised by a centre / hemisphere preset. They
    /// fit a disc to the raster and have no lat/lon-box extent to echo.
    Orthographic {
        lat0: f64,
        lon0: f64,
    },
    PolarStereographic {
        south_pole: bool,
        lon0: f64,
    },
    /// Pseudocylindrical equal-area world target parameterised by its central
    /// meridian; fits an ellipse to a 2:1 raster with no lat/lon-box extent.
    Mollweide {
        lon0: f64,
    },
    /// Pseudocylindrical *compromise* world target (Robinson's table), likewise
    /// parameterised by its central meridian only.
    Robinson {
        lon0: f64,
    },
    /// Pseudocylindrical equal-area world target (Equal Earth), likewise
    /// parameterised by its central meridian only.
    EqualEarth {
        lon0: f64,
    },
}

impl WarpTarget {
    fn label(self) -> &'static str {
        match self {
            WarpTarget::Equirectangular => "equirectangular",
            WarpTarget::WebMercator => "web mercator",
            WarpTarget::Orthographic { .. } => "orthographic",
            WarpTarget::PolarStereographic { .. } => "polar stereographic",
            WarpTarget::Mollweide { .. } => "mollweide",
            WarpTarget::Robinson { .. } => "robinson",
            WarpTarget::EqualEarth { .. } => "equal earth",
        }
    }
}

#[allow(clippy::type_complexity)]
fn warp_message(
    meta: &MessageMeta,
    raw: &[Option<f64>],
    target_kind: WarpTarget,
    resampling: Resampling,
    bounds_override: Option<RenderBounds>,
) -> napi::Result<ProjectionStage> {
    let ni = grid_ni(meta)?;
    let nj = grid_nj(meta)?;

    let sample = |i: usize, j: usize| -> Option<f64> {
        let k = j * ni as usize + i;
        raw.get(k).copied().flatten()
    };
    let sample_ref: &dyn Fn(usize, usize) -> Option<f64> = &sample;

    let (inverse_boxed, bbox_thunk) = warp_setup_for(meta, ni, nj)?;

    let inverse_ref: &dyn Fn(f64, f64) -> Option<GridIndex> = inverse_boxed.as_ref();
    let source = SourceGrid {
        ni,
        nj,
        sample: sample_ref,
        inverse_at: inverse_ref,
        periodic_i: source_grid_is_periodic(meta, ni),
    };
    // Construct the concrete target (shared with the overlay-projection path so
    // both paint into byte-identical geometry), then warp the source into it.
    let (built, used_bounds) = build_warp_target(target_kind, ni, nj, bbox_thunk, bounds_override)?;
    let warped = built.warp(&source, resampling);
    let resample_label = match resampling {
        Resampling::Nearest => "nearest",
        Resampling::Bilinear => "bilinear",
    };
    let summary = format!(
        "{} → {} ({resample_label})",
        source_projection_summary(meta),
        target_kind.label(),
    );
    Ok((
        warped.values,
        warped.mask,
        warped.width,
        warped.height,
        used_bounds,
        summary,
    ))
}

/// A concrete, constructed render target — the same value the warp paints
/// into and the overlay projects polylines onto. Centralising construction
/// here guarantees `render_grid` and `project_overlay` agree on the exact
/// raster (dims, clamped Mercator band, azimuthal disc side) pixel-for-pixel.
enum BuiltTarget {
    Equirect(TargetRaster),
    Mercator(WebMercator),
    Ortho(Orthographic),
    Polar(PolarStereographic),
    Moll(Mollweide),
    Robin(Robinson),
    EqEarth(EqualEarth),
}

impl BuiltTarget {
    fn dims(&self) -> (u32, u32) {
        match self {
            BuiltTarget::Equirect(t) => t.dims(),
            BuiltTarget::Mercator(t) => t.dims(),
            BuiltTarget::Ortho(t) => t.dims(),
            BuiltTarget::Polar(t) => t.dims(),
            BuiltTarget::Moll(t) => t.dims(),
            BuiltTarget::Robin(t) => t.dims(),
            BuiltTarget::EqEarth(t) => t.dims(),
        }
    }

    fn warp(&self, source: &SourceGrid<'_>, resampling: Resampling) -> WarpedRaster {
        match self {
            BuiltTarget::Equirect(t) => warp(source, t, resampling),
            BuiltTarget::Mercator(t) => warp(source, t, resampling),
            BuiltTarget::Ortho(t) => warp(source, t, resampling),
            BuiltTarget::Polar(t) => warp(source, t, resampling),
            BuiltTarget::Moll(t) => warp(source, t, resampling),
            BuiltTarget::Robin(t) => warp(source, t, resampling),
            BuiltTarget::EqEarth(t) => warp(source, t, resampling),
        }
    }

    /// Project geographic `(lat, lon)` rings onto this target's pixel space,
    /// applying `flip_y` to match a vertically-flipped render. Each target
    /// reports its own seam-split rule (`ForwardMap::seam_split`), so the only
    /// per-variant work here is preparing the concrete map.
    fn project(&self, flip_y: bool, latlon: &[f64], ring_lengths: &[u32]) -> ProjectedPolylines {
        let (w, h) = self.dims();
        match self {
            BuiltTarget::Equirect(t) => {
                project_polylines(&t.prepare(), w, h, flip_y, latlon, ring_lengths)
            }
            BuiltTarget::Mercator(t) => {
                project_polylines(&t.prepare(), w, h, flip_y, latlon, ring_lengths)
            }
            BuiltTarget::Ortho(t) => {
                project_polylines(&t.prepare(), w, h, flip_y, latlon, ring_lengths)
            }
            BuiltTarget::Polar(t) => {
                project_polylines(&t.prepare(), w, h, flip_y, latlon, ring_lengths)
            }
            BuiltTarget::Moll(t) => {
                project_polylines(&t.prepare(), w, h, flip_y, latlon, ring_lengths)
            }
            BuiltTarget::Robin(t) => {
                project_polylines(&t.prepare(), w, h, flip_y, latlon, ring_lengths)
            }
            BuiltTarget::EqEarth(t) => {
                project_polylines(&t.prepare(), w, h, flip_y, latlon, ring_lengths)
            }
        }
    }

    /// The `(lat, lon)` a single output pixel maps to — the inverse of the
    /// target projection, the same map [`warp`] walks per pixel. `None` when the
    /// pixel falls off the globe (e.g. outside an azimuthal disc). `py` is in
    /// the raster's own orientation (row 0 = top), so a flipped render must
    /// un-flip the click first.
    fn pixel_to_lonlat(&self, px: u32, py: u32) -> Option<(f64, f64)> {
        match self {
            BuiltTarget::Equirect(t) => t.prepare().pixel_to_lonlat(px, py),
            BuiltTarget::Mercator(t) => t.prepare().pixel_to_lonlat(px, py),
            BuiltTarget::Ortho(t) => t.prepare().pixel_to_lonlat(px, py),
            BuiltTarget::Polar(t) => t.prepare().pixel_to_lonlat(px, py),
            BuiltTarget::Moll(t) => t.prepare().pixel_to_lonlat(px, py),
            BuiltTarget::Robin(t) => t.prepare().pixel_to_lonlat(px, py),
            BuiltTarget::EqEarth(t) => t.prepare().pixel_to_lonlat(px, py),
        }
    }
}

/// `(BuiltTarget, used extent)` — the concrete warp target plus the lat/lon
/// box it actually rendered (`None` for the azimuthal targets).
type BuiltWarpTarget = (BuiltTarget, Option<(f64, f64, f64, f64)>);

/// Build the concrete [`BuiltTarget`] for a warp, returning the geographic
/// extent actually used for the lat/lon-box targets (echoed back to the UI)
/// or `None` for the azimuthal targets.
///
/// The lat/lon-box targets resolve a geographic extent (the lazy `bbox_thunk`,
/// possibly replaced by a manual window); the azimuthal targets fit a disc to
/// the raster, so they never invoke the thunk — skipping the perimeter-walk
/// bbox for planar sources — and report no box extent. Output dims size to the
/// source grid for the box targets; the azimuthal discs use a square raster
/// (`side = max(ni, nj)`) so the globe stays circular rather than elliptical.
fn build_warp_target(
    target_kind: WarpTarget,
    ni: u32,
    nj: u32,
    bbox_thunk: BboxThunk,
    bounds_override: Option<RenderBounds>,
) -> napi::Result<BuiltWarpTarget> {
    match target_kind {
        WarpTarget::Equirectangular => {
            let (lat_min, lat_max, lon_min, lon_max) =
                resolve_box_extent(bbox_thunk, bounds_override);
            let target = TargetRaster {
                width: ni,
                height: nj,
                lat_max,
                lat_min,
                lon_min,
                lon_max,
            };
            Ok((
                BuiltTarget::Equirect(target),
                Some((lat_min, lat_max, lon_min, lon_max)),
            ))
        }
        WarpTarget::WebMercator => {
            let (lat_min, lat_max, lon_min, lon_max) =
                resolve_box_extent(bbox_thunk, bounds_override);
            let merc = WebMercator::new(ni, nj, lat_min, lat_max, lon_min, lon_max);
            let (used_lat_min, used_lat_max, _, _) = merc.extent();
            // A lat band lying entirely outside the ±85.0511° Web Mercator
            // cutoff clamps to a single edge, collapsing the Y span to zero and
            // smearing every row to one latitude. Reject it rather than emit a
            // degenerate single-row raster.
            if used_lat_max - used_lat_min <= f64::EPSILON {
                return Err(napi::Error::from_reason(format!(
                    "Web Mercator latitude band [{lat_min}, {lat_max}] lies outside the \
                     renderable ±85.0511° range",
                )));
            }
            let used = merc.extent();
            Ok((BuiltTarget::Mercator(merc), Some(used)))
        }
        WarpTarget::Orthographic { lat0, lon0 } => {
            let side = ni.max(nj);
            Ok((
                BuiltTarget::Ortho(Orthographic::new(side, side, lat0, lon0)),
                None,
            ))
        }
        WarpTarget::PolarStereographic { south_pole, lon0 } => {
            let side = ni.max(nj);
            Ok((
                BuiltTarget::Polar(PolarStereographic::new(side, side, south_pole, lon0)),
                None,
            ))
        }
        WarpTarget::Mollweide { lon0 } => {
            let (w, h) = world_raster_dims(ni, nj, Mollweide::ASPECT_RATIO);
            Ok((BuiltTarget::Moll(Mollweide::new(w, h, lon0)), None))
        }
        WarpTarget::Robinson { lon0 } => {
            let (w, h) = world_raster_dims(ni, nj, Robinson::ASPECT_RATIO);
            Ok((BuiltTarget::Robin(Robinson::new(w, h, lon0)), None))
        }
        WarpTarget::EqualEarth { lon0 } => {
            let (w, h) = world_raster_dims(ni, nj, EqualEarth::ASPECT_RATIO);
            Ok((BuiltTarget::EqEarth(EqualEarth::new(w, h, lon0)), None))
        }
    }
}

/// Raster dims for a whole-world target of the given width : height ratio.
/// Height is the source's larger edge, so nothing is downsampled, and width
/// follows from the projection's own aspect so the map body keeps its true
/// proportions — 2:1 for Mollweide, ≈1.97:1 for Robinson, ≈2.05:1 for Equal
/// Earth. These targets have no lat/lon-box extent to echo back to the UI.
fn world_raster_dims(ni: u32, nj: u32, aspect: f64) -> (u32, u32) {
    let height = ni.max(nj);
    // A saturating `as u32` cast: `aspect` is a positive constant just under 2,
    // so an enormous source clamps at `u32::MAX` rather than wrapping to a tiny
    // raster. A zero-size source stays zero-size, as it did before.
    let width = (height as f64 * aspect).round() as u32;
    (width, height)
}

/// Dispatch to the per-grid-type warp setup, returning the source inverse map
/// and the lazy lat/lon-box extent thunk. Shared by the warp (`warp_message`)
/// and the overlay projection (`project_overlay`) so both derive identical
/// target geometry from the same source parameters.
fn warp_setup_for(meta: &MessageMeta, ni: u32, nj: u32) -> napi::Result<WarpSetup> {
    match meta.grid_type.as_deref().unwrap_or("") {
        // Reduced grids are widened to a regular raster before the warp runs, so
        // they share the regular lat/lon and Gaussian setups (ni = widest row).
        "latlon" | "reduced_latlon" => latlon_warp_setup(meta, ni, nj),
        "gaussian" | "reduced_gaussian" => gaussian_warp_setup(meta, ni, nj),
        "mercator" => mercator_warp_setup(meta, ni, nj),
        "rotated_latlon" => rotated_latlon_warp_setup(meta, ni, nj),
        "lambert" => lambert_warp_setup(meta, ni, nj),
        "polar_stereo" => polar_stereo_warp_setup(meta, ni, nj),
        "space_view" => geostationary_warp_setup(meta, ni, nj),
        other => Err(napi::Error::from_reason(format!(
            "reprojection not yet supported for grid type {other:?}"
        ))),
    }
}

/// Project geographic `(lat, lon)` polylines onto the warped raster for a
/// message, producing pixel-space runs for the render panel's overlay layer
/// (#72). Geometry-only: it rebuilds the *same* target the warp paints into
/// (via [`build_warp_target`]) but never decodes or samples values.
///
/// `latlon` is flat `[lat, lon, …]`; `ring_lengths[k]` is the vertex count of
/// input ring `k`. See [`fieldglass_core::project_polylines`] for the
/// run-splitting (visibility / antimeridian) rules.
///
/// The `"source"` projection paints grid point `(i, j)` at pixel `(i, j)`, so
/// the warp's own inverse map (lat/lon → fractional grid index) doubles as its
/// forward pixel map — the overlay projects straight through it
/// ([`SourceOverlayTarget`]), no separate geographic forward projection needed.
fn project_overlay_impl(
    meta: &MessageMeta,
    options: &RenderOptions,
    latlon: &[f64],
    ring_lengths: &[u32],
) -> napi::Result<ProjectedPolylines> {
    let resolved = ResolvedOptions::parse(options)?;
    let ni = grid_ni(meta)?;
    let nj = grid_nj(meta)?;
    let (inverse, bbox_thunk) = warp_setup_for(meta, ni, nj)?;
    match resolved.projection {
        // A source grid can wrap longitude (a global grid's seam, or the cut
        // meridian of a projected grid); `SourceOverlayTarget::seam_split`
        // returns `PixelHalfWidth`, so a raster-width jump breaks the run. On a
        // regional grid, out-of-coverage vertices invert to `None` and break
        // runs there instead.
        // The source raster is flipped to face north-up by default (#286); the
        // overlay must ride the same flip so coastlines track the field.
        TargetKind::Source => Ok(project_polylines(
            &SourceOverlayTarget::new(inverse.as_ref()),
            ni,
            nj,
            source_flip_y(meta, resolved.flip_y),
            latlon,
            ring_lengths,
        )),
        TargetKind::Warp(target_kind) => {
            let (built, _used_bounds) =
                build_warp_target(target_kind, ni, nj, bbox_thunk, resolved.bounds)?;
            Ok(built.project(resolved.flip_y, latlon, ring_lengths))
        }
    }
}

/// Build a grid-index → `(lat, lon)` forward map for the corner-pinned lat/lon
/// family (regular lat/lon, Mercator, rotated lat/lon, Gaussian), whose forward
/// maps are simple, round-trip-tested free functions. Projected grids (Lambert,
/// polar stereographic, geostationary) and reduced grids return an error for
/// now — their forward maps exist (`grid_point_lonlat`) but wiring them (and, for
/// reduced grids, the per-row longitudes) is a follow-up. Contours therefore
/// gate cleanly on grid type, like reprojection does.
/// A source grid's forward geolocation: grid index `(i, j)` → `(lat, lon)`, or
/// `None` for a malformed grid. Boxed so [`forward_geolocation_for`] can return
/// a different closure per grid type.
type ForwardGeo = Box<dyn Fn(u32, u32) -> Option<(f64, f64)>>;

fn forward_geolocation_for(meta: &MessageMeta, ni: u32, nj: u32) -> napi::Result<ForwardGeo> {
    match meta.grid_type.as_deref().unwrap_or("") {
        "latlon" => {
            let p = LatLonParams {
                ni,
                nj,
                lat_first: require_f64(meta.lat_first, "latFirst")?,
                lon_first: require_f64(meta.lon_first, "lonFirst")?,
                lat_last: require_f64(meta.lat_last, "latLast")?,
                lon_last: require_f64(meta.lon_last, "lonLast")?,
            };
            Ok(Box::new(move |i, j| latlon_point(&p, i, j)))
        }
        "mercator" => {
            let p = MercatorParams {
                ni,
                nj,
                lat_first: require_f64(meta.lat_first, "latFirst")?,
                lon_first: require_f64(meta.lon_first, "lonFirst")?,
                lat_last: require_f64(meta.lat_last, "latLast")?,
                lon_last: require_f64(meta.lon_last, "lonLast")?,
            };
            Ok(Box::new(move |i, j| mercator_point(&p, i, j)))
        }
        "rotated_latlon" => {
            let p = RotatedLatLonParams {
                ni,
                nj,
                lat_first: require_f64(meta.lat_first, "latFirst")?,
                lon_first: require_f64(meta.lon_first, "lonFirst")?,
                lat_last: require_f64(meta.lat_last, "latLast")?,
                lon_last: require_f64(meta.lon_last, "lonLast")?,
                south_pole_lat: require_f64(meta.rotated_south_pole_lat, "rotatedSouthPoleLat")?,
                south_pole_lon: require_f64(meta.rotated_south_pole_lon, "rotatedSouthPoleLon")?,
                angle_of_rotation: require_f64(
                    meta.rotated_angle_of_rotation,
                    "rotatedAngleOfRotation",
                )?,
            };
            Ok(Box::new(move |i, j| rotated_latlon_point(&p, i, j)))
        }
        "gaussian" => {
            let p = GaussianParams {
                ni,
                nj,
                lat_first: require_f64(meta.lat_first, "latFirst")?,
                lon_first: require_f64(meta.lon_first, "lonFirst")?,
                lat_last: require_f64(meta.lat_last, "latLast")?,
                lon_last: require_f64(meta.lon_last, "lonLast")?,
                n_parallels: meta.gaussian_n_parallels.ok_or_else(|| {
                    napi::Error::from_reason("missing gaussianNParallels".to_string())
                })? as u32,
            };
            let projector = GaussianProjector::new(p);
            Ok(Box::new(move |i, j| projector.grid_point_lonlat(i, j)))
        }
        other => Err(napi::Error::from_reason(format!(
            "contours not yet supported for grid type {other:?} \
             (only regular lat/lon, Mercator, rotated lat/lon, and Gaussian for now)"
        ))),
    }
}

/// `(lat, lon)` of a fractional grid position, bilinearly interpolated from the
/// four surrounding integer grid points via `forward`. A contour vertex sits on
/// a cell edge (one integer coordinate, one fractional), for which the bilinear
/// collapses to a linear interpolation along that edge. Longitudes come from the
/// forward map in the grid's own frame (monotonic within a cell), so the
/// interpolation never straddles the ±180° seam.
fn forward_bilinear(
    forward: &dyn Fn(u32, u32) -> Option<(f64, f64)>,
    ni: u32,
    nj: u32,
    fi: f64,
    fj: f64,
) -> Option<(f64, f64)> {
    if ni < 2 || nj < 2 {
        return None;
    }
    let i0 = (fi.floor().max(0.0) as u32).min(ni - 2);
    let j0 = (fj.floor().max(0.0) as u32).min(nj - 2);
    let fx = (fi - i0 as f64).clamp(0.0, 1.0);
    let fy = (fj - j0 as f64).clamp(0.0, 1.0);
    let a = forward(i0, j0)?;
    let b = forward(i0 + 1, j0)?;
    let c = forward(i0, j0 + 1)?;
    let d = forward(i0 + 1, j0 + 1)?;
    let bilerp = |va: f64, vb: f64, vc: f64, vd: f64| {
        let top = va + (vb - va) * fx;
        let bot = vc + (vd - vc) * fx;
        top + (bot - top) * fy
    };
    Some((bilerp(a.0, b.0, c.0, d.0), bilerp(a.1, b.1, c.1, d.1)))
}

/// Contour levels at every multiple of `step` strictly inside `(min, max)`. The
/// manual-interval override; guarded against a tiny step producing an unbounded
/// list.
fn levels_by_interval(min: f64, max: f64, step: f64) -> Vec<f64> {
    if step <= 0.0 || !step.is_finite() || min >= max {
        return Vec::new();
    }
    let start = (min / step).ceil() * step;
    let mut levels = Vec::new();
    let mut k = 0i64;
    loop {
        let v = start + k as f64 * step;
        k += 1;
        if v <= min {
            continue;
        }
        if v >= max {
            break;
        }
        levels.push(v);
        if levels.len() > 2000 {
            break;
        }
    }
    levels
}

/// Extract contour isolines from a decoded field and project them onto the same
/// raster the render / overlay use, returning pixel-space runs (#238). Levels
/// come from `interval` (a manual spacing) when given and positive, else from
/// [`nice_levels`] over the used range. The grid-space isolines are geolocated
/// through [`forward_geolocation_for`] and then run through the ordinary overlay
/// projection ([`project_overlay_impl`]), so they land on every target
/// projection with the same visibility / seam handling as the coastlines.
fn project_contours_impl(
    meta: &MessageMeta,
    raw: &[Option<f64>],
    options: &RenderOptions,
    interval: Option<f64>,
) -> napi::Result<ProjectedPolylines> {
    let ni = grid_ni(meta)?;
    let nj = grid_nj(meta)?;
    let forward = forward_geolocation_for(meta, ni, nj)?;

    // Levels span the same range the image is painted over, so contours line up
    // with the colours: a manual range override wins, else the present-cell min/max.
    let (used_min, used_max) = match (options.range_min, options.range_max) {
        (Some(min), Some(max)) if max > min => (min, max),
        _ => min_max_ignoring_mask(raw.iter().copied()).unwrap_or((0.0, 1.0)),
    };
    let levels = match interval {
        Some(step) if step > 0.0 => levels_by_interval(used_min, used_max, step),
        _ => nice_levels(used_min, used_max, 8),
    };

    // Each contour segment becomes a two-vertex ring in `(lat, lon)` order (what
    // `project_polylines` consumes); a vertex that can't be geolocated drops its
    // segment rather than the whole contour.
    let contours = contour_segments(raw, ni as usize, nj as usize, &levels);
    let mut latlon: Vec<f64> = Vec::new();
    let mut ring_lengths: Vec<u32> = Vec::new();
    for level in &contours {
        for seg in &level.segments {
            let p0 = forward_bilinear(forward.as_ref(), ni, nj, seg[0].0, seg[0].1);
            let p1 = forward_bilinear(forward.as_ref(), ni, nj, seg[1].0, seg[1].1);
            if let (Some((lat0, lon0)), Some((lat1, lon1))) = (p0, p1) {
                latlon.extend_from_slice(&[lat0, lon0, lat1, lon1]);
                ring_lengths.push(2);
            }
        }
    }

    project_overlay_impl(meta, options, &latlon, &ring_lengths)
}

/// The result of probing one output pixel (#172): the geographic point under
/// the pixel, the source grid cell it fell on, and the decoded value there.
#[napi(object)]
pub struct ProbeResult {
    /// Latitude under the pixel (degrees). `None` when the grid can't be
    /// geolocated (a source-projection view of a grid whose forward map isn't
    /// wired); the value is still reported.
    pub lat: Option<f64>,
    /// Longitude (degrees, normalised to `[-180, 180)`).
    pub lon: Option<f64>,
    /// The decoded value at the grid cell, or `None` when the pixel fell off the
    /// grid or onto a masked cell.
    pub value: Option<f64>,
    /// The source grid column / row the pixel resolved to; `None` off-grid.
    pub grid_i: Option<i32>,
    pub grid_j: Option<i32>,
}

/// Sample the field under one output pixel `(px, py)` in the *displayed*
/// raster (post-`flip_y`). Reproduces the warp's per-pixel map — output pixel →
/// `(lat, lon)` → source grid index → value — so the readout matches exactly
/// what the image shows. Returns `None` when the pixel is off the raster or off
/// the globe (outside an azimuthal disc), so there is nothing to report.
fn probe_impl(
    meta: &MessageMeta,
    raw: &[Option<f64>],
    options: &RenderOptions,
    px: u32,
    py: u32,
) -> napi::Result<Option<ProbeResult>> {
    let resolved = ResolvedOptions::parse(options)?;
    let ni = grid_ni(meta)?;
    let nj = grid_nj(meta)?;
    // Read the decoded value at an integer grid cell, `None` if out of range or
    // masked.
    let value_at = |gi: i64, gj: i64| -> Option<f64> {
        if gi < 0 || gj < 0 || gi as u32 >= ni || gj as u32 >= nj {
            return None;
        }
        raw.get(gj as usize * ni as usize + gi as usize)
            .copied()
            .flatten()
    };

    match resolved.projection {
        TargetKind::Source => {
            if px >= ni || py >= nj {
                return Ok(None);
            }
            // The source view paints grid (i, j) at pixel (i, j), then flips
            // vertically to face north-up; undo that flip to recover the row.
            let flip = source_flip_y(meta, resolved.flip_y);
            let gj = if flip { nj - 1 - py } else { py };
            let (gi, gj) = (px, gj);
            let value = value_at(gi as i64, gj as i64);
            // Geolocate when the grid's forward map is wired (lat/lon family);
            // otherwise report the value without a coordinate.
            let latlon = forward_geolocation_for(meta, ni, nj)
                .ok()
                .and_then(|f| f(gi, gj));
            let (lat, lon) = match latlon {
                Some((la, lo)) => (Some(la), Some(normalise_lon(lo))),
                None => (None, None),
            };
            Ok(Some(ProbeResult {
                lat,
                lon,
                value,
                grid_i: Some(gi as i32),
                grid_j: Some(gj as i32),
            }))
        }
        TargetKind::Warp(target) => {
            let (inverse, bbox) = warp_setup_for(meta, ni, nj)?;
            let (built, _) = build_warp_target(target, ni, nj, bbox, resolved.bounds)?;
            let (w, h) = built.dims();
            if px >= w || py >= h {
                return Ok(None);
            }
            // Undo the render's vertical flip to reach the warp raster row.
            let ry = if resolved.flip_y { h - 1 - py } else { py };
            let Some((lat, lon)) = built.pixel_to_lonlat(px, ry) else {
                // Off the globe (outside an azimuthal disc) — nothing there.
                return Ok(None);
            };
            let lon_n = normalise_lon(lon);
            match inverse(lat, lon) {
                Some(idx) => {
                    let (gi, gj) = (idx.i.round() as i64, idx.j.round() as i64);
                    Ok(Some(ProbeResult {
                        lat: Some(lat),
                        lon: Some(lon_n),
                        value: value_at(gi, gj),
                        grid_i: Some(gi as i32),
                        grid_j: Some(gj as i32),
                    }))
                }
                // On the globe but off this grid's coverage.
                None => Ok(Some(ProbeResult {
                    lat: Some(lat),
                    lon: Some(lon_n),
                    value: None,
                    grid_i: None,
                    grid_j: None,
                })),
            }
        }
    }
}

// --- Per-template warp-setup helpers ---------------------------------------

/// Lazily computes the source grid's `(lat_min, lat_max, lon_min, lon_max)`
/// extent. Deferred behind a thunk because the planar sources (Lambert,
/// polar stereographic) derive it from a 512-point-per-edge perimeter walk
/// that the azimuthal targets don't need — only the lat/lon-box targets
/// invoke it.
type BboxThunk = Box<dyn FnOnce() -> (f64, f64, f64, f64)>;

/// `(inverse map, lazy lat/lon-box extent)`. The inverse closure owns a
/// projector with its constants precomputed once; the bbox thunk builds its
/// own throwaway projector only if a lat/lon-box target calls it.
type WarpSetup = (Box<dyn Fn(f64, f64) -> Option<GridIndex>>, BboxThunk);

/// Resolve the lat/lon-box extent for a box target: the source bbox from the
/// thunk, replaced wholesale by the caller's manual window when present.
fn resolve_box_extent(
    bbox: BboxThunk,
    bounds_override: Option<RenderBounds>,
) -> (f64, f64, f64, f64) {
    match bounds_override {
        Some(b) => (b.lat_min, b.lat_max, b.lon_min, b.lon_max),
        None => bbox(),
    }
}

/// Axis-aligned lat/lon extent of a corner-pinned west-to-east grid, shared by
/// the lat/lon, Gaussian, and Mercator setups. Unwraps an antimeridian-crossing
/// grid (see `eastward_lon_span`) so the window is the grid's true span
/// (`lon_first .. lon_first + east_span`) rather than a collapsed `min..max`
/// sliver; `lon_max` may exceed 360°, which the warp targets accept (query
/// longitudes wrap to the nearest 360° multiple). A global grid's window
/// extends through the seam gap to the full 360° so the wrap column at the
/// eastern edge is painted too (the periodic sampler fills it).
fn latlon_family_bbox(
    lat_first: f64,
    lat_last: f64,
    lon_first: f64,
    lon_last: f64,
    ni: u32,
) -> (f64, f64, f64, f64) {
    let east_span = eastward_lon_span(lon_first, lon_last);
    let lon_span = if lon_grid_is_global(east_span, ni) {
        360.0
    } else {
        east_span
    };
    (
        lat_first.min(lat_last),
        lat_first.max(lat_last),
        lon_first,
        lon_first + lon_span,
    )
}

/// Whether the source grid is periodic in its column axis: a corner-pinned
/// west-to-east grid whose columns cover the full globe, so the warp may wrap
/// column indices across the seam (see `SourceGrid::periodic_i`). Rotated
/// lat/lon is judged in its rotated frame, matching its inverse map; planar
/// grids are never periodic.
fn source_grid_is_periodic(meta: &MessageMeta, ni: u32) -> bool {
    match meta.grid_type.as_deref() {
        Some("latlon")
        | Some("gaussian")
        | Some("mercator")
        | Some("rotated_latlon")
        | Some("reduced_latlon")
        | Some("reduced_gaussian") => match (meta.lon_first, meta.lon_last) {
            (Some(first), Some(last)) => lon_grid_is_global(eastward_lon_span(first, last), ni),
            _ => false,
        },
        _ => false,
    }
}

fn latlon_warp_setup(meta: &MessageMeta, ni: u32, nj: u32) -> napi::Result<WarpSetup> {
    let p = LatLonParams {
        ni,
        nj,
        lat_first: require_f64(meta.lat_first, "latFirst")?,
        lon_first: require_f64(meta.lon_first, "lonFirst")?,
        lat_last: require_f64(meta.lat_last, "latLast")?,
        lon_last: require_f64(meta.lon_last, "lonLast")?,
    };
    let inverse: Box<dyn Fn(f64, f64) -> Option<GridIndex>> =
        Box::new(move |lat, lon| latlon_inverse(&p, lat, lon));
    let bbox: BboxThunk = Box::new(move || {
        latlon_family_bbox(p.lat_first, p.lat_last, p.lon_first, p.lon_last, p.ni)
    });
    Ok((inverse, bbox))
}

fn gaussian_warp_setup(meta: &MessageMeta, ni: u32, nj: u32) -> napi::Result<WarpSetup> {
    let n_parallels = meta
        .gaussian_n_parallels
        .ok_or_else(|| napi::Error::from_reason("missing gaussianNParallels".to_string()))?
        as u32;
    let p = GaussianParams {
        ni,
        nj,
        lat_first: require_f64(meta.lat_first, "latFirst")?,
        lon_first: require_f64(meta.lon_first, "lonFirst")?,
        lat_last: require_f64(meta.lat_last, "latLast")?,
        lon_last: require_f64(meta.lon_last, "lonLast")?,
        n_parallels,
    };
    // `GaussianProjector` caches the row-ordered Gauss-Legendre lats
    // once; the inverse closure reuses it for every pixel.
    let projector = GaussianProjector::new(p);
    let inverse: Box<dyn Fn(f64, f64) -> Option<GridIndex>> =
        Box::new(move |lat, lon| projector.inverse(lat, lon));
    let bbox: BboxThunk = Box::new(move || {
        latlon_family_bbox(p.lat_first, p.lat_last, p.lon_first, p.lon_last, p.ni)
    });
    Ok((inverse, bbox))
}

fn mercator_warp_setup(meta: &MessageMeta, ni: u32, nj: u32) -> napi::Result<WarpSetup> {
    let p = MercatorParams {
        ni,
        nj,
        lat_first: require_f64(meta.lat_first, "latFirst")?,
        lon_first: require_f64(meta.lon_first, "lonFirst")?,
        lat_last: require_f64(meta.lat_last, "latLast")?,
        lon_last: require_f64(meta.lon_last, "lonLast")?,
    };
    let inverse: Box<dyn Fn(f64, f64) -> Option<GridIndex>> =
        Box::new(move |lat, lon| mercator_inverse(&p, lat, lon));
    // The §3.10 corner coordinates are geographic, so the source extent is the
    // axis-aligned box they span — same as the regular lat/lon source. (The
    // rows are non-uniform in latitude, but the box is still bounded by the
    // corner latitudes.)
    let bbox: BboxThunk = Box::new(move || {
        latlon_family_bbox(p.lat_first, p.lat_last, p.lon_first, p.lon_last, p.ni)
    });
    Ok((inverse, bbox))
}

fn rotated_latlon_warp_setup(meta: &MessageMeta, ni: u32, nj: u32) -> napi::Result<WarpSetup> {
    let p = RotatedLatLonParams {
        ni,
        nj,
        lat_first: require_f64(meta.lat_first, "latFirst")?,
        lon_first: require_f64(meta.lon_first, "lonFirst")?,
        lat_last: require_f64(meta.lat_last, "latLast")?,
        lon_last: require_f64(meta.lon_last, "lonLast")?,
        south_pole_lat: require_f64(meta.rotated_south_pole_lat, "rotatedSouthPoleLat")?,
        south_pole_lon: require_f64(meta.rotated_south_pole_lon, "rotatedSouthPoleLon")?,
        angle_of_rotation: require_f64(meta.rotated_angle_of_rotation, "rotatedAngleOfRotation")?,
    };
    // `RotatedLatLonProjector` caches the rotated-frame corner grid once; the
    // inverse closure reuses it for every output pixel.
    let projector = RotatedLatLonProjector::new(p);
    let inverse: Box<dyn Fn(f64, f64) -> Option<GridIndex>> =
        Box::new(move |lat, lon| projector.inverse(lat, lon));
    // The grid's corner coordinates are in the rotated frame, so — unlike the
    // geographic lat/lon source — the geographic extent is a curved region. The
    // projector walks the rotated-grid perimeter and unrotates it to derive a
    // tight lat/lon box.
    let bbox: BboxThunk = Box::new(move || RotatedLatLonProjector::new(p).lonlat_bbox());
    Ok((inverse, bbox))
}

fn lambert_warp_setup(meta: &MessageMeta, ni: u32, nj: u32) -> napi::Result<WarpSetup> {
    let p = LambertParams {
        earth_radius_m: meta
            .earth_radius_metres
            .unwrap_or(fieldglass_core::DEFAULT_EARTH_RADIUS_M),
        ni,
        nj,
        lat_first: require_f64(meta.lat_first, "latFirst")?,
        lon_first: require_f64(meta.lon_first, "lonFirst")?,
        lad: require_f64(meta.lambert_lad, "lambertLad")?,
        lov: require_f64(meta.lambert_lov, "lambertLov")?,
        dx_metres: require_nonzero_spacing(meta.lambert_dx_metres, "lambertDxMetres")?,
        dy_metres: require_nonzero_spacing(meta.lambert_dy_metres, "lambertDyMetres")?,
        latin1: require_f64(meta.lambert_latin1, "lambertLatin1")?,
        latin2: require_f64(meta.lambert_latin2, "lambertLatin2")?,
    };

    // `LambertProjector` precomputes the cone constants + origin once for the
    // inverse closure. The bbox thunk builds its own throwaway projector only
    // when a box target needs the antimeridian-aware perimeter bbox (see
    // `PlanarGridProjector::lonlat_bbox`).
    let projector = LambertProjector::new(p);
    let inverse: Box<dyn Fn(f64, f64) -> Option<GridIndex>> =
        Box::new(move |lat, lon| projector.inverse(lat, lon));
    let bbox: BboxThunk = Box::new(move || LambertProjector::new(p).lonlat_bbox());
    Ok((inverse, bbox))
}

fn polar_stereo_warp_setup(meta: &MessageMeta, ni: u32, nj: u32) -> napi::Result<WarpSetup> {
    let p = PolarStereoParams {
        earth_radius_m: meta
            .earth_radius_metres
            .unwrap_or(fieldglass_core::DEFAULT_EARTH_RADIUS_M),
        ni,
        nj,
        lat_first: require_f64(meta.lat_first, "latFirst")?,
        lon_first: require_f64(meta.lon_first, "lonFirst")?,
        lov: require_f64(meta.polar_stereo_lov, "polarStereoLov")?,
        lad: require_f64(meta.polar_stereo_lad, "polarStereoLad")?,
        dx_metres: require_nonzero_spacing(meta.polar_stereo_dx_metres, "polarStereoDxMetres")?,
        dy_metres: require_nonzero_spacing(meta.polar_stereo_dy_metres, "polarStereoDyMetres")?,
        south_pole: meta
            .polar_stereo_south_pole
            .ok_or_else(|| napi::Error::from_reason("missing polarStereoSouthPole".to_string()))?,
    };

    let projector = PolarStereoProjector::new(p);
    let inverse: Box<dyn Fn(f64, f64) -> Option<GridIndex>> =
        Box::new(move |lat, lon| projector.inverse(lat, lon));
    // Antimeridian-aware bbox of the four grid corners (same shape as
    // `lambert_warp_setup`). When the projection pole falls inside the grid,
    // every meridian is represented and the four-corner bbox is wrong: override
    // longitude to the full 360° and clamp latitude to the relevant pole. Built
    // lazily — only a box target invokes it.
    let bbox: BboxThunk = Box::new(move || {
        let projector = PolarStereoProjector::new(p);
        let pole_inside = projector.pole_inside_grid();
        let (mut lat_min, mut lat_max, mut lon_min, mut lon_max) = projector.lonlat_bbox();
        if pole_inside {
            lon_min = -180.0;
            lon_max = 180.0;
            if p.south_pole {
                lat_min = -90.0;
            } else {
                lat_max = 90.0;
            }
        }
        (lat_min, lat_max, lon_min, lon_max)
    });
    Ok((inverse, bbox))
}

fn geostationary_warp_setup(meta: &MessageMeta, ni: u32, nj: u32) -> napi::Result<WarpSetup> {
    let p = GeostationaryParams {
        ni,
        nj,
        h_metres: require_f64(meta.geos_height, "geosHeight")?,
        r_eq: require_f64(meta.geos_r_eq, "geosREq")?,
        r_pol: require_f64(meta.geos_r_pol, "geosRPol")?,
        sub_lon_deg: require_f64(meta.geos_sub_lon, "geosSubLon")?,
        sweep_x: meta
            .geos_sweep_x
            .ok_or_else(|| napi::Error::from_reason("missing geosSweepX".to_string()))?,
        x0: require_f64(meta.geos_x0, "geosX0")?,
        dx_rad: require_nonzero_spacing(meta.geos_dx_rad, "geosDxRad")?,
        y0: require_f64(meta.geos_y0, "geosY0")?,
        dy_rad: require_nonzero_spacing(meta.geos_dy_rad, "geosDyRad")?,
    };

    let projector = GeostationaryProjector::new(p);
    let inverse: Box<dyn Fn(f64, f64) -> Option<GridIndex>> =
        Box::new(move |lat, lon| projector.inverse(lat, lon));
    // Frame the on-disk extent: walk the scan-angle perimeter and take the tight
    // lat/lon box of the visible samples, so cropped sectors (GOES CONUS /
    // mesoscale, Meteosat sectors) frame their sector instead of a whole
    // hemisphere. A full disk whose perimeter is all limb has no on-disk sample;
    // fall back there to a generous box — the full latitude span and ±90° of
    // longitude around the sub-satellite point. Off-disk target pixels invert to
    // `None` and stay transparent, so the fallback only affects default framing,
    // never correctness.
    let bbox: BboxThunk = Box::new(move || {
        GeostationaryProjector::new(p)
            .lonlat_bbox()
            .unwrap_or_else(|| {
                let lon = p.sub_lon_deg;
                (-90.0, 90.0, lon - 90.0, lon + 90.0)
            })
    });
    Ok((inverse, bbox))
}

fn grid_ni(meta: &MessageMeta) -> napi::Result<u32> {
    meta.grid_ni
        .ok_or_else(|| napi::Error::from_reason("grid has no Ni".to_string()))
        .map(|n| n as u32)
}

fn grid_nj(meta: &MessageMeta) -> napi::Result<u32> {
    meta.grid_nj
        .ok_or_else(|| napi::Error::from_reason("grid has no Nj".to_string()))
        .map(|n| n as u32)
}

fn require_f64(value: Option<f64>, name: &str) -> napi::Result<f64> {
    match value {
        Some(v) if v.is_finite() => Ok(v),
        // A non-finite corner or parameter (a NaN NetCDF coordinate in a
        // corrupt file, say) would slip through the projection math as a NaN
        // grid index and paint wrong data; fail with a nameable field instead.
        Some(_) => Err(napi::Error::from_reason(format!("non-finite {name}"))),
        None => Err(napi::Error::from_reason(format!("missing {name}"))),
    }
}

/// Like [`require_f64`] but also rejects a zero grid spacing. Some sample GRIB2
/// files (e.g. eccodes' `polar_stereographic.grib2`) carry `Dx = Dy = 0`, which
/// collapses every grid point onto one location and would reproject to a blank
/// raster; surface a clear error instead. The webview also gates these grids
/// out of the picker via `MessageMeta.reprojectable`, so this is the belt-and-
/// braces guard for direct napi callers.
fn require_nonzero_spacing(value: Option<f64>, name: &str) -> napi::Result<f64> {
    match require_f64(value, name)? {
        v if v != 0.0 => Ok(v),
        _ => Err(napi::Error::from_reason(format!(
            "{name} is zero — this grid has no spatial extent and cannot be reprojected"
        ))),
    }
}

#[cfg(test)]
mod resolved_options_tests {
    use super::*;

    fn opts(projection: &str, resampling: &str) -> RenderOptions {
        RenderOptions {
            projection: projection.to_string(),
            projection_preset: None,
            center_lat: None,
            center_lon: None,
            resampling: resampling.to_string(),
            flip_y: false,
            range_min: None,
            range_max: None,
            bounds_lat_min: None,
            bounds_lat_max: None,
            bounds_lon_min: None,
            bounds_lon_max: None,
            colormap: None,
            reverse_colormap: None,
            scale_mode: None,
        }
    }

    #[test]
    fn bounds_parse_requires_all_four_edges_and_valid_box() {
        // Complete, valid box → Some.
        let mut o = opts("equirectangular", "nearest");
        o.bounds_lat_min = Some(10.0);
        o.bounds_lat_max = Some(50.0);
        o.bounds_lon_min = Some(-120.0);
        o.bounds_lon_max = Some(-40.0);
        assert_eq!(
            ResolvedOptions::parse(&o).unwrap().bounds,
            Some(RenderBounds {
                lat_min: 10.0,
                lat_max: 50.0,
                lon_min: -120.0,
                lon_max: -40.0,
            })
        );

        // Antimeridian window: lon_min < -180 is allowed (lon_max > lon_min).
        o.bounds_lon_min = Some(-183.0);
        o.bounds_lon_max = Some(-32.0);
        assert!(ResolvedOptions::parse(&o).unwrap().bounds.is_some());

        // Partial box → None (silent fallback to computed bounds).
        o.bounds_lon_max = None;
        assert!(ResolvedOptions::parse(&o).unwrap().bounds.is_none());

        // Inverted box → None.
        o.bounds_lon_min = Some(50.0);
        o.bounds_lon_max = Some(-50.0);
        assert!(ResolvedOptions::parse(&o).unwrap().bounds.is_none());
    }

    #[test]
    fn parses_valid_combinations() {
        let r = ResolvedOptions::parse(&opts("source", "nearest")).expect("source/nearest");
        assert!(matches!(r.projection, TargetKind::Source));
        assert_eq!(r.resampling, Resampling::Nearest);

        let r = ResolvedOptions::parse(&opts("equirectangular", "bilinear")).expect("eqr/bilinear");
        assert!(matches!(
            r.projection,
            TargetKind::Warp(WarpTarget::Equirectangular)
        ));
        assert_eq!(r.resampling, Resampling::Bilinear);

        let r = ResolvedOptions::parse(&opts("web_mercator", "nearest")).expect("merc/nearest");
        assert!(matches!(
            r.projection,
            TargetKind::Warp(WarpTarget::WebMercator)
        ));

        // Azimuthal targets resolve their preset into concrete parameters.
        let r = ResolvedOptions::parse(&opts("orthographic", "nearest")).expect("ortho/nearest");
        assert!(matches!(
            r.projection,
            TargetKind::Warp(WarpTarget::Orthographic { lat0, lon0 }) if lat0 == 0.0 && lon0 == 0.0
        ));
        let r =
            ResolvedOptions::parse(&opts("polar_stereographic", "nearest")).expect("polar/nearest");
        assert!(matches!(
            r.projection,
            TargetKind::Warp(WarpTarget::PolarStereographic {
                south_pole: false,
                ..
            })
        ));
    }

    #[test]
    fn orthographic_preset_selects_centre() {
        let mut o = opts("orthographic", "nearest");
        o.projection_preset = Some("pacific".to_string());
        assert!(matches!(
            ResolvedOptions::parse(&o).unwrap().projection,
            TargetKind::Warp(WarpTarget::Orthographic { lat0, lon0 }) if lat0 == 0.0 && lon0 == 180.0
        ));
        o.projection_preset = Some("indian".to_string());
        assert!(matches!(
            ResolvedOptions::parse(&o).unwrap().projection,
            TargetKind::Warp(WarpTarget::Orthographic { lat0, lon0 }) if lat0 == 0.0 && lon0 == 90.0
        ));
        o.projection_preset = Some("americas".to_string());
        assert!(matches!(
            ResolvedOptions::parse(&o).unwrap().projection,
            TargetKind::Warp(WarpTarget::Orthographic { lat0, lon0 }) if lat0 == 0.0 && lon0 == 270.0
        ));
        o.projection_preset = Some("north_pole".to_string());
        assert!(matches!(
            ResolvedOptions::parse(&o).unwrap().projection,
            TargetKind::Warp(WarpTarget::Orthographic { lat0, .. }) if lat0 == 90.0
        ));
        // Unknown preset falls back to the Atlantic default.
        o.projection_preset = Some("nonsense".to_string());
        assert!(matches!(
            ResolvedOptions::parse(&o).unwrap().projection,
            TargetKind::Warp(WarpTarget::Orthographic { lat0, lon0 }) if lat0 == 0.0 && lon0 == 0.0
        ));
    }

    #[test]
    fn polar_stereographic_preset_selects_hemisphere() {
        let mut o = opts("polar_stereographic", "nearest");
        o.projection_preset = Some("south".to_string());
        assert!(matches!(
            ResolvedOptions::parse(&o).unwrap().projection,
            TargetKind::Warp(WarpTarget::PolarStereographic {
                south_pole: true,
                ..
            })
        ));
    }

    #[test]
    fn orthographic_free_form_centre_overrides_preset() {
        // A free-form centre is honoured verbatim, with no preset present.
        let mut o = opts("orthographic", "nearest");
        o.center_lat = Some(37.5);
        o.center_lon = Some(-122.25);
        assert!(matches!(
            ResolvedOptions::parse(&o).unwrap().projection,
            TargetKind::Warp(WarpTarget::Orthographic { lat0, lon0 })
                if lat0 == 37.5 && lon0 == -122.25
        ));

        // Free-form centre wins over a preset that would say otherwise.
        o.projection_preset = Some("pacific".to_string());
        assert!(matches!(
            ResolvedOptions::parse(&o).unwrap().projection,
            TargetKind::Warp(WarpTarget::Orthographic { lat0, lon0 })
                if lat0 == 37.5 && lon0 == -122.25
        ));

        // Each component falls back independently: lon free-form, lat from preset.
        o.center_lat = None;
        o.center_lon = Some(10.0);
        assert!(matches!(
            ResolvedOptions::parse(&o).unwrap().projection,
            // "pacific" preset is (0.0, 180.0); lon overridden to 10, lat from preset.
            TargetKind::Warp(WarpTarget::Orthographic { lat0, lon0 })
                if lat0 == 0.0 && lon0 == 10.0
        ));
    }

    #[test]
    fn polar_stereographic_free_form_central_meridian() {
        // center_lon sets the central meridian; hemisphere still from the preset.
        let mut o = opts("polar_stereographic", "nearest");
        o.projection_preset = Some("south".to_string());
        o.center_lon = Some(-45.0);
        assert!(matches!(
            ResolvedOptions::parse(&o).unwrap().projection,
            TargetKind::Warp(WarpTarget::PolarStereographic { south_pole: true, lon0 })
                if lon0 == -45.0
        ));

        // No center_lon → central meridian defaults to 0°.
        o.center_lon = None;
        assert!(matches!(
            ResolvedOptions::parse(&o).unwrap().projection,
            TargetKind::Warp(WarpTarget::PolarStereographic { south_pole: true, lon0 })
                if lon0 == 0.0
        ));
    }

    #[test]
    fn world_targets_take_the_central_meridian_from_center_lon() {
        // center_lon sets the central meridian; no preset applies to these.
        for (name, lon0) in [
            ("mollweide", -100.0),
            ("robinson", 25.0),
            ("equal_earth", 0.0),
        ] {
            let mut o = opts(name, "nearest");
            o.center_lon = Some(lon0);
            let got = match ResolvedOptions::parse(&o).unwrap().projection {
                TargetKind::Warp(WarpTarget::Mollweide { lon0 })
                | TargetKind::Warp(WarpTarget::Robinson { lon0 })
                | TargetKind::Warp(WarpTarget::EqualEarth { lon0 }) => lon0,
                other => panic!("{name} did not resolve to a world target: {other:?}"),
            };
            assert_eq!(got, lon0, "{name} central meridian");
        }

        // Each name must resolve to *its own* target, not merely to some world
        // target — a copy-paste slip in the parse arms would pass the loop above.
        assert!(matches!(
            ResolvedOptions::parse(&opts("mollweide", "nearest"))
                .unwrap()
                .projection,
            TargetKind::Warp(WarpTarget::Mollweide { .. })
        ));
        assert!(matches!(
            ResolvedOptions::parse(&opts("robinson", "nearest"))
                .unwrap()
                .projection,
            TargetKind::Warp(WarpTarget::Robinson { .. })
        ));
        assert!(matches!(
            ResolvedOptions::parse(&opts("equal_earth", "nearest"))
                .unwrap()
                .projection,
            TargetKind::Warp(WarpTarget::EqualEarth { .. })
        ));

        // No center_lon → central meridian defaults to 0° (Greenwich-centred).
        assert!(matches!(
            ResolvedOptions::parse(&opts("robinson", "nearest"))
                .unwrap()
                .projection,
            TargetKind::Warp(WarpTarget::Robinson { lon0 }) if lon0 == 0.0
        ));
    }

    #[test]
    fn world_raster_keeps_each_projections_true_proportions() {
        // Height is the source's larger edge; width follows the projection's own
        // aspect, so the three world maps are *not* interchangeable rasters.
        assert_eq!(
            world_raster_dims(100, 200, Mollweide::ASPECT_RATIO),
            (400, 200)
        );
        assert_eq!(
            world_raster_dims(100, 200, Robinson::ASPECT_RATIO),
            (394, 200)
        );
        assert_eq!(
            world_raster_dims(100, 200, EqualEarth::ASPECT_RATIO),
            (411, 200)
        );
        // A degenerate source stays degenerate rather than wrapping.
        assert_eq!(world_raster_dims(0, 0, Robinson::ASPECT_RATIO), (0, 0));
    }

    #[test]
    fn colormap_defaults_to_viridis_and_resolves_by_name() {
        // A caller that never sets a colormap renders exactly as before.
        let o = opts("source", "nearest");
        let r = ResolvedOptions::parse(&o).expect("default colormap");
        assert_eq!(r.colormap.name(), "viridis");
        assert!(!r.reverse_colormap);

        // Every registered name resolves to itself, so the picker and the
        // renderer can't disagree about what a name means.
        for c in fieldglass_core::colormap::colormaps() {
            let mut o = opts("source", "nearest");
            o.colormap = Some(c.name().to_string());
            o.reverse_colormap = Some(true);
            let r = ResolvedOptions::parse(&o).expect("registered colormap");
            assert_eq!(r.colormap.name(), c.name());
            assert!(r.reverse_colormap);
        }
    }

    #[test]
    fn rejects_unknown_colormap_naming_the_known_ones() {
        let mut o = opts("source", "nearest");
        o.colormap = Some("jet".to_string());
        let err = ResolvedOptions::parse(&o).expect_err("unknown colormap must error");
        let msg = err.to_string();
        assert!(msg.contains("unknown colormap"), "{msg}");
        // The message must list what *is* available, or it isn't actionable.
        assert!(msg.contains("viridis"), "{msg}");
    }

    #[test]
    fn colormaps_export_feeds_the_picker_and_the_legend() {
        let all = colormaps();
        assert!(all.len() >= 8, "expected the full registry");
        assert_eq!(all[0].name, "viridis", "the default must come first");
        for c in &all {
            assert!(!c.label.is_empty(), "{} needs a picker label", c.name);
            assert!(
                c.kind == "sequential" || c.kind == "diverging",
                "{} has an odd kind {:?}",
                c.name,
                c.kind
            );
            // The legend needs real CSS colours, one per stop.
            assert_eq!(c.stops.len(), COLORMAP_STOPS);
            for s in &c.stops {
                assert!(
                    s.len() == 7 && s.starts_with('#'),
                    "{} emitted a bad stop {s:?}",
                    c.name
                );
            }
            // Every name the picker offers must resolve on the render side.
            let mut o = opts("source", "nearest");
            o.colormap = Some(c.name.clone());
            assert!(
                ResolvedOptions::parse(&o).is_ok(),
                "picker offers {} but the renderer rejects it",
                c.name
            );
        }
    }

    #[test]
    fn scale_mode_defaults_to_linear_and_resolves_log10() {
        // Unset renders exactly as before.
        let r = ResolvedOptions::parse(&opts("source", "nearest")).expect("default scale");
        assert_eq!(r.scale, ScaleMode::Linear);

        for (wire, want) in [("linear", ScaleMode::Linear), ("log10", ScaleMode::Log10)] {
            let mut o = opts("source", "nearest");
            o.scale_mode = Some(wire.to_string());
            let r = ResolvedOptions::parse(&o).expect("valid scale mode");
            assert_eq!(r.scale, want, "wire {wire:?}");
        }
    }

    #[test]
    fn rejects_unknown_scale_mode() {
        let mut o = opts("source", "nearest");
        o.scale_mode = Some("symlog".to_string());
        let err = ResolvedOptions::parse(&o).expect_err("unknown scale mode must error");
        let msg = err.to_string();
        assert!(msg.contains("unknown scale mode"), "{msg}");
        assert!(
            msg.contains("log10"),
            "message names the valid modes: {msg}"
        );
    }

    #[test]
    fn rejects_unknown_projection() {
        let err = ResolvedOptions::parse(&opts("aitoff", "nearest"))
            .expect_err("unknown projection must error");
        assert!(
            err.to_string().contains("unknown projection"),
            "error names the field, got: {err}"
        );
    }

    #[test]
    fn rejects_unknown_resampling() {
        let err = ResolvedOptions::parse(&opts("source", "bicubic"))
            .expect_err("unknown resampling must error");
        assert!(
            err.to_string().contains("unknown resampling"),
            "error names the field, got: {err}"
        );
    }
}

#[cfg(test)]
mod polar_stereo_warp_tests {
    use super::*;

    /// MessageMeta mirroring the `cmc_wind_300_2010052400_p012.grib`
    /// fixture: 135×95 polar-stereographic, 60 km at 60°N, north-polar.
    /// Only the fields warp_message actually consults are filled — every
    /// other Option-typed slot is left `None` to keep the synthetic
    /// minimal.
    fn cmc_polar_meta() -> MessageMeta {
        MessageMeta {
            earth_radius_metres: None,
            message_index: 0,
            offset_bytes: 0,
            parameter_name: String::new(),
            parameter_units: String::new(),
            parameter_abbreviation: String::new(),
            level: String::new(),
            level_type: String::new(),
            reference_time: String::new(),
            forecast_hours: 0,
            forecast_display: String::new(),
            originating_centre: String::new(),
            grid_type: Some("polar_stereo".to_string()),
            grid_ni: Some(135),
            grid_nj: Some(95),
            lat_first: Some(11.43),
            lon_first: Some(-110.27),
            lat_last: None,
            lon_last: None,
            format: "grib1".to_string(),
            edition: Some(1),
            discipline: None,
            total_length_bytes: None,
            production_status: None,
            data_type: None,
            lambert_lad: None,
            lambert_lov: None,
            lambert_dx_metres: None,
            lambert_dy_metres: None,
            lambert_latin1: None,
            lambert_latin2: None,
            gaussian_n_parallels: None,
            polar_stereo_lov: Some(247.0),
            polar_stereo_lad: Some(60.0),
            polar_stereo_dx_metres: Some(60_000.0),
            polar_stereo_dy_metres: Some(60_000.0),
            polar_stereo_south_pole: Some(false),
            rotated_south_pole_lat: None,
            rotated_south_pole_lon: None,
            rotated_angle_of_rotation: None,
            geos_sub_lon: None,
            geos_height: None,
            geos_r_eq: None,
            geos_r_pol: None,
            geos_sweep_x: None,
            geos_x0: None,
            geos_dx_rad: None,
            geos_y0: None,
            geos_dy_rad: None,
            packing: None,
            reprojectable: true,
            j_scans_positive: None,
        }
    }

    #[test]
    fn source_overlay_projects_onto_a_polar_stereo_grid() {
        // #72: the source-projection overlay must work for *projected* grids,
        // not just regular lat/lon — it reuses the grid's own inverse map. A
        // short polyline over North America (inside the CMC polar grid)
        // projects to a non-empty, shape-consistent run.
        let opts = RenderOptions {
            projection: "source".to_string(),
            projection_preset: None,
            center_lat: None,
            center_lon: None,
            resampling: "nearest".to_string(),
            flip_y: false,
            range_min: None,
            range_max: None,
            bounds_lat_min: None,
            bounds_lat_max: None,
            bounds_lon_min: None,
            bounds_lon_max: None,
            colormap: None,
            reverse_colormap: None,
            scale_mode: None,
        };
        let latlon = [40.0, -100.0, 41.0, -99.0, 42.0, -98.0];
        let out = project_overlay_impl(&cmc_polar_meta(), &opts, &latlon, &[3])
            .expect("source overlay on polar grid");
        let total: u32 = out.seg_lengths.iter().copied().sum();
        assert_eq!(total as usize * 2, out.xy.len(), "shape invariant");
        assert!(!out.xy.is_empty(), "polyline over the grid should project");
    }

    #[test]
    fn warps_polar_stereo_to_equirectangular() {
        let meta = cmc_polar_meta();
        // Synthetic uniform field — we're testing the warp geometry, not
        // value transport. Every present output pixel should read back
        // exactly 1.0.
        let raw: Vec<Option<f64>> = vec![Some(1.0); 135 * 95];
        let (values, mask, w, h, _bounds, summary) = warp_message(
            &meta,
            &raw,
            WarpTarget::Equirectangular,
            Resampling::Nearest,
            None,
        )
        .expect("warp");

        assert_eq!(w, 135);
        assert_eq!(h, 95);
        let present_count = mask.iter().filter(|&&m| m == 1).count();
        assert!(
            present_count > 0,
            "polar-stereo warp produced an entirely empty mask — \
             either the inverse map rejects every pixel or warp_setup \
             returned bad bounds"
        );
        for (i, &m) in mask.iter().enumerate() {
            if m == 1 {
                assert_eq!(values[i], 1.0, "present pixel {i} should be 1.0");
            }
        }
        assert!(
            summary.contains("polar_stereo") && summary.contains("equirectangular"),
            "summary should name source kind + target, got: {summary}"
        );
    }

    #[test]
    fn warps_polar_stereo_to_web_mercator() {
        // The Web Mercator target shares the polar-stereo source inverse map
        // and bbox; it just distributes rows in Mercator Y. Verify the path
        // produces a non-empty mask, transports values, clamps the latitude
        // extent into the Mercator band, and names the target in the summary.
        let meta = cmc_polar_meta();
        let raw: Vec<Option<f64>> = vec![Some(1.0); 135 * 95];
        let (values, mask, w, h, bounds, summary) = warp_message(
            &meta,
            &raw,
            WarpTarget::WebMercator,
            Resampling::Nearest,
            None,
        )
        .expect("mercator warp");

        assert_eq!(w, 135);
        assert_eq!(h, 95);
        let present_count = mask.iter().filter(|&&m| m == 1).count();
        assert!(present_count > 0, "mercator warp produced an empty mask");
        for (i, &m) in mask.iter().enumerate() {
            if m == 1 {
                assert_eq!(values[i], 1.0, "present pixel {i} should be 1.0");
            }
        }
        let (lat_min, lat_max, _, _) = bounds.expect("web mercator has bounds");
        assert!(
            lat_min >= -85.06 && lat_max <= 85.06,
            "lat extent must be clamped to the Mercator band, got {lat_min}..{lat_max}"
        );
        assert!(
            summary.contains("polar_stereo") && summary.contains("web mercator"),
            "summary should name source kind + target, got: {summary}"
        );
    }

    #[test]
    fn warps_polar_stereo_to_azimuthal_targets() {
        // The orthographic and polar-stereographic *targets* fit a disc to the
        // raster: they share the source inverse map but report no lat/lon-box
        // extent. Verify both produce a non-empty mask and the right summary,
        // and that bounds come back `None`.
        let meta = cmc_polar_meta();
        let raw: Vec<Option<f64>> = vec![Some(1.0); 135 * 95];

        for (target, name) in [
            (
                WarpTarget::Orthographic {
                    lat0: 90.0,
                    lon0: 0.0,
                },
                "orthographic",
            ),
            (
                WarpTarget::PolarStereographic {
                    south_pole: false,
                    lon0: 0.0,
                },
                "polar stereographic",
            ),
        ] {
            let (values, mask, w, h, bounds, summary) =
                warp_message(&meta, &raw, target, Resampling::Nearest, None)
                    .unwrap_or_else(|e| panic!("{name} warp failed: {e}"));
            // Azimuthal discs render into a square raster (side = the larger
            // source axis) so the globe stays circular rather than stretching
            // into an ellipse on the 135×95 source.
            assert_eq!((w, h), (135, 135));
            let present = mask.iter().filter(|&&m| m == 1).count();
            assert!(present > 0, "{name} target produced an empty mask");
            for (i, &m) in mask.iter().enumerate() {
                if m == 1 {
                    assert_eq!(values[i], 1.0, "{name} present pixel {i} should be 1.0");
                }
            }
            assert!(bounds.is_none(), "{name} target has no lat/lon-box extent");
            assert!(
                summary.contains("polar_stereo") && summary.contains(name),
                "{name} summary should name source + target, got: {summary}"
            );
        }
    }

    #[test]
    fn warps_south_polar_stereo_to_equirectangular() {
        // Mirror the CMC tile into the southern hemisphere: south-pole
        // projection, negative lat_first. Exercises the `sign = -1` branch
        // through the full warp_message → polar_stereo_warp_setup path,
        // not just the projection-level round-trip tests.
        let meta = MessageMeta {
            lat_first: Some(-11.43),
            polar_stereo_south_pole: Some(true),
            ..cmc_polar_meta()
        };
        let raw: Vec<Option<f64>> = vec![Some(1.0); 135 * 95];
        let (values, mask, w, h, _bounds, summary) = warp_message(
            &meta,
            &raw,
            WarpTarget::Equirectangular,
            Resampling::Nearest,
            None,
        )
        .expect("south-polar warp");

        assert_eq!(w, 135);
        assert_eq!(h, 95);
        let present_count = mask.iter().filter(|&&m| m == 1).count();
        assert!(
            present_count > 0,
            "south-polar warp produced an entirely empty mask"
        );
        for (i, &m) in mask.iter().enumerate() {
            if m == 1 {
                assert_eq!(values[i], 1.0, "present pixel {i} should be 1.0");
            }
        }
        assert!(summary.contains("polar_stereo") && summary.contains("equirectangular"));
    }

    #[test]
    fn warps_hemispheric_grid_with_pole_inside() {
        // A synthetic hemispheric grid whose projected extent surrounds the
        // pole (same geometry as the projection-level
        // `polar_stereo_pole_inside_grid_detection` test). Two things this
        // pins that the regional CMC test does not:
        //   1. negative `dy_metres` (south-scanning) is handled by the warp
        //      setup, not just the projector unit test;
        //   2. the 360°-longitude / pole-clamp override path in
        //      polar_stereo_warp_setup is reachable through warp_message
        //      and produces a fully-covered raster instead of a thin
        //      four-corner sliver.
        let meta = MessageMeta {
            grid_ni: Some(4),
            grid_nj: Some(4),
            lat_first: Some(50.8),
            lon_first: Some(-135.0),
            polar_stereo_lov: Some(0.0),
            polar_stereo_dx_metres: Some(2_000_000.0),
            polar_stereo_dy_metres: Some(-2_000_000.0),
            polar_stereo_south_pole: Some(false),
            ..cmc_polar_meta()
        };
        let raw: Vec<Option<f64>> = vec![Some(1.0); 4 * 4];
        let (_values, mask, w, h, _bounds, _summary) = warp_message(
            &meta,
            &raw,
            WarpTarget::Equirectangular,
            Resampling::Nearest,
            None,
        )
        .expect("hemispheric warp");

        assert_eq!((w, h), (4, 4));
        // With the pole inside the grid the target spans the full hemisphere,
        // so a clear majority of the 4×4 output pixels resolve to a source
        // sample rather than the handful a four-corner bbox would cover.
        let present_count = mask.iter().filter(|&&m| m == 1).count();
        assert!(
            present_count >= 8,
            "pole-inside grid should fill most of the raster, got {present_count}/16 present"
        );
    }

    #[test]
    fn warps_grid_with_pole_exactly_on_origin() {
        // lat_first = 90° puts the first scanned point at the pole, so the
        // projected grid origin is exactly (0, 0). `pole_inside_grid` uses
        // inclusive bounds, so this edge case must still take the
        // 360°-longitude override and warp without panicking.
        let meta = MessageMeta {
            grid_ni: Some(4),
            grid_nj: Some(4),
            lat_first: Some(90.0),
            lon_first: Some(0.0),
            polar_stereo_lov: Some(0.0),
            polar_stereo_dx_metres: Some(2_000_000.0),
            polar_stereo_dy_metres: Some(2_000_000.0),
            polar_stereo_south_pole: Some(false),
            ..cmc_polar_meta()
        };
        let raw: Vec<Option<f64>> = vec![Some(1.0); 4 * 4];
        let (_values, mask, w, h, _bounds, _summary) = warp_message(
            &meta,
            &raw,
            WarpTarget::Equirectangular,
            Resampling::Nearest,
            None,
        )
        .expect("pole-on-origin warp");
        assert_eq!((w, h), (4, 4));
        assert!(
            mask.contains(&1),
            "pole-on-origin grid should still resolve some pixels"
        );
    }

    #[test]
    fn polar_stereo_warp_errors_without_required_fields() {
        // Missing polarStereoLov — warp setup must surface a clear error
        // naming the field rather than silently falling back to a default.
        let bad = MessageMeta {
            polar_stereo_lov: None,
            ..cmc_polar_meta()
        };
        let raw: Vec<Option<f64>> = vec![Some(1.0); 135 * 95];
        let err = warp_message(
            &bad,
            &raw,
            WarpTarget::Equirectangular,
            Resampling::Nearest,
            None,
        )
        .expect_err("missing lov must error");
        assert!(
            err.to_string().contains("polarStereoLov"),
            "error should name the missing field, got: {err}"
        );
    }

    #[test]
    fn bounds_override_replaces_computed_extent_and_echoes_back() {
        let meta = cmc_polar_meta();
        let raw: Vec<Option<f64>> = vec![Some(1.0); 135 * 95];

        // Default: no override → echoed bounds are the computed source extent.
        let (.., default_bounds, _) = warp_message(
            &meta,
            &raw,
            WarpTarget::Equirectangular,
            Resampling::Nearest,
            None,
        )
        .expect("default warp");
        let default_bounds = default_bounds.expect("equirectangular has bounds");

        // Explicit window → that window is rendered and echoed back verbatim.
        let window = RenderBounds {
            lat_min: 30.0,
            lat_max: 60.0,
            lon_min: -140.0,
            lon_max: -60.0,
        };
        let (_v, _m, _w, _h, used, _s) = warp_message(
            &meta,
            &raw,
            WarpTarget::Equirectangular,
            Resampling::Nearest,
            Some(window),
        )
        .expect("windowed warp");
        assert_eq!(used, Some((30.0, 60.0, -140.0, -60.0)));
        assert_ne!(
            used.unwrap(),
            default_bounds,
            "override should differ from the computed default"
        );
    }

    #[test]
    fn signed_grid_increments_encode_scan_direction() {
        // Default scan (i: W→E, j: S→N) keeps positive magnitudes.
        assert_eq!(
            signed_grid_increments(5000.0, 5000.0, false, true),
            (5000.0, 5000.0)
        );
        // j scans north→south ⇒ dy negative.
        assert_eq!(
            signed_grid_increments(5000.0, 5000.0, false, false),
            (5000.0, -5000.0)
        );
        // i scans east→west ⇒ dx negative.
        assert_eq!(
            signed_grid_increments(5000.0, 5000.0, true, true),
            (-5000.0, 5000.0)
        );
        // Operates on magnitude, so it is idempotent on already-signed input.
        assert_eq!(
            signed_grid_increments(-5000.0, -5000.0, false, true),
            (5000.0, 5000.0)
        );
    }

    #[test]
    fn web_mercator_band_outside_clamp_is_rejected() {
        // A manual latitude band lying entirely poleward of the ±85.0511°
        // Web Mercator cutoff clamps to a single edge (zero Y span), which
        // would smear every row to one latitude. It must be rejected instead.
        let meta = cmc_polar_meta();
        let raw = vec![Some(1.0); 135 * 95];
        let band = RenderBounds {
            lat_min: 86.0,
            lat_max: 88.0,
            lon_min: -10.0,
            lon_max: 10.0,
        };
        let err = warp_message(
            &meta,
            &raw,
            WarpTarget::WebMercator,
            Resampling::Nearest,
            Some(band),
        )
        .expect_err("a lat band entirely outside ±85.05° must be rejected");
        assert!(
            err.reason.contains("Web Mercator"),
            "expected a Web-Mercator-band error, got: {}",
            err.reason
        );
    }

    /// MessageMeta for a GRIB2 §3.10 Mercator grid (100×100 over the western
    /// tropics). The Mercator inverse map is pinned by the corner coordinates,
    /// so — like the regular lat/lon source — no metric grid spacing is needed.
    fn mercator_meta() -> MessageMeta {
        MessageMeta {
            grid_type: Some("mercator".to_string()),
            grid_ni: Some(100),
            grid_nj: Some(100),
            lat_first: Some(0.0),
            lon_first: Some(-100.0),
            lat_last: Some(30.0),
            lon_last: Some(-60.0),
            format: "grib2".to_string(),
            edition: Some(2),
            ..cmc_polar_meta()
        }
    }

    #[test]
    fn warps_mercator_to_equirectangular() {
        // #119: a GRIB2 Mercator (§3.10) source grid must reproject. Synthetic
        // uniform field — testing warp geometry, not value transport.
        let meta = mercator_meta();
        let raw: Vec<Option<f64>> = vec![Some(1.0); 100 * 100];
        let (values, mask, w, h, bounds, summary) = warp_message(
            &meta,
            &raw,
            WarpTarget::Equirectangular,
            Resampling::Nearest,
            None,
        )
        .expect("mercator warp");

        assert_eq!((w, h), (100, 100));
        let present = mask.iter().filter(|&&m| m == 1).count();
        assert!(present > 0, "mercator warp produced an empty mask");
        for (i, &m) in mask.iter().enumerate() {
            if m == 1 {
                assert_eq!(values[i], 1.0, "present pixel {i} should be 1.0");
            }
        }
        // The source extent is the geographic corner box.
        let (lat_min, lat_max, lon_min, lon_max) = bounds.expect("mercator has bounds");
        assert!(
            lat_min >= -0.01 && lat_max <= 30.01,
            "lat box {lat_min}..{lat_max}"
        );
        assert!(
            lon_min >= -100.01 && lon_max <= -59.99,
            "lon box {lon_min}..{lon_max}"
        );
        assert!(
            summary.contains("mercator") && summary.contains("equirectangular"),
            "summary should name source kind + target, got: {summary}"
        );
    }

    /// MessageMeta for a GRIB2 §3.30 Lambert grid (100×100, 3 km, CONUS-like),
    /// the way `build_grib2_message_meta` populates one.
    fn lambert_meta() -> MessageMeta {
        MessageMeta {
            grid_type: Some("lambert".to_string()),
            grid_ni: Some(100),
            grid_nj: Some(100),
            lat_first: Some(21.14),
            lon_first: Some(-122.72),
            lat_last: None,
            lon_last: None,
            lambert_lad: Some(38.5),
            lambert_lov: Some(-97.5),
            lambert_dx_metres: Some(3000.0),
            lambert_dy_metres: Some(3000.0),
            lambert_latin1: Some(38.5),
            lambert_latin2: Some(38.5),
            format: "grib2".to_string(),
            edition: Some(2),
            ..cmc_polar_meta()
        }
    }

    #[test]
    fn warps_grib2_lambert_to_equirectangular() {
        // #119 audit half: confirm the GRIB2 Lambert (§3.30) params reach the
        // warp and reproject a non-empty field — the same path GRIB1 Lambert
        // already uses.
        let meta = lambert_meta();
        let raw: Vec<Option<f64>> = vec![Some(1.0); 100 * 100];
        let (values, mask, w, h, _bounds, summary) = warp_message(
            &meta,
            &raw,
            WarpTarget::Equirectangular,
            Resampling::Nearest,
            None,
        )
        .expect("lambert warp");

        assert_eq!((w, h), (100, 100));
        let present = mask.iter().filter(|&&m| m == 1).count();
        assert!(present > 0, "lambert warp produced an empty mask");
        for (i, &m) in mask.iter().enumerate() {
            if m == 1 {
                assert_eq!(values[i], 1.0, "present pixel {i} should be 1.0");
            }
        }
        assert!(
            summary.contains("lambert") && summary.contains("equirectangular"),
            "summary should name source kind + target, got: {summary}"
        );
    }

    /// MessageMeta for a GRIB2 §3.1 rotated lat/lon grid, mirroring the
    /// committed `rotated_latlon_surface.grib2` fixture: 16×31 grid, rotated
    /// corners (60,0)→(0,30), southern pole at geographic (0,0), no rotation.
    fn rotated_latlon_meta() -> MessageMeta {
        MessageMeta {
            grid_type: Some("rotated_latlon".to_string()),
            grid_ni: Some(16),
            grid_nj: Some(31),
            lat_first: Some(60.0),
            lon_first: Some(0.0),
            lat_last: Some(0.0),
            lon_last: Some(30.0),
            rotated_south_pole_lat: Some(0.0),
            rotated_south_pole_lon: Some(0.0),
            rotated_angle_of_rotation: Some(0.0),
            geos_sub_lon: None,
            geos_height: None,
            geos_r_eq: None,
            geos_r_pol: None,
            geos_sweep_x: None,
            geos_x0: None,
            geos_dx_rad: None,
            geos_y0: None,
            geos_dy_rad: None,
            format: "grib2".to_string(),
            edition: Some(2),
            ..cmc_polar_meta()
        }
    }

    #[test]
    fn warps_grib2_rotated_latlon_to_equirectangular() {
        // #120: a GRIB2 rotated lat/lon (§3.1) source grid must reproject. The
        // corners are rotated-frame coordinates, so the warp rotates each
        // geographic output point into the grid's frame before sampling.
        // Synthetic uniform field — testing warp geometry, not value transport.
        let meta = rotated_latlon_meta();
        let raw: Vec<Option<f64>> = vec![Some(1.0); 16 * 31];
        let (values, mask, w, h, bounds, summary) = warp_message(
            &meta,
            &raw,
            WarpTarget::Equirectangular,
            Resampling::Nearest,
            None,
        )
        .expect("rotated lat/lon warp");

        let present = mask.iter().filter(|&&m| m == 1).count();
        assert!(present > 0, "rotated warp produced an empty mask");
        for (i, &m) in mask.iter().enumerate() {
            if m == 1 {
                assert_eq!(values[i], 1.0, "present pixel {i} should be 1.0");
            }
        }
        // The geographic extent is the unrotated perimeter box. With the pole at
        // (0,0) the fixture's grid sweeps the high-latitude north side; the
        // reported box must be non-degenerate and stay within valid ranges.
        let (lat_min, lat_max, lon_min, lon_max) = bounds.expect("rotated grid has bounds");
        assert!(
            lat_max > lat_min && lat_max <= 90.01,
            "lat box {lat_min}..{lat_max}"
        );
        assert!(lon_max > lon_min, "lon box {lon_min}..{lon_max}");
        assert!(w > 0 && h > 0);
        assert!(
            summary.contains("rotated_latlon") && summary.contains("equirectangular"),
            "summary should name source kind + target, got: {summary}"
        );
    }
}

#[cfg(test)]
mod reprojectable_tests {
    use super::grid_is_reprojectable;

    #[test]
    fn latlon_gaussian_and_mercator_reproject_when_scanning_west_to_east() {
        // These three are pinned by their corner coordinates alone, so they
        // never depend on a metric grid spacing.
        assert!(grid_is_reprojectable(Some("latlon"), None, None, false));
        assert!(grid_is_reprojectable(Some("gaussian"), None, None, false));
        assert!(grid_is_reprojectable(Some("mercator"), None, None, false));
        // Rotated lat/lon is pinned by its rotated corners + pole position, so
        // it too needs no metric spacing.
        assert!(grid_is_reprojectable(
            Some("rotated_latlon"),
            None,
            None,
            false
        ));
    }

    #[test]
    fn descending_scan_keeps_corner_pinned_grids_in_source_projection() {
        // A −i (east-to-west) scan would be misread by the west-to-east
        // inverse maps as an antimeridian wrap, so those grids don't offer
        // reprojection.
        assert!(!grid_is_reprojectable(Some("latlon"), None, None, true));
        assert!(!grid_is_reprojectable(Some("gaussian"), None, None, true));
        assert!(!grid_is_reprojectable(Some("mercator"), None, None, true));
        assert!(!grid_is_reprojectable(
            Some("rotated_latlon"),
            None,
            None,
            true
        ));
        // The planar projections bake the scan sign into Dx, so the flag
        // doesn't gate them.
        assert!(grid_is_reprojectable(
            Some("lambert"),
            Some(-81_271.0),
            Some(81_271.0),
            true
        ));
    }

    #[test]
    fn planar_grids_need_nonzero_spacing() {
        // Real spacing → reprojectable.
        assert!(grid_is_reprojectable(
            Some("lambert"),
            Some(81_271.0),
            Some(81_271.0),
            false
        ));
        assert!(grid_is_reprojectable(
            Some("polar_stereo"),
            Some(60_000.0),
            Some(60_000.0),
            false
        ));
        // Space view carries its scan-angle increments through the same
        // spacing slots; a real apparent diameter → reprojectable.
        assert!(grid_is_reprojectable(
            Some("space_view"),
            Some(5.6e-5),
            Some(5.6e-5),
            false
        ));
        // Degenerate Dx/Dy (eccodes' polar_stereographic sample) → not.
        assert!(!grid_is_reprojectable(
            Some("polar_stereo"),
            Some(0.0),
            Some(0.0),
            false
        ));
        assert!(!grid_is_reprojectable(Some("lambert"), None, None, false));
        // Orthographic space view (no camera altitude) leaves the increments
        // unset, so it does not reproject.
        assert!(!grid_is_reprojectable(
            Some("space_view"),
            None,
            None,
            false
        ));
    }

    #[test]
    fn unsupported_grid_types_are_not_reprojectable() {
        assert!(!grid_is_reprojectable(Some("unknown"), None, None, false));
        assert!(!grid_is_reprojectable(None, None, None, false));
    }
}

#[cfg(test)]
mod friendly_packing_tests {
    use super::friendly_packing;

    #[test]
    fn maps_grib1_and_grib2_labels_to_friendly_names() {
        assert_eq!(friendly_packing("grid_simple"), "Simple grid-point");
        assert_eq!(friendly_packing("simple"), "Simple grid-point");
        assert_eq!(friendly_packing("grid_complex"), "Complex packing");
        assert_eq!(friendly_packing("complex"), "Complex packing");
        assert_eq!(friendly_packing("grid_ieee"), "IEEE float");
        assert_eq!(friendly_packing("ieee"), "IEEE float");
        assert_eq!(friendly_packing("grid_jpeg"), "JPEG 2000");
        assert_eq!(friendly_packing("jpeg"), "JPEG 2000");
        assert_eq!(friendly_packing("grid_png"), "PNG");
        assert_eq!(friendly_packing("png"), "PNG");
        assert_eq!(friendly_packing("grid_ccsds"), "CCSDS");
        assert_eq!(friendly_packing("ccsds"), "CCSDS");
        assert_eq!(friendly_packing("grid_run_length"), "Run-length");
        assert_eq!(friendly_packing("run_length"), "Run-length");
        assert_eq!(
            friendly_packing("grid_simple_log_preprocessing"),
            "Simple packing (log)"
        );
        assert_eq!(
            friendly_packing("simple_log_preprocessing"),
            "Simple packing (log)"
        );
        assert_eq!(friendly_packing("grid_simple_matrix"), "Matrix of values");
        assert_eq!(
            friendly_packing("grid_second_order"),
            "Second-order (SPD-2)"
        );
        assert_eq!(
            friendly_packing("grid_second_order_SPD3"),
            "Second-order (SPD-3)"
        );
        assert_eq!(
            friendly_packing("grid_second_order_row_by_row"),
            "Second-order (row-by-row)"
        );
        assert_eq!(friendly_packing("spectral"), "Spherical harmonic");
    }

    #[test]
    fn names_the_scheme_behind_grib2_unsupported_templates() {
        // Template 5.3 (complex + spatial differencing) is still undecoded, so it
        // surfaces via the unsupported path; the 5.2 arm is kept as a defensive
        // fallback even though 5.2 now decodes to "complex" (see the test above).
        assert_eq!(
            friendly_packing("unsupported(5.2)"),
            "Complex packing (5.2)"
        );
        assert_eq!(
            friendly_packing("unsupported(5.3)"),
            "Complex packing (5.3)"
        );
        // Templates 5.2 (complex), 5.4 (IEEE), 5.40 (JPEG 2000), 5.41 (PNG), and
        // 5.42 (CCSDS) are now decoded, so they surface as
        // "complex" / "ieee" / "jpeg" / "png" / "ccsds", never as
        // unsupported(5.N); the unsupported(5.N) arms below stay as defensive
        // fallbacks, and an unknown number still falls back to a generic label.
        assert_eq!(friendly_packing("unsupported(5.40)"), "JPEG 2000 (5.40)");
        assert_eq!(friendly_packing("unsupported(5.41)"), "PNG (5.41)");
        assert_eq!(friendly_packing("unsupported(5.42)"), "CCSDS (5.42)");
        assert_eq!(friendly_packing("unsupported(5.99)"), "Unsupported (5.99)");
    }

    #[test]
    fn falls_back_to_the_raw_identifier_when_unmapped() {
        assert_eq!(
            friendly_packing("some_future_packing"),
            "some_future_packing"
        );
    }
}

#[cfg(test)]
mod overlay_projection_tests {
    use super::*;

    /// A global 1°-resolution lat/lon grid (361×181, lat 90..-90, lon
    /// -180..180). Under the equirectangular target a vertex projects to a
    /// predictable pixel: `px = lon + 180`, `py = 90 - lat`.
    fn global_latlon_meta() -> MessageMeta {
        MessageMeta {
            earth_radius_metres: None,
            message_index: 0,
            offset_bytes: 0,
            parameter_name: String::new(),
            parameter_units: String::new(),
            parameter_abbreviation: String::new(),
            level: String::new(),
            level_type: String::new(),
            reference_time: String::new(),
            forecast_hours: 0,
            forecast_display: String::new(),
            originating_centre: String::new(),
            grid_type: Some("latlon".to_string()),
            grid_ni: Some(361),
            grid_nj: Some(181),
            lat_first: Some(90.0),
            lon_first: Some(-180.0),
            lat_last: Some(-90.0),
            lon_last: Some(180.0),
            format: "grib1".to_string(),
            edition: Some(1),
            discipline: None,
            total_length_bytes: None,
            production_status: None,
            data_type: None,
            lambert_lad: None,
            lambert_lov: None,
            lambert_dx_metres: None,
            lambert_dy_metres: None,
            lambert_latin1: None,
            lambert_latin2: None,
            gaussian_n_parallels: None,
            polar_stereo_lov: None,
            polar_stereo_lad: None,
            polar_stereo_dx_metres: None,
            polar_stereo_dy_metres: None,
            polar_stereo_south_pole: None,
            rotated_south_pole_lat: None,
            rotated_south_pole_lon: None,
            rotated_angle_of_rotation: None,
            geos_sub_lon: None,
            geos_height: None,
            geos_r_eq: None,
            geos_r_pol: None,
            geos_sweep_x: None,
            geos_x0: None,
            geos_dx_rad: None,
            geos_y0: None,
            geos_dy_rad: None,
            packing: None,
            reprojectable: true,
            j_scans_positive: None,
        }
    }

    fn overlay_opts(projection: &str) -> RenderOptions {
        RenderOptions {
            projection: projection.to_string(),
            projection_preset: None,
            center_lat: None,
            center_lon: None,
            resampling: "nearest".to_string(),
            flip_y: false,
            range_min: None,
            range_max: None,
            bounds_lat_min: None,
            bounds_lat_max: None,
            bounds_lon_min: None,
            bounds_lon_max: None,
            colormap: None,
            reverse_colormap: None,
            scale_mode: None,
        }
    }

    #[test]
    fn source_projection_projects_through_the_inverse_map() {
        // The source raster paints grid point (i, j) at pixel (i, j), so the
        // overlay projects through the grid's own inverse map. For this global
        // 1° lat/lon grid that lands at px = lon + 180, py = 90 - lat —
        // matching the equirectangular target's pixels.
        let out = project_overlay_impl(
            &global_latlon_meta(),
            &overlay_opts("source"),
            &[45.0, 0.0, 45.0, 10.0],
            &[2],
        )
        .expect("source overlay");
        assert_eq!(out.seg_lengths, vec![2]);
        assert!((out.xy[0] - 180.0).abs() < 1e-6, "lon 0 → px {}", out.xy[0]);
        assert!((out.xy[1] - 45.0).abs() < 1e-6, "lat 45 → py {}", out.xy[1]);
        assert!(
            (out.xy[2] - 190.0).abs() < 1e-6,
            "lon 10 → px {}",
            out.xy[2]
        );
    }

    #[test]
    fn equirectangular_projects_to_predictable_pixels() {
        let out = project_overlay_impl(
            &global_latlon_meta(),
            &overlay_opts("equirectangular"),
            &[45.0, 0.0, 45.0, 10.0], // (lat 45, lon 0), (lat 45, lon 10)
            &[2],
        )
        .expect("equirect overlay");
        assert_eq!(out.seg_lengths, vec![2]);
        assert_eq!(out.xy.len(), 4);
        // px = lon + 180, py = 90 - lat.
        assert!((out.xy[0] - 180.0).abs() < 1e-6, "lon 0 → px {}", out.xy[0]);
        assert!((out.xy[1] - 45.0).abs() < 1e-6, "lat 45 → py {}", out.xy[1]);
        assert!(
            (out.xy[2] - 190.0).abs() < 1e-6,
            "lon 10 → px {}",
            out.xy[2]
        );
    }

    #[test]
    fn flip_y_mirrors_overlay_rows() {
        let mut o = overlay_opts("equirectangular");
        o.flip_y = true;
        let out = project_overlay_impl(&global_latlon_meta(), &o, &[45.0, 0.0, 45.0, 10.0], &[2])
            .expect("flipped overlay");
        // height = nj = 181 → py flips to (181 - 1) - 45 = 135.
        assert!((out.xy[1] - 135.0).abs() < 1e-6, "flipped py {}", out.xy[1]);
        assert!((out.xy[0] - 180.0).abs() < 1e-6, "x unaffected by flip");
    }

    #[test]
    fn output_shape_invariant_holds() {
        // sum(seg_lengths) * 2 == xy.len() for every projection.
        for projection in [
            "source",
            "equirectangular",
            "web_mercator",
            "orthographic",
            "polar_stereographic",
        ] {
            let out = project_overlay_impl(
                &global_latlon_meta(),
                &overlay_opts(projection),
                &[80.0, 0.0, 81.0, 1.0, 82.0, 2.0],
                &[3],
            )
            .unwrap_or_else(|e| panic!("{projection} overlay: {e}"));
            let total: u32 = out.seg_lengths.iter().copied().sum();
            assert_eq!(
                total as usize * 2,
                out.xy.len(),
                "{projection}: seg_lengths must account for every xy pair"
            );
        }
    }

    #[test]
    fn source_flip_y_orients_south_to_north_grids() {
        let mut meta = global_latlon_meta();

        // North→south scan (jScansPositively = 0): row 0 is already north, so
        // the source view needs no intrinsic flip; the user toggle passes through.
        meta.j_scans_positive = Some(false);
        assert!(!source_flip_y(&meta, false));
        assert!(source_flip_y(&meta, true));

        // South→north scan (NBM): row 0 is south, so the source view flips by
        // default and the user toggle rides on top of that.
        meta.j_scans_positive = Some(true);
        assert!(source_flip_y(&meta, false));
        assert!(!source_flip_y(&meta, true));

        // No scan flag (predefined GRIB1, NetCDF): treated as no intrinsic flip.
        meta.j_scans_positive = None;
        assert!(!source_flip_y(&meta, false));
        assert!(source_flip_y(&meta, true));
    }
}

#[cfg(test)]
mod netcdf_slice_tests {
    use super::*;
    use fieldglass_netcdf::DatasetView;

    const ERSST: &[u8] =
        include_bytes!("../../fieldglass-netcdf/tests/fixtures/ersst_v5_187001_cdf1.nc");
    /// NetCDF-4 / HDF5 fixture: `temperature(time, lat, lon)` over a tiny regular
    /// lat/lon grid, with `lat`/`lon` coordinate variables and a pure `nv`
    /// dimension (decision 0003).
    const DIMSCALE: &[u8] =
        include_bytes!("../../fieldglass-netcdf/tests/fixtures/netcdf4_dimscale.nc");

    /// Build a handle from raw bytes without crossing the napi `Buffer` boundary
    /// (which would need a Node runtime) — mirrors `from_bytes`'s body.
    fn handle(bytes: &[u8]) -> NetcdfHandle {
        let reader = NetcdfReader::from_bytes(bytes.to_vec()).unwrap();
        let view = match &reader.backing {
            NetcdfBacking::Classic(h) => DatasetView::from_classic(h),
            NetcdfBacking::Hdf5(_) => {
                DatasetView::from_hdf5(&reader.hdf5_metadata().expect("dimension-scale resolution"))
            }
        };
        NetcdfHandle {
            reader,
            view,
            decoded: Mutex::new(std::collections::HashMap::new()),
        }
    }

    fn opts(projection: &str) -> RenderOptions {
        RenderOptions {
            projection: projection.to_string(),
            projection_preset: None,
            center_lat: None,
            center_lon: None,
            resampling: "nearest".to_string(),
            flip_y: false,
            range_min: None,
            range_max: None,
            bounds_lat_min: None,
            bounds_lat_max: None,
            bounds_lon_min: None,
            bounds_lon_max: None,
            colormap: None,
            reverse_colormap: None,
            scale_mode: None,
        }
    }

    /// The synthesised geometry is a reprojectable `"latlon"` grid; without
    /// coordinate arrays it degrades to a source-only assumed grid.
    #[test]
    fn synth_meta_is_latlon_and_reprojectable_with_geometry() {
        let geom = fieldglass_netcdf::SliceGeometry {
            ni: 180,
            nj: 89,
            lat_first: 88.0,
            lat_last: -88.0,
            lon_first: 0.0,
            lon_last: 358.0,
            irregular: false,
            lon_descending: false,
        };
        let meta = synth_latlon_meta("sst", "degree_C", 180, 89, Some(geom));
        assert_eq!(meta.grid_type.as_deref(), Some("latlon"));
        assert_eq!(meta.format, "netcdf");
        assert_eq!(meta.grid_ni, Some(180));
        assert_eq!(meta.lat_first, Some(88.0));
        assert!(meta.reprojectable);

        let assumed = synth_latlon_meta("sst", "", 180, 89, None);
        assert!(assumed.lat_first.is_none());
        assert!(!assumed.reprojectable, "no corners ⇒ source only");
    }

    #[test]
    fn global_grids_sample_periodically_regional_grids_do_not() {
        // 0..358° over 180 columns (2° step): one more step wraps to the
        // first column, so the warp may sample across the seam.
        let global = fieldglass_netcdf::SliceGeometry {
            ni: 180,
            nj: 89,
            lat_first: 88.0,
            lat_last: -88.0,
            lon_first: 0.0,
            lon_last: 358.0,
            irregular: false,
            lon_descending: false,
        };
        let meta = synth_latlon_meta("sst", "degree_C", 180, 89, Some(global));
        assert!(source_grid_is_periodic(&meta, 180));

        // A 90°-wide regional window is not periodic.
        let regional = fieldglass_netcdf::SliceGeometry {
            lon_first: 0.0,
            lon_last: 90.0,
            ni: 10,
            ..global
        };
        let meta = synth_latlon_meta("t", "K", 10, 89, Some(regional));
        assert!(!source_grid_is_periodic(&meta, 10));

        // Planar grid types never wrap, whatever their corners say.
        let mut planar = synth_latlon_meta("t", "K", 180, 89, Some(global));
        planar.grid_type = Some("lambert".to_string());
        assert!(!source_grid_is_periodic(&planar, 180));
    }

    /// End-to-end: open the committed classic ERSST fixture, pick the bottom
    /// time/level slice of `sst`, synthesise geometry from the coordinate
    /// arrays, and render — both source and equirectangular must paint a full
    /// 180×89 raster without error.
    #[test]
    fn ersst_sst_slice_renders_source_and_equirectangular() {
        let handle = handle(ERSST);

        let vars = handle.variables();
        let sst = vars.iter().find(|v| v.name == "sst").expect("sst present");
        let (y, x) = (
            sst.detected_y_dim.unwrap() as u32,
            sst.detected_x_dim.unwrap() as u32,
        );
        // sst is time × lev × lat × lon; hold time=0, lev=0.
        let indices = vec![0u32; sst.dims.len()];

        let source = handle
            .render_slice(
                sst.variable_index as u32,
                y,
                x,
                indices.clone(),
                opts("source"),
            )
            .expect("source render");
        assert_eq!((source.width, source.height), (180, 89));

        let warped = handle
            .render_slice(
                sst.variable_index as u32,
                y,
                x,
                indices,
                opts("equirectangular"),
            )
            .expect("equirectangular render");
        assert!(warped.width > 0 && warped.height > 0);
        assert!(warped.used_lat_min.is_some(), "warp echoes back its extent");
    }

    #[test]
    fn log10_render_needs_a_positive_lower_bound() {
        let handle = handle(ERSST);
        let vars = handle.variables();
        let sst = vars.iter().find(|v| v.name == "sst").expect("sst present");
        let (y, x) = (
            sst.detected_y_dim.unwrap() as u32,
            sst.detected_x_dim.unwrap() as u32,
        );
        let indices = vec![0u32; sst.dims.len()];

        let log_opts = |min: f64, max: f64| {
            let mut o = opts("source");
            o.scale_mode = Some("log10".to_string());
            o.range_min = Some(min);
            o.range_max = Some(max);
            o
        };

        // A manual range that dips to/below zero has no logarithm: refuse with
        // an actionable message rather than paint garbage. (`RenderedGrid` isn't
        // `Debug`, so match rather than `expect_err`.)
        match handle.render_slice(
            sst.variable_index as u32,
            y,
            x,
            indices.clone(),
            log_opts(-5.0, 5.0),
        ) {
            Ok(_) => panic!("log10 with a non-positive minimum must error"),
            Err(e) => assert!(
                e.to_string()
                    .contains("log10 scaling needs a positive minimum"),
                "actionable message, got: {e}"
            ),
        }

        // A positive manual range renders, and the true (unlogged) bounds are
        // echoed back so the colorbar labels stay in data units.
        let ok = handle
            .render_slice(
                sst.variable_index as u32,
                y,
                x,
                indices,
                log_opts(1.0, 40.0),
            )
            .expect("log10 with a positive range renders");
        assert_eq!((ok.used_min, ok.used_max), (1.0, 40.0));
        assert_eq!(ok.rgba.len(), (ok.width * ok.height * 4) as usize);
    }

    #[test]
    fn parse_combine_op_accepts_the_five_ops_and_rejects_others() {
        for tag in ["a_minus_b", "b_minus_a", "a_plus_b", "mean", "ratio"] {
            assert!(parse_combine_op(tag).is_ok(), "{tag} should parse");
        }
        let err = parse_combine_op("product").expect_err("unknown op must error");
        assert!(err.to_string().contains("unknown combine op"), "{err}");
    }

    #[test]
    fn grids_match_requires_identical_geometry_not_identical_parameters() {
        let a = base_netcdf_meta("sst", "K", 180, 89);
        // Same grid, different parameter — exactly what a difference map compares.
        let b = base_netcdf_meta("t2m", "K", 180, 89);
        assert!(
            grids_match(&a, &b),
            "same grid, different parameter must match"
        );

        // Each geometry difference must break the match, or misaligned fields
        // would combine cell-for-cell against the wrong locations.
        let mut nj = base_netcdf_meta("sst", "K", 180, 90);
        nj.parameter_name = "sst".into();
        assert!(!grids_match(&a, &nj), "different Nj must not match");

        let mut corner = base_netcdf_meta("sst", "K", 180, 89);
        corner.lat_first = Some(88.0);
        assert!(!grids_match(&a, &corner), "different corner must not match");

        let mut scan = base_netcdf_meta("sst", "K", 180, 89);
        scan.j_scans_positive = Some(true);
        assert!(
            !grids_match(&a, &scan),
            "different scan direction must not match"
        );

        let mut proj = base_netcdf_meta("sst", "K", 180, 89);
        proj.lambert_dx_metres = Some(3000.0);
        assert!(
            !grids_match(&a, &proj),
            "different projection param must not match"
        );
    }

    #[test]
    fn slice_combined_self_difference_is_zero_and_rejects_a_bad_op() {
        let handle = handle(ERSST);
        let vars = handle.variables();
        let sst = vars.iter().find(|v| v.name == "sst").expect("sst present");
        let (y, x) = (
            sst.detected_y_dim.unwrap() as u32,
            sst.detected_x_dim.unwrap() as u32,
        );
        let indices = vec![0u32; sst.dims.len()];
        let vi = sst.variable_index as u32;

        // A − A: every present cell is exactly 0, so the used range collapses to
        // (0, 0). This exercises the whole combined path end-to-end.
        let combined = handle
            .render_slice_combined(
                vi,
                y,
                x,
                indices.clone(),
                vi,
                indices.clone(),
                "a_minus_b".to_string(),
                opts("source"),
            )
            .expect("self-difference renders");
        assert_eq!((combined.used_min, combined.used_max), (0.0, 0.0));
        assert_eq!(
            combined.rgba.len(),
            (combined.width * combined.height * 4) as usize
        );

        // An unknown op is rejected before any decode work.
        match handle.render_slice_combined(
            vi,
            y,
            x,
            indices.clone(),
            vi,
            indices,
            "product".to_string(),
            opts("source"),
        ) {
            Ok(_) => panic!("unknown combine op must error"),
            Err(e) => assert!(e.to_string().contains("unknown combine op"), "{e}"),
        }
    }

    /// A regular lat/lon meta over a small region, for the contour tests.
    fn latlon_meta(ni: i32, nj: i32) -> MessageMeta {
        let mut meta = base_netcdf_meta("t", "K", ni, nj);
        meta.grid_type = Some("latlon".to_string());
        meta.lat_first = Some(40.0);
        meta.lat_last = Some(10.0);
        meta.lon_first = Some(0.0);
        meta.lon_last = Some(40.0);
        meta
    }

    #[test]
    fn levels_by_interval_walks_the_range_on_multiples() {
        assert_eq!(levels_by_interval(0.0, 10.0, 2.0), vec![2.0, 4.0, 6.0, 8.0]);
        // Endpoints are excluded; a start below the range is skipped.
        assert_eq!(levels_by_interval(-3.0, 3.0, 3.0), vec![0.0]);
        // Degenerate inputs yield nothing.
        assert!(levels_by_interval(5.0, 5.0, 1.0).is_empty());
        assert!(levels_by_interval(0.0, 10.0, 0.0).is_empty());
        assert!(levels_by_interval(0.0, 10.0, -1.0).is_empty());
    }

    #[test]
    fn forward_geolocation_latlon_places_corners_then_interior() {
        let meta = latlon_meta(5, 4);
        let fwd = forward_geolocation_for(&meta, 5, 4).expect("latlon forward map");
        // Corner (0,0) is (latFirst, lonFirst); (4,3) is (latLast, lonLast).
        assert_eq!(fwd(0, 0), Some((40.0, 0.0)));
        assert_eq!(fwd(4, 3), Some((10.0, 40.0)));
        // A fractional vertex on the bottom edge interpolates the longitude.
        let (lat, lon) = forward_bilinear(fwd.as_ref(), 5, 4, 2.5, 0.0).expect("interior");
        assert!(
            (lat - 40.0).abs() < 1e-9,
            "on the first row, lat = latFirst"
        );
        assert!(
            (lon - 25.0).abs() < 1e-9,
            "i=2.5 over 0..40/4 steps → lon 25, got {lon}"
        );
    }

    #[test]
    fn project_contours_latlon_runs_within_raster_and_gates_unknown_grids() {
        // value = column index → a smooth west-to-east ramp with interior isolines.
        let meta = latlon_meta(5, 4);
        let raw: Vec<Option<f64>> = (0..5 * 4).map(|k| Some((k % 5) as f64)).collect();

        let out = project_contours_impl(&meta, &raw, &opts("source"), None)
            .expect("latlon contours project");
        assert!(!out.xy.is_empty(), "a ramp field has interior contours");
        assert_eq!(out.xy.len() % 2, 0, "xy is flat (x, y) pairs");
        // Source projection paints grid (i, j) at pixel (i, j); every vertex is
        // inside the raster (a small margin for edge rounding).
        for pair in out.xy.chunks(2) {
            assert!(
                pair[0] >= -0.5 && pair[0] <= 5.5,
                "x {} within raster",
                pair[0]
            );
            assert!(
                pair[1] >= -0.5 && pair[1] <= 4.5,
                "y {} within raster",
                pair[1]
            );
        }

        // A manual interval selects the levels; a coarse interval still finds the
        // interior crossings of a 0..4 ramp.
        let manual = project_contours_impl(&meta, &raw, &opts("source"), Some(1.0))
            .expect("manual-interval contours project");
        assert!(!manual.xy.is_empty());

        // A grid type whose forward map isn't wired yet is a clear error.
        let mut lambert = latlon_meta(5, 4);
        lambert.grid_type = Some("lambert".to_string());
        match project_contours_impl(&lambert, &raw, &opts("source"), None) {
            Ok(_) => panic!("lambert contours must error for now"),
            Err(e) => assert!(e.to_string().contains("contours not yet supported"), "{e}"),
        }
    }

    #[test]
    fn probe_source_reads_the_cell_value_and_its_coordinate() {
        // value = column index over the 5×4 lat/lon grid.
        let meta = latlon_meta(5, 4);
        let raw: Vec<Option<f64>> = (0..5 * 4).map(|k| Some((k % 5) as f64)).collect();

        // Source view: pixel (i, j) is grid (i, j) (this grid scans N→S, no flip).
        let r = probe_impl(&meta, &raw, &opts("source"), 2, 1)
            .expect("probe ok")
            .expect("pixel is on the grid");
        assert_eq!(r.value, Some(2.0), "column-index ramp reads its column");
        assert_eq!((r.grid_i, r.grid_j), (Some(2), Some(1)));
        // Geolocated: row 1 of lat 40→10 over 4 rows = 30°, col 2 of lon 0→40 = 20°.
        assert!((r.lat.unwrap() - 30.0).abs() < 1e-9, "lat {:?}", r.lat);
        assert!((r.lon.unwrap() - 20.0).abs() < 1e-9, "lon {:?}", r.lon);

        // Off the raster → nothing to report.
        assert!(
            probe_impl(&meta, &raw, &opts("source"), 99, 0)
                .unwrap()
                .is_none(),
            "a click past the grid returns None"
        );
    }

    #[test]
    fn probe_equirectangular_maps_a_pixel_back_to_its_value() {
        let meta = latlon_meta(5, 4);
        let raw: Vec<Option<f64>> = (0..5 * 4).map(|k| Some((k % 5) as f64)).collect();

        // Top-left pixel of the equirect raster is the grid's NW corner
        // (lat_max, lon_min) → grid (0, 0) → value 0.
        let r = probe_impl(&meta, &raw, &opts("equirectangular"), 0, 0)
            .expect("probe ok")
            .expect("corner is on the grid");
        assert_eq!(r.value, Some(0.0));
        assert!(
            (r.lat.unwrap() - 40.0).abs() < 1.0,
            "near the north edge, {:?}",
            r.lat
        );
        assert!(
            r.lon.unwrap().abs() < 1.0,
            "near the west edge, {:?}",
            r.lon
        );
    }

    #[test]
    fn probe_off_the_globe_and_off_the_raster_report_nothing() {
        let meta = latlon_meta(5, 4);
        let raw: Vec<Option<f64>> = (0..5 * 4).map(|_| Some(1.0)).collect();
        // Orthographic fits a disc to a square raster; a corner pixel is outside
        // the disc, so there is no point under it.
        assert!(
            probe_impl(&meta, &raw, &opts("orthographic"), 0, 0)
                .unwrap()
                .is_none(),
            "an off-disc corner returns None"
        );
        // Past the raster edge is likewise nothing.
        assert!(
            probe_impl(&meta, &raw, &opts("equirectangular"), 999, 999)
                .unwrap()
                .is_none()
        );
    }

    /// End-to-end on the NetCDF-4 / HDF5 backing (#169): open the dimension-scale
    /// fixture, pick `temperature`'s lat/lon plane at time=0, and render. The
    /// detected axes must come from the `lat`/`lon` coordinate variables, the
    /// decode must read `temperature` (not the `nv` pure dimension that sits
    /// between them in dataset order), and the synthesised grid must reproject.
    #[test]
    fn hdf5_temperature_slice_renders_with_detected_axes() {
        let handle = handle(DIMSCALE);

        let vars = handle.variables();
        let temp = vars
            .iter()
            .find(|v| v.name == "temperature")
            .expect("temperature is renderable");
        // time × lat × lon ⇒ lat is axis 1, lon is axis 2.
        assert_eq!(temp.detected_y_dim, Some(1), "lat detected as the Y axis");
        assert_eq!(temp.detected_x_dim, Some(2), "lon detected as the X axis");

        let indices = vec![0u32; temp.dims.len()]; // hold time = 0.
        let source = handle
            .render_slice(
                temp.variable_index as u32,
                1,
                2,
                indices.clone(),
                opts("source"),
            )
            .expect("source render");
        // ni = lon length (4), nj = lat length (3).
        assert_eq!((source.width, source.height), (4, 3));

        let warped = handle
            .render_slice(
                temp.variable_index as u32,
                1,
                2,
                indices,
                opts("equirectangular"),
            )
            .expect("equirectangular render");
        assert!(warped.width > 0 && warped.height > 0);
        assert!(
            warped.used_lat_min.is_some(),
            "synthesised lat/lon grid reprojects"
        );
    }

    #[test]
    fn render_slice_rejects_a_non_renderable_index() {
        let handle = handle(ERSST);
        // Index 9999 is not a renderable variable.
        let err = handle
            .render_slice(9999, 0, 1, vec![0, 0], opts("source"))
            .map(|_| ())
            .expect_err("non-renderable index must error");
        assert!(err.reason.contains("not a renderable"));
    }

    /// WRF `wrfout`-style classic fixture: the Lambert projection lives in global
    /// attributes (`MAP_PROJ = 1`), with 2-D `XLAT`/`XLONG` fixing the origin
    /// (decision 0004 / #168).
    const WRF: &[u8] = include_bytes!("../../fieldglass-netcdf/tests/fixtures/wrf_lambert.nc");
    /// The same `wrfout` shape with `MAP_PROJ = 2` (polar stereographic, #220).
    const WRF_POLAR: &[u8] = include_bytes!("../../fieldglass-netcdf/tests/fixtures/wrf_polar.nc");
    /// The same `wrfout` shape with `MAP_PROJ = 3` (Mercator, #220).
    const WRF_MERCATOR: &[u8] =
        include_bytes!("../../fieldglass-netcdf/tests/fixtures/wrf_mercator.nc");
    /// The same `wrfout` shape with `MAP_PROJ = 6` (unrotated lat-lon, #226).
    const WRF_LATLON: &[u8] =
        include_bytes!("../../fieldglass-netcdf/tests/fixtures/wrf_latlon.nc");
    /// GOES ABI-style NetCDF-4 fixture: a CF `geostationary` `grid_mapping` and
    /// 1-D `x`/`y` scan-angle coordinate variables stored as scaled `int16`.
    const GOES: &[u8] =
        include_bytes!("../../fieldglass-netcdf/tests/fixtures/goes_geostationary.nc");

    /// The CF mapping guardrail (decision 0004): only `geostationary` and
    /// `latitude_longitude` are routed to a grid; every other projected mapping
    /// (and a malformed/missing one) is classified so it cannot mis-georeference.
    #[test]
    fn cf_mapping_classification_guards_unsupported_projections() {
        assert_eq!(
            classify_cf_mapping(Some("geostationary")),
            CfMapping::Geostationary
        );
        assert_eq!(
            classify_cf_mapping(Some("latitude_longitude")),
            CfMapping::LatLon
        );
        assert_eq!(classify_cf_mapping(None), CfMapping::LatLon);
        // Projected mappings we don't read yet must NOT fall through to lat/lon.
        for unsupported in ["lambert_conformal_conic", "polar_stereographic", "mercator"] {
            assert_eq!(
                classify_cf_mapping(Some(unsupported)),
                CfMapping::Unsupported,
                "{unsupported} must fall back to source-only, not mis-georeference",
            );
        }
    }

    /// Shared walkthrough for the WRF `wrfout` fixtures: resolve `T2`'s slice
    /// meta on the projected axes, assert it is reprojectable with the expected
    /// grid type, render the 6×5 source raster, and reproject into a flat
    /// target that must paint and echo its extent. Returns the meta for the
    /// per-projection assertions. `T2` is Time × south_north × west_east with
    /// no 1-D coordinate variables for the projected axes, so the axes are
    /// picked explicitly.
    fn wrf_t2_meta_after_renders(fixture: &[u8], grid_type: &str) -> MessageMeta {
        let handle = handle(fixture);
        let vars = handle.variables();
        let t2 = vars.iter().find(|v| v.name == "T2").expect("T2 present");
        let (y, x) = (1u32, 2u32);

        let meta = handle
            .slice_meta(
                &handle.renderable(t2.variable_index as u32).unwrap(),
                y as usize,
                x as usize,
            )
            .expect("slice meta");
        assert_eq!(meta.grid_type.as_deref(), Some(grid_type));
        assert!(meta.reprojectable, "WRF {grid_type} reprojects");

        let indices = vec![0u32; t2.dims.len()];
        let source = handle
            .render_slice(
                t2.variable_index as u32,
                y,
                x,
                indices.clone(),
                opts("source"),
            )
            .expect("source render");
        assert_eq!((source.width, source.height), (6, 5));

        let warped = handle
            .render_slice(
                t2.variable_index as u32,
                y,
                x,
                indices,
                opts("equirectangular"),
            )
            .expect("equirectangular render");
        assert!(warped.width > 0 && warped.height > 0);
        assert!(
            warped.used_lat_min.is_some(),
            "{grid_type} warp echoes its extent"
        );
        meta
    }

    /// WRF Lambert (#168): `T2` resolves to a reprojectable `"lambert"` grid from
    /// the global attributes, and both source and a flat target paint a raster.
    #[test]
    fn wrf_t2_slice_renders_as_reprojected_lambert() {
        let meta = wrf_t2_meta_after_renders(WRF, "lambert");
        assert_eq!(meta.lambert_latin1, Some(30.0));
        assert_eq!(meta.lambert_latin2, Some(60.0));
    }

    /// WRF polar stereographic (#220): `T2` resolves to a reprojectable
    /// `"polar_stereo"` grid from the `MAP_PROJ = 2` global attributes, and both
    /// source and a flat target paint a raster.
    #[test]
    fn wrf_t2_slice_renders_as_reprojected_polar_stereo() {
        let meta = wrf_t2_meta_after_renders(WRF_POLAR, "polar_stereo");
        assert_eq!(meta.polar_stereo_lad, Some(60.0), "true scale at TRUELAT1");
        assert_eq!(meta.polar_stereo_lov, Some(-100.0));
        assert_eq!(
            meta.polar_stereo_south_pole,
            Some(false),
            "positive TRUELAT1 = north-pole projection"
        );
    }

    /// WRF Mercator (#220): `T2` resolves to a reprojectable `"mercator"` grid
    /// whose corners come from both ends of `XLAT`/`XLONG`, and both source and
    /// a flat target paint a raster.
    #[test]
    fn wrf_t2_slice_renders_as_reprojected_mercator() {
        let meta = wrf_t2_meta_after_renders(WRF_MERCATOR, "mercator");
        let lat_last = meta.lat_last.expect("far corner latitude");
        let lon_last = meta.lon_last.expect("far corner longitude");
        assert!(
            lat_last > meta.lat_first.unwrap() && lon_last > meta.lon_first.unwrap(),
            "far corner is north-east of the origin (+DX/+DY scan)"
        );
    }

    /// WRF unrotated lat-lon (#226): `T2` resolves to a reprojectable `"latlon"`
    /// grid whose four corners come from both ends of `XLAT`/`XLONG`, and both
    /// source and a flat target paint a raster.
    #[test]
    fn wrf_t2_slice_renders_as_reprojected_latlon() {
        let meta = wrf_t2_meta_after_renders(WRF_LATLON, "latlon");
        let lat_last = meta.lat_last.expect("far corner latitude");
        let lon_last = meta.lon_last.expect("far corner longitude");
        assert!(
            lat_last > meta.lat_first.unwrap() && lon_last > meta.lon_first.unwrap(),
            "far corner is north-east of the origin (+DX/+DY scan)"
        );
    }

    /// GOES geostationary (#168): `Rad` resolves to a reprojectable
    /// `"space_view"` grid from the CF `grid_mapping` plus the scaled `x`/`y`
    /// radian coordinates, and both source and a flat target paint a raster.
    #[test]
    fn goes_rad_slice_renders_as_reprojected_geostationary() {
        let handle = handle(GOES);
        let vars = handle.variables();
        let rad = vars.iter().find(|v| v.name == "Rad").expect("Rad present");
        // y/x carry CF axis attributes, so detection fills them in.
        let (y, x) = (
            rad.detected_y_dim.expect("y detected") as u32,
            rad.detected_x_dim.expect("x detected") as u32,
        );

        let meta = handle
            .slice_meta(
                &handle.renderable(rad.variable_index as u32).unwrap(),
                y as usize,
                x as usize,
            )
            .expect("slice meta");
        assert_eq!(meta.grid_type.as_deref(), Some("space_view"));
        assert!(meta.reprojectable, "geostationary reprojects");
        assert_eq!(meta.geos_sub_lon, Some(-75.0));
        assert_eq!(meta.geos_sweep_x, Some(true), "GOES sweeps about x");
        // x/y were scaled int16: the recovered scan angle is radians (~±0.02),
        // not raw integer codes in the tens of thousands.
        assert!(
            meta.geos_x0.unwrap().abs() < 1.0,
            "x0 is radians after CF scaling"
        );

        let indices = vec![0u32; rad.dims.len()];
        let source = handle
            .render_slice(
                rad.variable_index as u32,
                y,
                x,
                indices.clone(),
                opts("source"),
            )
            .expect("source render");
        assert_eq!((source.width, source.height), (6, 6));

        let warped = handle
            .render_slice(
                rad.variable_index as u32,
                y,
                x,
                indices,
                opts("equirectangular"),
            )
            .expect("equirectangular render");
        assert!(warped.width > 0 && warped.height > 0);
        assert!(
            warped.used_lat_min.is_some(),
            "geostationary warp echoes its extent"
        );
    }
}

#[cfg(test)]
mod space_view_geos_tests {
    use super::*;
    use fieldglass_grib2::SpaceViewTemplate;

    /// A minimal §3.90 template: an 11×11 central crop of a disk that is 15
    /// grid lengths across, GRS80 ellipsoid, sub-satellite point at grid
    /// centre, GOES-East longitude, default (i+, j-) scan.
    fn space_view_template() -> SpaceViewTemplate {
        SpaceViewTemplate {
            shape_of_earth: 5,
            r_eq: 6_378_137.0,
            r_pol: 6_356_752.314,
            nx: 11,
            ny: 11,
            lap: 0.0,
            lop: -75.0,
            dx: 15,
            dy: 15,
            xp: 5.0,
            yp: 5.0,
            orientation: 0.0,
            nr: Some(6_610_710),
            xo: 0,
            yo: 0,
            resolution_flags: 0,
            scanning_mode: 0,
        }
    }

    #[test]
    fn scan_grid_places_subsatellite_point_at_its_grid_index() {
        let g = space_view_scan_grid(&space_view_template()).expect("scan grid");
        // Default scan (i+, j-): sub-satellite point resolves to (5, 5).
        let p = GeostationaryParams {
            ni: 11,
            nj: 11,
            h_metres: g.height,
            r_eq: g.r_eq,
            r_pol: g.r_pol,
            sub_lon_deg: g.sub_lon,
            sweep_x: g.sweep_x,
            x0: g.x0,
            dx_rad: g.dx_rad,
            y0: g.y0,
            dy_rad: g.dy_rad,
        };
        let proj = GeostationaryProjector::new(p);
        let idx = proj.inverse(0.0, -75.0).expect("sub-sat on grid");
        assert!((idx.i - 5.0).abs() < 1e-6, "i = {}", idx.i);
        assert!((idx.j - 5.0).abs() < 1e-6, "j = {}", idx.j);
        // Camera height is Nr (×10⁻⁶) Earth radii from the centre, ~6.6 r_eq.
        assert!((g.height / g.r_eq - 6.610_71).abs() < 1e-4);
        assert!(g.sweep_x, "GRIB2 §3.90 is the GOES-R sweep-x convention");
        // Off-disk far-side longitude is not on the grid.
        assert!(proj.inverse(0.0, 105.0).is_none());

        // Orientation must match the stored data order eccodes decodes (the
        // row loop is reversed, the column loop is not). With this template's
        // j-scans-negative mode, stored row 0 is the northernmost, so a point
        // north of the sub-satellite point indexes a SMALLER row, a southern
        // point a LARGER row, and an eastern point a LARGER column.
        let north = proj.inverse(20.0, -75.0).expect("north on grid");
        let south = proj.inverse(-20.0, -75.0).expect("south on grid");
        let east = proj.inverse(0.0, -60.0).expect("east on grid");
        assert!(north.j < 5.0, "north row {} should be < centre", north.j);
        assert!(south.j > 5.0, "south row {} should be > centre", south.j);
        assert!(east.i > 5.0, "east col {} should be > centre", east.i);
        // North and south are symmetric about the centre row for points
        // equidistant in latitude.
        assert!(
            ((north.j - 5.0) + (south.j - 5.0)).abs() < 1e-6,
            "north/south not symmetric: {} / {}",
            north.j,
            south.j
        );
    }

    #[test]
    fn orthographic_view_has_no_scan_grid() {
        // Nr missing ⇒ orthographic projection, which we don't reproject.
        let mut t = space_view_template();
        t.nr = None;
        assert!(space_view_scan_grid(&t).is_none());
    }

    /// A space-view `MessageMeta` with only the fields the warp consults set;
    /// every other slot is left empty so the synthetic stays minimal.
    fn space_view_meta() -> MessageMeta {
        let g = space_view_scan_grid(&space_view_template()).unwrap();
        MessageMeta {
            earth_radius_metres: None,
            message_index: 0,
            offset_bytes: 0,
            parameter_name: String::new(),
            parameter_units: String::new(),
            parameter_abbreviation: String::new(),
            level: String::new(),
            level_type: String::new(),
            reference_time: String::new(),
            forecast_hours: 0,
            forecast_display: String::new(),
            originating_centre: String::new(),
            grid_type: Some("space_view".to_string()),
            grid_ni: Some(11),
            grid_nj: Some(11),
            lat_first: None,
            lon_first: None,
            lat_last: None,
            lon_last: None,
            format: "grib2".to_string(),
            edition: Some(2),
            discipline: None,
            total_length_bytes: None,
            production_status: None,
            data_type: None,
            lambert_lad: None,
            lambert_lov: None,
            lambert_dx_metres: None,
            lambert_dy_metres: None,
            lambert_latin1: None,
            lambert_latin2: None,
            gaussian_n_parallels: None,
            polar_stereo_lov: None,
            polar_stereo_lad: None,
            polar_stereo_dx_metres: None,
            polar_stereo_dy_metres: None,
            polar_stereo_south_pole: None,
            rotated_south_pole_lat: None,
            rotated_south_pole_lon: None,
            rotated_angle_of_rotation: None,
            geos_sub_lon: Some(g.sub_lon),
            geos_height: Some(g.height),
            geos_r_eq: Some(g.r_eq),
            geos_r_pol: Some(g.r_pol),
            geos_sweep_x: Some(g.sweep_x),
            geos_x0: Some(g.x0),
            geos_dx_rad: Some(g.dx_rad),
            geos_y0: Some(g.y0),
            geos_dy_rad: Some(g.dy_rad),
            packing: None,
            reprojectable: true,
            j_scans_positive: None,
        }
    }

    #[test]
    fn space_view_warp_setup_round_trips_through_meta() {
        // Drive the full napi path: §3.90 meta → warp_setup_for → inverse.
        let meta = space_view_meta();
        let (inverse, _bbox) = warp_setup_for(&meta, 11, 11).expect("space view warp setup");
        let idx = inverse(0.0, -75.0).expect("sub-sat on grid");
        assert!((idx.i - 5.0).abs() < 1e-6 && (idx.j - 5.0).abs() < 1e-6);
        assert!(inverse(0.0, 105.0).is_none(), "far side must be off-grid");
    }

    #[test]
    fn space_view_bbox_frames_on_disk_extent() {
        // The 11×11 central crop is on-disk, so the thunk frames its extent
        // tightly — strictly inside the ±90° hemisphere fallback.
        let meta = space_view_meta();
        let (_inverse, bbox) = warp_setup_for(&meta, 11, 11).expect("space view warp setup");
        let (lat_min, lat_max, lon_min, lon_max) = bbox();
        assert!(
            lat_min > -90.0 && lat_max < 90.0,
            "lat {lat_min}..{lat_max}"
        );
        assert!(
            lon_min > -165.0 && lon_max < 15.0,
            "lon {lon_min}..{lon_max} should be inside the hemisphere fallback"
        );
        // Spans the sub-satellite meridian (-75°).
        assert!(lon_min < -75.0 && lon_max > -75.0, "box not around sub-lon");
    }

    #[test]
    fn space_view_bbox_falls_back_when_perimeter_off_disk() {
        // A scan window wider than the apparent disk (±0.16 rad > ~0.152 rad
        // limb) has no on-disk perimeter sample, so the thunk returns the
        // generous ±90° hemisphere box around the sub-satellite point.
        let mut meta = space_view_meta();
        let half = 0.16;
        meta.geos_x0 = Some(-half);
        meta.geos_y0 = Some(-half);
        meta.geos_dx_rad = Some(2.0 * half / 10.0);
        meta.geos_dy_rad = Some(2.0 * half / 10.0);
        let (_inverse, bbox) = warp_setup_for(&meta, 11, 11).expect("space view warp setup");
        let (lat_min, lat_max, lon_min, lon_max) = bbox();
        assert_eq!((lat_min, lat_max), (-90.0, 90.0));
        assert_eq!((lon_min, lon_max), (-165.0, 15.0));
    }
}
