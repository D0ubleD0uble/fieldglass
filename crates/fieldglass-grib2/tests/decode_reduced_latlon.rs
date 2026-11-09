//! End-to-end decode and placement of a GRIB2 reduced lat/lon grid (#1760).
//!
//! WMO FM 92 GRIB2 writes a reduced lat/lon grid as template 3.0 with `Ni`
//! coded missing (all ones) and §3's optional list of numbers of points, one
//! entry per row. ECMWF's wave model writes one. It used to be refused on
//! decode with `grid dimensions 4294967295×Nj`, while its geometry said it was
//! a placed lat/lon grid 4294967295 points wide.
//!
//! The fixture is `reduced_latlon_10deg.grib2` (see `fixtures/NOTICE.md`): a
//! global 10° grid of 19 rows whose polar rows hold no points, with a bitmap.
//! The oracle is `reduced_latlon_10deg.grib_get_data.txt`, the pinned eccodes
//! 2.34.1's `grib_get_data -m MISS -L "%.9f %.9f"` of it, committed so the
//! suite needs no eccodes at run time.

use fieldglass_core::GridGeometry;
use fieldglass_grib2::Grib2Reader;

const FIXTURE: &[u8] = include_bytes!("fixtures/reduced_latlon_10deg.grib2");
const ORACLE: &str = include_str!("fixtures/reduced_latlon_10deg.grib_get_data.txt");

/// The row widths the builder wrote: `round(36 cos(lat))`, none on the poles.
const PL: [u32; 19] = [
    0, 6, 12, 18, 23, 28, 31, 34, 35, 36, 35, 34, 31, 28, 23, 18, 12, 6, 0,
];

/// One stored point as eccodes reports it.
struct Point {
    lat: f64,
    lon: f64,
    value: Option<f64>,
}

fn oracle() -> Vec<Point> {
    ORACLE
        .lines()
        .skip(1) // "Latitude Longitude Value"
        .map(|line| {
            let mut cols = line.split_whitespace();
            let mut next = || cols.next().expect("three columns");
            let lat = next().parse().expect("latitude");
            let lon = next().parse().expect("longitude");
            let value = match next() {
                "MISS" => None,
                v => Some(v.parse().expect("value")),
            };
            Point { lat, lon, value }
        })
        .collect()
}

fn reader(bytes: &[u8]) -> Grib2Reader {
    Grib2Reader::from_bytes(bytes.to_vec()).expect("fixture parses")
}

/// Where §3 starts in a single-message file: walk the sections after §0.
fn section_3_offset(bytes: &[u8]) -> usize {
    let mut at = 16;
    loop {
        let len = u32::from_be_bytes(bytes[at..at + 4].try_into().expect("4 octets")) as usize;
        if bytes[at + 4] == 3 {
            return at;
        }
        at += len;
    }
}

#[test]
fn the_grid_reads_as_a_reduced_lat_lon_grid() {
    let reader = reader(FIXTURE);
    let gds = &reader.messages[0].gds;
    assert_eq!(gds.points_per_row(), Some(PL.as_slice()), "the PL list");
    // eccodes' `reduced_ll`, and the name GRIB1 gives the same grid.
    assert_eq!(gds.template_name(), "reduced_latlon");
    // The raster: the widest row by the row count. Never 4294967295.
    assert_eq!(gds.dimensions(), Some((36, 19)));
    assert_eq!(gds.num_data_points, 410, "sum(PL)");
    assert_eq!(oracle().len(), 410, "and eccodes lists 410 points");
    // A lat/lon grid's size is its `Ni x Nj`, as GRIB1 reports it: no label.
    assert_eq!(gds.size_label(), None);
}

/// Every stored point, value and mask, against eccodes' decode.
#[test]
fn decode_yields_every_stored_point_eccodes_decodes() {
    let reader = reader(FIXTURE);
    let values = reader.decode_message_values(0).expect("decodes");
    let oracle = oracle();
    assert_eq!(values.len(), oracle.len());
    let mut masked = 0;
    for (index, (got, want)) in values.iter().zip(&oracle).enumerate() {
        match (got, want.value) {
            (Some(g), Some(w)) => assert!((g - w).abs() < 1e-9, "point {index}: {g} vs {w}"),
            (None, None) => masked += 1,
            _ => panic!("point {index}: {got:?} vs eccodes {:?}", want.value),
        }
    }
    assert_eq!(masked, 38, "the bitmap's masked points");
}

/// The raster and the geometry together put each stored point where eccodes
/// puts it.
///
/// Checked from geometry rather than by restating the expansion: every raster
/// cell sits on its row's latitude, and shows the eccodes point of that row
/// nearest its longitude (within half the row's spacing, around the circle).
/// An empty row is masked across. Every stored point is shown somewhere.
#[test]
fn the_raster_places_every_point_where_eccodes_does() {
    let reader = reader(FIXTURE);
    let gds = &reader.messages[0].gds;
    let raster = reader.decode_message_raster(0).expect("raster");
    let geometry = GridGeometry::from(gds);
    assert_eq!(
        geometry.kind(),
        "latlon",
        "placed as the regular grid it expands to"
    );
    assert_eq!(
        geometry.dims(),
        Some((36, 19)),
        "geometry and section agree"
    );
    assert_eq!(raster.len(), 36 * 19);

    let oracle = oracle();
    let mut row_start = 0;
    let mut shown = vec![false; oracle.len()];
    for (j, &n) in PL.iter().enumerate() {
        let row = &oracle[row_start..row_start + n as usize];
        for k in 0..36u32 {
            let (lat, lon) = geometry.forward(k, j as u32).expect("placed");
            let cell = raster[j * 36 + k as usize];
            assert!(
                (lat - (90.0 - 10.0 * j as f64)).abs() < 1e-9,
                "row {j}: latitude {lat}"
            );
            if n == 0 {
                assert_eq!(cell, None, "row {j} holds no points");
                continue;
            }
            // The points within half the row's spacing of the cell: one, or two
            // when the cell sits exactly between them, and either is right.
            let half_step = 180.0 / f64::from(n);
            let near: Vec<usize> = (0..row.len())
                .filter(|&q| circular(row[q].lon, lon) <= half_step + 1e-9)
                .collect();
            assert!(
                (1..=2).contains(&near.len()),
                "row {j} col {k}: {} points near {lon}",
                near.len()
            );
            let q = *near
                .iter()
                .find(|&&q| row[q].value == cell)
                .unwrap_or_else(|| {
                    panic!("row {j} col {k} at {lon}: {cell:?} is none of the nearest points")
                });
            assert!((row[q].lat - lat).abs() < 1e-9, "row {j}: eccodes latitude");
            shown[row_start + q] = true;
        }
        row_start += n as usize;
    }
    assert!(
        shown.iter().all(|&s| s),
        "every stored point is on the raster"
    );
}

fn circular(a: f64, b: f64) -> f64 {
    let d = (a - b).rem_euclid(360.0);
    d.min(360.0 - d)
}

/// A reduced lat/lon grid whose list cannot be read is refused in words, and
/// its geometry agrees that it is not placed.
#[test]
fn a_reduced_grid_without_a_usable_list_is_refused_by_name() {
    let s3 = section_3_offset(FIXTURE);
    // §3 octet 11: no list at all. Octet 12 code 3: the list is latitudes.
    for (octet, byte, found) in [
        (10, 0u8, "no readable list"),
        (11, 3, "a list of latitudes"),
    ] {
        let mut bytes = FIXTURE.to_vec();
        bytes[s3 + octet] = byte;
        let reader = reader(&bytes);
        let gds = &reader.messages[0].gds;
        assert_eq!(gds.dimensions(), None, "{found}: no size");
        assert!(
            matches!(GridGeometry::from(gds), GridGeometry::Unsupported { .. }),
            "{found}: grid info must not say it is placed"
        );
        let err = reader
            .decode_message_values(0)
            .expect_err("refused")
            .to_string();
        assert!(err.contains("reduced lat/lon grid"), "{err}");
        assert!(err.contains(found), "{err}");
        assert!(err.contains("19 rows"), "{err}");
        assert!(!err.contains("4294967295"), "{err}");
    }
}

/// The same for a count coded missing on a grid that is not reduced: the
/// refusal names the count rather than printing all ones.
#[test]
fn a_count_coded_missing_is_named_not_printed() {
    let regular = include_bytes!("fixtures/regular_latlon_surface.grib2");
    let s3 = section_3_offset(regular);
    // §3 octets 35-38 are template 3.0's `Nj`.
    let mut bytes = regular.to_vec();
    bytes[s3 + 34..s3 + 38].copy_from_slice(&u32::MAX.to_be_bytes());
    let reader = reader(&bytes);
    let gds = &reader.messages[0].gds;
    assert_eq!(gds.dimensions(), None);
    assert!(matches!(
        GridGeometry::from(gds),
        GridGeometry::Unsupported { .. }
    ));
    let err = reader
        .decode_message_values(0)
        .expect_err("refused")
        .to_string();
    assert!(
        err.contains("number of points along a meridian is coded missing"),
        "{err}"
    );
    assert!(!err.contains("4294967295"), "{err}");
}
