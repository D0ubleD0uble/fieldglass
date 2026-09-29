//! The Bit Map Section through the real reader.
//!
//! No committed simple-packed fixture carries a BMS, so these tests splice one
//! into the CMC polar-stereographic message (12 825 points, simple packing):
//! set the PDS "BMS present" flag, insert the section after the GDS and patch
//! the total length. The BDS is left alone, so it still holds a value for
//! every point; a bitmap that clears some points makes the decoder take the
//! first `present` of them, in order.

use fieldglass_core::FieldglassError;
use fieldglass_grib1::Grib1Reader;

const FIXTURE: &[u8] = include_bytes!("fixtures/cmc_wind_300_2010052400_p012.grib");

/// Grid points in the fixture (135 × 95).
const POINTS: usize = 12_825;

/// Byte offset of the BDS in the fixture: an 8-byte IS, a 40-byte PDS and a
/// 32-byte GDS.
const BDS_OFFSET: usize = 80;

/// PDS octet 8, the section flags; `0x40` means a BMS follows.
const PDS_FLAGS: usize = 8 + 7;

/// The fixture with a BMS whose body is `body` and whose octet 4 says the last
/// `unused` bits of it are padding.
fn with_bms(body: &[u8], unused: u8) -> Vec<u8> {
    let bms_len = 6 + body.len();
    let mut msg = FIXTURE[..BDS_OFFSET].to_vec();
    msg.extend_from_slice(&u32::try_from(bms_len).unwrap().to_be_bytes()[1..]);
    msg.extend_from_slice(&[unused, 0, 0]);
    msg.extend_from_slice(body);
    msg.extend_from_slice(&FIXTURE[BDS_OFFSET..]);
    msg[PDS_FLAGS] |= 0x40;
    let total = u32::try_from(msg.len()).unwrap().to_be_bytes();
    msg[4..7].copy_from_slice(&total[1..]);
    msg
}

/// MSB-first packing of `flags`, padded with zero bits to whole octets.
fn pack(flags: &[bool]) -> Vec<u8> {
    let mut out = vec![0u8; flags.len().div_ceil(8)];
    for (i, _) in flags.iter().enumerate().filter(|(_, f)| **f) {
        out[i / 8] |= 0x80 >> (i % 8);
    }
    out
}

fn decode(bytes: Vec<u8>) -> Result<Vec<Option<f64>>, FieldglassError> {
    let reader = Grib1Reader::from_bytes(bytes).expect("message parses");
    reader.decode_message_values(0)
}

#[test]
fn an_all_present_bitmap_decodes_as_no_bitmap() {
    let plain = decode(FIXTURE.to_vec()).expect("plain decode");
    let flags = vec![true; POINTS];
    // 12 825 bits in 1 604 octets: the last 7 bits are padding.
    let masked = decode(with_bms(&pack(&flags), 7)).expect("bitmap decode");
    assert_eq!(masked, plain);
}

#[test]
fn a_cleared_bit_is_a_missing_point_and_the_rest_keep_their_order() {
    let plain: Vec<f64> = decode(FIXTURE.to_vec())
        .expect("plain decode")
        .into_iter()
        .map(|v| v.expect("no bitmap, every point present"))
        .collect();
    // Clear every third point, and bit 0 and the last bit: the two ends are
    // where an off-by-one in the MSB-first indexing would show.
    let flags: Vec<bool> = (0..POINTS)
        .map(|i| i % 3 != 1 && i != 0 && i != POINTS - 1)
        .collect();
    let masked = decode(with_bms(&pack(&flags), 7)).expect("bitmap decode");
    assert_eq!(masked.len(), POINTS);
    let mut next = plain.iter();
    for (i, (&present, value)) in flags.iter().zip(&masked).enumerate() {
        if present {
            assert_eq!(*value, next.next().copied(), "point {i}");
        } else {
            assert_eq!(*value, None, "point {i}");
        }
    }
}

/// A bitmap has one bit per grid point (WMO FM 92 GRIB edition 1, Section 3,
/// octet 7 onwards: "contiguous bits with a bit to data point
/// correspondence"). One that declares fewer bits than the grid has points
/// cannot say which points are present, so it is an error. The reader used
/// to take the bits there were and return a field 9 points short; eccodes
/// 2.34.1 also returns 12 816 values for this message, and its geoiterator
/// then refuses it ("numberOfPoints != size(values)").
#[test]
fn a_bitmap_shorter_than_the_grid_is_an_error() {
    // 12 816 bits, no padding: 9 bits short.
    let short = with_bms(&pack(&vec![true; POINTS - 9]), 0);
    let Err(err) = decode(short) else {
        panic!("short bitmap must error");
    };
    assert!(
        matches!(&err, FieldglassError::Parse(m) if m.contains("12816") && m.contains("12825")),
        "names both counts, got {err:?}"
    );
}

/// The padding count is part of the length: a body long enough for the grid
/// whose octet 4 marks too many trailing bits as unused is short as well.
#[test]
fn padding_that_eats_into_the_grid_is_an_error() {
    let body = pack(&vec![true; POINTS]);
    // 1 604 octets hold 12 832 bits; 8 unused leaves 12 824, one short.
    let Err(err) = decode(with_bms(&body, 8)) else {
        panic!("one bit short must error");
    };
    assert!(matches!(err, FieldglassError::Parse(_)), "got {err:?}");
}

/// Bits beyond the grid are ignored: GRIB1 sections are padded to an even
/// length, and not every encoder counts that padding in octet 4.
#[test]
fn bits_past_the_grid_are_ignored() {
    let plain = decode(FIXTURE.to_vec()).expect("plain decode");
    let mut body = pack(&vec![true; POINTS]);
    body.extend_from_slice(&[0xFF, 0x00]);
    let masked = decode(with_bms(&body, 0)).expect("bitmap decode");
    assert_eq!(masked, plain);
}
