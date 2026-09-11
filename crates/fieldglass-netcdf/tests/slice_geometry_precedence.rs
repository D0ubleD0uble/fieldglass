//! The precedence tree, and the four guards that were reachable only through
//! the Node addon until #549.
//!
//! `projected_grids.rs` already checks that each *resolver* reproduces an
//! independent oracle. This file checks the thing above them: that the right
//! resolver is chosen, and — more importantly — that the wrong one is refused.
//! Every case here is a `slice_geometry` call on a committed fixture, because
//! the defect being guarded against is a file resolving to a family it is not.

use fieldglass_core::projection::GridGeometry;
use fieldglass_netcdf::NetcdfReader;
use fieldglass_netcdf::array::Attribute;
use fieldglass_netcdf::resolve::{CfMapping, SOURCE_ONLY, classify_grid_mapping};

const WRF_LAMBERT: &[u8] = include_bytes!("fixtures/wrf_lambert.nc");
const WRF_MERCATOR: &[u8] = include_bytes!("fixtures/wrf_mercator.nc");
const GOES: &[u8] = include_bytes!("fixtures/goes_geostationary.nc");
const MIRS: &[u8] = include_bytes!("fixtures/mirs_swath_n21.nc");
const ERSST: &[u8] = include_bytes!("fixtures/ersst_v5_187001_cdf1.nc");

/// Resolve the first renderable variable's slice geometry, on its own detected
/// horizontal axes — the same choice the render path makes.
fn first_slice(bytes: &[u8]) -> GridGeometry {
    let reader = NetcdfReader::from_bytes(bytes.to_vec()).expect("fixture opens");
    let view = reader.view().expect("view resolves");
    let var = view
        .renderable_variables()
        .into_iter()
        .find(|v| v.dims.len() >= 2)
        .expect("a renderable variable");
    // The axis choice every host makes: the CF-detected pair when the file
    // names one, otherwise the trailing two dimensions. A WRF or GOES file has
    // no detectable latitude axis, which is the whole reason it needs a
    // projection resolved rather than coordinate arrays read.
    let (y, x) = match (var.detected_y_dim, var.detected_x_dim) {
        (Some(y), Some(x)) => (y, x),
        _ => (var.dims.len() - 2, var.dims.len() - 1),
    };
    reader
        .slice_geometry(&view, &var, y, x)
        .expect("the slice resolves")
}

fn family(g: &GridGeometry) -> &'static str {
    match g {
        GridGeometry::LatLon(_) => "latlon",
        GridGeometry::Gaussian(_) => "gaussian",
        GridGeometry::Mercator(_) => "mercator",
        GridGeometry::RotatedLatLon(_) => "rotated_latlon",
        GridGeometry::Lambert(_) => "lambert",
        GridGeometry::PolarStereo(_) => "polar_stereo",
        GridGeometry::TransverseMercator(_) => "transverse_mercator",
        GridGeometry::LambertAzimuthal(_) => "lambert_azimuthal",
        GridGeometry::Geostationary(_) => "space_view",
        GridGeometry::Lookup(_) => "lookup",
        GridGeometry::Unsupported { .. } => "unsupported",
        _ => "unknown",
    }
}

/// **Guard 1 — the Gulf of Guinea.**
///
/// A WRF Lambert domain's `x`/`y` are projected metres. Read as degrees they
/// collapse to a few degrees either side of (0, 0), which is open ocean off
/// West Africa — a plausible-looking raster in entirely the wrong place. The
/// only outcomes that are ever correct here are the real family or an explicit
/// refusal; `LatLon` is the bug.
#[test]
fn a_projected_domain_never_resolves_to_latlon() {
    let g = first_slice(WRF_LAMBERT);
    assert!(
        matches!(
            g,
            GridGeometry::Lambert(_) | GridGeometry::Unsupported { .. }
        ),
        "a Lambert-CRS file resolved to {} — reading projected metres as \
         degrees puts this domain in the Gulf of Guinea",
        family(&g)
    );
    // And the placement is the real one, not the refusal: this fixture's
    // projection *is* resolvable, so a bare `Unsupported` would be a silent
    // regression that the assertion above would still accept.
    let GridGeometry::Lambert(p) = &g else {
        panic!("expected Lambert for wrf_lambert.nc, got {}", family(&g));
    };
    assert!(
        (20.0..75.0).contains(&p.lat_first) && (-170.0..-40.0).contains(&p.lon_first),
        "the origin should be over North America, got ({}, {})",
        p.lat_first,
        p.lon_first
    );
}

/// **Guard 2 — an unrecognised `grid_mapping` stops rather than falls through.**
///
/// The classifier is the whole of the guard, so it is checked directly on each
/// of the three routes. A future mapping name added to the CF table must arrive
/// as `Geostationary` or `LatLon` deliberately; anything unknown has to be
/// refused, because falling through reads projected `x`/`y` as degrees.
#[test]
fn an_unknown_grid_mapping_is_refused_not_assumed_geographic() {
    let named = |n: &str| vec![Attribute::text("grid_mapping_name", n)];
    assert_eq!(
        classify_grid_mapping(&named("geostationary")),
        CfMapping::Geostationary
    );
    assert_eq!(
        classify_grid_mapping(&named("latitude_longitude")),
        CfMapping::LatLon
    );
    // The families this build has projectors for but no CF classification yet.
    for projected in ["lambert_conformal_conic", "polar_stereographic", "mercator"] {
        assert_eq!(
            classify_grid_mapping(&named(projected)),
            CfMapping::Unsupported,
            "{projected} must not fall through to the coordinate arrays"
        );
    }
    // Whitespace is trimmed, so a padded attribute still classifies.
    assert_eq!(
        classify_grid_mapping(&named("  geostationary  ")),
        CfMapping::Geostationary
    );
    // A mapping variable with no name at all is the documented geographic
    // default — safe only because CF requires a projected CRS to state one.
    assert_eq!(classify_grid_mapping(&[]), CfMapping::LatLon);
}

/// **Guard 3 — 2-D coordinates beat every formula below them.**
///
/// A swath product carries `Latitude`/`Longitude` arrays giving each cell's
/// position. Nothing below could reconstruct them, so the precedence must not
/// reach the 1-D path even though the file also has dimension coordinates.
#[test]
fn a_swath_resolves_to_the_cell_list_not_a_formula() {
    let g = first_slice(MIRS);
    assert!(
        matches!(g, GridGeometry::Lookup(_)),
        "a 2-D-coordinate swath resolved to {}, so its per-cell positions were \
         discarded in favour of a formula",
        family(&g)
    );
}

/// **Guard 4 — the families that do resolve, resolve.**
///
/// The counterweight to guards 1–3: a precedence that refused everything would
/// satisfy them all. Each fixture must reach its own family.
#[test]
fn each_fixture_reaches_its_own_family() {
    for (bytes, want, name) in [
        (GOES, "space_view", "goes_geostationary.nc"),
        (WRF_MERCATOR, "mercator", "wrf_mercator.nc"),
        (ERSST, "latlon", "ersst_v5_187001_cdf1.nc"),
    ] {
        let g = first_slice(bytes);
        assert_eq!(family(&g), want, "{name} resolved to the wrong family");
    }
}

/// A slice asked for on one axis twice is a caller error, not a geometry.
#[test]
fn the_same_axis_twice_is_refused() {
    let reader = NetcdfReader::from_bytes(ERSST.to_vec()).expect("fixture opens");
    let view = reader.view().expect("view resolves");
    let var = view
        .renderable_variables()
        .into_iter()
        .find(|v| v.detected_y_dim.is_some())
        .expect("a renderable variable");
    let y = var.detected_y_dim.expect("y axis");
    assert!(
        reader.slice_geometry(&view, &var, y, y).is_err(),
        "the X and Y axes must be different dimensions"
    );
}

/// The refusal is spelled one way, so a host printing it says one thing.
#[test]
fn the_source_only_label_is_the_named_constant() {
    assert_eq!(SOURCE_ONLY, "source");
}

/// The scan travels beside the geometry because it cannot be recovered from it.
///
/// `lon_descending` means **every** step of the longitude axis runs east to
/// west. A grid whose corners merely decrease across a non-monotonic axis is a
/// different thing, and deriving the flag from `lon_first`/`lon_last` would
/// reproject those files the wrong way round. This pins the distinction so a
/// later simplification cannot quietly make the flag corner-derived.
#[test]
fn the_scan_is_read_from_the_axis_not_inferred_from_the_corners() {
    use fieldglass_netcdf::SliceGeometry;
    use fieldglass_netcdf::synthesize_geometry;

    // Strictly descending: the flag is set, and the corners agree with it.
    let down: SliceGeometry = synthesize_geometry(&[0.0, 1.0], &[10.0, 5.0]).expect("resolves");
    assert!(down.lon_descending);
    assert!(down.lon_last < down.lon_first, "corners agree here");

    // Corners decrease, but the axis is not monotonic — so the flag is *not*
    // set, and a corner-derived one would have been.
    let ragged: SliceGeometry =
        synthesize_geometry(&[0.0, 1.0], &[10.0, 20.0, 5.0]).expect("resolves");
    assert!(
        ragged.lon_last < ragged.lon_first,
        "the corners decrease, which is what makes this the interesting case"
    );
    assert!(
        !ragged.lon_descending,
        "a non-monotonic axis is not an east-to-west scan"
    );
}
