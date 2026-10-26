//! A field decoded to draw, at a reduced resolution, through `Session` (#1082).
//!
//! `fieldglass-grib2` has decoded a JPEG 2000 message at a coarser wavelet
//! level since #463, typed so that nothing that measures accepts it. This is the
//! host half: `Session::decode_display` returns a `DisplayField`, which
//! `render_display`, `warp_display` and `palette_display` draw and which
//! `probe`, `contours` and `combine` do not accept. That last part is a compile
//! error, shown by the `compile_fail` block on `Session::decode_display`; what
//! is checked here is what the drawing calls do with one.
//!
//! The geometry is checked through a warp rather than only by comparing two
//! `GridGeometry` values: a coarse field placed on the message's own grid would
//! cover a quarter of the area it should, and the warp onto one window at one
//! size is where that shows.

use fieldglass::{
    DecodeOptions, DisplayDecodeOptions, Dtype, Field, PaletteOptions, Session, Stats, WarpOptions,
};

const RAP: &str = "../fieldglass-grib2/tests/fixtures/rap_jpeg2000_lambert.grib2";
const J2K_LATLON: &str = "../fieldglass-grib2/tests/fixtures/jpeg2000_regular_latlon.grib2";
const SIMPLE: &str = "../fieldglass-grib2/tests/fixtures/regular_latlon_surface.grib2";
const GRIB1: &str = "../fieldglass-grib1/tests/fixtures/cmc_wind_300_2010052400_p012.grib";
const NETCDF: &str = "../fieldglass-netcdf/tests/fixtures/cf_packed_data.nc";

fn open(path: &str) -> Session {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    Session::open(bytes).unwrap_or_else(|e| panic!("{path} opens: {e:?}"))
}

fn full(session: &Session) -> Field {
    session
        .decode(0, &DecodeOptions::new(Dtype::Auto))
        .expect("the full field decodes")
}

/// The present values' mean and count.
fn present_mean(values: &[f64], mask: &[u8]) -> (f64, usize) {
    let present: Vec<f64> = values
        .iter()
        .zip(mask)
        .filter(|&(_, &m)| m == 1)
        .map(|(v, _)| *v)
        .collect();
    let n = present.len();
    (present.iter().sum::<f64>() / n as f64, n)
}

/// The acceptance case: the RAP message at reduction 1 is half the size on
/// each axis, on the grid `GridGeometry::subsampled(1)` derives from the
/// message's own, and keeps the message's labels.
#[test]
fn a_rap_message_decodes_at_reduction_one_onto_the_derived_grid() {
    let session = open(RAP);
    let full = full(&session);
    let coarse = session
        .decode_display(0, &DisplayDecodeOptions::new(1))
        .expect("a JPEG 2000 Lambert message decodes at reduction 1");

    assert_eq!(coarse.reduction, 1);
    assert_eq!(
        (coarse.ni, coarse.nj),
        (full.ni.div_ceil(2), full.nj.div_ceil(2))
    );
    assert_eq!(coarse.values.len(), (coarse.ni * coarse.nj) as usize);
    assert_eq!(coarse.mask.len(), coarse.values.len());
    let derived = full
        .georef
        .geometry
        .subsampled(1)
        .expect("a Lambert grid has a coarse grid");
    assert_eq!(
        coarse.georef.geometry, derived,
        "the derived grid, not the GDS"
    );
    assert_ne!(coarse.georef.geometry, full.georef.geometry);
    assert_eq!(
        (coarse.georef.ni, coarse.georef.nj),
        (Some(coarse.ni), Some(coarse.nj))
    );
    // The same family, scan and labels as the message.
    assert_eq!(coarse.georef.label, full.georef.label);
    assert_eq!(coarse.georef.kind, full.georef.kind);
    assert_eq!(coarse.georef.scan, full.georef.scan);
    assert_eq!(coarse.parameter, full.parameter);
    assert_eq!(coarse.units, full.units);
    assert_eq!(coarse.truncation, None);

    // A low-pass of the field: its mean tracks the full field's.
    let (coarse_mean, _) = present_mean(&coarse.values.to_f64(), &coarse.mask);
    let (full_mean, _) = present_mean(&full.values.to_f64(), &full.mask);
    assert!(
        (coarse_mean - full_mean).abs() < 0.5,
        "coarse mean {coarse_mean} against full mean {full_mean}"
    );
}

/// Reduction zero is `decode`'s field, for a message with a pyramid and for
/// one without, GRIB1 included: a host that picks the reduction from the zoom
/// has one code path.
#[test]
fn reduction_zero_is_the_decoded_field() {
    for path in [RAP, J2K_LATLON, SIMPLE, GRIB1] {
        let session = open(path);
        for dtype in [Dtype::Auto, Dtype::F64] {
            let field = session
                .decode(0, &DecodeOptions::new(dtype.clone()))
                .unwrap_or_else(|e| panic!("{path} decodes: {e:?}"));
            let mut options = DisplayDecodeOptions::new(0);
            options.dtype = dtype;
            let display = session
                .decode_display(0, &options)
                .unwrap_or_else(|e| panic!("{path} decodes to display: {e:?}"));
            assert_eq!(display.reduction, 0, "{path}");
            assert_eq!(display.values, field.values, "{path}");
            assert_eq!(display.mask, field.mask, "{path}");
            assert_eq!((display.ni, display.nj), (field.ni, field.nj), "{path}");
            assert_eq!(display.georef, field.georef, "{path}");
            assert_eq!(display.parameter, field.parameter, "{path}");
            assert_eq!(display.units, field.units, "{path}");
            assert_eq!(display.truncation, field.truncation, "{path}");
        }
    }
}

/// The coarse field renders onto its own raster and warps onto the same
/// window as the full field, covering the same ground. Placed on the
/// message's own grid it would cover a quarter of it.
#[test]
fn a_coarse_field_renders_and_warps_where_the_full_field_does() {
    let session = open(RAP);
    let full = full(&session);
    let coarse = session
        .decode_display(0, &DisplayDecodeOptions::new(1))
        .expect("decodes at reduction 1");

    let palette = PaletteOptions::new(Some("viridis"), None);
    let raster = session
        .render_display(&coarse, Some(&full.stats), &palette, false)
        .expect("a display field renders");
    assert_eq!((raster.width, raster.height), (coarse.ni, coarse.nj));
    assert_eq!(raster.rgba.len(), (coarse.ni * coarse.nj * 4) as usize);
    let opaque = raster
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|px| px[3] != 0)
        .count();
    assert_eq!(opaque, coarse.mask.iter().filter(|&&m| m == 1).count());

    // One window, one raster size, for both fields: the source's own extent.
    let window = session
        .warp(&full, &WarpOptions::new(false))
        .expect("the full field warps")
        .bounds;
    let mut options = WarpOptions::new(true);
    options.bounds = Some(window);
    options.width = Some(200);
    options.height = Some(150);
    let full_warp = session.warp(&full, &options).expect("full warps");
    let coarse_warp = session
        .warp_display(&coarse, &options)
        .expect("a display field warps");
    assert_eq!((coarse_warp.width, coarse_warp.height), (200, 150));
    assert_eq!(coarse_warp.bounds, full_warp.bounds);

    let as_f64 = |v: &[f32]| v.iter().map(|&x| f64::from(x)).collect::<Vec<_>>();
    let (full_mean, full_n) = present_mean(&as_f64(&full_warp.values), &full_warp.mask);
    let (coarse_mean, coarse_n) = present_mean(&as_f64(&coarse_warp.values), &coarse_warp.mask);
    // The coarse grid's last kept point can fall one fine cell short of the
    // message's corner on each axis, so a few edge pixels differ; a field
    // drawn at half its size would lose three quarters of them.
    let lost = full_n.abs_diff(coarse_n) as f64 / full_n as f64;
    assert!(
        lost < 0.02,
        "{coarse_n} present pixels against {full_n} ({:.1}% apart)",
        lost * 100.0
    );
    assert!(
        (coarse_mean - full_mean).abs() < 0.5,
        "warped mean {coarse_mean} against {full_mean}"
    );

    // With no window or size, the warp is the coarse grid's own shape.
    let own = session
        .warp_display(&coarse, &WarpOptions::new(true))
        .expect("warps onto its own extent");
    assert_eq!((own.width, own.height), (coarse.ni, coarse.nj));
}

/// The automatic range of a display field is the full field's, which the
/// caller supplies, and never the coarse field's own: a wavelet low-pass is
/// not bounded by the field it came from (on this fixture its maximum is
/// 0.69 K above the file's), so the two differ, and the coarse map must color
/// a value as the full map does (#880 rule 6).
#[test]
fn the_automatic_range_is_the_full_fields_not_the_coarse_ones() {
    let session = open(RAP);
    let full = full(&session);
    let coarse = session
        .decode_display(0, &DisplayDecodeOptions::new(1))
        .expect("decodes at reduction 1");

    // The premise: the coarse field's own range is not the file's.
    let own: Vec<f64> = coarse
        .values
        .to_f64()
        .iter()
        .zip(&coarse.mask)
        .filter(|&(_, &m)| m == 1)
        .map(|(v, _)| *v)
        .collect();
    let own_min = own.iter().copied().fold(f64::INFINITY, f64::min);
    let own_max = own.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let (full_min, full_max) = (full.stats.min.unwrap(), full.stats.max.unwrap());
    assert!(
        (own_min, own_max) != (full_min, full_max),
        "the coarse range [{own_min}, {own_max}] is the full one [{full_min}, {full_max}], \
         so this fixture cannot tell the two rules apart"
    );

    let options = PaletteOptions::new(None, None);
    let raster = session
        .render_display(&coarse, Some(&full.stats), &options, false)
        .expect("renders");
    assert_eq!(raster.range.used_min, Some(full_min));
    assert_eq!(raster.range.used_max, Some(full_max));
    // The coarse cells above the file's maximum take the end's color, and the
    // range counts them, as it counts any out-of-range cell.
    let above = own.iter().filter(|&&v| v > full_max).count() as u64;
    assert_eq!(raster.range.above, above);

    let shown = session
        .palette_display(&coarse, Some(&full.stats), &options)
        .expect("a palette");
    let reference = session.palette(&full, &options).expect("the full palette");
    assert_eq!((shown.t0, shown.t1), (reference.t0, reference.t1));
    assert_eq!(shown.lut, reference.lut);

    // One manual end: the other is still the full field's.
    let mut low = PaletteOptions::new(None, None);
    low.min = Some(280.0);
    let raster = session
        .render_display(&coarse, Some(&full.stats), &low, false)
        .expect("renders with one manual end");
    assert_eq!(
        (raster.range.used_min, raster.range.used_max),
        (Some(280.0), Some(full_max))
    );
}

/// What a caller may hand over as the full field's stats, and what is refused.
#[test]
fn the_full_field_stats_are_checked() {
    let session = open(RAP);
    let full = full(&session);
    let coarse = session
        .decode_display(0, &DisplayDecodeOptions::new(1))
        .expect("decodes at reduction 1");
    let auto = PaletteOptions::new(None, None);

    // No stats and an end left to the data: there is nothing to take it from.
    let err = session
        .palette_display(&coarse, None, &auto)
        .expect_err("no stats for an automatic range");
    assert_eq!(err.code(), "invalid_option", "{err:?}");
    let err = session
        .render_display(&coarse, None, &auto, false)
        .expect_err("render takes the same rule");
    assert_eq!(err.code(), "invalid_option", "{err:?}");

    // Both ends manual: no stats needed.
    let mut manual = PaletteOptions::new(None, None);
    manual.min = Some(270.0);
    manual.max = Some(320.0);
    let raster = session
        .render_display(&coarse, None, &manual, false)
        .expect("a manual range needs no stats");
    assert_eq!(
        (raster.range.used_min, raster.range.used_max),
        (Some(270.0), Some(320.0))
    );

    // Stats that are not a range.
    let mut not_finite = full.stats.clone();
    not_finite.min = Some(f64::NAN);
    let mut inverted = full.stats.clone();
    (inverted.min, inverted.max) = (full.stats.max, full.stats.min);
    let mut half = full.stats.clone();
    half.max = None;
    // Stats saying no cell is present, over a field with present cells: it
    // would paint transparent and say nothing.
    let mut empty: Stats = full.stats.clone();
    (empty.min, empty.max, empty.valid_count) = (None, None, 0);
    for (what, stats) in [
        ("a NaN end", not_finite),
        ("an inverted range", inverted),
        ("one end missing", half),
        ("no present cell", empty),
    ] {
        let err = session
            .palette_display(&coarse, Some(&stats), &auto)
            .expect_err(what);
        assert_eq!(err.code(), "invalid_option", "{what}: {err:?}");
        assert!(err.range_refusal().is_none(), "{what}: not a range refusal");
        // Both ends manual reads no data, so the same stats are not refused.
        let palette = session
            .palette_display(&coarse, Some(&stats), &manual)
            .unwrap_or_else(|e| panic!("{what} with a manual range: {e:?}"));
        assert_eq!((palette.t0, palette.t1), (270.0, 320.0), "{what}");
    }
}

/// Every message without a coarse field refuses a non-zero reduction as
/// `unsupported`, so a caller falls back to reduction zero; a variable
/// container is the other addressing mode.
#[test]
fn a_message_without_a_pyramid_refuses_a_reduction() {
    for (what, path) in [("simple packing", SIMPLE), ("GRIB1", GRIB1)] {
        let err = open(path)
            .decode_display(0, &DisplayDecodeOptions::new(1))
            .expect_err(what);
        assert_eq!(err.code(), "unsupported", "{what}: {err:?}");
    }
    let session = open(RAP);
    let err = session
        .decode_display(1, &DisplayDecodeOptions::new(1))
        .expect_err("past the end");
    assert_eq!(err.code(), "no_such_message", "{err:?}");
    // More levels than the codestream has.
    let err = session
        .decode_display(0, &DisplayDecodeOptions::new(30))
        .expect_err("thirty levels");
    assert_eq!(err.code(), "unsupported", "{err:?}");

    let netcdf = open(NETCDF);
    for reduce in [0, 1] {
        let err = netcdf
            .decode_display(0, &DisplayDecodeOptions::new(reduce))
            .expect_err("a variable container");
        assert_eq!(err.code(), "wrong_addressing", "{err:?}");
        assert!(err.message().contains("decode_display"), "{err:?}");
    }
}

/// The options read from a host's JSON: `reduction` and `dtype` both default.
#[test]
fn the_options_read_from_json() {
    let options: DisplayDecodeOptions = serde_json::from_str("{}").expect("empty is the default");
    assert_eq!(options, DisplayDecodeOptions::new(0));
    let options: DisplayDecodeOptions =
        serde_json::from_str(r#"{"reduction":2,"dtype":"f32"}"#).expect("both");
    assert_eq!(options.reduction, 2);
    assert_eq!(options.dtype, Dtype::F32);
}

/// The RAP message with the back half of its §7 codestream overwritten: the
/// sections still frame, and the packets are garbage.
fn corrupt_rap() -> Vec<u8> {
    let mut bytes = std::fs::read(RAP).expect("RAP");
    // GRIB2: a 16-byte §0, then sections of `length (4) | number (1) | …`.
    let mut at = 16;
    loop {
        let len = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
        if bytes[at + 4] == 7 {
            let (start, end) = (at + 5, at + len);
            let half = start + (end - start) / 2;
            bytes[half..end].fill(0xff);
            return bytes;
        }
        at += len;
    }
}

/// A corrupt codestream is a decode failure, not a message that "has no
/// coarse field": `unsupported` tells a caller to ask at reduction 0, which
/// would fail the same way.
#[test]
fn a_corrupt_codestream_is_a_decode_error_not_a_refusal() {
    let session = Session::open(corrupt_rap()).expect("the sections still frame");
    let err = session
        .decode_display(0, &DisplayDecodeOptions::new(1))
        .expect_err("garbage packets");
    assert_eq!(err.code(), "decode", "{err:?}");
    // A level past the pyramid on an intact message is still a refusal.
    let err = open(RAP)
        .decode_display(0, &DisplayDecodeOptions::new(6))
        .expect_err("six levels of a six-resolution codestream");
    assert_eq!(err.code(), "unsupported", "{err:?}");
}

/// The RAP message with `patch` applied to the bytes of its §7 codestream.
fn rap_with_codestream(patch: impl FnOnce(&mut [u8])) -> Vec<u8> {
    let mut bytes = std::fs::read(RAP).expect("RAP");
    let mut at = 16;
    loop {
        let len = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
        if bytes[at + 4] == 7 {
            patch(&mut bytes[at + 5..at + len]);
            return bytes;
        }
        at += len;
    }
}

/// A SIZ image origin that is not zero has no coarse raster (batch review 23):
/// a reduced level of it is `ceil(x1 / 2^r) - ceil(x0 / 2^r)` wide, which is
/// the wrong width for some origins and the odd columns for others. Reduction
/// zero reads it as it always did, so the answer above it is `unsupported`.
#[test]
fn a_non_zero_image_origin_is_unsupported_above_reduction_zero() {
    let bytes = rap_with_codestream(|cs| {
        // SIZ: SOC (2), marker (2), Lsiz (2), Rsiz (2), Xsiz at 8, Ysiz 12,
        // XOsiz 16, YOsiz 20, XTsiz 24. Origin 1, with the extent and tile
        // grown by one so the image stays the same size.
        for at in [8, 24] {
            let v = u32::from_be_bytes(cs[at..at + 4].try_into().unwrap()) + 1;
            cs[at..at + 4].copy_from_slice(&v.to_be_bytes());
        }
        cs[16..20].copy_from_slice(&1u32.to_be_bytes());
    });
    let session = Session::open(bytes).expect("the sections still frame");
    let err = session
        .decode_display(0, &DisplayDecodeOptions::new(1))
        .expect_err("an origin at one");
    assert_eq!(err.code(), "unsupported", "{err:?}");
    assert!(err.message().contains("image origin"), "{err:?}");
    // Reduction zero is not refused on the origin.
    session
        .decode_display(0, &DisplayDecodeOptions::new(0))
        .expect("reduction zero still decodes");
}

/// The codec reports `Unsupported` for more than a reduction past the pyramid
/// (#1182): here the COD's code-block style sets an HTJ2K bit, which it does
/// not decode at any level. Asking at reduction zero is no answer, so the
/// reply is `decode`, as it is at reduction zero, and a level the codestream
/// does carry is not blamed on the level.
#[test]
fn a_codestream_the_codec_refuses_for_another_reason_is_a_decode_error() {
    let bytes = rap_with_codestream(|cs| {
        // Walk the main header's marker segments (SOC, then `marker | Lseg |
        // body`) to the COD, rather than searching for its bytes, which could
        // match inside another segment.
        let mut cod = 2;
        while cs[cod..cod + 2] != [0xff, 0x52] {
            assert_ne!(cs[cod..cod + 2], [0xff, 0x90], "the main header has a COD");
            cod += 2 + usize::from(u16::from_be_bytes([cs[cod + 2], cs[cod + 3]]));
        }
        // Marker (2), Lcod (2), Scod (1), SGcod (4), then NL, xcb, ycb, style.
        cs[cod + 12] |= 0x40;
    });
    let session = Session::open(bytes).expect("the sections still frame");
    for reduce in [0, 1, 5] {
        let err = session
            .decode_display(0, &DisplayDecodeOptions::new(reduce))
            .expect_err("a code-block style the codec does not decode");
        assert_eq!(err.code(), "decode", "reduce {reduce}: {err:?}");
        // The premise: the codec's refusal is the style bit, not a damaged
        // header.
        assert!(err.message().contains("code-block style"), "{err:?}");
    }
    // The same message, intact, still tells a caller the pyramid's depth.
    let err = open(RAP)
        .decode_display(0, &DisplayDecodeOptions::new(6))
        .expect_err("six levels of a five-level pyramid");
    assert_eq!(err.code(), "unsupported", "{err:?}");
    open(RAP)
        .decode_display(0, &DisplayDecodeOptions::new(5))
        .expect("the last level the pyramid carries");
}
