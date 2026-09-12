//! One `MessageMeta` builder, over what `Session` reports (#726).
//!
//! `build_grib1_message_meta` and `build_grib2_message_meta` are two hand-written
//! mappings — about 450 lines between them — that read the format crates'
//! own grid templates and product sections. They exist because this binding
//! held its own readers. ADR-0006 says it should not, and the conventions say
//! WMO table lookups belong in the Rust tables rather than at a binding layer.
//!
//! This is their replacement: the scalar half from [`MessageInfo`], the geometry
//! half from the [`Georef`] beside it. One function for both editions, because
//! `GridGeometry` is one type where `GridDescription` and `GridTemplate` are
//! two.
//!
//! **Which `Georef` the caller passes is the whole difference between a
//! declared and a resolved meta**, and it replaces a three-way split
//! (`message_meta` / `grid_meta` / `resolved_meta`) with an argument:
//!
//! - `Session::message(i).grid` — what the message declares. A spectral message
//!   declares `spherical_harmonic`, which nothing can place a point on.
//! - `Session::place_message(i)` — where its values land, which for a
//!   synthesised family is the global lat/lon raster it is transformed onto.
//!
//! `session_parity.rs` holds this to the two builders it replaces, field by
//! field, over every message of every GRIB fixture. It reproduces them on every
//! field but five, and those five are recorded there as `KNOWN_GAPS` with what
//! closing them needs.
//!
//! **Nothing calls this yet**, which is why it is test-gated: it is written and
//! proven first, and wiring it up is the handle migration. A builder shipped
//! unused would be dead code in a `.node`.

use fieldglass::{Georef, MessageInfo};
use fieldglass_core::GridGeometry;

use crate::{MessageMeta, friendly_packing, gate_reprojection};

/// The `MessageMeta` a host shows for one message.
///
/// `grid` is the placement to describe — see the module docs for why the caller
/// chooses it. `None` for a GRIB1 message carrying no §2, where there is no
/// grid to describe and every geometry field stays absent.
pub(crate) fn meta_from_session(
    info: &MessageInfo,
    grid: Option<&Georef>,
    format: &str,
) -> MessageMeta {
    let base = MessageMeta {
        message_index: i32::try_from(info.index).unwrap_or(i32::MAX),
        // A file offset reaching JavaScript, which has no integers past 2^53.
        // Every GRIB file this can index is far inside that.
        offset_bytes: info.offset_bytes as f64,
        parameter_name: info.parameter.clone(),
        parameter_units: info.units.clone(),
        parameter_abbreviation: info.abbreviation.clone(),
        level: info.level.clone(),
        level_type: info.level_type.clone(),
        reference_time: info.reference_time.clone().unwrap_or_default(),
        // The handle has always substituted zero for a template that states no
        // lead time; the DTO reports the absence instead, and this is where the
        // display default is applied.
        forecast_hours: info.forecast_hours.unwrap_or(0),
        forecast_display: info.forecast.clone(),
        p1_octet: info.p1_octet,
        originating_centre: info.originating_centre.clone(),
        sub_centre: info.sub_centre.clone(),
        format: format.to_string(),
        edition: info.edition,
        discipline: info.discipline.clone(),
        total_length_bytes: info.total_length_bytes.map(|v| v as f64),
        production_status: info.production_status.clone(),
        data_type: info.data_type.clone(),
        // The identifier is the DTO's; the label is this host's rendering of it
        // (#727). Kept here rather than pushed into the DTO because it is
        // display text, and lossy on purpose.
        packing: Some(friendly_packing(&info.packing)),
        // Named by the grid, below, and absent when there is none.
        grid_size_label: info.size_label.clone(),
        ..MessageMeta::default()
    };
    let Some(georef) = grid else {
        return base;
    };
    // `grid_type` is the family the file names, not the geometry's kind: a
    // reduced grid widened onto its regular sibling's raster still reports
    // `reduced_gg`, which is what `Georef::label` carries and what the two
    // builders read from `grid_type_name` / `template_name`.
    // A family with no raster of its own has no dimensions and no row order to
    // report, and `Georef` spells that as `(0, 0)` rather than absent. Claiming
    // `Some(0)` here would tell a message table a spectral field is zero cells
    // wide, where the handles have always left all three absent.
    let placed = georef.geometry.dims().is_some();
    let named = MessageMeta {
        grid_type: Some(georef.label.clone()),
        grid_ni: placed.then(|| i32::try_from(georef.ni).unwrap_or(i32::MAX)),
        grid_nj: placed.then(|| i32::try_from(georef.nj).unwrap_or(i32::MAX)),
        j_scans_positive: placed.then_some(georef.scan.j_positive),
        ..base
    };
    let meta = match &georef.geometry {
        GridGeometry::LatLon(g) => MessageMeta {
            lat_first: Some(g.lat_first),
            lon_first: Some(g.lon_first),
            lat_last: Some(g.lat_last),
            lon_last: Some(g.lon_last),
            ..named
        },
        GridGeometry::Gaussian(g) => MessageMeta {
            lat_first: Some(g.lat_first),
            lon_first: Some(g.lon_first),
            lat_last: Some(g.lat_last),
            lon_last: Some(g.lon_last),
            gaussian_n_parallels: Some(i32::try_from(g.n_parallels).unwrap_or(i32::MAX)),
            ..named
        },
        GridGeometry::Mercator(g) => MessageMeta {
            lat_first: Some(g.lat_first),
            lon_first: Some(g.lon_first),
            lat_last: Some(g.lat_last),
            lon_last: Some(g.lon_last),
            ..named
        },
        GridGeometry::RotatedLatLon(g) => MessageMeta {
            lat_first: Some(g.lat_first),
            lon_first: Some(g.lon_first),
            lat_last: Some(g.lat_last),
            lon_last: Some(g.lon_last),
            rotated_south_pole_lat: Some(g.south_pole_lat),
            rotated_south_pole_lon: Some(g.south_pole_lon),
            rotated_angle_of_rotation: Some(g.angle_of_rotation),
            ..named
        },
        GridGeometry::Lambert(g) => MessageMeta {
            earth_radius_metres: Some(g.earth_radius_m),
            lat_first: Some(g.lat_first),
            lon_first: Some(g.lon_first),
            lambert_lad: Some(g.lad),
            lambert_lov: Some(g.lov),
            lambert_dx_metres: Some(g.dx_metres),
            lambert_dy_metres: Some(g.dy_metres),
            lambert_latin1: Some(g.latin1),
            lambert_latin2: Some(g.latin2),
            ..named
        },
        GridGeometry::PolarStereo(g) => MessageMeta {
            earth_radius_metres: Some(g.earth_radius_m),
            lat_first: Some(g.lat_first),
            lon_first: Some(g.lon_first),
            polar_stereo_lov: Some(g.lov),
            polar_stereo_lad: Some(g.lad),
            polar_stereo_dx_metres: Some(g.dx_metres),
            polar_stereo_dy_metres: Some(g.dy_metres),
            polar_stereo_south_pole: Some(g.south_pole),
            ..named
        },
        GridGeometry::TransverseMercator(g) => MessageMeta {
            transverse_mercator_semi_major_metres: Some(g.semi_major_m),
            transverse_mercator_semi_minor_metres: Some(g.semi_minor_m),
            transverse_mercator_lat_ref: Some(g.lat_ref),
            transverse_mercator_lon_ref: Some(g.lon_ref),
            transverse_mercator_scale_factor: Some(g.scale_factor),
            transverse_mercator_false_easting_metres: Some(g.false_easting_m),
            transverse_mercator_false_northing_metres: Some(g.false_northing_m),
            transverse_mercator_x1_metres: Some(g.x1_metres),
            transverse_mercator_y1_metres: Some(g.y1_metres),
            transverse_mercator_dx_metres: Some(g.dx_metres),
            transverse_mercator_dy_metres: Some(g.dy_metres),
            ..named
        },
        GridGeometry::LambertAzimuthal(g) => MessageMeta {
            lambert_azimuthal_semi_major_metres: Some(g.semi_major_m),
            lambert_azimuthal_semi_minor_metres: Some(g.semi_minor_m),
            lat_first: Some(g.lat_first),
            lon_first: Some(g.lon_first),
            lambert_azimuthal_standard_parallel: Some(g.standard_parallel),
            lambert_azimuthal_central_longitude: Some(g.central_longitude),
            lambert_azimuthal_dx_metres: Some(g.dx_metres),
            lambert_azimuthal_dy_metres: Some(g.dy_metres),
            ..named
        },
        GridGeometry::Geostationary(g) => MessageMeta {
            geos_sub_lon: Some(g.sub_lon_deg),
            geos_height: Some(g.h_metres),
            geos_r_eq: Some(g.r_eq),
            geos_r_pol: Some(g.r_pol),
            geos_sweep_x: Some(g.sweep_x),
            geos_x0: Some(g.x0),
            geos_dx_rad: Some(g.dx_rad),
            geos_y0: Some(g.y0),
            geos_dy_rad: Some(g.dy_rad),
            ..named
        },
        // A family with no placement of its own: spectral coefficients before
        // synthesis, HEALPix pixels, a cell list. The name is still reported —
        // it is what a message table shows — and no coordinates are, because
        // there are none to report.
        _ => named,
    };
    gate_reprojection(meta, georef.scan)
}
