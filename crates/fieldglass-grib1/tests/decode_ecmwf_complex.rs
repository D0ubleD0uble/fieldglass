//! Variant-detection tests for the ECMWF complex / second-order packing.
//!
//! Fixture: the first message extracted from a 64-message ECMWF GRIB1 file
//! (LFPW MARS-derived analysis on a 240 × 121 lat-long grid, 2006-12-10
//! 18Z + 24h, geopotential at 50 hPa). Provided by the user as
//! representative of the file class that today's simple-packing decoder
//! refuses with `unsupported section`.
//!
//! Until the second-order packing decoder is implemented, this test pins:
//!
//! 1. The whole file parses (BDS header recognises `complex_extended`).
//! 2. `decode_message_values` surfaces the eccodes-style packingType label
//!    in its error, so users can pivot directly into the eccodes docs.
//!
//! Once the decoder lands, this test will be expanded to assert decoded
//! values match the eccodes-derived oracle in
//! `tests/fixtures/ecmwf_lfpw_msg0_expected.json`.

use fieldglass_core::FieldglassError;
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
fn decode_surfaces_variant_specific_error() {
    let reader = Grib1Reader::from_bytes(FIXTURE.to_vec()).expect("fixture parses");
    let err = reader
        .decode_message_values(0)
        .expect_err("complex packing decode is not yet implemented");

    match err {
        FieldglassError::UnsupportedSection(msg) => {
            assert!(
                msg.contains("grid_second_order"),
                "error should name the eccodes packingType, got {msg:?}"
            );
            assert!(
                !msg.contains("grid_second_order_SPD"),
                "should be plain grid_second_order (orderOfSPD = 2 maps to eccodes' canonical name), got {msg:?}"
            );
        }
        other => panic!("expected UnsupportedSection, got {other:?}"),
    }
}
