//! Smoke tests for malformed / out-of-spec inputs. The reader must surface
//! a structured `FieldglassError::Parse` (or return zero messages) for
//! these — never panic and never silently misinterpret garbage as valid
//! data.
//!
//! These cover the failure modes most likely to arrive over the VS Code
//! `workspace.fs.readFile` API in the wild: truncated downloads, files of
//! the wrong format, files with `GRIB` substrings inside binary payloads,
//! empty buffers, and length-field mismatches.

use fieldglass_core::FieldglassError;
use fieldglass_grib1::Grib1Reader;
use fieldglass_grib1::bms::parse_bitmap;

const FIXTURE: &[u8] = include_bytes!("fixtures/cmc_wind_300_2010052400_p012.grib");

#[test]
fn empty_buffer_yields_zero_messages() {
    let reader = Grib1Reader::from_bytes(Vec::new()).expect("empty buffer parses");
    assert_eq!(reader.message_count(), 0);
}

#[test]
fn buffer_too_short_for_indicator_yields_zero_messages() {
    // Anything under 8 bytes can't be a complete IS — the scanner should
    // return cleanly with no messages rather than out-of-bounds-indexing.
    let reader = Grib1Reader::from_bytes(b"GR".to_vec()).expect("short buffer parses");
    assert_eq!(reader.message_count(), 0);

    let reader = Grib1Reader::from_bytes(b"GRIB".to_vec()).expect("4-byte buffer parses");
    assert_eq!(reader.message_count(), 0);
}

#[test]
fn buffer_with_no_grib_marker_yields_zero_messages() {
    let buf = b"this is just some random bytes, not GRIB at all".to_vec();
    let reader = Grib1Reader::from_bytes(buf).expect("non-grib bytes parse");
    assert_eq!(reader.message_count(), 0);
}

#[test]
fn grib_substring_inside_payload_does_not_misparse() {
    // A buffer that contains the literal "GRIB" substring but not as a real
    // message header. The scanner must skip past it without crashing or
    // claiming a phantom message.
    let mut buf = Vec::new();
    buf.extend_from_slice(b"some prefix GRIB but not a real message header padding");
    let reader = Grib1Reader::from_bytes(buf).expect("buffer with GRIB substring parses");
    assert_eq!(reader.message_count(), 0);
}

#[test]
fn truncated_message_returns_parse_error() {
    // Take a real message and lop off the trailing half so the IS-declared
    // total length runs past the end of the buffer.
    let mut buf = FIXTURE.to_vec();
    buf.truncate(FIXTURE.len() / 2);

    let Err(err) = Grib1Reader::from_bytes(buf) else {
        panic!("truncated buffer must error");
    };
    assert!(
        matches!(err, FieldglassError::Parse(_)),
        "expected FieldglassError::Parse, got {err:?}"
    );
}

#[test]
fn missing_7777_trailer_returns_parse_error() {
    // Replace the last 4 bytes of a real message with garbage so the End
    // Section validator trips.
    let mut buf = FIXTURE.to_vec();
    let len = buf.len();
    buf[len - 4..].copy_from_slice(b"AAAA");

    let Err(err) = Grib1Reader::from_bytes(buf) else {
        panic!("trailer-corrupt buffer must error");
    };
    let FieldglassError::Parse(msg) = err else {
        panic!("expected Parse error");
    };
    assert!(
        msg.contains("7777"),
        "error should mention the 7777 marker, got: {msg}"
    );
}

#[test]
fn wrong_grib_edition_byte_skips_message() {
    // The fixture is GRIB edition 1. Patch byte 7 (the edition octet) to a
    // value the GRIB1 reader doesn't handle. The scanner is supposed to
    // skip non-edition-1 messages forward by one byte rather than panic.
    let mut buf = FIXTURE.to_vec();
    buf[7] = 2; // pretend it's GRIB2
    let reader = Grib1Reader::from_bytes(buf).expect("non-edition-1 buffer parses cleanly");
    assert_eq!(
        reader.message_count(),
        0,
        "GRIB1 reader should ignore edition-2 messages"
    );
}

/// BMS regression: an empty bitmap body with `unused_trailing > 0` previously
/// underflowed `len*8 - unused_trailing`. Must now surface as a parse error.
#[test]
fn bms_empty_body_with_unused_trailing_returns_parse_error() {
    // section_len = 6 → bitmap body is empty; unused_trailing = 5 underflows
    // the naive total_bits computation.
    let bms = vec![0u8, 0, 6, 5, 0, 0];
    let err = parse_bitmap(&bms, 0).expect_err("empty body + nonzero trailing must error");
    assert!(
        matches!(err, FieldglassError::Parse(_)),
        "expected Parse error, got {err:?}"
    );
}

/// BMS regression: `unused_trailing` larger than 8 × body bytes also underflows.
#[test]
fn bms_unused_trailing_exceeds_body_returns_parse_error() {
    // Body = 1 byte = 8 bits; trailing = 200 makes the subtraction underflow.
    let bms = vec![0u8, 0, 7, 200, 0, 0, 0xFF];
    let err = parse_bitmap(&bms, 8).expect_err("oversize trailing must error");
    assert!(matches!(err, FieldglassError::Parse(_)));
}

/// Out-of-spec GDS regression: a header that declares `ni = nj = 65535`
/// (~4.3B points) must be rejected by `decode_message_values` before the
/// allocation runs.
#[test]
fn oversized_grid_dimensions_rejected_by_cap() {
    let mut buf = FIXTURE.to_vec();
    // GDS starts at IS (8 bytes) + PDS section_len (3-byte big-endian at PDS
    // offset 0). ni and nj are u16-BE at GDS offsets 6 and 8 (lat/lon and
    // gaussian grids share this layout — see gds::parse_latlon).
    let pds_len = u32::from_be_bytes([0, buf[8], buf[9], buf[10]]) as usize;
    let gds_off = 8 + pds_len;
    buf[gds_off + 6..gds_off + 8].copy_from_slice(&0xFFFFu16.to_be_bytes());
    buf[gds_off + 8..gds_off + 10].copy_from_slice(&0xFFFFu16.to_be_bytes());

    let reader = Grib1Reader::from_bytes(buf).expect("scan still succeeds");
    let err = reader
        .decode_message_values(0)
        .expect_err("oversized dimensions must error");
    let FieldglassError::Parse(msg) = err else {
        panic!("expected Parse error, got {err:?}");
    };
    assert!(
        msg.contains("exceeds cap"),
        "error should mention the grid-points cap, got: {msg}"
    );
}

/// Scanner regression (found by the `decode` fuzz target): an Indicator
/// Section that declares a total length shorter than its own 8 bytes
/// previously underflowed `msg_end - 4` in the trailing-`7777` check.
#[test]
fn implausible_total_length_returns_parse_error() {
    // total_length = 0 (octets 5–7), edition 1 (octet 8).
    let buf = b"GRIB\x00\x00\x00\x01".to_vec();
    let Err(err) = Grib1Reader::from_bytes(buf) else {
        panic!("implausible total length must error, not panic");
    };
    assert!(
        matches!(err, FieldglassError::Parse(_)),
        "expected Parse error, got {err:?}"
    );
}

/// Build a minimal 38-byte GRIB1 message whose PDS sets `flag` (the GDS/BMS
/// present bits) but whose declared length leaves only 2 bytes after the PDS
/// — fewer than the 3-byte length field a following GDS or BMS opens with.
fn message_with_truncated_trailing_section(flag: u8) -> Vec<u8> {
    let total_length = 38u8;
    let mut buf = vec![0u8; total_length as usize];
    buf[0..4].copy_from_slice(b"GRIB");
    buf[4..7].copy_from_slice(&[0, 0, total_length]); // IS total length
    buf[7] = 1; // GRIB edition 1
    buf[8..11].copy_from_slice(&[0, 0, 28]); // PDS section_len = WMO minimum 28
    buf[15] = flag; // PDS flag octet (GDS/BMS present bits)
    buf[34..38].copy_from_slice(b"7777"); // End Section at msg_end - 4
    buf
}

/// Scanner regression (found by the `decode` fuzz target): when the GDS-present
/// flag is set but the message ends mid-length-field, reading the 3-byte GDS
/// length previously indexed out of bounds.
#[test]
fn truncated_gds_length_field_returns_parse_error() {
    let buf = message_with_truncated_trailing_section(0x80); // has_gds
    let Err(err) = Grib1Reader::from_bytes(buf) else {
        panic!("truncated GDS length field must error, not panic");
    };
    assert!(
        matches!(err, FieldglassError::Parse(_)),
        "expected Parse error, got {err:?}"
    );
}

/// Same regression for the Bit Map Section, whose length field is read the
/// same way immediately after the GDS.
#[test]
fn truncated_bms_length_field_returns_parse_error() {
    let buf = message_with_truncated_trailing_section(0x40); // has_bms
    let Err(err) = Grib1Reader::from_bytes(buf) else {
        panic!("truncated BMS length field must error, not panic");
    };
    assert!(
        matches!(err, FieldglassError::Parse(_)),
        "expected Parse error, got {err:?}"
    );
}

#[test]
fn decode_grid_for_out_of_range_index_returns_error() {
    let reader = Grib1Reader::from_bytes(FIXTURE.to_vec()).expect("fixture parses");
    let err = reader
        .decode_message_values(99)
        .expect_err("out-of-range index must error");
    assert!(
        matches!(err, FieldglassError::OutOfRange),
        "expected FieldglassError::OutOfRange, got {err:?}"
    );
}

/// A reduced grid's stored field and the raster it expands into are two
/// different sizes, and it is the raster a consumer allocates (#503).
///
/// The cap in front of the decode has always measured `sum(PL)`, which stays
/// small here: 4,000 rows of one point each plus one row of 65,535, a
/// 69,535-point field. But every row widens to the widest one, so the raster is
/// 65,535 × 4,001 ≈ 262 million points — past the cap — from a file of 8 KB.
/// Without the raster check the reader would hand that field to
/// `expand_reduced_to_regular`, which would ask for ~4 GiB.
///
/// GRIB2 carries the same guard and the same test; this is the older of the two
/// paths, and the one that had gone without it since reduced grids landed (#47).
#[test]
fn a_reduced_grid_whose_raster_exceeds_the_cap_is_refused_before_expansion() {
    const ROWS: u32 = 4001;
    const WIDEST: u16 = 65_535;

    let widths: Vec<u16> = std::iter::repeat_n(1u16, ROWS as usize - 1)
        .chain(std::iter::once(WIDEST))
        .collect();
    let stored: usize = widths.iter().map(|&w| w as usize).sum();
    assert!(
        stored < fieldglass_grib1::MAX_GRID_POINTS,
        "the stored field must pass the sum(PL) cap, or this proves nothing"
    );

    let reader = Grib1Reader::from_bytes(splice_reduced_gds(REDUCED_GG, &widths))
        .expect("the framing is well-formed");
    let gds = reader.messages[0].gds.as_ref().expect("a GDS");
    assert_eq!(
        gds.points_per_row().map(<[u32]>::len),
        Some(ROWS as usize),
        "the hostile PL list is read as a PL list",
    );
    assert_eq!(gds.dimensions(), Some((WIDEST as u32, ROWS)));

    let err = reader
        .decode_message_values(0)
        .expect_err("an over-cap raster must error, not allocate");
    let FieldglassError::Parse(msg) = err else {
        panic!("expected Parse error, got {err:?}");
    };
    assert!(
        msg.contains("raster") && msg.contains("cap"),
        "the error should name the raster cap, got: {msg}"
    );
}

const REDUCED_GG: &[u8] = include_bytes!("fixtures/reduced_gg_n32.grib1");

/// Rewrite `message`'s GDS `Nj` and `PL` list in place, keeping every other
/// section, so the result scans like the fixture it came from. GRIB1 sections
/// are `<length: u24><body>`; the GDS is the one after the PDS when the PDS's
/// section-2 flag is set, which it is for this fixture.
fn splice_reduced_gds(message: &[u8], widths: &[u16]) -> Vec<u8> {
    let u24 = |v: usize| [(v >> 16) as u8, (v >> 8) as u8, v as u8];
    let len_at = |b: &[u8], at: usize| {
        ((b[at] as usize) << 16) | ((b[at + 1] as usize) << 8) | b[at + 2] as usize
    };

    // §0 "GRIB" + total length (3) + edition (1); §1 PDS follows.
    let pds_start = 8;
    let pds_len = len_at(message, pds_start);
    let gds_start = pds_start + pds_len;
    let gds_len = len_at(message, gds_start);
    let gds = &message[gds_start..gds_start + gds_len];

    // The `PL` list sits at `pvlLocation` (octet 5, 1-based) past `NV` vertical
    // coordinate words; the fixture has none, so the list simply starts there.
    let pl_start = (gds[4] as usize).saturating_sub(1) + (gds[3] as usize) * 4;
    let mut new_gds = gds[..pl_start].to_vec();
    // Nj lives at GDS octets 9-10 (0-based 8..10) for a Gaussian grid.
    new_gds[8..10].copy_from_slice(&(widths.len() as u16).to_be_bytes());
    for &w in widths {
        new_gds.extend_from_slice(&w.to_be_bytes());
    }
    let new_len = new_gds.len();
    new_gds[0..3].copy_from_slice(&u24(new_len));

    let mut out = message[..gds_start].to_vec();
    out.extend_from_slice(&new_gds);
    out.extend_from_slice(&message[gds_start + gds_len..]);
    let total = out.len();
    out[4..7].copy_from_slice(&u24(total));
    out
}
