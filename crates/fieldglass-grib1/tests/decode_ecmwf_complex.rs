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
// for provenance. (eccodes cannot re-encode the no_SPD / SPD1 siblings — it
// errors with "array too small" — so those two general-extended orders, while
// already handled by the same decoder, await a real-world fixture.)

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
