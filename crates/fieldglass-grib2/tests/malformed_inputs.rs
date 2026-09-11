//! Smoke tests for malformed / out-of-spec GRIB2 inputs. The reader must
//! surface a structured `FieldglassError::Parse` (or return zero messages)
//! for these — never panic, over-read, or silently misinterpret garbage as
//! valid data.
//!
//! These cover the failure modes most likely to arrive over the VS Code
//! `workspace.fs.readFile` API in the wild: truncated downloads, files of the
//! wrong format/edition, files with `GRIB` substrings inside binary payloads,
//! empty buffers, and section-length-field mismatches. The section-length
//! regressions below were found by reading the same scan-plus-decode path the
//! `fuzz/` `decode` target exercises.

use fieldglass_core::FieldglassError;
use fieldglass_grib2::Grib2Reader;

const FIXTURE: &[u8] = include_bytes!("fixtures/regular_latlon_surface.grib2");

#[test]
fn empty_buffer_yields_zero_messages() {
    let reader = Grib2Reader::from_bytes(Vec::new()).expect("empty buffer parses");
    assert_eq!(reader.message_count(), 0);
}

#[test]
fn buffer_too_short_for_indicator_yields_zero_messages() {
    // Anything under the 16-byte IS can't be a complete message — the scanner
    // should return cleanly with no messages rather than out-of-bounds-index.
    let reader = Grib2Reader::from_bytes(b"GR".to_vec()).expect("short buffer parses");
    assert_eq!(reader.message_count(), 0);

    let reader = Grib2Reader::from_bytes(b"GRIB".to_vec()).expect("4-byte buffer parses");
    assert_eq!(reader.message_count(), 0);
}

#[test]
fn buffer_with_no_grib_marker_yields_zero_messages() {
    let buf = b"this is just some random bytes, not GRIB at all".to_vec();
    let reader = Grib2Reader::from_bytes(buf).expect("non-grib bytes parse");
    assert_eq!(reader.message_count(), 0);
}

#[test]
fn grib_substring_inside_payload_does_not_misparse() {
    // A buffer that contains the literal "GRIB" substring but not as a real
    // message header. The scanner must skip past it (the edition byte won't be
    // 2) without crashing or claiming a phantom message.
    let buf = b"some prefix GRIB but not a real edition-2 message header padding".to_vec();
    let reader = Grib2Reader::from_bytes(buf).expect("buffer with GRIB substring parses");
    assert_eq!(reader.message_count(), 0);
}

#[test]
fn truncated_message_returns_parse_error() {
    // Take a real message and lop off the trailing half so the IS-declared
    // total length runs past the end of the buffer.
    let mut buf = FIXTURE.to_vec();
    buf.truncate(FIXTURE.len() / 2);

    let Err(err) = Grib2Reader::from_bytes(buf) else {
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

    let Err(err) = Grib2Reader::from_bytes(buf) else {
        panic!("trailer-corrupt buffer must error");
    };
    let FieldglassError::Parse(msg) = err else {
        panic!("expected Parse error, got {err:?}");
    };
    assert!(
        msg.contains("7777"),
        "error should mention the 7777 marker, got: {msg}"
    );
}

#[test]
fn wrong_grib_edition_byte_skips_message() {
    // The fixture is GRIB edition 2. Patch the edition octet (byte 7) to 1 so
    // it looks like GRIB1. The scanner is supposed to skip non-edition-2
    // messages forward by one byte rather than panic.
    let mut buf = FIXTURE.to_vec();
    buf[7] = 1; // pretend it's GRIB1
    let reader = Grib2Reader::from_bytes(buf).expect("non-edition-2 buffer parses cleanly");
    assert_eq!(
        reader.message_count(),
        0,
        "GRIB2 reader should ignore edition-1 messages"
    );
}

#[test]
fn implausible_total_length_returns_parse_error() {
    // A well-formed IS magic + edition 2, but a total length (octets 9–16,
    // u64-BE) of 0 — smaller than the IS itself. Must error, not underflow.
    let mut buf = vec![0u8; 16];
    buf[0..4].copy_from_slice(b"GRIB");
    buf[7] = 2; // edition 2
    // total_length stays 0.
    let Err(err) = Grib2Reader::from_bytes(buf) else {
        panic!("implausible total length must error, not panic");
    };
    assert!(
        matches!(err, FieldglassError::Parse(_)),
        "expected Parse error, got {err:?}"
    );
}

/// Scanner regression (found by the `decode` fuzz target): a message found at a
/// non-zero offset whose IS total-length octets are near `u64::MAX` made
/// `offset + total_length` overflow `u64`, which panics under the fuzzer's
/// overflow checks. The `checked_add` guard turns it into the ordinary
/// "claims more than the buffer holds" parse error.
#[test]
fn total_length_overflowing_u64_returns_parse_error() {
    // One leading non-"GRIB" byte so the message starts at offset 1, then a
    // GRIB2 IS whose 8-byte total length is u64::MAX.
    let mut buf = vec![0x00u8];
    buf.extend_from_slice(b"GRIB");
    buf.extend_from_slice(&[0xFF, 0xFF]); // reserved
    buf.push(0x00); // discipline
    buf.push(2); // edition 2
    buf.extend_from_slice(&u64::MAX.to_be_bytes()); // total length
    let Err(err) = Grib2Reader::from_bytes(buf) else {
        panic!("u64-overflowing total length must error, not panic");
    };
    assert!(
        matches!(err, FieldglassError::Parse(_)),
        "expected Parse error, got {err:?}"
    );
}

#[test]
fn decode_for_out_of_range_index_returns_error() {
    let reader = Grib2Reader::from_bytes(FIXTURE.to_vec()).expect("fixture parses");
    let err = reader
        .decode_message_values(99)
        .expect_err("out-of-range index must error");
    // The context, not just the variant (#554): a host reporting "message 99
    // of 1" needs both numbers, and reading them out of the Display string was
    // the thing this variant exists to stop.
    let FieldglassError::OutOfRange { index, bound } = err else {
        panic!("expected FieldglassError::OutOfRange, got {err:?}");
    };
    assert_eq!(index, 99, "the index the caller asked for");
    assert_eq!(
        bound,
        reader.messages.len(),
        "the bound is what the file actually holds"
    );
}

/// Overwrite the 4-byte big-endian section-length field that starts the section
/// at `section_start` with `new_len`, returning the mutated buffer.
fn with_section_length(base: &[u8], section_start: usize, new_len: u32) -> Vec<u8> {
    let mut buf = base.to_vec();
    buf[section_start..section_start + 4].copy_from_slice(&new_len.to_be_bytes());
    buf
}

/// Scanner regression: a §6 BMS that declares a length larger than the bytes
/// remaining in the message previously advanced the cursor past `msg_end`, so
/// slicing for the following §7 header (`&data[cursor..msg_end]`) panicked on
/// an inverted range. It must surface as a structured parse error instead.
#[test]
fn oversized_bms_section_length_returns_parse_error() {
    let reader = Grib2Reader::from_bytes(FIXTURE.to_vec()).expect("fixture parses");
    let bms_start = reader.messages[0].bms_range.start as usize;

    let buf = with_section_length(FIXTURE, bms_start, u32::MAX);
    let Err(err) = Grib2Reader::from_bytes(buf) else {
        panic!("oversized BMS length must error, not panic");
    };
    assert!(
        matches!(err, FieldglassError::Parse(_)),
        "expected Parse error, got {err:?}"
    );
}

/// Scanner regression: a §7 DS that declares a length running past the buffer
/// previously recorded a `ds_range` whose end exceeded `data.len()`, so the
/// decode-time slice `&self.data[ds_start..ds_end]` panicked. Validating the
/// declared length while scanning rejects it up front.
#[test]
fn oversized_ds_section_length_returns_parse_error() {
    let reader = Grib2Reader::from_bytes(FIXTURE.to_vec()).expect("fixture parses");
    let ds_start = reader.messages[0].ds_range.start as usize;

    let buf = with_section_length(FIXTURE, ds_start, u32::MAX);
    let Err(err) = Grib2Reader::from_bytes(buf) else {
        panic!("oversized DS length must error, not panic");
    };
    assert!(
        matches!(err, FieldglassError::Parse(_)),
        "expected Parse error, got {err:?}"
    );
}

/// Decode regression (found by the `decode` fuzz target): this message's §3
/// grid template names a 16 × 8388639 ≈ 134-million-point grid — under the
/// MAX_GRID_POINTS cap — while the GDS's own "number of data points" field
/// (and the rest of the file) describes a tiny 16 × 31 grid. The constant-field
/// simple-packing path (`bits_per_value == 0`) then tried to allocate
/// `vec![Some(f64); 134_218_224]` (~2 GiB) for a file carrying no such data,
/// which libFuzzer reported as an out-of-memory. Decode must now reject the
/// dimensions/`num_data_points` mismatch up front. Byte-for-byte the libFuzzer
/// artifact (`fuzz/artifacts/decode/oom-…`), kept as the canonical regression.
#[rustfmt::skip]
const FUZZ_OOM_GRID_NP_MISMATCH: &[u8] = &[
    71, 82, 73, 66, 255, 255, 0, 2, 0, 0, 0, 0, 0, 0, 0, 191, 0, 0, 0, 21, 1, 0, 98, 0, 0, 4, 0, 1,
    7, 215, 3, 23, 12, 0, 0, 0, 2, 0, 0, 0, 84, 3, 0, 0, 0, 1, 240, 0, 0, 0, 1, 6, 255, 255, 255,
    255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 0, 0, 0, 16, 0, 128, 0, 31, 0, 0,
    0, 0, 255, 255, 255, 255, 3, 147, 135, 0, 0, 0, 0, 0, 48, 0, 0, 0, 0, 1, 201, 195, 128, 0, 30,
    132, 128, 0, 30, 132, 128, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 34, 4, 0, 0, 0, 0,
    0, 0, 0, 255, 128, 0, 0, 0, 1, 0, 0, 0, 0, 1, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255,
    255, 0, 0, 0, 21, 5, 0, 0, 1, 240, 0, 0, 67, 136, 147, 51, 0, 0, 0, 0, 0, 0, 0, 0, 0, 6, 6,
    255, 0, 0, 0, 5, 7, 55, 55, 55, 55,
];

/// The artifact itself: refused before it can allocate, which is the property
/// the fuzzer found missing.
///
/// **Which check refuses it moved with #707.** Its 134-million-point grid sat
/// under the old 200 M cap, so the mismatch check below was what caught it; the
/// cap is now `fieldglass_core::MAX_FIELD_POINTS` (64 Mi, the number the GRIB1
/// reader always used) and refuses it one step earlier. Either answer prevents
/// the OOM, so this asserts that rather than which — and
/// `dimensions_disagreeing_with_num_data_points_are_refused_under_the_cap` keeps
/// the mismatch check itself under test, on a grid the cap lets through.
#[test]
fn grid_dimensions_disagreeing_with_num_data_points_rejected_before_allocation() {
    let reader =
        Grib2Reader::from_bytes(FUZZ_OOM_GRID_NP_MISMATCH.to_vec()).expect("scan succeeds");
    let err = reader
        .decode_message_values(0)
        .expect_err("dimensions/num_data_points mismatch must error, not OOM");
    let FieldglassError::Parse(msg) = err else {
        panic!("expected Parse error, got {err:?}");
    };
    assert!(
        msg.contains("exceeds cap") || (msg.contains("disagree") && msg.contains("data points")),
        "error should refuse the grid before allocating it, got: {msg}"
    );
}

/// The grid cap is core's field cap, to the point — and it is no longer 200 M.
///
/// This is the defect #707 opens with. This reader capped a field at
/// `200_000_000` under a doc comment saying it matched the GRIB1 reader's
/// `64 * 1024 * 1024`, which it did not, so a field between 67 M and 200 M points
/// was accepted here and refused there. Both now name
/// `fieldglass_core::MAX_FIELD_POINTS`, and this checks the number the refusal
/// prints, since a caller reads the message and not the constant.
///
/// **A field of exactly the cap is not refused, and that is argued rather than
/// run**, for the reason the GRIB1 twin of this test gives: the guard is
/// `expected_count > CAP`, so naming `CAP` as the bound exceeded proves `CAP`
/// passes it — and executing that half would let the constant-field path
/// (`bits_per_value == 0`, which is what this very artifact exercises) allocate
/// a gigabyte.
///
/// `5 × 13,421,773 = 67,108,865` is one point over, and both factors fit the
/// `u32` a §3 extent is. The GDS's own data-point count is spliced to agree, or
/// the mismatch check would refuse it first and prove nothing about the cap.
#[test]
fn the_grid_cap_is_the_core_field_cap_to_the_point() {
    const CAP: usize = fieldglass_core::MAX_FIELD_POINTS;
    const NI: u32 = 5;
    const NJ: u32 = 13_421_773;
    assert_eq!(NI as usize * NJ as usize, CAP + 1, "one point over the cap");

    let mut bytes = FUZZ_OOM_GRID_NP_MISMATCH.to_vec();
    let ni_nj = bytes
        .windows(8)
        .position(|w| w == [0, 0, 0, 16, 0, 128, 0, 31])
        .expect("the artifact's Ni/Nj pair");
    bytes[ni_nj..ni_nj + 4].copy_from_slice(&NI.to_be_bytes());
    bytes[ni_nj + 4..ni_nj + 8].copy_from_slice(&NJ.to_be_bytes());

    // §3's own "number of data points" is octets 7–10 of the section, which
    // begins at the `0,0,0,84, 3` prefix. Located by value so a change to the
    // artifact cannot silently splice the wrong field.
    let s3 = bytes
        .windows(5)
        .position(|w| w == [0, 0, 0, 84, 3])
        .expect("the artifact's §3");
    bytes[s3 + 6..s3 + 10].copy_from_slice(&(NI * NJ).to_be_bytes());

    let reader = Grib2Reader::from_bytes(bytes).expect("scan succeeds");
    let err = reader
        .decode_message_values(0)
        .expect_err("one point past the cap must be refused, not allocated");
    let FieldglassError::Parse(msg) = err else {
        panic!("expected Parse error, got {err:?}");
    };
    assert!(
        msg.contains("exceeds cap") && msg.contains(&CAP.to_string()),
        "the refusal must name the cap it enforces, not 200000000: {msg}"
    );
    assert!(
        msg.contains(&(CAP + 1).to_string()),
        "and the count it refused: {msg}"
    );
}

/// The mismatch check, on a grid small enough that the cap does not reach it.
///
/// The artifact above used to be this test's subject and stopped being it when
/// #707 tightened the GRIB2 cap to the GRIB1 number. Same bytes, with `Nj`
/// spliced from 8,388,639 down to 4,000,000: `16 × 4,000,000 = 64,000,000`
/// points, under the 67,108,864 cap, while the GDS's own "number of data points"
/// still says 496. Without the mismatch check the constant-field path would
/// allocate 64 million elements — a gigabyte — for a file carrying no such data.
#[test]
fn dimensions_disagreeing_with_num_data_points_are_refused_under_the_cap() {
    let mut bytes = FUZZ_OOM_GRID_NP_MISMATCH.to_vec();
    // `Ni` is the big-endian `0, 0, 0, 16` in §3, and `Nj` the four octets
    // after it. Found by value rather than by offset so a change to the
    // artifact cannot silently splice the wrong field.
    let ni_nj = bytes
        .windows(8)
        .position(|w| w == [0, 0, 0, 16, 0, 128, 0, 31])
        .expect("the artifact's Ni/Nj pair");
    const NJ: u32 = 4_000_000;
    bytes[ni_nj + 4..ni_nj + 8].copy_from_slice(&NJ.to_be_bytes());
    assert!(
        16 * u64::from(NJ) < fieldglass_core::MAX_FIELD_POINTS as u64,
        "the spliced grid must pass the cap, or this proves nothing"
    );

    let reader = Grib2Reader::from_bytes(bytes).expect("scan succeeds");
    let err = reader
        .decode_message_values(0)
        .expect_err("dimensions/num_data_points mismatch must error, not allocate");
    let FieldglassError::Parse(msg) = err else {
        panic!("expected Parse error, got {err:?}");
    };
    assert!(
        msg.contains("disagree") && msg.contains("data points"),
        "error should name the dimensions/num_data_points mismatch, got: {msg}"
    );
}

/// A reduced grid's stored field and the raster it expands into are two
/// different sizes, and it is the raster a consumer allocates (#503).
///
/// `sum(PL)` is what the cap in front of the decode has always measured, and it
/// stays small here: 4,000 rows of one point each, plus one row of 65,535. That
/// is a 69,535-point field — but the widest row is what every row widens to, so
/// the raster is 65,535 × 4,001 ≈ 262 million points, comfortably past the cap.
/// A file describing it is 8 KB. Without the raster check the reader would hand
/// that field to `expand_reduced_to_regular` and the expansion would ask for
/// ~4 GiB.
///
/// Built by splicing a §3 onto a real message so the scan reaches the decode:
/// the hostile part is the `PL` list, not the framing.
#[test]
fn a_reduced_grid_whose_raster_exceeds_the_cap_is_refused_before_expansion() {
    const ROWS: u32 = 4001;
    const WIDEST: u32 = 65_535;

    let widths: Vec<u32> = std::iter::repeat_n(1u32, ROWS as usize - 1)
        .chain(std::iter::once(WIDEST))
        .collect();
    let stored: u32 = widths.iter().sum();
    assert!(
        (stored as usize) < 200_000_000,
        "the stored field must pass the sum(PL) cap, or this proves nothing"
    );

    let bytes = splice_reduced_gds(FIXTURE_REDUCED, &widths, stored);
    let reader = Grib2Reader::from_bytes(bytes).expect("the framing is well-formed");
    let gds = &reader.messages[0].gds;
    assert_eq!(
        gds.points_per_row().map(<[u32]>::len),
        Some(ROWS as usize),
        "the hostile PL list is read as a PL list",
    );
    assert_eq!(gds.dimensions(), Some((WIDEST, ROWS)));

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

const FIXTURE_REDUCED: &[u8] = include_bytes!("fixtures/reduced_gaussian_pressure_level.grib2");

/// Replace `message`'s §3 with a reduced Gaussian one carrying `widths`, and
/// declare `num_data_points`. Every other section is carried over untouched, so
/// the result scans like the fixture it came from.
fn splice_reduced_gds(message: &[u8], widths: &[u32], num_data_points: u32) -> Vec<u8> {
    // §0 is 16 octets; sections then run <length: u32><number: u8><body>.
    let mut cursor = 16usize;
    let mut out = message[..16].to_vec();
    loop {
        if &message[cursor..cursor + 4] == b"7777" {
            out.extend_from_slice(b"7777");
            break;
        }
        let len = u32::from_be_bytes(message[cursor..cursor + 4].try_into().unwrap()) as usize;
        let section = &message[cursor..cursor + len];
        if section[4] == 3 {
            // Keep the template payload (octets 15..) and swap the list behind
            // it: 2 octets per entry, one per row, `Nj` set to match.
            let template = &section[14..14 + 58];
            let mut body = template.to_vec();
            body[20..24].copy_from_slice(&(widths.len() as u32).to_be_bytes());
            let mut list = Vec::new();
            for &w in widths {
                list.extend_from_slice(&(w as u16).to_be_bytes());
            }
            let total = 14 + body.len() + list.len();
            out.extend_from_slice(&(total as u32).to_be_bytes());
            out.push(3);
            out.push(section[5]); // source of grid definition
            out.extend_from_slice(&num_data_points.to_be_bytes());
            out.push(2); // octets per optional-list entry
            out.push(1); // Code Table 3.11: numbers of points per parallel
            out.extend_from_slice(&40u16.to_be_bytes());
            out.extend_from_slice(&body);
            out.extend_from_slice(&list);
        } else {
            out.extend_from_slice(section);
        }
        cursor += len;
    }
    let total = out.len() as u64;
    out[8..16].copy_from_slice(&total.to_be_bytes());
    out
}

const SPECTRAL: &[u8] = include_bytes!("fixtures/spectral_simple_t63.grib2");

/// Rebuild the T63 spectral fixture with a declared truncation of `t` and a
/// zero bit-width, with §7 cut down to nothing.
///
/// Zero is the legal constant-field bit width, and it is the one case where the
/// bit budget §7 imposes on a declared truncation is vacuous — no bits are
/// needed, so `J` alone sizes the coefficient array (#631).
fn hostile_spectral(t: u32) -> Vec<u8> {
    let mut out = SPECTRAL[..16].to_vec();
    let mut cursor = 16;
    while cursor + 4 <= SPECTRAL.len() {
        if &SPECTRAL[cursor..cursor + 4] == b"7777" {
            break;
        }
        let len = u32::from_be_bytes(SPECTRAL[cursor..cursor + 4].try_into().unwrap()) as usize;
        let section = &SPECTRAL[cursor..cursor + len];
        match section[4] {
            // §3 template 3.50: J, K and M are octets 15-18, 19-22, 23-26.
            3 => {
                let mut body = section.to_vec();
                for at in [14, 18, 22] {
                    body[at..at + 4].copy_from_slice(&t.to_be_bytes());
                }
                out.extend_from_slice(&body);
            }
            // §5 template 5.50: `bitsPerValue` is octet 20.
            5 => {
                let mut body = section.to_vec();
                body[19] = 0;
                out.extend_from_slice(&body);
            }
            // §7 keeps only its own header.
            7 => {
                out.extend_from_slice(&5u32.to_be_bytes());
                out.push(7);
            }
            _ => out.extend_from_slice(section),
        }
        cursor += len;
    }
    out.extend_from_slice(b"7777");
    let total = out.len() as u64;
    out[8..16].copy_from_slice(&total.to_be_bytes());
    out
}

#[test]
fn a_spectral_truncation_past_the_cap_is_refused_rather_than_allocated() {
    // Inside this crate's old 200 M-value envelope (1.6 GB), outside the
    // ceiling every spectral path now shares.
    let bytes = hostile_spectral(10_000);
    assert!(
        bytes.len() < 200,
        "the hostile message is {} bytes",
        bytes.len()
    );

    let reader = Grib2Reader::from_bytes(bytes).expect("the message still scans");
    let Err(err) = reader.decode_spectral_message(0) else {
        panic!("a truncation past the cap must be refused");
    };
    assert!(
        matches!(&err, FieldglassError::Parse(m) if m.contains("exceeds the cap")),
        "{err}"
    );
}

#[test]
fn a_spectral_truncation_inside_the_cap_still_decodes_at_zero_bit_width() {
    // The control for the test above: the refusal must come from the ceiling,
    // not from the surgery.
    let reader = Grib2Reader::from_bytes(hostile_spectral(63)).expect("scans");
    let coeffs = reader
        .decode_spectral_message(0)
        .expect("a constant-field spectral message decodes");
    assert_eq!(coeffs.coefficients.len(), 64 * 65);
}
