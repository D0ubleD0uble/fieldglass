//! The curvilinear (2-D coordinate) corpus (#444).
//!
//! Every geolocated grid Fieldglass reads today is described by a *formula*: a
//! lat/lon box and a spacing, or a CF grid mapping with projection parameters.
//! A curvilinear grid is described by a *list* — two auxiliary coordinate
//! variables `lat(y, x)` and `lon(y, x)` giving the position of every cell,
//! with no formula behind them and nothing to recover one from. ADR-0004 calls
//! that "Model B" and defers it; #445 implements it and #218 renders it.
//!
//! This file is the corpus those two are written against, and it pins **today's
//! behaviour**: both files parse, list their variables, and fall back to the
//! source projection because CF axis detection finds no 1-D coordinate
//! variable to key on. Every assertion below that says "not yet" is a line #445
//! is expected to change, and it should fail loudly when it does rather than
//! passing quietly against a stale expectation.
//!
//! Two files, because the irregularity has two shapes and an implementation can
//! pass one while failing the other. See `tests/fixtures/NOTICE.md` for
//! provenance and `tools/build_netcdf_curvilinear_fixtures.py` for the windows.

use fieldglass_netcdf::{AxisKind, DatasetView, NetcdfBacking, NetcdfReader, VarView, detect_axis};
use serde_json::Value;

/// NCEP RTOFS, a global HYCOM run: a 200 × 260 window of the bipolar Arctic
/// patch, centred on the pole the grid folds around.
const TRIPOLAR: &[u8] = include_bytes!("fixtures/rtofs_tripolar_arctic.nc");
const TRIPOLAR_ORACLE: &str = include_str!("fixtures/rtofs_tripolar_arctic.nc.oracle.json");

/// NOAA-21 MiRS imagery: 100 scanlines × 96 fields of view of a microwave
/// sounder's cross-track scan, at the end of the descending pass.
const SWATH: &[u8] = include_bytes!("fixtures/mirs_swath_n21.nc");
const SWATH_ORACLE: &str = include_str!("fixtures/mirs_swath_n21.nc.oracle.json");

fn view(bytes: &[u8]) -> (NetcdfReader, DatasetView) {
    let reader = NetcdfReader::from_bytes(bytes.to_vec()).expect("fixture parses");
    let view = match &reader.backing {
        NetcdfBacking::Hdf5(_) => {
            DatasetView::from_hdf5(&reader.hdf5_metadata().expect("hdf5 metadata"))
        }
        other => panic!("expected HDF5 backing, got {:?}", other.label()),
    };
    (reader, view)
}

fn var<'a>(view: &'a DatasetView, name: &str) -> &'a VarView {
    view.vars
        .iter()
        .find(|v| v.name == name)
        .unwrap_or_else(|| panic!("{name} is present"))
}

fn attr<'a>(v: &'a VarView, name: &str) -> Option<&'a str> {
    v.attrs
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, value)| value.as_str())
}

/// Both files parse and enumerate the variables they carry.
///
/// This is the whole of #444's acceptance: the corpus is committed and readable
/// before any geolocation work starts, so the licence and size questions are
/// settled separately from the code.
#[test]
fn both_fixtures_parse_and_list_their_variables() {
    let (_, tripolar) = view(TRIPOLAR);
    for name in [
        "Latitude",
        "Longitude",
        "ice_coverage",
        "ice_thickness",
        "ice_temperature",
    ] {
        let v = var(&tripolar, name);
        assert!(!v.dim_names.is_empty(), "{name} has dimensions");
    }
    assert_eq!(
        var(&tripolar, "ice_thickness").dim_names,
        ["MT", "Y", "X"],
        "the field is time × row × column"
    );

    let (_, swath) = view(SWATH);
    for name in ["Latitude", "Longitude", "TPW", "RR", "SIce", "TSkin"] {
        let v = var(&swath, name);
        assert!(!v.dim_names.is_empty(), "{name} has dimensions");
    }
    assert_eq!(
        var(&swath, "TPW").dim_names,
        ["Scanline", "Field_of_view"],
        "the field is scanline × field of view"
    );
}

/// Each file's data variables point at 2-D auxiliary coordinates by name.
///
/// This is the detection signal #445 keys on, and it is worth asserting on the
/// corpus rather than on a synthetic file: the CF attribute is written by the
/// producing centre, and the two here spell it differently enough to matter —
/// RTOFS lists a third, non-spatial name (`Date`) alongside the pair.
#[test]
fn the_cf_coordinates_attribute_names_two_dimensional_lat_lon() {
    let (_, tripolar) = view(TRIPOLAR);
    assert_eq!(
        attr(var(&tripolar, "ice_thickness"), "coordinates"),
        Some("Longitude Latitude Date"),
        "RTOFS names a time coordinate in the same list"
    );
    let (_, swath) = view(SWATH);
    assert_eq!(
        attr(var(&swath, "TPW"), "coordinates"),
        Some("Longitude Latitude"),
        "MiRS names the pair alone"
    );

    // The names really do resolve to 2-D variables over the field's own
    // dimensions — a `coordinates` attribute naming a 1-D variable would be an
    // ordinary CF file, not a curvilinear one.
    for (label, view_, dims) in [
        ("tripolar", &tripolar, ["Y", "X"]),
        ("swath", &view(SWATH).1, ["Scanline", "Field_of_view"]),
    ] {
        for name in ["Latitude", "Longitude"] {
            let v = var(view_, name);
            assert_eq!(v.dim_names, dims, "{label}: {name} is 2-D over the field");
        }
    }
}

/// The 2-D coordinates resolve, and say which is which (#445).
///
/// The seam is narrower than "CF detection does not see them". `detect_axis`
/// classifies by `units` and `standard_name`, and it recognises both variables
/// perfectly well — RTOFS writes `degrees_north`, MiRS writes `degrees` with a
/// latitude-shaped name. What used to reject them was one step later:
/// `axis_by_dim` offers only *coordinate variables* to detection, and a
/// coordinate variable is 1-D and named for its own dimension.
/// `curvilinear_coords` is the path that does offer them.
#[test]
fn the_two_dimensional_coordinates_resolve_and_are_told_apart() {
    for (label, bytes, y_dim, x_dim, field) in [
        ("tripolar", TRIPOLAR, "Y", "X", "ice_thickness"),
        ("swath", SWATH, "Scanline", "Field_of_view", "TPW"),
    ] {
        let (_, view_) = view(bytes);
        for (name, kind) in [
            ("Latitude", AxisKind::Latitude),
            ("Longitude", AxisKind::Longitude),
        ] {
            let v = var(&view_, name);
            assert_eq!(
                detect_axis(v),
                Some(kind),
                "{label}: {name} is classifiable on its own attributes"
            );
            assert_ne!(
                v.dim_names[0], v.name,
                "{label}: {name} is not a CF coordinate variable, so the 1-D \
                 path never sees it"
            );
        }
        let coords = view_
            .curvilinear_coords(var(&view_, field), y_dim, x_dim)
            .unwrap_or_else(|| panic!("{label}: {field} names a usable 2-D pair"));
        assert_eq!(
            coords.lat_index,
            var(&view_, "Latitude").decode_index,
            "{label}: latitude resolved"
        );
        assert_eq!(
            coords.lon_index,
            var(&view_, "Longitude").decode_index,
            "{label}: longitude resolved"
        );
    }
}

/// A curvilinear variable pre-selects the plane that is actually the image.
///
/// This is the last mile of #218: the geolocation shipped in #445, but the
/// slice picker had nothing to pre-select from — a curvilinear variable has no
/// 1-D coordinate variable to detect an axis from — so it fell back to the
/// variable's first two dimensions.
///
/// For the swath that fallback is right by accident: its dimensions *are*
/// `(Scanline, Field_of_view)`. For the ocean field it is wrong, and visibly
/// so — `ice_thickness` is `(MT, Y, X)`, and the first two dimensions are a
/// length-1 time axis against Y, which renders as a 200x1 sliver of the wrong
/// plane with no geolocation at all. The 2-D coordinate arrays already name the
/// answer: they span exactly the two dimensions that are the image.
///
/// The swath case is kept even though it passed before, because it is the one
/// that would silently keep passing if the resolution broke and the fallback
/// took over again.
#[test]
fn a_curvilinear_variable_pre_selects_its_real_image_axes() {
    for (label, bytes, field, expected) in [
        ("tripolar", TRIPOLAR, "ice_thickness", (1usize, 2usize)),
        ("swath", SWATH, "TPW", (0, 1)),
    ] {
        let (_, view_) = view(bytes);
        let renderable = view_
            .renderable_variables()
            .into_iter()
            .find(|v| v.name == field)
            .unwrap_or_else(|| panic!("{label}: {field} is renderable"));
        assert_eq!(
            (renderable.detected_y_dim, renderable.detected_x_dim),
            (Some(expected.0), Some(expected.1)),
            "{label}: {field} should pre-select its 2-D coordinate axes"
        );
        // And the axes it names really are the ones the coordinates span.
        let source = var(&view_, field);
        assert_eq!(view_.curvilinear_axes(source), Some(expected), "{label}");
    }

    // The tripolar case is the one the old fallback got wrong: not (0, 1).
    let (_, view_) = view(TRIPOLAR);
    let ice = var(&view_, "ice_thickness");
    assert_eq!(ice.dim_names, ["MT", "Y", "X"]);
    assert_ne!(
        view_.curvilinear_axes(ice),
        Some((0, 1)),
        "dimension 0 is a length-1 time axis, not an image axis"
    );
}

/// A 2-D coordinate is not offered as a field to draw.
///
/// Found opening the full RTOFS sample during the 0.5.0 test pass: the picker
/// defaults to the first renderable variable, and `Latitude` sorted first — so
/// the file opened on a picture of latitude rather than on the ice.
///
/// A 1-D coordinate was already excluded, by `is_coordinate`, which requires a
/// variable be named for its own single dimension. A 2-D coordinate is not
/// named for a dimension at all, so it slipped through a rule that was never
/// written for it.
#[test]
fn a_two_dimensional_coordinate_is_not_offered_as_a_field() {
    for (label, bytes, gone, first) in [
        (
            "tripolar",
            TRIPOLAR,
            ["Latitude", "Longitude"],
            "ice_coverage",
        ),
        ("swath", SWATH, ["Latitude", "Longitude"], "RR"),
    ] {
        let (_, view_) = view(bytes);
        let names: Vec<String> = view_
            .renderable_variables()
            .into_iter()
            .map(|v| v.name)
            .collect();
        for coord in gone {
            assert!(
                !names.contains(&coord.to_string()),
                "{label}: {coord} is a coordinate, not a field to draw"
            );
        }
        assert_eq!(
            names.first().map(String::as_str),
            Some(first),
            "{label}: the file should open on a real field"
        );
        // They are still present as variables — only the *picker* excludes them,
        // because the geolocation reads them by name.
        assert!(view_.vars.iter().any(|v| v.name == "Latitude"), "{label}");
    }
}

/// A regular 1-D grid keeps detecting its axes the way it always did.
///
/// The curvilinear resolution is a *fallback*: it fills the axes only where the
/// 1-D coordinate-variable path found none, so a file with real `lat`/`lon`
/// axes is untouched by any of this.
#[test]
fn a_regular_grid_still_detects_its_own_axes() {
    const ERSST: &[u8] = include_bytes!("fixtures/ersst_v5_187001_cdf1.nc");
    let reader = NetcdfReader::from_bytes(ERSST.to_vec()).expect("fixture parses");
    let NetcdfBacking::Classic(header) = &reader.backing else {
        panic!("expected a classic backing");
    };
    let view_ = DatasetView::from_classic(header);
    let sst = view_
        .renderable_variables()
        .into_iter()
        .find(|v| v.name == "sst")
        .expect("sst is renderable");
    // `sst(time, lev, lat, lon)` — detected from the 1-D coordinate variables.
    assert_eq!(
        (sst.detected_y_dim, sst.detected_x_dim),
        (Some(2), Some(3)),
        "a 1-D lat/lon grid detects its own axes"
    );
    let source = view_
        .vars
        .iter()
        .find(|v| v.name == "sst")
        .expect("the variable");
    assert_eq!(
        view_.curvilinear_axes(source),
        None,
        "and names no 2-D pair to fall back to"
    );
}

/// A pair laid out over different dimensions places nothing.
///
/// `lat(a, b)` with `lon(b, a)` describes no single raster, and picking one
/// order would put the field somewhere wrong. The corpus has no such file, so
/// this is asserted on the guard directly: both fixtures' pairs *do* agree, and
/// that agreement is what the resolution requires.
#[test]
fn the_two_coordinates_must_agree_on_their_dimensions() {
    for (label, bytes, dims) in [
        ("tripolar", TRIPOLAR, ["Y", "X"]),
        ("swath", SWATH, ["Scanline", "Field_of_view"]),
    ] {
        let (_, view_) = view(bytes);
        assert_eq!(var(&view_, "Latitude").dim_names, dims, "{label}");
        assert_eq!(
            var(&view_, "Longitude").dim_names,
            dims,
            "{label}: the pair agrees, which is what makes the raster single"
        );
    }
}

/// RTOFS names a time coordinate in the same attribute, and it is ignored.
///
/// `coordinates = "Longitude Latitude Date"` — `Date` is a 1-D time variable,
/// not a spatial one. A resolver that assumed the attribute held exactly two
/// names, or that took the first two, would read this file wrongly; one that
/// filtered by shape and axis kind does not notice it at all.
#[test]
fn a_non_spatial_name_in_the_coordinates_attribute_is_ignored() {
    let (_, view_) = view(TRIPOLAR);
    let named = attr(var(&view_, "ice_thickness"), "coordinates").expect("attribute");
    assert_eq!(named.split_whitespace().count(), 3, "three names");
    assert!(named.split_whitespace().any(|n| n == "Date"));
    assert_eq!(
        var(&view_, "Date").dim_names,
        ["MT"],
        "Date is 1-D over time, so the shape filter drops it"
    );
    assert!(
        view_
            .curvilinear_coords(var(&view_, "ice_thickness"), "Y", "X")
            .is_some(),
        "the pair still resolves"
    );
}

/// A regular 1-D grid names no 2-D pair, so nothing changes for it.
#[test]
fn a_regular_grid_resolves_no_curvilinear_coordinates() {
    let (_, view_) = view(TRIPOLAR);
    // Asking with the dimensions transposed describes a grid the coordinate
    // variables are not laid out over, and must not resolve.
    assert_eq!(
        view_.curvilinear_coords(var(&view_, "ice_thickness"), "X", "Y"),
        None,
        "the coordinates are (Y, X), not (X, Y)"
    );
}

/// The window is the one the fixture claims: the tripolar fold runs through it.
///
/// South of about 47 °N the RTOFS mesh is an ordinary Mercator lat/lon, where a
/// row is a parallel and its latitude is constant to float precision. The
/// committed window is inside the bipolar patch, where a single row runs up
/// over the pole and back down — so latitude *varies along a row*, which is the
/// property that makes the grid curvilinear at all. A window taken from the
/// regular part of the same file would satisfy every other assertion here and
/// prove nothing.
#[test]
fn the_tripolar_window_actually_contains_the_fold() {
    let oracle: Value = serde_json::from_str(TRIPOLAR_ORACLE).expect("oracle parses");
    let (reader, view_) = view(TRIPOLAR);
    let lat = plane(&reader, &view_, "Latitude");
    let (rows, cols) = (200usize, 260usize);
    assert_eq!(lat.len(), rows * cols);

    // Every row of this window varies in latitude — 1.7° at the bottom rising
    // to 5.0° at the row that runs over the pole. A row of the regular mesh
    // south of 47 °N is flat to float precision, so any spread at all is the
    // signal; the threshold is set well below the measured minimum rather than
    // at it, so a re-cut window has room to move without silently going flat.
    let spreads: Vec<f64> = (0..rows)
        .map(|j| {
            let row = &lat[j * cols..(j + 1) * cols];
            row.iter().cloned().fold(f64::MIN, f64::max)
                - row.iter().cloned().fold(f64::MAX, f64::min)
        })
        .collect();
    let flattest = spreads.iter().cloned().fold(f64::MAX, f64::min);
    assert!(
        flattest > 1.0,
        "row {} spans only {flattest}° of latitude — a regular mesh row, not the fold",
        spreads.iter().position(|s| *s == flattest).unwrap_or(0)
    );
    assert!(
        lat.iter().cloned().fold(f64::MIN, f64::max) > 89.9,
        "the window should reach the pole the grid folds around"
    );

    // Longitudes in the source are not normalised to [-180, 180] — this window
    // runs past 360° — which is a trap for any consumer that assumes otherwise.
    let lon_range = oracle["lon_range"].as_array().expect("lon_range");
    let lon_max = lon_range[1].as_f64().expect("number");
    assert!(
        lon_max > 360.0,
        "the fixture should keep the source's unnormalised longitudes, got {lon_max}"
    );
}

/// The swath window crosses the antimeridian and converges on the pole.
///
/// Both are cases a naive implementation gets wrong in opposite directions: a
/// reader that unwraps longitude by adding 360 to the negatives smears the
/// crossing scanlines across the globe, and one that assumes rows are parallels
/// cannot represent a scan that sweeps past 85 °S.
#[test]
fn the_swath_window_crosses_the_antimeridian_near_the_pole() {
    let (reader, view_) = view(SWATH);
    let lat = plane(&reader, &view_, "Latitude");
    let lon = plane(&reader, &view_, "Longitude");
    let (rows, cols) = (100usize, 96usize);
    assert_eq!(lat.len(), rows * cols);

    let crossing = (0..rows)
        .filter(|&j| {
            let row = &lon[j * cols..(j + 1) * cols];
            let (lo, hi) = row
                .iter()
                .fold((f64::MAX, f64::MIN), |(lo, hi), &v| (lo.min(v), hi.max(v)));
            hi - lo > 180.0
        })
        .count();
    assert!(
        crossing > 0,
        "no scanline spans the antimeridian — the window is the wrong one"
    );

    let southernmost = lat.iter().cloned().fold(f64::MAX, f64::min);
    assert!(
        southernmost < -80.0,
        "the window should reach the polar convergence, got {southernmost}"
    );
}

/// The swath window spans both edges, where the cells are four times as wide.
///
/// `planned/02-trait-seams.md` names "a tripolar fold or a swath edge that the
/// test grid happens not to contain" as the lookup grid's version of the trap
/// that hid #488 — so the window keeps all 96 fields of view rather than a
/// convenient strip around nadir. A cross-track sounder's footprint grows with
/// scan angle: adjacent fields of view are about 17 km apart at nadir and 70 km
/// apart at the edge of the same scan, a 4:1 ratio across one row.
///
/// That ratio is what makes a nearest-cell lookup non-trivial here. An index
/// built on the assumption of uniform cell size is correct at nadir and wrong
/// at the edges by tens of kilometres, and a window that never left nadir would
/// not show it.
#[test]
fn the_swath_window_keeps_the_edges_where_cells_are_widest() {
    let (reader, view_) = view(SWATH);
    let lat = plane(&reader, &view_, "Latitude");
    let lon = plane(&reader, &view_, "Longitude");
    let cols = 96usize;

    // Row 10: far enough from the polar convergence that the growth being
    // measured is scan geometry rather than meridian crowding.
    let row = 10usize;
    let at = |i: usize| (lat[row * cols + i], lon[row * cols + i]);
    let nadir = great_circle_km(at(cols / 2), at(cols / 2 + 1));
    let edge = great_circle_km(at(0), at(1));
    assert!(
        edge / nadir > 3.0,
        "edge cells are only {:.1}x nadir ({edge:.1} km vs {nadir:.1} km) — \
         the window looks like a nadir strip, not a full scan",
        edge / nadir
    );
    let span = great_circle_km(at(0), at(cols - 1));
    assert!(
        span > 2000.0,
        "the scan spans only {span:.0} km — not the full swath"
    );
}

/// Great-circle distance in kilometres, for comparing cell sizes across a scan.
/// A spherical earth is ample: the assertion is about a 4:1 ratio, not metres.
fn great_circle_km((lat_a, lon_a): (f64, f64), (lat_b, lon_b): (f64, f64)) -> f64 {
    let (phi_a, phi_b) = (lat_a.to_radians(), lat_b.to_radians());
    let (d_phi, d_lambda) = ((lat_b - lat_a).to_radians(), (lon_b - lon_a).to_radians());
    let h =
        (d_phi / 2.0).sin().powi(2) + phi_a.cos() * phi_b.cos() * (d_lambda / 2.0).sin().powi(2);
    6371.0 * 2.0 * h.sqrt().asin()
}

/// Sampled cells read back the coordinates the source file holds.
///
/// The oracle is taken from the source granule with the Unidata `netCDF4`
/// bindings, so this is two libraries reading the same octets rather than the
/// reader agreeing with itself — the check that the 2-D coordinate arrays are
/// decoded, chunked and unshuffled correctly, before anything tries to
/// geolocate with them.
#[test]
fn sampled_cells_match_the_coordinates_the_source_file_holds() {
    for (label, bytes, oracle_json, cols) in [
        ("tripolar", TRIPOLAR, TRIPOLAR_ORACLE, 260usize),
        ("swath", SWATH, SWATH_ORACLE, 96),
    ] {
        let oracle: Value = serde_json::from_str(oracle_json).expect("oracle parses");
        let (reader, view_) = view(bytes);
        let lat = plane(&reader, &view_, "Latitude");
        let lon = plane(&reader, &view_, "Longitude");

        let samples = oracle["samples"].as_array().expect("samples");
        assert!(samples.len() >= 20, "{label}: too few sampled cells");
        for sample in samples {
            let y = sample["y"].as_u64().expect("y") as usize;
            let x = sample["x"].as_u64().expect("x") as usize;
            let flat = y * cols + x;
            // float32 storage, so compare at float32 precision rather than f64.
            for (name, got, want) in [
                ("lat", lat[flat], sample["lat"].as_f64().expect("lat")),
                ("lon", lon[flat], sample["lon"].as_f64().expect("lon")),
            ] {
                assert!(
                    (got - want).abs() < 1e-4,
                    "{label} ({y},{x}) {name}: read {got}, source holds {want}"
                );
            }
        }
    }
}

/// Decode one 2-D variable to a dense `f64` plane, dropping masked cells to NaN
/// so index arithmetic stays simple. The coordinate arrays carry no fill.
fn plane(reader: &NetcdfReader, view_: &DatasetView, name: &str) -> Vec<f64> {
    reader
        .decode_variable_values(var(view_, name).decode_index)
        .unwrap_or_else(|e| panic!("{name} decodes: {e}"))
        .into_iter()
        .map(|v| v.unwrap_or(f64::NAN))
        .collect()
}
