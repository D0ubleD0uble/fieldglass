//! `sz::decompress` against libsz 1.1.7, over the corpus's szip cases.
//!
//! Every `sz_*` stream was written by libsz's `SZ_BufftoBuffCompress` and its
//! expected output by `SZ_BufftoBuffDecompress` (`tests/fixtures/NOTICE.md`).
//! libsz decoded every one completely, so its bytes are the oracle. The
//! parameters in the manifest are in `SZ_com_t` order, which is the order
//! `SzParams::new` takes.
//!
//! The rest of the file holds the edges libsz is not the oracle for: short
//! output and partial pixels, where this crate refuses what libsz returns
//! `SZ_OK` for, and bad codes after the last pixel, which this crate never
//! reads and libsz fails on (all in ADR-0012 decision 4); and the parameter
//! checks, which mirror `sz_compat.c:229-235`.

mod common;

use common::{Bits, FIXTURES, SZ_CASES, manifest, rows, sha256_hex, text, uint};
use fieldglass_aec::AecError;
use fieldglass_aec::sz::{self, SzParams};

struct Case {
    name: String,
    params: SzParams,
    stream: Vec<u8>,
    dest_len: usize,
    output_sha256: String,
}

fn narrow_u32(row: &serde_json::Value, key: &str) -> u32 {
    u32::try_from(uint(row, key)).unwrap_or_else(|_| panic!("`{key}` does not fit u32 in {row}"))
}

fn cases() -> Vec<Case> {
    let manifest = manifest();
    rows(&manifest, "sz_cases", SZ_CASES)
        .iter()
        .map(|row| {
            let name = text(row, "name").to_owned();
            let params = SzParams::new(
                narrow_u32(row, "options_mask"),
                narrow_u32(row, "bits_per_pixel"),
                narrow_u32(row, "pixels_per_block"),
                narrow_u32(row, "pixels_per_scanline"),
            )
            .unwrap_or_else(|e| panic!("{name}: libsz decoded it, but {e}"));
            let path = format!("{FIXTURES}/{}", text(row, "stream"));
            let stream = std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
            Case {
                name,
                params,
                stream,
                dest_len: usize::try_from(uint(row, "dest_len")).unwrap(),
                output_sha256: text(row, "output_sha256").to_owned(),
            }
        })
        .collect()
}

fn case(name: &str) -> Case {
    cases()
        .into_iter()
        .find(|c| c.name == name)
        .unwrap_or_else(|| panic!("the corpus has no case {name}"))
}

fn decompress(case: &Case, len: usize) -> Result<Vec<u8>, AecError> {
    let mut out = vec![0xEE; len];
    sz::decompress(&case.stream, &case.params, &mut out).map(|()| out)
}

#[test]
fn every_szip_case_decodes_to_libszs_output() {
    let mut failures = Vec::new();
    let mut checked = 0;
    for case in cases() {
        match decompress(&case, case.dest_len) {
            Ok(out) if sha256_hex(&out) == case.output_sha256 => {}
            Ok(_) => failures.push(format!("{}: bytes differ from libsz's", case.name)),
            Err(e) => failures.push(format!("{}: {e}", case.name)),
        }
        checked += 1;
    }
    assert!(failures.is_empty(), "{failures:#?}");
    assert_eq!(checked, SZ_CASES);
}

/// The corpus reaches every path through the szip layer: each pixel width,
/// padded and unpadded scanlines, one block per scanline, and both option
/// bits that matter, in both states.
#[test]
fn the_corpus_covers_every_path() {
    let cases = cases();
    let has = |f: &dyn Fn(&SzParams) -> bool| cases.iter().any(|c| f(&c.params));
    for bpp in [8, 12, 16, 24, 32, 64] {
        assert!(has(&|p| p.bits_per_pixel() == bpp), "{bpp} bits");
        let padded = |p: &SzParams| !p.pixels_per_scanline().is_multiple_of(p.pixels_per_block());
        assert!(
            has(&|p| p.bits_per_pixel() == bpp && padded(p)),
            "{bpp} bits padded"
        );
        assert!(
            has(&|p| p.bits_per_pixel() == bpp && !padded(p)),
            "{bpp} bits unpadded"
        );
    }
    assert!(has(&|p| p.pixels_per_scanline() < p.pixels_per_block()));
    assert!(has(&|p| p.pixels_per_scanline() == 1));
    for mask in [sz::NN_OPTION_MASK, sz::MSB_OPTION_MASK] {
        assert!(has(&|p| p.options_mask() & mask != 0), "mask {mask} set");
        assert!(has(&|p| p.options_mask() & mask == 0), "mask {mask} clear");
    }
    // A partial last scanline, whose trailing pads are never decoded.
    assert!(cases.iter().any(|c| {
        let pixels = c.dest_len / c.params.bytes_per_pixel();
        pixels % c.params.pixels_per_scanline() as usize != 0
    }));
}

/// 32-bit pixels are coded as four byte planes of `P` bytes each. Here `P` is
/// 150 and a scanline is 64 samples, so each plane ends part-way through a
/// scanline and the next plane's first bytes share that scanline.
#[test]
fn a_32_bit_plane_edge_that_falls_mid_scanline() {
    let case = case("sz_b32_plane_edge_mid_scanline");
    let p = case.params;
    let plane = case.dest_len / 4;
    assert_eq!(
        (p.bits_per_pixel(), plane, p.pixels_per_scanline()),
        (32, 150, 64)
    );
    assert_ne!(plane % p.pixels_per_scanline() as usize, 0);
    let out = decompress(&case, case.dest_len).unwrap();
    assert_eq!(sha256_hex(&out), case.output_sha256);
}

/// The same at 64 bits, with padded scanlines as well: eight planes of 77
/// bytes over scanlines of 45 samples padded to 50.
#[test]
fn a_64_bit_plane_edge_that_falls_mid_padded_scanline() {
    let case = case("sz_b64_plane_edge_mid_scanline");
    let p = case.params;
    assert_eq!((p.bits_per_pixel(), case.dest_len / 8), (64, 77));
    assert_ne!(p.pixels_per_scanline() % p.pixels_per_block(), 0);
    let out = decompress(&case, case.dest_len).unwrap();
    assert_eq!(sha256_hex(&out), case.output_sha256);
}

/// Only NN and MSB change a decode. Setting any other bit, on any case, gives
/// libsz's bytes still.
#[test]
fn the_other_option_bits_change_nothing() {
    let ignored = sz::ALLOW_K13_OPTION_MASK
        | sz::CHIP_OPTION_MASK
        | sz::EC_OPTION_MASK
        | sz::LSB_OPTION_MASK
        | 64
        | sz::RAW_OPTION_MASK
        | 0xFFFF_FF00;
    let mut seen = 0;
    for mut case in cases() {
        let p = case.params;
        case.params = SzParams::new(
            p.options_mask() | ignored,
            p.bits_per_pixel(),
            p.pixels_per_block(),
            p.pixels_per_scanline(),
        )
        .unwrap();
        let out = decompress(&case, case.dest_len).unwrap();
        assert_eq!(sha256_hex(&out), case.output_sha256, "{}", case.name);
        seen += 1;
    }
    assert_eq!(seen, SZ_CASES);
    // The corpus's own case with K13, CHIP and RAW set.
    assert_eq!(case("sz_b16_ignored_options").params.options_mask(), 171);
}

/// A stream that runs out is an error here, never a shorter success.
///
/// libsz returns `SZ_OK` for both halves of this test, in two ways. With
/// unpadded scanlines it lowers `destLen` (`sz_compat.c:302-303`): the 39
/// unpadded cases. With padded scanlines it reports the full length
/// (`sz_compat.c:295`) with the undecoded tail taken from an uninitialised
/// buffer: all 39 padded cases, cut in half, return `SZ_OK` at their full
/// `destLen` from libsz 1.1.7.
#[test]
fn a_stream_that_decodes_short_is_an_error() {
    for case in cases() {
        let pixel = case.params.bytes_per_pixel();
        let samples_per_pixel = if matches!(case.params.bits_per_pixel(), 32 | 64) {
            pixel
        } else {
            1
        };
        // The stream cut in half, and the whole stream asked for one more
        // scanline than it holds.
        let extra = case.params.pixels_per_scanline() as usize * pixel;
        let whole = &case.stream[..];
        let cut = &whole[..whole.len() / 2];
        for (stream, len) in [(cut, case.dest_len), (whole, case.dest_len + extra)] {
            let mut out = vec![0; len];
            let err = sz::decompress(stream, &case.params, &mut out).unwrap_err();
            let AecError::Truncated { decoded, requested } = err else {
                panic!("{}: {err}", case.name);
            };
            assert_eq!(requested * pixel, len * samples_per_pixel, "{}", case.name);
            assert!(decoded < requested, "{}", case.name);
        }
    }
}

/// A length that is not a whole number of pixels is refused before anything
/// is decoded.
#[test]
fn a_length_that_is_not_a_whole_number_of_pixels_is_an_error() {
    let mut checked = 0;
    for case in cases() {
        let pixel = case.params.bytes_per_pixel();
        if pixel == 1 {
            // Every length is a whole number of 1-byte pixels.
            continue;
        }
        for len in [case.dest_len - 1, case.dest_len + 1] {
            assert_eq!(
                decompress(&case, len),
                Err(AecError::OutputLength {
                    len,
                    bytes_per_sample: pixel
                }),
                "{}",
                case.name
            );
            checked += 1;
        }
    }
    assert!(checked > 100, "{checked}");
}

/// libsz 1.1.7 decodes this case into a 599-byte buffer with `SZ_OK`: it
/// deinterleaves with planes of `599 / 4 = 149` bytes instead of 150, so 165
/// bytes are misplaced and the last three are never written (168 differ from
/// the 600-byte decode; `sz_compat.c:84-93, 305-306`; reproduction in
/// `tests/fixtures/NOTICE.md`). That only happens at a length that is not
/// whole pixels, and it is an error here.
#[test]
fn a_partial_32_bit_pixel_is_an_error_where_libsz_scrambles_it() {
    let case = case("sz_b32_plane_edge_mid_scanline");
    assert_eq!(
        decompress(&case, 599),
        Err(AecError::OutputLength {
            len: 599,
            bytes_per_sample: 4
        })
    );
}

/// An empty output reads nothing and succeeds, as libsz does.
#[test]
fn an_empty_output_reads_nothing() {
    let params = SzParams::new(sz::NN_OPTION_MASK, 64, 32, 1).unwrap();
    sz::decompress(&[], &params, &mut []).unwrap();
}

/// A prefix of whole scanlines decodes to libsz's prefix: the stream is read
/// only as far as the output needs.
#[test]
fn a_prefix_of_the_output_decodes_from_the_same_stream() {
    for case in cases() {
        if matches!(case.params.bits_per_pixel(), 32 | 64) {
            // Byte planes depend on the whole length.
            continue;
        }
        let full = decompress(&case, case.dest_len).unwrap();
        let line = case.params.pixels_per_scanline() as usize * case.params.bytes_per_pixel();
        let half = (case.dest_len / 2).div_ceil(line) * line;
        let half = half.min(case.dest_len);
        let part = decompress(&case, half).unwrap();
        assert_eq!(part[..], full[..half], "{}", case.name);
    }
}

/// `sz_compat.c:229-235`, then the block-size bound `aec_decode_init` adds
/// behind it. Each row's libsz verdict was taken from libsz 1.1.7 itself
/// (`SZ_BufftoBuffDecompress`: 0 is `SZ_OK`, -1 is `SZ_PARAM_ERROR`).
#[test]
fn parameter_checks_mirror_libsz() {
    const OK: i32 = 0;
    const PARAM_ERROR: i32 = -1;
    // (bits per pixel, pixels per block, pixels per scanline, libsz, ours)
    let table: [(u32, u32, u32, i32, Option<AecError>); 22] = [
        (8, 16, 0, PARAM_ERROR, Some(AecError::PixelsPerScanline(0))),
        (8, 16, 1, OK, None),
        (8, 16, 4096, OK, None),
        (
            8,
            16,
            4097,
            PARAM_ERROR,
            Some(AecError::PixelsPerScanline(4097)),
        ),
        (8, 0, 16, PARAM_ERROR, Some(AecError::PixelsPerBlock(0))),
        (8, 15, 16, PARAM_ERROR, Some(AecError::PixelsPerBlock(15))),
        (8, 2, 16, OK, None),
        (8, 34, 64, OK, None),
        (8, 256, 16, OK, None),
        (8, 256, 4096, OK, None),
        (8, 258, 16, PARAM_ERROR, Some(AecError::PixelsPerBlock(258))),
        (0, 16, 16, PARAM_ERROR, Some(AecError::BitsPerPixel(0))),
        (1, 16, 16, OK, None),
        (31, 16, 16, OK, None),
        (32, 16, 16, OK, None),
        (33, 16, 16, PARAM_ERROR, Some(AecError::BitsPerPixel(33))),
        (63, 16, 16, PARAM_ERROR, Some(AecError::BitsPerPixel(63))),
        (64, 16, 16, OK, None),
        (65, 16, 16, PARAM_ERROR, Some(AecError::BitsPerPixel(65))),
        (8, 2, 1, OK, None),
        // libsz checks the scanline first, then the block, then the width.
        (0, 0, 0, PARAM_ERROR, Some(AecError::PixelsPerScanline(0))),
        (0, 0, 16, PARAM_ERROR, Some(AecError::PixelsPerBlock(0))),
    ];
    for (bpp, ppb, pps, libsz, want) in table {
        let got = SzParams::new(0, bpp, ppb, pps).err();
        assert_eq!(got, want, "{bpp}/{ppb}/{pps}");
        assert_eq!(got.is_none(), libsz == OK, "{bpp}/{ppb}/{pps}");
    }
    assert_eq!(
        SzParams::new(0, u32::MAX, u32::MAX, u32::MAX),
        Err(AecError::PixelsPerScanline(u32::MAX))
    );
}

#[test]
fn the_accessors_return_what_was_validated() {
    let p = SzParams::new(171, 24, 10, 45).unwrap();
    assert_eq!(
        (
            p.options_mask(),
            p.bits_per_pixel(),
            p.pixels_per_block(),
            p.pixels_per_scanline()
        ),
        (171, 24, 10, 45)
    );
    let width = |bpp| SzParams::new(0, bpp, 8, 8).unwrap().bytes_per_pixel();
    assert_eq!(
        [1, 8, 9, 12, 16, 17, 24, 25, 31, 32, 64].map(width),
        [1, 1, 2, 2, 2, 4, 4, 4, 4, 4, 8]
    );
}

/// A bad code's position is reported in output pixels, pads not counted.
#[test]
fn an_invalid_code_is_placed_in_output_pixels() {
    // 8-bit pixels, blocks of 2, one pixel per scanline: each block is a
    // pixel and its pad. The first block is uncompressed; the second is FS
    // (id 0b001) with a value of 256, too wide for 8 bits.
    let params = SzParams::new(0, 8, 2, 1).unwrap();
    let mut bits = Bits::default();
    bits.put(0b111, 3).put(5, 8).put(0, 8);
    bits.put(0b001, 3).fs(256).fs(0);
    let stream = bits.done();
    let mut out = [0u8; 2];
    let err = sz::decompress(&stream, &params, &mut out).unwrap_err();
    assert!(
        matches!(err, AecError::InvalidCode { sample: 1, .. }),
        "{err:?}"
    );
    // The first pixel alone decodes.
    let mut out = [0u8; 1];
    sz::decompress(&stream, &params, &mut out).unwrap();
    assert_eq!(out, [5]);
}

/// A zero-block run that covers a scanline's pixels and its pads writes the
/// pixels and skips the pads, in one call to the sink.
#[test]
fn a_zero_run_across_pads_repeats_the_last_pixel() {
    // 16-bit, NN, MSB; blocks of 4, 6 pixels per scanline, padded to 8.
    // One scanline: a zero block with reference sample 0x1234, covering
    // both blocks (fs = 1: two blocks).
    let params = SzParams::new(sz::NN_OPTION_MASK | sz::MSB_OPTION_MASK, 16, 4, 6).unwrap();
    let mut bits = Bits::default();
    bits.put(0, 4).put(0, 1).put(0x1234, 16).fs(1);
    let stream = bits.done();
    let mut out = [0u8; 12];
    sz::decompress(&stream, &params, &mut out).unwrap();
    assert_eq!(out, [0x12, 0x34].repeat(6)[..]);
}

/// libsz decodes padded scanlines whole, so it reads the blocks after the
/// last pixel of a partial last scanline and fails on a bad code there. This
/// crate stops at the last pixel and never reads them (ADR-0012 decision 4).
///
/// 8-bit pixels, blocks of 2, 5 pixels per scanline (3 blocks, padded to 6
/// samples). The first scanline is three uncompressed blocks: 1 2, 3 4, 5
/// and a pad. The second is uncompressed 6 7, uncompressed 8 9, then a zero
/// block of two blocks where one is left in the interval, a run past its end.
/// A 6-pixel output ends at the second scanline's first sample.
///
/// libsz 1.1.7, `SZ_BufftoBuffDecompress` with mask 0, 8 bits, 2 per block,
/// 5 per scanline: `destLen` 6 gives -3 (`AEC_DATA_ERROR`, `decode.c:529-541`)
/// and 11 gives -3; `destLen` 5, a whole first scanline, gives `SZ_OK` and
/// 1 2 3 4 5.
#[test]
fn a_bad_code_after_the_last_pixel_is_never_read() {
    let params = SzParams::new(0, 8, 2, 5).unwrap();
    let mut bits = Bits::default();
    for (a, b) in [(1, 2), (3, 4), (5, 0), (6, 7), (8, 9)] {
        bits.put(0b111, 3).put(a, 8).put(b, 8);
    }
    bits.put(0b000, 3).put(0, 1).fs(1);
    let stream = bits.done();

    let mut out = [0u8; 6];
    sz::decompress(&stream, &params, &mut out).unwrap();
    assert_eq!(out, [1, 2, 3, 4, 5, 6]);

    // Asking for the pixels that run covers reaches the bad code.
    let mut out = [0u8; 10];
    assert!(matches!(
        sz::decompress(&stream, &params, &mut out),
        Err(AecError::InvalidCode { sample: 9, .. })
    ));
}
