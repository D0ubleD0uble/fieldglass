//! Coordinates whose values are no latitudes or longitudes place nothing
//! (#1719).
//!
//! A projected `y` and `x` in meters that lost their `units` and
//! `standard_name` are read as latitude and longitude from their names, and a
//! -1000 to 1000 "latitude" drew the whole world as a blob in the middle of
//! the field. A latitude lies in -90 to 90. A longitude is periodic, so a 1-D
//! longitude axis is refused only when it runs more than twice around the
//! globe or past -720 to 720, and a 2-D pair's longitudes are not checked:
//! RTOFS's run from 74 to 1019. A 2-D latitude cell its own attributes call
//! missing is no position and is not checked. The check is on the values, so it holds however the axes were read:
//! a 1-D pair stating `degrees_north` and a 2-D pair named by a `coordinates`
//! attribute alike.
//!
//! Each case is a committed fixture read through the real NetCDF reader, with
//! one coordinate's values moved before [`slice_placement`] sees them.

use std::ops::Range;

use fieldglass_core::FieldglassError;
use fieldglass_core::array::{ArraySource, Attribute, Group};
use fieldglass_core::cf::{
    AxisKind, CurvilinearPair, SOURCE_ONLY, curvilinear_lookup, curvilinear_pair,
    renderable_arrays, slice_placement, values_out_of_range,
};
use fieldglass_core::projection::GridGeometry;
use fieldglass_netcdf::{NetcdfArrays, NetcdfReader};

const ERSST: &[u8] = include_bytes!("fixtures/ersst_v5_187001_cdf1.nc");
const MIRS: &[u8] = include_bytes!("fixtures/mirs_swath_n21.nc");
const RTOFS: &[u8] = include_bytes!("fixtures/rtofs_tripolar_arctic.nc");

/// A fixture whose one coordinate array reads as `f(value)`, and whose
/// description and further arrays a test may edit.
struct Moved {
    arrays: NetcdfArrays,
    group: Group,
    array: String,
    f: fn(f64) -> f64,
    /// What the array's first stored cell reads as instead.
    first: Option<f64>,
    /// Arrays the file does not hold, each 2-D and read from `values` in C
    /// order with `columns` to a row.
    added: Vec<(String, usize, Vec<f64>)>,
}

impl Moved {
    fn new(bytes: &[u8], array: &str, f: fn(f64) -> f64) -> Self {
        let reader = NetcdfReader::from_bytes(bytes.to_vec()).expect("fixture opens");
        let arrays = NetcdfArrays::open(reader).expect("a view");
        Self {
            group: arrays.group().clone(),
            arrays,
            array: array.to_string(),
            f,
            first: None,
            added: Vec::new(),
        }
    }

    /// The moved array with its first cell reading as `value`.
    fn first_cell(mut self, value: f64) -> Self {
        self.first = Some(value);
        self
    }

    /// `array`'s attributes, to edit.
    fn attributes(&mut self, array: &str) -> &mut Vec<Attribute> {
        &mut self
            .group
            .arrays
            .iter_mut()
            .find(|a| a.name == array)
            .expect("the array is in the root group")
            .attributes
    }
}

impl ArraySource for Moved {
    fn group(&self) -> &Group {
        &self.group
    }

    fn read_region(
        &self,
        array: &str,
        region: &[Range<u64>],
    ) -> Result<Vec<Option<f64>>, FieldglassError> {
        if let Some((_, columns, values)) = self.added.iter().find(|(name, ..)| name == array) {
            let (rows, cols) = (region[0].clone(), region[1].clone());
            let at = |r: u64, c: u64| {
                usize::try_from(r).unwrap() * columns + usize::try_from(c).unwrap()
            };
            return Ok(rows
                .flat_map(|r| cols.clone().map(move |c| (r, c)))
                .map(|(r, c)| Some(values[at(r, c)]))
                .collect());
        }
        let values = self.arrays.read_region(array, region)?;
        if array != self.array {
            return Ok(values);
        }
        let mut values: Vec<Option<f64>> = values.into_iter().map(|v| v.map(self.f)).collect();
        if let (Some(first), true) = (self.first, region.iter().all(|r| r.start == 0)) {
            values[0] = Some(first);
        }
        Ok(values)
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
    // Latitudes past a pole, a longitude axis stretched to run more than
    // twice around the globe (ERSST's runs 0 to 358), and one stored more
    // than two turns from 0.
    for (array, f, case) in [
        ("lat", (|v| v * 20.0) as fn(f64) -> f64, "latitude × 20"),
        ("lat", |v| v + 100.0, "latitude + 100"),
        ("lon", |v| v * 3.0, "longitude × 3"),
        ("lon", |v| v + 1000.0, "longitude + 1000"),
        ("lon", |v| v - 1000.0, "longitude - 1000"),
        ("lon", |v| v + 400.0, "longitude + 400, up to 758"),
    ] {
        let geometry = ersst(array, f);
        assert!(is_source_only(&geometry), "{case}: placed as {geometry:?}");
    }
    // A longitude axis stored a turn away from 0 to 360 is still one, and so
    // is one stretched to just under two turns: 0 to 716, -180 to 178, and
    // -180 to 536.
    for (f, case) in [
        ((|v| v * 2.0) as fn(f64) -> f64, "× 2"),
        (|v| v - 180.0, "-180"),
        (|v| v * 2.0 - 180.0, "× 2 - 180"),
        (|v| v + 360.0, "+360"),
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
    // Without its `valid_range`, which would call every moved cell missing
    // (see below).
    let mut moved = Moved::new(MIRS, &pair.lat, |v| v * 20.0);
    moved
        .attributes(&pair.lat)
        .retain(|a| a.name != "valid_range");
    let geometry = place(&moved);
    assert!(is_source_only(&geometry), "placed as {geometry:?}");
    // The index a host caches says the same, so a host that asks for it
    // first does not place what `slice_placement` refuses.
    assert!(lookup(&moved).is_none());
    // A 2-D pair's longitudes are not checked.
    let moved = Moved::new(MIRS, &pair.lon, |v| v * 20.0);
    assert!(matches!(place(&moved), GridGeometry::Lookup(_)));
}

/// A latitude cell outside the pair's own `valid_range` is missing, as CF
/// says, so it is no position and does not refuse the pair. MIRS declares
/// `valid_range = -90, 90` and `missing_value = -999` on its `Latitude`; a
/// stray -9999 is neither the sentinel nor inside the range.
#[test]
fn a_latitude_cell_outside_its_valid_range_is_not_checked() {
    let (var, pair, y, x) = paired(MIRS);
    let place = |source: &dyn ArraySource| {
        slice_placement(source, &var, y, x)
            .expect("the slice resolves")
            .geometry
    };
    let stray = Moved::new(MIRS, &pair.lat, |v| v).first_cell(-9999.0);
    assert!(
        matches!(place(&stray), GridGeometry::Lookup(_)),
        "a cell outside valid_range"
    );
    assert!(
        curvilinear_lookup(&stray, &var, y, x)
            .expect("reads")
            .is_some()
    );

    // Bounded by `valid_min` alone, the same.
    let mut bounded = Moved::new(MIRS, &pair.lat, |v| v).first_cell(-9999.0);
    let attributes = bounded.attributes(&pair.lat);
    attributes.retain(|a| a.name != "valid_range");
    attributes.push(Attribute::number("valid_min", -90.0));
    assert!(
        matches!(place(&bounded), GridGeometry::Lookup(_)),
        "a cell under valid_min"
    );

    // With no bound declared, the cell is a latitude past the pole, and the
    // pair is refused.
    let mut undeclared = Moved::new(MIRS, &pair.lat, |v| v).first_cell(-9999.0);
    undeclared
        .attributes(&pair.lat)
        .retain(|a| a.name != "valid_range");
    assert!(
        is_source_only(&place(&undeclared)),
        "an undeclared stray cell"
    );
}

/// A cell its `valid_range` calls missing has no position (#1727). Before,
/// it was left out of the check and then indexed at its stored value all the
/// same, so a 95 the file says is not a latitude sat near 85 on the far side
/// of the pole.
#[test]
fn a_latitude_cell_outside_its_valid_range_has_no_position() {
    let (var, pair, y, x) = paired(MIRS);
    let index = |source: &dyn ArraySource| match curvilinear_lookup(source, &var, y, x) {
        Ok(Some(GridGeometry::Lookup(index))) => index,
        other => panic!("placed as {other:?}"),
    };
    let committed = index(&Moved::new(MIRS, &pair.lat, |v| v));
    assert!(
        committed.centre(0, 0).is_some(),
        "the premise: (0, 0) is placed"
    );
    let stray = index(&Moved::new(MIRS, &pair.lat, |v| v).first_cell(95.0));
    assert_eq!(stray.centre(0, 0), None);
    // The rest are where they were.
    assert_eq!(stray.centre(1, 0), committed.centre(1, 0));
}

/// A pair whose every latitude cell is missing places nothing (#1727): an
/// index with no cell in it would place the field nowhere and say it placed
/// it. MIRS's `Latitude` with every value moved out of its `valid_range`,
/// the `missing_value` cells left as they are.
#[test]
fn a_pair_whose_latitudes_are_all_missing_places_nothing() {
    let (var, pair, y, x) = paired(MIRS);
    let missing = Moved::new(
        MIRS,
        &pair.lat,
        |v| {
            if v == -999.0 { v } else { v + 1000.0 }
        },
    );
    let geometry = slice_placement(&missing, &var, y, x)
        .expect("the slice resolves")
        .geometry;
    assert!(is_source_only(&geometry), "placed as {geometry:?}");
    assert!(
        curvilinear_lookup(&missing, &var, y, x)
            .expect("reads")
            .is_none()
    );
}

/// ERSST's sst(time, lev, lat, lon) with a `coordinates` attribute naming a
/// 2-D pair over its own `lat` and `lon`, whose latitudes are `lat` of each
/// row's and which carry `attributes` beside ERSST's own for `lat`.
fn ersst_paired(lat: fn(f64) -> f64, attributes: Vec<Attribute>) -> Moved {
    let mut source = Moved::new(ERSST, "lat", |v| v);
    let (rows, columns) = (89_usize, 180_usize);
    let lats: Vec<f64> = (0..rows * columns)
        .map(|at| lat(-88.0 + 2.0 * (at / columns) as f64))
        .collect();
    let lons: Vec<f64> = (0..rows * columns)
        .map(|at| 2.0 * (at % columns) as f64)
        .collect();
    for (name, from) in [("lat2d", "lat"), ("lon2d", "lon")] {
        let mut array = source
            .group
            .arrays
            .iter()
            .find(|a| a.name == from)
            .expect("ERSST's axis")
            .clone();
        array.name = name.to_string();
        array.dimensions = vec!["lat".to_string(), "lon".to_string()];
        source.group.arrays.push(array);
    }
    source.attributes("lat2d").extend(attributes);
    source.added = vec![
        ("lat2d".to_string(), columns, lats),
        ("lon2d".to_string(), columns, lons),
    ];
    source
        .attributes("sst")
        .push(Attribute::text("coordinates", "lat2d lon2d"));
    assert!(
        curvilinear_pair(source.group(), "sst").is_some(),
        "the premise: sst names the pair"
    );
    source
}

/// A 2-D pair whose latitudes are no degrees stops the precedence: the
/// variable's own 1-D `lat` and `lon` axes, which would place it, are not
/// reached, so every field naming the pair is answered the same way.
#[test]
fn an_impossible_two_d_pair_is_not_passed_over_for_good_one_d_axes() {
    let place = |source: &Moved| {
        slice_placement(source, "sst", 2, 3)
            .expect("the slice resolves")
            .geometry
    };
    // The premise: the pair, in degrees, places the slice by itself.
    let degrees = place(&ersst_paired(|v| v, Vec::new()));
    assert!(matches!(degrees, GridGeometry::Lookup(_)), "{degrees:?}");

    // The latitudes × 20.
    let geometry = place(&ersst_paired(|v| v * 20.0, Vec::new()));
    assert!(is_source_only(&geometry), "placed as {geometry:?}");

    // Every latitude cell outside a `valid_max` the pair states (#1727):
    // stored and finite, and none of them a position, so the pair says the
    // field is nowhere rather than giving way to the 1-D axes.
    let geometry = place(&ersst_paired(
        |v| v,
        vec![Attribute::number("valid_max", -100.0)],
    ));
    assert!(is_source_only(&geometry), "placed as {geometry:?}");
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
    // A longitude axis may run up to two turns, and sit up to two turns
    // from 0.
    assert_eq!(values_out_of_range(Longitude, &[-360.0, 360.0]), None);
    assert_eq!(values_out_of_range(Longitude, &[0.0, 720.0]), None);
    assert_eq!(values_out_of_range(Longitude, &[-180.0, 540.0]), None);
    assert_eq!(values_out_of_range(Longitude, &[-720.0, -400.0]), None);
    assert_eq!(
        values_out_of_range(Longitude, &[1000.0, 1300.0]),
        Some((1000.0, 1300.0))
    );
    assert_eq!(
        values_out_of_range(Longitude, &[-721.0, -700.0]),
        Some((-721.0, -700.0))
    );
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

/// A pole stored a hair past 90 is placed at the pole, exactly, through the
/// NetCDF reader as through a Zarr store (#1305). ERSST's latitudes run
/// -88 to 88; stretched to ±90.0005, the grid's corners are ±90.
#[test]
fn a_pole_a_hair_past_90_is_placed_at_the_pole() {
    let GridGeometry::LatLon(p) = ersst("lat", |v| v * (90.0005 / 88.0)) else {
        panic!("placed");
    };
    let mut ends = [p.lat_first, p.lat_last];
    ends.sort_by(f64::total_cmp);
    assert_eq!(ends, [-90.0, 90.0], "{p:?}");
    // Past the slack, still not placed.
    assert!(is_source_only(&ersst("lat", |v| v * (90.002 / 88.0))));
}
