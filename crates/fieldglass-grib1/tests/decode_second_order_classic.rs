//! End-to-end decode of the three "classic" (pre-ECMWF-extended) GRIB1
//! second-order packings: `grid_second_order_row_by_row`,
//! `grid_second_order_constant_width`, and `grid_second_order_general_grib1`.
//!
//! eccodes 2.34 refuses to *encode* these layouts ("not implemented"), so —
//! as with the no_SPD/SPD1 fixtures — each is a minimal BDS hand-assembled to
//! the wire layout documented in eccodes' `grib1/data.grid_second_order_*.def`
//! and the matching `DataG1SecondOrder*Packing::unpack` reference source
//! (WMO No. 306 Vol I.2 FM 92 GRIB1; ECMWF
//! <https://codes.ecmwf.int/grib/format/grib1/packing/3/>), spliced onto the
//! real IS/PDS/GDS of `ecmwf_spd3_msg0.grib1` (240×121, no bitmap). eccodes
//! *decodes* all three (the refusal is encode-only), so the committed
//! `*_expected.json` oracles are independent, and the synthetic fields are
//! exactly hand-computable. Construction recorded in `tests/fixtures/NOTICE.md`.

use fieldglass_grib1::{Grib1Reader, parse_bds_header};

const NI: usize = 240; // columns / points per row
const NJ: usize = 121; // rows

// ---------------------------------------------------------------------------
// grid_second_order_row_by_row
// ---------------------------------------------------------------------------
//
// One group per row (implied secondary bitmap), per-row group widths. The
// fixture makes even rows zero-width (every point = the row's first-order
// value r*10) and odd rows width-4 (point c = r*10 + c%16), exercising both
// the run-length (width 0) and residual (width > 0) paths and variable widths.

const ROW_BY_ROW_FIXTURE: &[u8] = include_bytes!("fixtures/hand_second_order_row_by_row.grib1");

fn expected_row_by_row(r: usize, c: usize) -> f64 {
    (r * 10 + if r.is_multiple_of(2) { 0 } else { c % 16 }) as f64
}

#[test]
fn row_by_row_header_reports_variant() {
    let reader = Grib1Reader::from_bytes(ROW_BY_ROW_FIXTURE.to_vec()).expect("fixture parses");
    let msg = &reader.messages[0];
    let (s, e) = msg.bds_range;
    let bds = parse_bds_header(&ROW_BY_ROW_FIXTURE[s..e]).expect("BDS header parses");
    let ext = bds
        .complex_extended
        .as_ref()
        .expect("complex_extended populated");

    assert!(!ext.general_extended_2ordr());
    assert!(ext.second_order_of_different_width());
    assert!(!ext.secondary_bitmap_present());
    assert!(!ext.matrix_of_values());
    assert_eq!(ext.packing_type_label(), "grid_second_order_row_by_row");
}

#[test]
fn decode_row_by_row_matches_eccodes_oracle() {
    let reader = Grib1Reader::from_bytes(ROW_BY_ROW_FIXTURE.to_vec()).expect("fixture parses");
    let present: Vec<f64> = reader
        .decode_message_values(0)
        .expect("row_by_row decode succeeds")
        .into_iter()
        .map(|v| v.expect("no missing values"))
        .collect();

    assert_eq!(present.len(), NI * NJ);
    let tol = 1e-9;
    for r in 0..NJ {
        for c in 0..NI {
            let got = present[r * NI + c];
            let want = expected_row_by_row(r, c);
            assert!(
                (got - want).abs() < tol,
                "row {r} col {c}: got {got}, want {want}"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// grid_second_order_constant_width
// ---------------------------------------------------------------------------
//
// Explicit secondary bitmap (a 1 bit marks where each group begins) plus a
// single shared group width. The fixture lays out 121 groups of 240 points,
// firstOrderValues g*100, one shared width of 4, and residual n%16, so
// value[n] = (n / 240) * 100 + n % 16 — exercising the bitmap group walk and
// the single-width residual path.

const CONSTANT_WIDTH_FIXTURE: &[u8] =
    include_bytes!("fixtures/hand_second_order_constant_width.grib1");

#[test]
fn constant_width_header_reports_variant() {
    let reader = Grib1Reader::from_bytes(CONSTANT_WIDTH_FIXTURE.to_vec()).expect("fixture parses");
    let msg = &reader.messages[0];
    let (s, e) = msg.bds_range;
    let bds = parse_bds_header(&CONSTANT_WIDTH_FIXTURE[s..e]).expect("BDS header parses");
    let ext = bds
        .complex_extended
        .as_ref()
        .expect("complex_extended populated");

    assert!(!ext.general_extended_2ordr());
    assert!(!ext.second_order_of_different_width());
    assert!(ext.secondary_bitmap_present());
    assert!(!ext.matrix_of_values());
    assert_eq!(ext.packing_type_label(), "grid_second_order_constant_width");
}

#[test]
fn decode_constant_width_matches_eccodes_oracle() {
    let reader = Grib1Reader::from_bytes(CONSTANT_WIDTH_FIXTURE.to_vec()).expect("fixture parses");
    let present: Vec<f64> = reader
        .decode_message_values(0)
        .expect("constant_width decode succeeds")
        .into_iter()
        .map(|v| v.expect("no missing values"))
        .collect();

    assert_eq!(present.len(), NI * NJ);
    let tol = 1e-9;
    for (n, &got) in present.iter().enumerate() {
        let want = ((n / NI) * 100 + n % 16) as f64;
        assert!((got - want).abs() < tol, "n {n}: got {got}, want {want}");
    }
}

// ---------------------------------------------------------------------------
// grid_second_order_general_grib1
// ---------------------------------------------------------------------------
//
// The most general classic layout: variable-length groups delimited by the
// secondary bitmap's 1 bits, plus per-group widths. The fixture uses 120
// groups alternating length 200 and 284 (sum 29040), even groups zero-width
// (constant = firstOrderValues g*50, the run-length case) and odd groups
// width-5 (residual = within-group offset % 32) — exercising variable lengths,
// variable widths, and the run-length path together.

const GENERAL_FIXTURE: &[u8] = include_bytes!("fixtures/hand_second_order_general_grib1.grib1");

/// Mirror of the fixture construction: groups g of length 200 (even) / 284
/// (odd), base g*50, odd groups add (offset within group) % 32.
fn expected_general(n: usize) -> f64 {
    let mut start = 0usize;
    let mut g = 0usize;
    loop {
        let len = if g.is_multiple_of(2) { 200 } else { 284 };
        if n < start + len {
            let off = n - start;
            let base = g * 50;
            return (base + if g.is_multiple_of(2) { 0 } else { off % 32 }) as f64;
        }
        start += len;
        g += 1;
    }
}

#[test]
fn general_grib1_header_reports_variant() {
    let reader = Grib1Reader::from_bytes(GENERAL_FIXTURE.to_vec()).expect("fixture parses");
    let msg = &reader.messages[0];
    let (s, e) = msg.bds_range;
    let bds = parse_bds_header(&GENERAL_FIXTURE[s..e]).expect("BDS header parses");
    let ext = bds
        .complex_extended
        .as_ref()
        .expect("complex_extended populated");

    assert!(!ext.general_extended_2ordr());
    assert!(ext.second_order_of_different_width());
    assert!(ext.secondary_bitmap_present());
    assert!(!ext.matrix_of_values());
    assert_eq!(ext.packing_type_label(), "grid_second_order_general_grib1");
}

#[test]
fn decode_general_grib1_matches_eccodes_oracle() {
    let reader = Grib1Reader::from_bytes(GENERAL_FIXTURE.to_vec()).expect("fixture parses");
    let present: Vec<f64> = reader
        .decode_message_values(0)
        .expect("general_grib1 decode succeeds")
        .into_iter()
        .map(|v| v.expect("no missing values"))
        .collect();

    assert_eq!(present.len(), NI * NJ);
    let tol = 1e-9;
    for (n, &got) in present.iter().enumerate() {
        let want = expected_general(n);
        assert!((got - want).abs() < tol, "n {n}: got {got}, want {want}");
    }
}
