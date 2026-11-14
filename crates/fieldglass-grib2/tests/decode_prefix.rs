//! Display decode of a GRIB2 message cut short in §7 (#1785).
//!
//! A host streaming a JPEG 2000 (§5.40) message draws it coarse first. The
//! claims checked here:
//!
//! **A prefix cut at a level boundary is that level of the whole message.**
//! Byte for byte: the same raster, shape and geometry as
//! `decode_message_raster_with` gives for the whole message at that level. The
//! boundaries are the codestream byte counts at which the codec first reports
//! a level complete, measured on the committed fixtures by decoding every
//! prefix with `strict` off; they are restated below and each one is checked
//! from both sides, so a wrong one fails.
//!
//! **One byte short is the level below, and the result says so** through
//! `ReducedRaster::reduction`.
//!
//! **Every refusal of a reduced decode holds for a prefix**, whatever level is
//! asked, and each is reached here through bytes: a parsed message cannot be
//! edited inside a `MessagePrefix`, so sections are spliced between fixtures
//! or patched in place, and each test names what it changed.

use fieldglass_grib2::{DecodeOptions, FieldglassError, Grib2Reader, MessagePrefix, ReducedRaster};
use std::path::Path;

const RAP: &str = "rap_jpeg2000_lambert.grib2";
const LATLON: &str = "jpeg2000_regular_latlon.grib2";

fn read(fixture: &str) -> Vec<u8> {
    let bytes = std::fs::read(Path::new("tests/fixtures").join(fixture))
        .unwrap_or_else(|e| panic!("read fixture {fixture}: {e}"));
    assert_eq!(&bytes[..4], b"GRIB", "{fixture} starts with its message");
    bytes
}

/// The sections of the message at the start of `bytes`: `(number, start,
/// length)`, from §1 to §7.
fn sections(bytes: &[u8]) -> Vec<(u8, usize, usize)> {
    let mut out = Vec::new();
    let mut at = 16usize;
    while &bytes[at..at + 4] != b"7777" {
        let len = u32::from_be_bytes(bytes[at..at + 4].try_into().expect("4 bytes")) as usize;
        out.push((bytes[at + 4], at, len));
        at += len;
    }
    out
}

/// Where the codestream starts: after §7's five-byte header.
fn codestream_start(bytes: &[u8]) -> usize {
    let (_, start, _) = *sections(bytes)
        .iter()
        .find(|(n, _, _)| *n == 7)
        .expect("a §7");
    start + 5
}

/// The message with §0 to §6 and the first `n` bytes of the codestream.
fn cut(bytes: &[u8], n: usize) -> Vec<u8> {
    bytes[..codestream_start(bytes) + n].to_vec()
}

/// `base`'s §1 to §4 with `donor`'s §5 to §7, §0's length fixed to match.
fn splice(base: &[u8], donor: &[u8]) -> Vec<u8> {
    let mut out = base[..16].to_vec();
    for (n, start, len) in sections(base) {
        if n < 5 {
            out.extend_from_slice(&base[start..start + len]);
        }
    }
    for (n, start, len) in sections(donor) {
        if n >= 5 {
            out.extend_from_slice(&donor[start..start + len]);
        }
    }
    out.extend_from_slice(b"7777");
    let total = out.len() as u64;
    out[8..16].copy_from_slice(&total.to_be_bytes());
    out
}

fn prefix(bytes: Vec<u8>) -> MessagePrefix {
    MessagePrefix::parse(bytes).expect("§0 to §6 parse")
}

fn display(bytes: &[u8], n: usize, reduction: u8) -> Result<ReducedRaster, FieldglassError> {
    prefix(cut(bytes, n)).decode_display(DecodeOptions::new(reduction))
}

fn bits(values: &[Option<f64>]) -> Vec<Option<u64>> {
    values.iter().map(|v| v.map(f64::to_bits)).collect()
}

/// The raster `got` is `want`, bit for bit, on the same grid.
fn assert_same(got: &ReducedRaster, want: &ReducedRaster, what: &str) {
    assert_eq!(got.reduction(), want.reduction(), "{what}: level");
    assert_eq!(
        (got.ni(), got.nj()),
        (want.ni(), want.nj()),
        "{what}: shape"
    );
    assert_eq!(got.geometry(), want.geometry(), "{what}: geometry");
    assert_eq!(
        bits(got.display_values()),
        bits(want.display_values()),
        "{what}: values"
    );
    assert_eq!(
        got.exact_values().map(bits),
        want.exact_values().map(bits),
        "{what}: exact values"
    );
}

/// Codestream bytes at which each level is first complete, finest first:
/// `boundaries[r]` for reduction `r`. Measured as the module doc says.
const BOUNDARIES: &[(&str, &[usize])] = &[
    (RAP, &[91_647, 27_071, 8_045, 2_440, 798, 314]),
    (LATLON, &[1_034, 409, 224, 166, 139]),
    ("jpeg2000_local_40000.grib2", &[1_034, 409, 224, 166, 139]),
];

/// A prefix cut at each level's boundary gives that level of the whole
/// message, whether it was asked for that level or a finer one. Where the whole
/// message refuses the level (the 16 × 31 grid has one column left at
/// reduction 4, and no spacing), the prefix refuses it with the same words.
#[test]
fn a_prefix_cut_at_a_level_boundary_is_that_level_of_the_whole_message() {
    for (fixture, boundaries) in BOUNDARIES {
        let bytes = read(fixture);
        let whole = Grib2Reader::from_bytes(bytes.clone()).expect("the fixture parses");
        for (level, &n) in boundaries.iter().enumerate() {
            let level = level as u8;
            let want = whole.decode_message_raster_with(0, DecodeOptions::new(level));
            for asked in 0..=level {
                let what = format!("{fixture}: {n} bytes asked for reduction {asked}");
                let got = display(&bytes, n, asked);
                match (&got, &want) {
                    (Ok(got), Ok(want)) => assert_same(got, want, &what),
                    (Err(got), Err(want)) => {
                        assert!(
                            matches!(got, FieldglassError::UnsupportedSection(_)),
                            "{what}: {got:?}"
                        );
                        assert_eq!(got.to_string(), want.to_string(), "{what}");
                    }
                    _ => panic!("{what}: {got:?} where the whole message gives {want:?}"),
                }
            }
        }
    }
}

/// One byte short of a level's boundary is the level below, and the raster's
/// `reduction` says which level it is. Short of the coarsest level there is
/// nothing to draw yet, and the refusal says why.
#[test]
fn a_prefix_one_byte_short_is_the_level_below() {
    let bytes = read(RAP);
    let whole = Grib2Reader::from_bytes(bytes.clone()).expect("RAP parses");
    let boundaries = BOUNDARIES[0].1;
    let coarsest = (boundaries.len() - 1) as u8;
    for (level, &n) in boundaries.iter().enumerate() {
        let level = level as u8;
        let got = display(&bytes, n - 1, level);
        if level == coarsest {
            let err = got.expect_err("no level is whole yet");
            assert!(
                matches!(err, FieldglassError::UnsupportedSection(_))
                    && err
                        .to_string()
                        .contains("every packet of the coarsest resolution"),
                "{err:?}"
            );
            continue;
        }
        let got = got.unwrap_or_else(|e| panic!("{} bytes: {e:?}", n - 1));
        assert_eq!(
            got.reduction(),
            level + 1,
            "{} bytes asked for reduction {level}",
            n - 1
        );
        let want = whole
            .decode_message_raster_with(0, DecodeOptions::new(level + 1))
            .expect("the whole message at the level below");
        assert_same(&got, &want, &format!("{} bytes", n - 1));
    }
}

/// A level coarser than the bytes reach is served as asked: a prefix is never
/// decoded finer than the caller wants.
#[test]
fn a_coarser_level_than_the_bytes_reach_is_the_one_asked_for() {
    let bytes = read(RAP);
    let whole = Grib2Reader::from_bytes(bytes.clone()).expect("RAP parses");
    let got = display(&bytes, BOUNDARIES[0].1[2], 4).expect("reduction 2 is whole");
    let want = whole
        .decode_message_raster_with(0, DecodeOptions::new(4))
        .expect("the whole message at reduction 4");
    assert_same(&got, &want, "reduction 4 from a reduction 2 prefix");
}

/// Bytes that hold the whole message are today's display decode of it, for
/// every packing and at every level, refusals included: the trailing `7777`
/// and anything after the message change nothing.
#[test]
fn the_whole_message_is_todays_display_decode() {
    for (fixture, reduction) in [
        (RAP, 0u8),
        (RAP, 2),
        (LATLON, 1),
        // Simple packing: reduction 0 serves it, as it serves every packing,
        // and reduction 1 is refused.
        ("regular_latlon_surface.grib2", 0),
        ("regular_latlon_surface.grib2", 1),
    ] {
        let bytes = read(fixture);
        let want = Grib2Reader::from_bytes(bytes.clone())
            .expect("parses")
            .decode_message_raster_with(0, DecodeOptions::new(reduction));
        let mut padded = bytes.clone();
        padded.extend_from_slice(b"trailing bytes after the message");
        for bytes in [bytes, padded] {
            let got = prefix(bytes).decode_display(DecodeOptions::new(reduction));
            match (&got, &want) {
                (Ok(got), Ok(want)) => assert_same(got, want, fixture),
                (Err(got), Err(want)) => assert_eq!(got.to_string(), want.to_string()),
                _ => panic!("{fixture} at {reduction}: {got:?} against {want:?}"),
            }
        }
    }
}

/// A refusal of a partial message: `UnsupportedSection`, naming `expected`, at
/// reduction 0 and 1 alike. Reduction 0 is refused too because a level of a
/// partial §7 is a reduced decode, whatever was asked.
fn assert_refused(bytes: Vec<u8>, expected: &str) {
    let p = prefix(bytes);
    for reduction in [0u8, 1] {
        let err = p
            .decode_display(DecodeOptions::new(reduction))
            .expect_err("refused");
        assert!(
            matches!(err, FieldglassError::UnsupportedSection(_)),
            "reduction {reduction}: {err:?}"
        );
        assert!(
            err.to_string().contains(expected),
            "reduction {reduction}: expected {expected:?} in {err}"
        );
    }
}

/// Only §5.40 has a pyramid a prefix holds part of. Simple packing, cut a few
/// bytes short of its end.
#[test]
fn a_packing_other_than_jpeg2000_is_refused() {
    let bytes = read("regular_latlon_surface.grib2");
    let short = bytes[..bytes.len() - 10].to_vec();
    assert_refused(short, "only JPEG 2000");
}

/// A §6 bitmap has no low-pass. Built as `decode_reduced.rs` builds it: the
/// 16 × 31 fixture with an all-present bitmap in place of its empty §6.
#[test]
fn a_bitmap_is_refused() {
    let bytes = read(LATLON);
    // 496 points, so 62 whole octets and no padding bits.
    let points = 16 * 31;
    let mut bitmapped = bytes[..16].to_vec();
    for (n, start, len) in sections(&bytes) {
        if n == 6 {
            assert_eq!(bytes[start + 5], 255, "the fixture has no bitmap");
            let bitmap = vec![0xffu8; points / 8];
            bitmapped.extend_from_slice(&((6 + bitmap.len()) as u32).to_be_bytes());
            bitmapped.extend_from_slice(&[6, 0]);
            bitmapped.extend_from_slice(&bitmap);
        } else {
            bitmapped.extend_from_slice(&bytes[start..start + len]);
        }
    }
    bitmapped.extend_from_slice(b"7777");
    let total = bitmapped.len() as u64;
    bitmapped[8..16].copy_from_slice(&total.to_be_bytes());
    // The rebuilt message is valid: whole, it still decodes.
    Grib2Reader::from_bytes(bitmapped.clone())
        .expect("parses")
        .decode_message_raster(0)
        .expect("decodes");
    assert_refused(cut(&bitmapped, 500), "§6 declares a bitmap");
}

/// A reduced grid's rows differ in width. A reduced Gaussian message given the
/// RAP fixture's §5 to §7: no file pairs the two.
#[test]
fn a_reduced_grid_is_refused() {
    let spliced = splice(&read("reduced_gaussian_pressure_level.grib2"), &read(RAP));
    let p = prefix(cut(&spliced, 5_000));
    assert!(p.message().gds.points_per_row().is_some(), "a reduced grid");
    assert_refused(cut(&spliced, 5_000), "reduced grid");
}

/// A Gaussian grid has no coarse geometry, at whatever level the bytes serve.
/// A regular Gaussian message given the RAP fixture's §5 to §7, cut where only
/// reduction 5 is whole.
#[test]
fn a_gaussian_grid_is_refused() {
    let spliced = splice(&read("regular_gaussian_f32.grib2"), &read(RAP));
    assert_refused(
        cut(&spliced, 400),
        "no derivable geometry on a gaussian grid",
    );
    // Refused even when the bytes hold the whole field and only the end
    // marker is missing, so a partial message's answer does not hang on how
    // far it has come.
    assert_refused(
        spliced[..spliced.len() - 1].to_vec(),
        "no derivable geometry on a gaussian grid",
    );
}

/// Alternate-row and `j`-consecutive scanning are undone after the packing
/// decodes. Set in the 16 × 31 fixture's §3 (template 3.0, scanning mode at
/// octet 72).
#[test]
fn an_exotic_scan_order_is_refused() {
    for (flag, expected) in [(0x10u8, "alternate-row"), (0x20, "j-consecutive")] {
        let mut bytes = read(LATLON);
        let (_, gds, _) = *sections(&bytes)
            .iter()
            .find(|(n, _, _)| *n == 3)
            .expect("a §3");
        bytes[gds + 71] |= flag;
        let short = cut(&bytes, 500);
        let scan = prefix(short.clone()).message().gds.scanning_mode();
        assert_eq!(scan.map(|s| s & flag), Some(flag), "the flag is set");
        assert_refused(short, expected);
    }
}

/// A codestream whose SIZ origin is not zero has no coarse raster. The RAP
/// codestream with its origin moved to 1 and its extent and tile grown to
/// match, as `display_field.rs` patches it, cut where reduction 1 is not yet
/// whole: the level served is coarse, so it is refused.
#[test]
fn a_non_zero_image_origin_is_refused_at_a_coarse_level() {
    let mut bytes = read(RAP);
    let cs = codestream_start(&bytes);
    for at in [8, 24] {
        let at = cs + at;
        let v = u32::from_be_bytes(bytes[at..at + 4].try_into().expect("4 bytes")) + 1;
        bytes[at..at + 4].copy_from_slice(&v.to_be_bytes());
    }
    bytes[cs + 16..cs + 20].copy_from_slice(&1u32.to_be_bytes());
    assert_refused(cut(&bytes, 20_000), "image origin");
}

/// A level past the pyramid is refused as the whole message refuses it.
#[test]
fn a_level_past_the_pyramid_is_refused() {
    let bytes = read(RAP);
    let want = Grib2Reader::from_bytes(bytes.clone())
        .expect("parses")
        .decode_message_raster_with(0, DecodeOptions::new(6))
        .expect_err("the whole message refuses it too");
    // With most of the codestream, and with too little for any level: more
    // bytes never answer this one, so it is not the refusal that waits.
    for n in [20_000, 200] {
        let err = display(&bytes, n, 6).expect_err("six levels of six resolutions");
        assert!(
            matches!(err, FieldglassError::UnsupportedSection(_)),
            "{err:?}"
        );
        assert_eq!(err.to_string(), want.to_string(), "{n} bytes");
    }
}

/// A layout with no raster has no coarse one either: spherical-harmonic
/// coefficients and a HEALPix pixel list, each cut short.
#[test]
fn a_layout_with_no_raster_is_refused() {
    for fixture in ["spectral_complex_t63.grib2", "healpix_n4_ring.grib2"] {
        let bytes = read(fixture);
        assert_refused(bytes[..bytes.len() - 10].to_vec(), "no coarse one either");
    }
}

/// A prefix that serves no level yet is refused, and says what it lacks: it
/// ends inside §7's header, inside the codestream's main header, or before the
/// coarsest resolution is whole.
#[test]
fn a_prefix_that_serves_no_level_yet_is_refused() {
    let bytes = read(RAP);
    let ds = codestream_start(&bytes) - 5;
    assert_refused(bytes[..ds + 3].to_vec(), "inside §7's header");
    assert_refused(cut(&bytes, 0), "do not reach the codestream's SOC marker");
    assert_refused(cut(&bytes, 60), "inside the codestream's main header");
    assert_refused(cut(&bytes, 313), "every packet of the coarsest resolution");
}

/// What is not a prefix of a valid message is a parse error, not a refusal:
/// bytes that end inside §3, and a §7 that is not a codestream.
#[test]
fn a_damaged_message_is_a_parse_error() {
    let bytes = read(RAP);
    let (_, gds, _) = *sections(&bytes)
        .iter()
        .find(|(n, _, _)| *n == 3)
        .expect("a §3");
    let err = MessagePrefix::parse(bytes[..gds + 20].to_vec()).expect_err("§3 is cut");
    assert!(matches!(err, FieldglassError::Parse(_)), "{err:?}");

    let mut not_soc = cut(&bytes, 400);
    not_soc[codestream_start(&bytes)] = 0;
    let err = prefix(not_soc)
        .decode_display(DecodeOptions::new(1))
        .expect_err("no SOC");
    assert!(
        matches!(err, FieldglassError::Parse(_)) && err.to_string().contains("SOC"),
        "{err:?}"
    );
}
