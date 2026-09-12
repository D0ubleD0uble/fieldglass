//! The same matrix-of-values field, encoded in both GRIB editions, decodes
//! identically through the two independent readers.
//!
//! A cross-edition check on the shared matrix reshape, and the closest thing to
//! an oracle for a variant eccodes crashes on.
//!
//! It lived in `fieldglass-napi` until #726, which was the wrong home: it tests
//! two decoders against each other and has nothing to do with a host. A crate
//! that reaches both decoders is where it belongs, and a host that no longer
//! names either decoder in its manifest could not keep it anyway.

#[test]
fn grib1_and_grib2_matrix_decodes_agree() {
    // 16×31, NR=1/NC=2, all present, value k % 256.
    let g1 = fieldglass_grib1::Grib1Reader::from_bytes(
        include_bytes!("../../fieldglass-grib1/tests/fixtures/hand_matrix_of_values.grib1")
            .to_vec(),
    )
    .expect("grib1 parse");
    let g2 = fieldglass_grib2::Grib2Reader::from_bytes(
        include_bytes!("../../fieldglass-grib2/tests/fixtures/matrix_reshape_16x31.grib2").to_vec(),
    )
    .expect("grib2 parse");
    let f1 = g1.decode_matrix_message(0).expect("grib1 matrix decode");
    let f2 = g2.decode_matrix_message(0).expect("grib2 matrix decode");
    assert_eq!((f1.ni, f1.nj, f1.nr, f1.nc), (f2.ni, f2.nj, f2.nr, f2.nc));
    assert_eq!(
        f1.values, f2.values,
        "GRIB1 and GRIB2 matrix decodes of the same field agree"
    );
}
