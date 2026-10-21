//! `Grib1Reader::pds_extension` returns the PDS octets past 28 as written
//! (#994). The expected octets are `grib_dump -O` (eccodes 2.34.1) of each
//! fixture's section 1: octets 29-40 reserved, then ECMWF local definition 1
//! from octet 41 (local definition number, MARS class, type, stream,
//! experiment `0001`, perturbation number, ensemble size, one octet of
//! padding).

use fieldglass_grib1::Grib1Reader;

const ECMWF_LOCAL_1: &[u8] = include_bytes!("fixtures/reduced_gg_n32.grib1");
const CMC: &[u8] = include_bytes!("fixtures/cmc_wind_300_2010052400_p012.grib");
const BARE: &[u8] = include_bytes!("fixtures/ecmwf_lfpw_msg0.grib1");

#[test]
fn an_ecmwf_local_definition_is_returned_as_written() {
    let reader = Grib1Reader::from_bytes(ECMWF_LOCAL_1.to_vec()).expect("fixture parses");
    let ext = reader.pds_extension(0).expect("reads");
    let mut expected = vec![0u8; 12];
    // localDefinitionNumber 1, marsClass 1 (od), marsType 2 (an),
    // marsStream 1025 (oper), experimentVersionNumber "0001",
    // perturbationNumber 0, numberOfForecastsInEnsemble 0, padding.
    expected.extend_from_slice(&[1, 1, 2, 0x04, 0x01, b'0', b'0', b'0', b'1', 0, 0, 0]);
    assert_eq!(&*ext, &expected[..]);
}

/// The CMC fixture writes a 40-octet PDS: twelve reserved octets, all zero.
#[test]
fn reserved_octets_are_returned_too() {
    let reader = Grib1Reader::from_bytes(CMC.to_vec()).expect("fixture parses");
    assert_eq!(&*reader.pds_extension(0).expect("reads"), &[0u8; 12][..]);
}

#[test]
fn a_28_octet_pds_has_no_extension() {
    let reader = Grib1Reader::from_bytes(BARE.to_vec()).expect("fixture parses");
    assert!(reader.pds_extension(0).expect("reads").is_empty());
}

#[test]
fn an_out_of_range_index_is_an_error() {
    let reader = Grib1Reader::from_bytes(BARE.to_vec()).expect("fixture parses");
    assert!(reader.pds_extension(1).is_err());
}
