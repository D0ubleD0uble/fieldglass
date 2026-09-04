//! `Grib1Reader::message_kind` routes each committed fixture to the decode
//! entry point that actually accepts it.
//!
//! The pair that matters is `matrix_simple_cmc_wind.grib1` and
//! `hand_matrix_of_values.grib1`: both carry `grid_simple_matrix` packing, and
//! only the second is a matrix. A consumer routing on the packing label sends
//! the first to `decode_matrix_message`, which rejects it — which is the whole
//! reason this predicate reads the matrix sub-header rather than the label.

use fieldglass_grib1::{Grib1MessageKind, Grib1Reader};

fn reader(bytes: &[u8]) -> Grib1Reader {
    Grib1Reader::from_bytes(bytes.to_vec()).expect("fixture parses")
}

#[test]
fn a_scalar_grid_message_routes_to_the_grid_decoder() {
    let r = reader(include_bytes!("fixtures/cmc_wind_300_2010052400_p012.grib"));
    assert_eq!(r.message_kind(0), Grib1MessageKind::Grid);
    assert!(r.decode_message_values(0).is_ok());
}

#[test]
fn a_spherical_harmonic_message_routes_to_the_spectral_decoder() {
    let r = reader(include_bytes!("fixtures/spectral_simple_t63.grib1"));
    assert_eq!(r.message_kind(0), Grib1MessageKind::Spectral);
    assert!(r.decode_spectral_message(0).is_ok());
    // And the grid path is the one that refuses it.
    assert!(r.decode_message_values(0).is_err());
}

#[test]
fn a_matrix_of_values_message_routes_to_the_matrix_decoder() {
    let r = reader(include_bytes!("fixtures/hand_matrix_of_values.grib1"));
    assert_eq!(r.message_kind(0), Grib1MessageKind::Matrix);
    assert!(r.decode_matrix_message(0).is_ok());
    assert!(r.decode_message_values(0).is_err());
}

/// `grid_simple_matrix` packing with `matrixOfValues = 0`: the label says
/// matrix, the sub-header says scalar, and the scalar path is the one that
/// decodes it.
#[test]
fn matrix_packing_without_the_matrix_flag_is_still_a_grid() {
    let r = reader(include_bytes!("fixtures/matrix_simple_cmc_wind.grib1"));
    assert_eq!(r.packing_label(0), Some("grid_simple_matrix"));
    assert_eq!(r.message_kind(0), Grib1MessageKind::Grid);
    assert!(r.decode_message_values(0).is_ok());
    assert!(r.decode_matrix_message(0).is_err());
}

#[test]
fn an_out_of_range_index_has_no_entry_point() {
    let r = reader(include_bytes!("fixtures/cmc_wind_300_2010052400_p012.grib"));
    assert_eq!(r.message_kind(1), Grib1MessageKind::Unsupported);
    assert_eq!(r.message_kind(usize::MAX), Grib1MessageKind::Unsupported);
}

/// A grid type this parser does not model has no decode entry point either:
/// `decode_message_values` cannot report its dimensions and it is not spectral.
/// Built by rewriting the polar fixture's GDS data-representation type (GDS
/// octet 6) to an unassigned number, so everything else about the message stays
/// a real one.
#[test]
fn an_unmodelled_grid_type_has_no_entry_point() {
    let mut bytes = include_bytes!("fixtures/cmc_wind_300_2010052400_p012.grib").to_vec();
    // IS is 8 octets; the PDS states its own 3-octet length, and the GDS
    // follows it.
    let pds_len = u32::from_be_bytes([0, bytes[8], bytes[9], bytes[10]]) as usize;
    let gds_offset = 8 + pds_len;
    assert_eq!(bytes[gds_offset + 5], 5, "fixture is polar stereographic");
    bytes[gds_offset + 5] = 13; // unassigned in ON388 Table 6
    let r = reader(&bytes);
    assert_eq!(r.message_kind(0), Grib1MessageKind::Unsupported);
    assert!(r.decode_message_values(0).is_err());
}

/// No GDS and no predefined grid to fill one in: the message has metadata but
/// nothing to decode its values onto. Hand-assembled to the section structure
/// only, as in `predefined_grid.rs`.
#[test]
fn a_message_with_no_resolvable_grid_has_no_entry_point() {
    const BDS_LEN: usize = 12;
    let total_len = 8 + 28 + BDS_LEN + 4;
    let mut msg = Vec::with_capacity(total_len);
    msg.extend_from_slice(b"GRIB");
    msg.extend_from_slice(&[
        (total_len >> 16) as u8,
        (total_len >> 8) as u8,
        total_len as u8,
    ]);
    msg.push(1);
    let mut pds = [0u8; 28];
    pds[0..3].copy_from_slice(&[0, 0, 28]);
    pds[6] = 255; // grid number 255 = no predefined grid
    pds[7] = 0x00; // no GDS, no BMS
    msg.extend_from_slice(&pds);
    msg.extend_from_slice(&[0u8; BDS_LEN]);
    msg.extend_from_slice(b"7777");

    let r = reader(&msg);
    assert!(r.messages[0].gds.is_none());
    assert_eq!(r.message_kind(0), Grib1MessageKind::Unsupported);
}
