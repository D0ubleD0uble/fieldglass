//! End-to-end decode of the ECMWF complex / second-order packing.
//!
//! Fixture: the first message extracted from a 64-message ECMWF GRIB1 file
//! (LFPW MARS-derived analysis on a 240 × 121 lat-long grid, 2006-12-10
//! 18Z + 24h, geopotential at 50 hPa). Provided by the user as
//! representative of the file class that today's simple-packing decoder
//! refused with `unsupported section`.
//!
//! Pinned against an eccodes 2.34.1 `grib_get_data` snapshot at
//! `tests/fixtures/ecmwf_lfpw_msg0_expected.json` (12 anchored sample
//! values + count/min/max/mean).

use fieldglass_grib1::{Grib1Reader, parse_bds_header};

const FIXTURE: &[u8] = include_bytes!("fixtures/ecmwf_lfpw_msg0.grib1");

#[test]
fn parses_with_complex_extended_header_populated() {
    let reader = Grib1Reader::from_bytes(FIXTURE.to_vec()).expect("fixture parses");
    assert_eq!(reader.message_count(), 1);

    let msg = &reader.messages[0];
    let (bds_start, bds_end) = msg.bds_range;
    let bds = parse_bds_header(&FIXTURE[bds_start..bds_end]).expect("BDS header parses");

    assert!(bds.is_complex_packing, "complex packing flag");
    assert!(bds.has_extra_flags, "extra-flags bit set");

    let ext = bds
        .complex_extended
        .as_ref()
        .expect("complex_extended populated when both flags set");

    // Cross-checked with `grib_dump -O` for this exact file:
    //   secondaryBitmapPresent      = 0
    //   secondOrderOfDifferentWidth = 1
    //   generalExtended2ordr        = 1
    //   boustrophedonicOrdering     = 1
    //   twoOrdersOfSPD              = 1
    //   plusOneinOrdersOfSPD        = 0  → orderOfSPD = 2
    assert!(!ext.secondary_bitmap_present());
    assert!(ext.second_order_of_different_width());
    assert!(ext.general_extended_2ordr());
    assert!(ext.boustrophedonic());
    assert!(ext.two_orders_of_spd());
    assert!(!ext.plus_one_in_orders_of_spd());
    assert_eq!(ext.order_of_spd(), 2);

    assert_eq!(ext.packing_type_label(), "grid_second_order");
}

#[test]
fn decode_matches_eccodes_oracle() {
    let reader = Grib1Reader::from_bytes(FIXTURE.to_vec()).expect("fixture parses");
    let values = reader
        .decode_message_values(0)
        .expect("second-order decode succeeds");

    // No bitmap on this message, so every entry is present.
    let present: Vec<f64> = values
        .into_iter()
        .map(|v| v.expect("no missing values"))
        .collect();

    // From `tests/fixtures/ecmwf_lfpw_msg0_expected.json`.
    assert_eq!(present.len(), 29_040);
    let min = present.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = present.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let mean: f64 = present.iter().sum::<f64>() / present.len() as f64;

    let tol = 1e-3;
    assert!(
        (min - 19_074.872_559).abs() < tol,
        "min was {min}, expected 19074.872559"
    );
    assert!(
        (max - 20_717.558_594).abs() < tol,
        "max was {max}, expected 20717.558594"
    );
    assert!(
        (mean - 20_216.718_135_691_048).abs() < tol,
        "mean was {mean}, expected 20216.7181"
    );

    // Anchored samples — eccodes-derived ground truth at specific scan-order indices.
    let samples: &[(usize, f64)] = &[
        (0, 19_080.708_496),
        (1, 19_080.708_496),
        (119, 19_080.708_496),
        (120, 19_080.708_496),
        (121, 19_080.708_496),
        (240, 19_085.677_856),
        (14_400, 20_563.404_663),
        (14_520, 20_522.094_849),
        (14_640, 20_564.169_189),
        (28_800, 19_917.864_38),
        (28_919, 19_917.864_38),
        (29_039, 19_917.864_38),
    ];
    for (i, expected) in samples {
        let got = present[*i];
        assert!(
            (got - expected).abs() < tol,
            "values[{i}] was {got}, expected {expected} (tol {tol})"
        );
    }
}

/// End-to-end regression for the PDS sign-magnitude D fix.
///
/// The shipped fixture has decimal scale factor D = 0, so a buggy
/// two's-complement read of octets 26-27 happens to round-trip. To exercise
/// the sign-magnitude path through the whole decode pipeline we patch the
/// PDS D bytes from `0x0000` to `0x8002` (sign bit + magnitude 2 → D = -2).
///
/// With the correct sign-magnitude decode, every decoded value is scaled by
/// `10^(-D) = 10^2 = 100`. With the previous two's-complement decode, the
/// parser would read D = -32766 and multiply by `10^32766` → `+inf`,
/// silently corrupting every value.
///
/// Cross-checked against `grib_set -s decimalScaleFactor=-2` on the same
/// fixture (eccodes 2.34.1): min/max/mean and the same anchored samples
/// land at exactly 100× the original oracle values.
#[test]
fn pds_negative_d_propagates_sign_magnitude_through_decode() {
    let mut bytes = FIXTURE.to_vec();
    let grib_off = bytes
        .windows(4)
        .position(|w| w == b"GRIB")
        .expect("fixture has GRIB magic");
    let pds_start = grib_off + 8;
    assert_eq!(bytes[pds_start + 26], 0x00);
    assert_eq!(bytes[pds_start + 27], 0x00);
    bytes[pds_start + 26] = 0x80;
    bytes[pds_start + 27] = 0x02;

    let reader = Grib1Reader::from_bytes(bytes).expect("patched fixture parses");
    let values = reader
        .decode_message_values(0)
        .expect("second-order decode succeeds with negative D");
    let present: Vec<f64> = values
        .into_iter()
        .map(|v| v.expect("no missing values"))
        .collect();

    let min = present.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = present.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let mean: f64 = present.iter().sum::<f64>() / present.len() as f64;

    let tol = 1e-1;
    assert!(
        (min - 1_907_487.255_9).abs() < tol,
        "min was {min}, expected 1_907_487.2559"
    );
    assert!(
        (max - 2_071_755.859_4).abs() < tol,
        "max was {max}, expected 2_071_755.8594"
    );
    assert!(
        (mean - 2_021_671.813_569_104_5).abs() < tol,
        "mean was {mean}, expected 2_021_671.8135"
    );

    let samples: &[(usize, f64)] = &[
        (0, 1_908_070.849_6),
        (240, 1_908_567.785_6),
        (14_400, 2_056_340.466_3),
        (29_039, 1_991_786.438_0),
    ];
    for (i, expected) in samples {
        let got = present[*i];
        assert!(
            (got - expected).abs() < tol,
            "values[{i}] was {got}, expected {expected} (tol {tol})"
        );
    }
}

// ---------------------------------------------------------------------------
// orderOfSPD = 3 (`grid_second_order_SPD3`)
// ---------------------------------------------------------------------------
//
// Fixture: the LFPW message above re-encoded by eccodes 2.34.1 into the
// third-order spatial-differencing variant
// (`grib_set -r -s boustrophedonicOrdering=0,packingType=grid_second_order`
// then orderOfSPD=3), which keeps the same decoded field but exercises the
// orderOfSPD=3 read path (three SPD seeds + bias). Pinned against the eccodes
// `grib_get_data` snapshot in `ecmwf_spd3_msg0_expected.json`. See NOTICE.md
// for provenance. (This message has boustrophedonic ordering off; the
// boustrophedonic-order-3 path is covered separately below.)

const SPD3_FIXTURE: &[u8] = include_bytes!("fixtures/ecmwf_spd3_msg0.grib1");

#[test]
fn spd3_header_reports_order_three() {
    let reader = Grib1Reader::from_bytes(SPD3_FIXTURE.to_vec()).expect("fixture parses");
    let msg = &reader.messages[0];
    let (bds_start, bds_end) = msg.bds_range;
    let bds = parse_bds_header(&SPD3_FIXTURE[bds_start..bds_end]).expect("BDS header parses");

    let ext = bds
        .complex_extended
        .as_ref()
        .expect("complex_extended populated");
    assert!(!ext.secondary_bitmap_present());
    assert!(ext.second_order_of_different_width());
    assert!(ext.general_extended_2ordr());
    assert!(
        !ext.boustrophedonic(),
        "re-encoded with boustrophedonic off"
    );
    assert_eq!(ext.order_of_spd(), 3);
    assert_eq!(ext.packing_type_label(), "grid_second_order_SPD3");
}

#[test]
fn decode_spd3_matches_eccodes_oracle() {
    let reader = Grib1Reader::from_bytes(SPD3_FIXTURE.to_vec()).expect("fixture parses");
    let present: Vec<f64> = reader
        .decode_message_values(0)
        .expect("order-3 second-order decode succeeds")
        .into_iter()
        .map(|v| v.expect("no missing values"))
        .collect();

    // From `tests/fixtures/ecmwf_spd3_msg0_expected.json` (same field as the
    // SPD-2 fixture — eccodes re-packed identical values at orderOfSPD=3).
    assert_eq!(present.len(), 29_040);
    let min = present.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = present.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let mean: f64 = present.iter().sum::<f64>() / present.len() as f64;

    let tol = 1e-3;
    assert!((min - 19_074.872_559).abs() < tol, "min was {min}");
    assert!((max - 20_717.558_594).abs() < tol, "max was {max}");
    assert!(
        (mean - 20_216.718_135_690_968).abs() < tol,
        "mean was {mean}"
    );

    let samples: &[(usize, f64)] = &[
        (0, 19_080.708_496),
        (1, 19_080.708_496),
        (119, 19_080.708_496),
        (120, 19_080.708_496),
        (121, 19_080.708_496),
        (240, 19_085.677_856),
        (1_000, 19_124.872_559),
        (14_400, 20_563.404_663),
        (14_520, 20_522.094_849),
        (14_640, 20_564.169_189),
        (20_000, 20_621.075_439),
        (28_800, 19_917.864_38),
        (28_919, 19_917.864_38),
        (29_039, 19_917.864_38),
    ];
    for (i, expected) in samples {
        let got = present[*i];
        assert!(
            (got - expected).abs() < tol,
            "values[{i}] was {got}, expected {expected} (tol {tol})"
        );
    }
}

// ---------------------------------------------------------------------------
// orderOfSPD = 3 with boustrophedonic ordering (`grid_second_order_SPD3`,
// boustrophedonicOrdering = 1)
// ---------------------------------------------------------------------------
//
// The non-boustrophedonic SPD-3 fixture above exercises the order-3 inverse
// but not the row-reversal that runs *after* reconstruction. eccodes 2.34
// can't *encode* a boustrophedonic SPD-3 file (ECC-1402: the encoder
// miscounts `numericValues` by `orderOfSPD - 2` when re-packing), so this
// fixture is `ecmwf_spd3_msg0.grib1` with the boustrophedonic flag bit
// (BDS extended-flag bit 6, 0x04) flipped on in place — a byte edit, no
// re-encode. eccodes decodes the result fine (the bug is encode-only) and
// applies the odd-row reversal, giving an independent oracle for the
// combined order-3 + boustrophedonic path. Pinned against
// `ecmwf_spd3_boust_msg0_expected.json`; provenance in NOTICE.md.
//
// The decoded field is no longer the meteorological original — odd rows are
// reversed relative to it — but the bytes are a valid boustrophedonic
// SPD-3 message, so matching eccodes byte-for-byte is the point.

const SPD3_BOUST_FIXTURE: &[u8] = include_bytes!("fixtures/ecmwf_spd3_boust_msg0.grib1");

#[test]
fn spd3_boustrophedonic_header_reports_order_three_and_zigzag() {
    let reader = Grib1Reader::from_bytes(SPD3_BOUST_FIXTURE.to_vec()).expect("fixture parses");
    let msg = &reader.messages[0];
    let (bds_start, bds_end) = msg.bds_range;
    let bds = parse_bds_header(&SPD3_BOUST_FIXTURE[bds_start..bds_end]).expect("BDS header parses");

    let ext = bds
        .complex_extended
        .as_ref()
        .expect("complex_extended populated");
    assert!(ext.boustrophedonic(), "boustrophedonic flag set");
    assert_eq!(ext.order_of_spd(), 3);
    assert_eq!(ext.packing_type_label(), "grid_second_order_SPD3");
}

#[test]
fn decode_spd3_boustrophedonic_matches_eccodes_oracle() {
    let reader = Grib1Reader::from_bytes(SPD3_BOUST_FIXTURE.to_vec()).expect("fixture parses");
    let present: Vec<f64> = reader
        .decode_message_values(0)
        .expect("boustrophedonic order-3 decode succeeds")
        .into_iter()
        .map(|v| v.expect("no missing values"))
        .collect();

    // From `ecmwf_spd3_boust_msg0_expected.json`. The value *multiset* is the
    // same as the non-boustrophedonic SPD-3 fixture (min/max/mean unchanged);
    // only the ordering differs, on the 60 odd rows of the 240×121 grid.
    assert_eq!(present.len(), 29_040);
    let min = present.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = present.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let mean: f64 = present.iter().sum::<f64>() / present.len() as f64;

    let tol = 1e-3;
    assert!((min - 19_074.872_559).abs() < tol, "min was {min}");
    assert!((max - 20_717.558_594).abs() < tol, "max was {max}");
    assert!(
        (mean - 20_216.718_135_691_048).abs() < tol,
        "mean was {mean}"
    );

    // Indices on even rows (0, 480) are untouched; indices 240/241/479 are on
    // row 1 (odd) and must be the reversal of that row. The reversal proof:
    // boust[240] (row 1, col 0) equals non-boust row-1 col-239 = 19085.575928,
    // and boust[479] (row 1, col 239) equals non-boust row-1 col-0 = 19085.677856.
    let samples: &[(usize, f64)] = &[
        (0, 19_080.708_496),   // row 0, col 0   (even row — unchanged)
        (120, 19_080.708_496), // row 0, col 120
        (240, 19_085.575_928), // row 1, col 0   (ODD row — reversed)
        (241, 19_085.448_486), // row 1, col 1
        (479, 19_085.677_856), // row 1, col 239 (== non-boust row-1 col-0)
        (480, 19_091.895_996), // row 2, col 0   (even — unchanged)
        (14_400, 20_563.404_663),
        (29_039, 19_917.864_38),
    ];
    for (i, expected) in samples {
        let got = present[*i];
        assert!(
            (got - expected).abs() < tol,
            "values[{i}] was {got}, expected {expected} (tol {tol})"
        );
    }
}

// ---------------------------------------------------------------------------
// orderOfSPD = 0 (`grid_second_order_no_SPD`) and orderOfSPD = 1
// (`grid_second_order_SPD1`) — hand-built fixtures
// ---------------------------------------------------------------------------
//
// eccodes 2.34 refuses to *encode* these two orders ("Passed array is too
// small"), so there is no re-encoded MARS sample to lean on. Instead each
// fixture is a minimal general-extended second-order BDS hand-assembled to
// the wire layout in `grib1/data.grid_second_order_no_SPD.def` /
// `data.grid_second_order_SPD1.def` (the SPD block is present only
// `if (orderOfSPD)`; everything else is identical across orders), spliced
// onto the real IS/PDS/GDS of `ecmwf_spd3_msg0.grib1` (240×121, no bitmap).
// eccodes *decodes* both — the bug is encode-only — so the committed oracles
// (`hand_second_order_{no_SPD,SPD1}_expected.json`) are independent, and the
// fields are also exactly hand-computable. Construction recorded in NOTICE.md.
//
// no_SPD: two zero-width groups with firstOrderValues 100 and 200 ⇒ the
// first 14 520 points are 100, the rest 200. Order 0 applies no inverse
// differencing, so this directly exercises the shared decoder's order-0 path
// (multi-group iteration + zero-width fill, no SPD seeds, no widthOfSPD octet).

const NO_SPD_FIXTURE: &[u8] = include_bytes!("fixtures/hand_second_order_no_SPD.grib1");

#[test]
fn no_spd_header_reports_order_zero() {
    let reader = Grib1Reader::from_bytes(NO_SPD_FIXTURE.to_vec()).expect("fixture parses");
    let msg = &reader.messages[0];
    let (bds_start, bds_end) = msg.bds_range;
    let bds = parse_bds_header(&NO_SPD_FIXTURE[bds_start..bds_end]).expect("BDS header parses");
    let ext = bds
        .complex_extended
        .as_ref()
        .expect("complex_extended populated");

    assert!(ext.general_extended_2ordr());
    assert!(ext.second_order_of_different_width());
    assert!(!ext.secondary_bitmap_present());
    assert!(!ext.boustrophedonic());
    assert_eq!(ext.order_of_spd(), 0);
    assert_eq!(ext.packing_type_label(), "grid_second_order_no_SPD");
}

#[test]
fn decode_no_spd_matches_eccodes_oracle() {
    let reader = Grib1Reader::from_bytes(NO_SPD_FIXTURE.to_vec()).expect("fixture parses");
    let present: Vec<f64> = reader
        .decode_message_values(0)
        .expect("order-0 (no_SPD) decode succeeds")
        .into_iter()
        .map(|v| v.expect("no missing values"))
        .collect();

    // From `hand_second_order_no_SPD_expected.json`: step field 100 → 200.
    assert_eq!(present.len(), 29_040);
    let tol = 1e-9;
    for (i, &v) in present.iter().enumerate() {
        let expected = if i < 14_520 { 100.0 } else { 200.0 };
        assert!(
            (v - expected).abs() < tol,
            "values[{i}] was {v}, expected {expected}"
        );
    }
}

// SPD1: seed = 0, bias = 0, one zero-width group with firstOrderValues = 1.
// Order-1 reconstruction is the cumulative sum y += X[i] + bias starting at
// the seed, so 0 followed by 29 039 ones reconstructs the ramp 0,1,2,…,29039.
// This exercises the order-1 SPD inverse end-to-end (seed + bias read, then
// `apply_spd_inverse(.., 1, ..)`), the path the README claimed but never
// covered.

const SPD1_FIXTURE: &[u8] = include_bytes!("fixtures/hand_second_order_SPD1.grib1");

#[test]
fn spd1_header_reports_order_one() {
    let reader = Grib1Reader::from_bytes(SPD1_FIXTURE.to_vec()).expect("fixture parses");
    let msg = &reader.messages[0];
    let (bds_start, bds_end) = msg.bds_range;
    let bds = parse_bds_header(&SPD1_FIXTURE[bds_start..bds_end]).expect("BDS header parses");
    let ext = bds
        .complex_extended
        .as_ref()
        .expect("complex_extended populated");

    assert!(ext.general_extended_2ordr());
    assert_eq!(ext.order_of_spd(), 1);
    assert_eq!(ext.packing_type_label(), "grid_second_order_SPD1");
}

#[test]
fn decode_spd1_matches_eccodes_oracle() {
    let reader = Grib1Reader::from_bytes(SPD1_FIXTURE.to_vec()).expect("fixture parses");
    let present: Vec<f64> = reader
        .decode_message_values(0)
        .expect("order-1 (SPD1) decode succeeds")
        .into_iter()
        .map(|v| v.expect("no missing values"))
        .collect();

    // From `hand_second_order_SPD1_expected.json`: the ramp value[i] = i.
    assert_eq!(present.len(), 29_040);
    let tol = 1e-9;
    for (i, &v) in present.iter().enumerate() {
        assert!(
            (v - i as f64).abs() < tol,
            "values[{i}] was {v}, expected {i}"
        );
    }
}
