//! End-to-end decode and geolocation of GRIB2 reduced Gaussian grids (#503).
//!
//! ECMWF's ordinary output is a reduced Gaussian grid: each parallel holds a
//! different number of points, so the message has no `Ni` and the field is
//! `sum(PL)` values long rather than `Ni·Nj`. Until this landed, GRIB2 parsed
//! such a message as metadata and refused to decode it, where `fieldglass_grib1`
//! had decoded its reduced grids since #47.
//!
//! Two fixtures, both eccodes 2.34.1 samples (see `fixtures/NOTICE.md`): the
//! classic `N32` (`reduced_gaussian_pressure_level.grib2`, 6114 points) and the
//! octahedral `O32` built for #500 (`octahedral_gaussian_o32.grib2`, 5248).
//! Values are cross-checked by the fixture walk in `eccodes_reference.rs`, which
//! both fixtures rejoined when their value exemptions were dropped; what this
//! file pins is the *geometry*, which no value oracle can see.

use fieldglass_grib2::{Grib2Reader, GridTemplate};

const CLASSIC: &[u8] = include_bytes!("fixtures/reduced_gaussian_pressure_level.grib2");
const OCTAHEDRAL: &[u8] = include_bytes!("fixtures/octahedral_gaussian_o32.grib2");

/// `grib_get_data` on the classic fixture, grouped by parallel: 64 rows whose
/// widths rise from 20 at the pole to 128 at the equator and fall back.
const CLASSIC_PL: [u32; 64] = [
    20, 27, 36, 40, 45, 50, 60, 64, 72, 75, 80, 90, 90, 96, 100, 108, 108, 120, 120, 120, 128, 128,
    128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128, 128,
    128, 128, 128, 120, 120, 120, 108, 108, 100, 96, 90, 90, 80, 75, 72, 64, 60, 50, 45, 40, 36,
    27, 20,
];

/// The same for the octahedral fixture: widths step by exactly four per row,
/// 20 at the pole to 144 at the equator — which is *wider* than the classic
/// grid's 128 at the same `N`, and the reason the two rasters differ.
const OCTAHEDRAL_PL: [u32; 64] = [
    20, 24, 28, 32, 36, 40, 44, 48, 52, 56, 60, 64, 68, 72, 76, 80, 84, 88, 92, 96, 100, 104, 108,
    112, 116, 120, 124, 128, 132, 136, 140, 144, 144, 140, 136, 132, 128, 124, 120, 116, 112, 108,
    104, 100, 96, 92, 88, 84, 80, 76, 72, 68, 64, 60, 56, 52, 48, 44, 40, 36, 32, 28, 24, 20,
];

#[test]
fn the_pl_list_survives_the_parse_and_names_the_raster() {
    for (bytes, pl, label, width, stored) in [
        (CLASSIC, CLASSIC_PL.as_slice(), "N32", 128u32, 6114usize),
        (OCTAHEDRAL, OCTAHEDRAL_PL.as_slice(), "O32", 144, 5248),
    ] {
        let reader = Grib2Reader::from_bytes(bytes.to_vec()).expect("fixture parses");
        let gds = &reader.messages[0].gds;
        assert_eq!(gds.points_per_row(), Some(pl), "{label}: PL list");
        // Named the way eccodes names it (`reduced_gg`) and the way GRIB1 does,
        // so the message table reads the same in both editions.
        assert_eq!(gds.template_name(), "reduced_gaussian", "{label}");
        // The raster is the widest row by the row count — derived, not stated.
        assert_eq!(gds.dimensions(), Some((width, 64)), "{label}: raster");
        // What the file itself says its size is, which a display prefers.
        assert_eq!(gds.size_label().as_deref(), Some(label));
        // §3's own point count is the field length, and it is smaller than the
        // raster: that gap is the whole reason a reduced grid exists.
        assert_eq!(gds.num_data_points as usize, stored, "{label}: sum(PL)");
        assert!(
            stored < width as usize * 64,
            "{label}: {stored} points in a {width}x64 raster"
        );
    }
}

/// Decode yields the native `sum(PL)` field, not the raster.
///
/// The expansion to `max(PL) × Nj` is a display step and belongs at the one
/// boundary that does it, so the reader returns what the message stores — which
/// is also what eccodes reports, and what the `.eccodes.ref.json` value block
/// counts.
#[test]
fn decode_yields_the_stored_point_count_eccodes_reports() {
    for (bytes, label, stored, first) in [
        (CLASSIC, "N32", 6114usize, 247.464_843_75f64),
        (OCTAHEDRAL, "O32", 5248, 0.0),
    ] {
        let reader = Grib2Reader::from_bytes(bytes.to_vec()).expect("fixture parses");
        let values = reader
            .decode_message_values(0)
            .unwrap_or_else(|e| panic!("{label} decodes: {e}"));
        assert_eq!(values.len(), stored, "{label}: sum(PL), not Ni·Nj");
        assert!(
            values.iter().all(|v| v.is_some()),
            "{label}: no bitmap, so no masked points"
        );
        let v = values[0].expect("present");
        assert!((v - first).abs() < 1e-6, "{label}: value[0] = {v}");
    }
}

/// The octahedral fixture carries a sawtooth ramp, so every point pins its own
/// index.
///
/// `tools/build_grib2_octahedral_fixture.py` writes `index mod 50` (see
/// `NOTICE.md`), and `grib_get_data` reads it back the same way. That is a much
/// stronger oracle than the statistics in the `.eccodes.ref.json` value block:
/// a decode that dropped, duplicated or reordered a point would show here,
/// where a min/max/average check would not. The 50-point period also means an
/// off-by-one in the bit walk cannot hide behind a monotone ramp.
#[test]
fn the_octahedral_field_decodes_point_for_point() {
    let reader = Grib2Reader::from_bytes(OCTAHEDRAL.to_vec()).expect("fixture parses");
    let values = reader.decode_message_values(0).expect("decodes");
    assert_eq!(values.len(), 5248);
    for (index, value) in values.iter().enumerate() {
        let v = value.expect("present");
        let expected = (index % 50) as f64;
        assert!(
            (v - expected).abs() < 1e-6,
            "value[{index}] = {v}, expected {expected}"
        );
    }
}

/// The row latitudes are the Gauss–Legendre nodes, and the points within each
/// row are equispaced around the full circle.
///
/// This is the model `expand_reduced_to_regular` assumes, so it is worth
/// checking against the data rather than against itself. Verified once against
/// `grib_get_data -L "%.9f %.9f"` on both fixtures: every row's latitude matches
/// `gaussian_latitudes(32)` to 1e-6°, and every point's longitude matches
/// `k·360/PL[j]` to 5e-10°. Held here analytically so the suite needs no
/// eccodes at run time.
#[test]
fn rows_sit_on_gaussian_parallels_and_span_the_full_circle() {
    let lats = fieldglass_core::gaussian_latitudes(32);
    assert_eq!(lats.len(), 64, "2N nodes, north to south");
    // eccodes' first and last parallel for N32, to the precision it prints.
    assert!((lats[0] - 87.863_798_839).abs() < 1e-6, "{}", lats[0]);
    assert!((lats[63] + 87.863_798_839).abs() < 1e-6, "{}", lats[63]);

    for (bytes, label) in [(CLASSIC, "N32"), (OCTAHEDRAL, "O32")] {
        let reader = Grib2Reader::from_bytes(bytes.to_vec()).expect("fixture parses");
        let gds = &reader.messages[0].gds;
        let GridTemplate::Gaussian(t) = &gds.template else {
            panic!("{label}: expected a Gaussian template");
        };
        // The declared corners are the Gaussian nodes, so the parallels the
        // renderer walks are the parallels the file is on.
        assert!((t.la1 - lats[0]).abs() < 1e-3, "{label}: la1 {}", t.la1);
        assert!((t.la2 - lats[63]).abs() < 1e-3, "{label}: la2 {}", t.la2);
        assert_eq!(t.lo1, 0.0, "{label}: rows start at Greenwich");
        assert_eq!(t.n_parallels, 32, "{label}: N");
    }
}

/// The raster's east edge is derived from its width, never from the file.
///
/// Both fixtures declare `longitudeOfLastGridPoint = 357.1875`, which is
/// `360 - 360/128` — the last column of the *reference regular* grid, `4N` wide.
/// For the classic `N32` that is also the widest row, so the two agree. For the
/// octahedral `O32` the widest row is 144, and the raster's last column sits at
/// `360 - 360/144 = 357.5`. Placing 144 columns on the declared 357.1875 span
/// would draw every column up to an eighth of a cell west of its data, so the
/// render seam derives the value instead (`reduced_raster_lon_last`).
#[test]
fn the_expanded_raster_east_edge_is_derived_not_declared() {
    for (bytes, label, declared, derived) in [
        (CLASSIC, "N32", 357.1875f64, 357.1875f64),
        (OCTAHEDRAL, "O32", 357.1875, 357.5),
    ] {
        let reader = Grib2Reader::from_bytes(bytes.to_vec()).expect("fixture parses");
        let gds = &reader.messages[0].gds;
        let (_, lon_first, _, lon_last) = gds.bounds().expect("a reduced grid has bounds");
        assert!(
            (lon_last - declared).abs() < 1e-3,
            "{label}: bounds() stays faithful to the file: {lon_last}"
        );
        let (width, _) = gds.dimensions().expect("a raster shape");
        let raster_lon_last = fieldglass_core::reduced_raster_lon_last(lon_first, width);
        assert!(
            (raster_lon_last - derived).abs() < 1e-9,
            "{label}: raster east edge {raster_lon_last}, expected {derived}"
        );
    }
    // The two differ for exactly one of them, which is what makes the
    // distinction worth drawing rather than a restatement of the same number.
    assert_ne!(357.1875f64, 357.5f64);
}

/// Expanding the decoded field fills the raster the dimensions promise.
#[test]
fn expansion_fills_the_raster_and_keeps_the_widest_rows_intact() {
    let reader = Grib2Reader::from_bytes(CLASSIC.to_vec()).expect("fixture parses");
    let gds = &reader.messages[0].gds;
    let pl = gds.points_per_row().expect("a reduced grid");
    let (width, height) = gds.dimensions().expect("a raster shape");
    let values = reader.decode_message_values(0).expect("decodes");

    let expanded = fieldglass_core::expand_reduced_to_regular(&values, pl, width as usize);
    assert_eq!(expanded.len(), (width as usize) * (height as usize));
    assert!(expanded.iter().all(|v| v.is_some()), "no holes");

    // A row already at the raster width is copied through untouched — for this
    // grid that is rows 20..=43, and their storage offset is sum(PL[..20]).
    let offset: usize = pl[..20].iter().map(|&n| n as usize).sum();
    assert_eq!(pl[20], width, "row 20 is a widest row");
    assert_eq!(
        &expanded[20 * width as usize..21 * width as usize],
        &values[offset..offset + width as usize],
    );

    // Every value in the raster came from the field: expansion resamples, it
    // never invents. (The polar rows repeat points; none is new.)
    let stored: std::collections::BTreeSet<u64> = values
        .iter()
        .map(|v| v.expect("present").to_bits())
        .collect();
    assert!(
        expanded
            .iter()
            .all(|v| stored.contains(&v.expect("present").to_bits())),
        "expansion introduced a value the message does not contain"
    );
}
