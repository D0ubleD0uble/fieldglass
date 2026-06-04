//! `Grib1Reader::packing_label` reports the eccodes-style `packingType` for a
//! message's BDS without decoding it. Each fixture here is already pinned to an
//! eccodes oracle by a sibling decode test (see `NOTICE.md` for provenance);
//! this asserts the *label* the metadata layer surfaces matches the variant
//! eccodes' `packingType` concept would print for the same message.

use fieldglass_grib1::Grib1Reader;

/// `(fixture bytes, expected packingType label)`.
const CASES: &[(&[u8], &str)] = &[
    (
        include_bytes!("fixtures/cmc_wind_300_2010052400_p012.grib"),
        "grid_simple",
    ),
    (
        include_bytes!("fixtures/ieee32_cmc_wind.grib1"),
        "grid_ieee",
    ),
    (
        include_bytes!("fixtures/ieee64_cmc_wind.grib1"),
        "grid_ieee",
    ),
    (
        include_bytes!("fixtures/matrix_simple_cmc_wind.grib1"),
        "grid_simple_matrix",
    ),
    (
        include_bytes!("fixtures/hand_matrix_of_values.grib1"),
        "grid_simple_matrix",
    ),
    (
        include_bytes!("fixtures/ecmwf_lfpw_msg0.grib1"),
        "grid_second_order",
    ),
    (
        include_bytes!("fixtures/ecmwf_spd3_msg0.grib1"),
        "grid_second_order_SPD3",
    ),
    (
        include_bytes!("fixtures/hand_second_order_no_SPD.grib1"),
        "grid_second_order_no_SPD",
    ),
    (
        include_bytes!("fixtures/hand_second_order_SPD1.grib1"),
        "grid_second_order_SPD1",
    ),
    (
        include_bytes!("fixtures/hand_second_order_row_by_row.grib1"),
        "grid_second_order_row_by_row",
    ),
    (
        include_bytes!("fixtures/hand_second_order_constant_width.grib1"),
        "grid_second_order_constant_width",
    ),
    (
        include_bytes!("fixtures/hand_second_order_general_grib1.grib1"),
        "grid_second_order_general_grib1",
    ),
];

#[test]
fn packing_label_matches_eccodes_packing_type_for_each_variant() {
    for (bytes, expected) in CASES {
        let reader = Grib1Reader::from_bytes(bytes.to_vec()).expect("fixture parses");
        let label = reader
            .packing_label(0)
            .expect("message 0 has a parseable BDS header");
        assert_eq!(
            label, *expected,
            "packing label mismatch for a {expected} fixture"
        );
    }
}

#[test]
fn packing_label_is_none_for_an_out_of_range_index() {
    let reader = Grib1Reader::from_bytes(
        include_bytes!("fixtures/cmc_wind_300_2010052400_p012.grib").to_vec(),
    )
    .expect("fixture parses");
    assert_eq!(reader.packing_label(99_999), None);
}
