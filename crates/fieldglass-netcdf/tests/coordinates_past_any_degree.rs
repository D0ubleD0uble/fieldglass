//! Coordinates whose values are no latitudes or longitudes place nothing
//! (#1719).
//!
//! A projected `y` and `x` in metres that lost their `units` and
//! `standard_name` are read as latitude and longitude from their names, and a
//! -1000 to 1000 "latitude" drew the whole world as a blob in the middle of
//! the field. A latitude lies in -90 to 90. A longitude is periodic, so a 1-D
//! longitude axis is refused only when it runs more than twice around the
//! globe, and a 2-D pair's longitudes are not checked: RTOFS's run from 74 to
//! 1019. The check is on the values, so it holds however the axes were read:
//! a 1-D pair stating `degrees_north` and a 2-D pair named by a `coordinates`
//! attribute alike.
//!
//! Each case is a committed fixture read through the real NetCDF reader, with
//! one coordinate's values moved before [`slice_placement`] sees them.

use std::ops::Range;

use fieldglass_core::FieldglassError;
use fieldglass_core::array::{ArraySource, Group};
use fieldglass_core::cf::{
    AxisKind, CurvilinearPair, SOURCE_ONLY, curvilinear_lookup, curvilinear_pair,
    renderable_arrays, slice_placement, values_out_of_range,
};
use fieldglass_core::projection::GridGeometry;
use fieldglass_netcdf::{NetcdfArrays, NetcdfReader};

const ERSST: &[u8] = include_bytes!("fixtures/ersst_v5_187001_cdf1.nc");
const MIRS: &[u8] = include_bytes!("fixtures/mirs_swath_n21.nc");
const RTOFS: &[u8] = include_bytes!("fixtures/rtofs_tripolar_arctic.nc");

/// A fixture whose one coordinate array reads as `f(value)`.
struct Moved {
    arrays: NetcdfArrays,
    array: String,
    f: fn(f64) -> f64,
}

impl Moved {
    fn new(bytes: &[u8], array: &str, f: fn(f64) -> f64) -> Self {
        let reader = NetcdfReader::from_bytes(bytes.to_vec()).expect("fixture opens");
        Self {
            arrays: NetcdfArrays::open(reader).expect("a view"),
            array: array.to_string(),
            f,
        }
    }
}

impl ArraySource for Moved {
    fn group(&self) -> &Group {
        self.arrays.group()
    }

    fn read_region(
        &self,
        array: &str,
        region: &[Range<u64>],
    ) -> Result<Vec<Option<f64>>, FieldglassError> {
        let values = self.arrays.read_region(array, region)?;
        if array != self.array {
            return Ok(values);
        }
        Ok(values.into_iter().map(|v| v.map(self.f)).collect())
    }
}

fn is_source_only(geometry: &GridGeometry) -> bool {
    matches!(geometry, GridGeometry::Unsupported { label, declared: None } if label == SOURCE_ONLY)
}

/// ERSST's `sst(time, lev, lat, lon)` on its own `lat` and `lon`, which state
/// `degrees_north` and `degrees_east`.
fn ersst(array: &str, f: fn(f64) -> f64) -> GridGeometry {
    slice_placement(&Moved::new(ERSST, array, f), "sst", 2, 3)
        .expect("the slice resolves")
        .geometry
}

#[test]
fn a_one_d_axis_that_is_no_latitude_or_longitude_places_nothing() {
    assert!(
        matches!(ersst("lat", |v| v), GridGeometry::LatLon(_)),
        "the premise: the fixture is placed"
    );
    // Latitudes past a pole, and a longitude axis stretched to run more than
    // twice around the globe (ERSST's runs 0 to 358).
    for (array, f, case) in [
        ("lat", (|v| v * 20.0) as fn(f64) -> f64, "latitude × 20"),
        ("lat", |v| v + 100.0, "latitude + 100"),
        ("lon", |v| v * 3.0, "longitude × 3"),
    ] {
        let geometry = ersst(array, f);
        assert!(is_source_only(&geometry), "{case}: placed as {geometry:?}");
    }
    // A longitude axis stored a turn or more away from 0 to 360 is still
    // one, and so is one stretched to just under two turns.
    for (f, case) in [
        ((|v| v + 1000.0) as fn(f64) -> f64, "+1000"),
        (|v| v - 1000.0, "-1000"),
        (|v| v * 2.0, "× 2"),
    ] {
        assert!(
            matches!(ersst("lon", f), GridGeometry::LatLon(_)),
            "longitude {case}"
        );
    }
}

/// The scan is still read off the axes when the values place nothing, so the
/// raster keeps the orientation it had when it was placed.
#[test]
fn the_rows_keep_their_order_when_nothing_places_them() {
    let placed = slice_placement(&Moved::new(ERSST, "lat", |v| v), "sst", 2, 3).expect("places");
    let unplaced =
        slice_placement(&Moved::new(ERSST, "lat", |v| v * 20.0), "sst", 2, 3).expect("resolves");
    assert!(is_source_only(&unplaced.geometry));
    assert_eq!(unplaced.scan, placed.scan);
}

/// The first variable in `bytes` placed by a 2-D pair, with the pair and the
/// variable's two axes it spans.
fn paired(bytes: &[u8]) -> (String, CurvilinearPair, usize, usize) {
    let reader = NetcdfReader::from_bytes(bytes.to_vec()).expect("fixture opens");
    let arrays = NetcdfArrays::open(reader).expect("a view");
    renderable_arrays(arrays.group())
        .into_iter()
        .find_map(|v| {
            let pair = curvilinear_pair(arrays.group(), &v.name)?;
            let y = v.dims.iter().position(|d| d.name == pair.y_dim)?;
            let x = v.dims.iter().position(|d| d.name == pair.x_dim)?;
            Some((v.name, pair, y, x))
        })
        .expect("a variable placed by a 2-D pair")
}

#[test]
fn a_two_d_pair_whose_latitudes_run_past_the_poles_places_nothing() {
    let (var, pair, y, x) = paired(MIRS);
    let place = |source: &dyn ArraySource| {
        slice_placement(source, &var, y, x)
            .expect("the slice resolves")
            .geometry
    };
    let lookup = |source: &dyn ArraySource| curvilinear_lookup(source, &var, y, x).expect("reads");
    let committed = Moved::new(MIRS, &pair.lat, |v| v);
    assert!(
        matches!(place(&committed), GridGeometry::Lookup(_)),
        "the premise: the fixture is placed by its pair"
    );
    let moved = Moved::new(MIRS, &pair.lat, |v| v * 20.0);
    let geometry = place(&moved);
    assert!(is_source_only(&geometry), "placed as {geometry:?}");
    // The index a host caches says the same, so a host that asks for it
    // first does not place what `slice_placement` refuses.
    assert!(lookup(&moved).is_none());
    // A 2-D pair's longitudes are not checked.
    let moved = Moved::new(MIRS, &pair.lon, |v| v * 20.0);
    assert!(matches!(place(&moved), GridGeometry::Lookup(_)));
}

/// RTOFS's tripolar longitudes run from 74 to 1019 across the Arctic fold,
/// more than twice around the globe, and the grid is placed by them.
#[test]
fn a_tripolar_grid_is_still_placed() {
    let (var, _, y, x) = paired(RTOFS);
    let reader = NetcdfReader::from_bytes(RTOFS.to_vec()).expect("fixture opens");
    let arrays = NetcdfArrays::open(reader).expect("a view");
    let geometry = slice_placement(&arrays, &var, y, x)
        .expect("the slice resolves")
        .geometry;
    assert!(matches!(geometry, GridGeometry::Lookup(_)), "{geometry:?}");
}

#[test]
fn a_latitude_lies_in_minus_90_to_90_and_a_longitude_axis_within_two_turns() {
    use AxisKind::{Latitude, Longitude};
    assert_eq!(values_out_of_range(Latitude, &[-90.0, 0.0, 90.0]), None);
    // A pole stored as `f32`, or scaled from a packed integer, lands a little
    // past 90.
    assert_eq!(values_out_of_range(Latitude, &[-90.0005, 90.0005]), None);
    assert_eq!(
        values_out_of_range(Latitude, &[-1000.0, 1000.0]),
        Some((-1000.0, 1000.0))
    );
    assert_eq!(
        values_out_of_range(Latitude, &[0.0, 91.0]),
        Some((0.0, 91.0))
    );
    // Any longitude value names one; only the span is checked.
    assert_eq!(values_out_of_range(Longitude, &[1000.0, 1300.0]), None);
    assert_eq!(values_out_of_range(Longitude, &[-360.0, 360.0]), None);
    assert_eq!(values_out_of_range(Longitude, &[0.0, 720.0]), None);
    assert_eq!(
        values_out_of_range(Longitude, &[0.0, 721.0]),
        Some((0.0, 721.0))
    );
    assert_eq!(
        values_out_of_range(Longitude, &[-2000.0, 0.0, 2000.0]),
        Some((-2000.0, 2000.0))
    );
    // Masked cells of a 2-D pair read as NaN and are skipped, and so is an
    // array with nothing finite in it.
    assert_eq!(values_out_of_range(Latitude, &[f64::NAN, 10.0]), None);
    assert_eq!(
        values_out_of_range(Latitude, &[f64::NAN, 100.0]),
        Some((100.0, 100.0))
    );
    assert_eq!(values_out_of_range(Latitude, &[f64::NAN]), None);
    assert_eq!(values_out_of_range(Latitude, &[]), None);
}
