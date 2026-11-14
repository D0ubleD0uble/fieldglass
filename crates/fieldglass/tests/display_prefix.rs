//! `Session::decode_display_prefix`: a field to draw from a GRIB2 message of
//! which only the start has arrived (#1785).
//!
//! `fieldglass-grib2`'s `decode_prefix.rs` checks the reader against the
//! whole message at every level boundary and reaches every refusal. This is
//! the host half: the field a prefix gives is the `DisplayField` that
//! `decode_display` gives for the whole message at that level, a refusal is
//! `unsupported` and a damaged message is `decode`, and the field draws.

use fieldglass::{DisplayDecodeOptions, DisplayField, Dtype, Error, PaletteOptions, Session};

const RAP: &str = "../fieldglass-grib2/tests/fixtures/rap_jpeg2000_lambert.grib2";
const SIMPLE: &str = "../fieldglass-grib2/tests/fixtures/regular_latlon_surface.grib2";
const GRIB1: &str = "../fieldglass-grib1/tests/fixtures/cmc_wind_300_2010052400_p012.grib";
const SPECTRAL: &str = "../fieldglass-grib2/tests/fixtures/spectral_complex_t63.grib2";

/// Where RAP's codestream starts: §0 to §6 are 181 bytes, and §7's header 5.
const RAP_CODESTREAM: usize = 186;
/// Codestream bytes at which RAP's reductions 2 and 1 are first whole.
const RAP_LEVEL_2: usize = 8_045;
const RAP_LEVEL_1: usize = 27_071;

fn read(path: &str) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn options(reduction: u8) -> DisplayDecodeOptions {
    let mut options = DisplayDecodeOptions::new(reduction);
    options.dtype = Dtype::F64;
    options
}

fn prefix(bytes: &[u8], codestream: usize, reduction: u8) -> Result<DisplayField, Error> {
    Session::decode_display_prefix(
        bytes[..RAP_CODESTREAM + codestream].to_vec(),
        &options(reduction),
    )
}

fn json(field: &DisplayField) -> serde_json::Value {
    serde_json::to_value(field).expect("a display field serializes")
}

/// The whole message's display field at `reduction`.
fn whole(bytes: &[u8], reduction: u8) -> DisplayField {
    Session::open(bytes.to_vec())
        .expect("RAP opens")
        .decode_display(0, &options(reduction))
        .expect("the whole message decodes")
}

/// The fixture is what the constants say: §7 starts where they put it.
#[test]
fn the_rap_codestream_starts_where_the_constants_say() {
    let bytes = read(RAP);
    assert_eq!(bytes[RAP_CODESTREAM - 1], 7, "§7's section number");
    assert_eq!(&bytes[RAP_CODESTREAM..RAP_CODESTREAM + 2], &[0xFF, 0x4F]);
}

/// A prefix at a level's boundary is the whole message's field at that level,
/// labels and placement included; one byte short is the level below, and its
/// `reduction` says so.
#[test]
fn a_prefix_is_the_whole_messages_field_at_the_level_it_reaches() {
    let bytes = read(RAP);
    let at_1 = prefix(&bytes, RAP_LEVEL_1, 1).expect("reduction 1 is whole");
    assert_eq!(at_1.reduction, 1);
    assert_eq!(json(&at_1), json(&whole(&bytes, 1)));

    let short = prefix(&bytes, RAP_LEVEL_1 - 1, 1).expect("reduction 2 is whole");
    assert_eq!(short.reduction, 2, "the level below, and the field says so");
    assert_eq!(json(&short), json(&whole(&bytes, 2)));

    // Asked for the full field, a prefix gives the finest level it holds.
    let finest = prefix(&bytes, RAP_LEVEL_2, 0).expect("reduction 2 is whole");
    assert_eq!(finest.reduction, 2);
    assert_eq!(json(&finest), json(&whole(&bytes, 2)));
}

/// Bytes that hold the whole message are `decode_display` of it, for any
/// message a session opens by offset: reduction 0 of simple packing, a
/// spectral message, which has no raster to decode, and a GRIB1 message,
/// which has no pyramid.
#[test]
fn the_whole_message_is_decode_display() {
    for path in [RAP, SIMPLE, SPECTRAL, GRIB1] {
        let bytes = read(path);
        let got = Session::decode_display_prefix(bytes.clone(), &options(0))
            .unwrap_or_else(|e| panic!("{path}: {e:?}"));
        let want = Session::open_message_at(bytes.clone(), 0)
            .expect("opens")
            .decode_display(0, &options(0))
            .expect("decodes");
        assert_eq!(json(&got), json(&want), "{path}");
        if path != RAP {
            let err = Session::decode_display_prefix(bytes, &options(1)).expect_err("no pyramid");
            assert_eq!(err.code(), "unsupported", "{path}: {err:?}");
        }
    }
}

/// A partial message that has no coarse field is `unsupported` at every
/// reduction, so a host waits for the whole message; so is one whose bytes
/// serve no level yet. A message that does not parse is `decode`.
#[test]
fn a_refusal_is_unsupported_and_damage_is_decode() {
    let simple = read(SIMPLE);
    for reduction in [0, 1] {
        let err = Session::decode_display_prefix(
            simple[..simple.len() - 10].to_vec(),
            &options(reduction),
        )
        .expect_err("simple packing has no pyramid");
        assert_eq!(err.code(), "unsupported", "{err:?}");
        assert!(err.message().contains("only JPEG 2000"), "{err:?}");
    }

    let rap = read(RAP);
    let err = prefix(&rap, 313, 0).expect_err("no level is whole yet");
    assert_eq!(err.code(), "unsupported", "{err:?}");
    assert!(err.message().contains("coarsest resolution"), "{err:?}");

    // Cut inside §3.
    let err =
        Session::decode_display_prefix(rap[..60].to_vec(), &options(1)).expect_err("§3 is cut");
    assert_eq!(err.code(), "decode", "{err:?}");
    // Not a message at all.
    let err = Session::decode_display_prefix(b"not a message".to_vec(), &options(1))
        .expect_err("not GRIB");
    assert_ne!(err.code(), "unsupported", "{err:?}");
}

/// A prefix's field draws: any session's `render_display` paints it over the
/// full field's range, at the coarse size.
#[test]
fn a_prefix_field_renders() {
    let bytes = read(RAP);
    let session = Session::open(bytes.clone()).expect("RAP opens");
    let stats = session
        .decode(0, &fieldglass::DecodeOptions::new(Dtype::Auto))
        .expect("full decode")
        .stats;
    let field = prefix(&bytes, RAP_LEVEL_2, 2).expect("reduction 2");
    let raster = session
        .render_display(&field, Some(&stats), &PaletteOptions::default(), false)
        .expect("renders");
    assert_eq!((raster.width, raster.height), (field.ni, field.nj));
}
