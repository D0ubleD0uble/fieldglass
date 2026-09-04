//! Characterisation golden for the colour path.
//!
//! [`Palette`] (#485) extracted the 256-entry lookup table and the
//! normalisation rule out of `paint_grid_rgba` so a GPU host can read them as
//! data. The extraction is meant to be invisible: these hashes were captured
//! from the painter *before* it existed, on the commit this test was written
//! against, and pin every colormap × both scales × reversed × flipped against
//! that recording rather than against the refactored code's own output.
//!
//! A failure here means the painted bytes moved. That is a behaviour change,
//! not a test to re-baseline — regenerate the table only alongside a deliberate,
//! documented change to the colours or the scale rule.

// The colour path is `render`-gated, and the format crates build `core` with
// default features off. Without this the file would fail to compile in that
// configuration, which `cargo clippy -p fieldglass-core --no-default-features`
// checks on every commit. It sits *below* the crate doc: with `render` off the
// `cfg` strips everything under it, and a crate doc stripped along with the
// rest reads to `missing_docs` as a crate carrying none.
#![cfg(feature = "render")]

use fieldglass_core::colormap::{
    PALETTE_LUT_LEN, Palette, ScaleMode, colormaps, paint_grid_rgba, scale_position,
};

const W: u32 = 37;
const H: u32 = 23;

/// Hashes of the pre-`Palette` painter's output, keyed by
/// `(colormap, scale)` and folding the four `reversed` × `flip_y` combinations.
const GOLDEN: &[(&str, ScaleMode, u64)] = &[
    ("viridis", ScaleMode::Linear, 0x151e93cd18ff583b),
    ("viridis", ScaleMode::Log10, 0x68a2b320a25632d9),
    ("plasma", ScaleMode::Linear, 0x25e8415b809e6433),
    ("plasma", ScaleMode::Log10, 0xd07614aeae86645b),
    ("cividis", ScaleMode::Linear, 0x7a96534339b97e39),
    ("cividis", ScaleMode::Log10, 0x7f359f91c5970321),
    ("turbo", ScaleMode::Linear, 0x299b69ab0e87f921),
    ("turbo", ScaleMode::Log10, 0xe93ebac1ff323787),
    ("grayscale", ScaleMode::Linear, 0x2e725afb6a4c47c3),
    ("grayscale", ScaleMode::Log10, 0xf483329dd9716adf),
    ("rdbu", ScaleMode::Linear, 0x56764cf4a5c348f7),
    ("rdbu", ScaleMode::Log10, 0xfe39a5ea6858fc23),
    ("brbg", ScaleMode::Linear, 0xd66f60f76e1d50ef),
    ("brbg", ScaleMode::Log10, 0xae0970038a85a0b7),
    ("coolwarm", ScaleMode::Linear, 0x0b306e8f1eeb569d),
    ("coolwarm", ScaleMode::Log10, 0xcf3ce900c0d5afbd),
];

/// The display range each scale is exercised over. The linear minimum is below
/// the field's floor and the log10 minimum above it, so both a clamped low end
/// and a dropped-to-missing low end are covered.
const RANGES: [(ScaleMode, f64, f64); 2] = [
    (ScaleMode::Linear, -5.0, 1250.0),
    (ScaleMode::Log10, 0.5, 1250.0),
];

/// A fixed field carrying every case the painter branches on: ordinary positive
/// values spanning three decades, a `NaN`, negative values, an exact zero, and
/// masked cells that fall on none of those.
fn field() -> (Vec<f64>, Vec<u8>) {
    let n = (W * H) as usize;
    let mut values = Vec::with_capacity(n);
    let mut mask = Vec::with_capacity(n);
    // A plain LCG rather than a dependency: the field has to be identical on
    // every platform and every run for the hashes to mean anything.
    let mut s: u64 = 0x2545F491_4F6CDD1D;
    for i in 0..n {
        s = s
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let u = ((s >> 11) as f64) / ((1u64 << 53) as f64);
        values.push(match i % 17 {
            3 => f64::NAN,
            7 => -2.5 + u,
            11 => 0.0,
            _ => 0.25 + u * 1200.0,
        });
        mask.push(if i % 13 == 5 { 0 } else { 1 });
    }
    (values, mask)
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

#[test]
fn painted_bytes_match_the_pre_palette_golden() {
    let (values, mask) = field();
    let mut checked = 0;
    for cm in colormaps() {
        for (scale, min, max) in RANGES {
            let mut h: u64 = 0xcbf29ce484222325;
            for reversed in [false, true] {
                for flip in [false, true] {
                    let rgba = paint_grid_rgba(
                        &values,
                        Some(&mask),
                        W,
                        H,
                        min,
                        max,
                        flip,
                        cm,
                        reversed,
                        scale,
                    );
                    assert_eq!(rgba.len(), (W * H * 4) as usize);
                    h ^= fnv1a(&rgba);
                    h = h.wrapping_mul(0x100000001b3);
                }
            }
            let want = GOLDEN
                .iter()
                .find(|(n, s, _)| *n == cm.name() && *s == scale)
                .unwrap_or_else(|| panic!("no golden for {} / {}", cm.name(), scale.as_str()))
                .2;
            assert_eq!(
                h,
                want,
                "{} / {} painted differently than the pre-Palette recording",
                cm.name(),
                scale.as_str()
            );
            checked += 1;
        }
    }
    // A colormap added without a golden row would otherwise pass silently.
    assert_eq!(checked, GOLDEN.len(), "every golden row must be exercised");
}

#[test]
fn palette_paint_is_the_wrapper_it_replaced() {
    let (values, mask) = field();
    for cm in colormaps() {
        for (scale, min, max) in RANGES {
            for reversed in [false, true] {
                for flip in [false, true] {
                    let direct = Palette::build(cm, reversed, min, max, scale).paint(
                        &values,
                        Some(&mask),
                        W,
                        H,
                        flip,
                    );
                    let wrapped = paint_grid_rgba(
                        &values,
                        Some(&mask),
                        W,
                        H,
                        min,
                        max,
                        flip,
                        cm,
                        reversed,
                        scale,
                    );
                    assert_eq!(direct, wrapped, "{} / {}", cm.name(), scale.as_str());
                }
            }
        }
    }
}

#[test]
fn index_and_paint_agree_entry_for_entry() {
    let (values, _) = field();
    for cm in colormaps() {
        for (scale, min, max) in RANGES {
            for reversed in [false, true] {
                let pal = Palette::build(cm, reversed, min, max, scale);
                // One row, so pixel i is value i and flipping is a no-op.
                let rgba = pal.paint(&values, None, values.len() as u32, 1, false);
                for (i, &v) in values.iter().enumerate() {
                    let want = match pal.index(v) {
                        Some(idx) => {
                            let o = idx as usize * 4;
                            &pal.lut[o..o + 4]
                        }
                        None => &pal.masked_rgba[..],
                    };
                    assert_eq!(
                        &rgba[i * 4..i * 4 + 4],
                        want,
                        "{} / {} value {v} at {i}",
                        cm.name(),
                        scale.as_str()
                    );
                }
            }
        }
    }
}

#[test]
fn the_palette_lut_is_the_colormap_lut_widened() {
    for cm in colormaps() {
        for reversed in [false, true] {
            let rgb = cm.lut(reversed);
            let pal = Palette::build(cm, reversed, 0.0, 1.0, ScaleMode::Linear);
            for i in 0..256 {
                assert_eq!(
                    &pal.lut[i * 4..i * 4 + 3],
                    &rgb[i * 3..i * 3 + 3],
                    "{} entry {i}",
                    cm.name()
                );
                assert_eq!(pal.lut[i * 4 + 3], 255, "{} entry {i} alpha", cm.name());
                // The same entry a per-sample caller gets, so the legend strip,
                // a point sample, and the painted grid share one ramp.
                assert_eq!(
                    &pal.lut[i * 4..i * 4 + 3],
                    &cm.sample(i as f64 / 255.0, reversed)[..],
                    "{} sample {i}",
                    cm.name()
                );
            }
        }
    }
}

#[test]
fn normalise_takes_the_logarithm_once_and_still_agrees_with_scale_position() {
    let (values, _) = field();
    for (scale, min, max) in RANGES {
        let pal = Palette::build(&colormaps()[0], false, min, max, scale);
        for &v in &values {
            let free = scale_position(v, min, max, scale);
            match (pal.normalise(v), free) {
                (None, None) => {}
                (Some(t), Some(want)) => {
                    // `normalise` is the shader-facing f32 narrowing of the same
                    // rule, so it agrees to f32 precision, not to the bit.
                    assert!(
                        (t as f64 - want).abs() <= 1e-6,
                        "{} v={v}: {t} vs {want}",
                        scale.as_str()
                    );
                }
                (a, b) => panic!("{} v={v}: {a:?} vs {b:?}", scale.as_str()),
            }
        }
    }
}

#[test]
fn a_palette_survives_a_json_round_trip() {
    let pal = Palette::build(&colormaps()[3], true, 0.5, 1250.0, ScaleMode::Log10);
    let json = serde_json::to_string(&pal).expect("serialise");
    let back: Palette = serde_json::from_str(&json).expect("deserialise");
    assert_eq!(pal, back);
    // The transformed domain is what crosses, not the caller's display range.
    assert!(json.contains(&format!("\"t0\":{}", 0.5f64.log10())));
    assert!(json.contains("\"scale\":\"log10\""));
}

#[test]
fn a_wrong_length_lookup_table_is_rejected() {
    let pal = Palette::build(&colormaps()[0], false, 0.0, 1.0, ScaleMode::Linear);
    let mut doc: serde_json::Value =
        serde_json::from_str(&serde_json::to_string(&pal).expect("serialise")).expect("parse");
    let full = doc["lut"].as_array().expect("lut is an array").clone();
    assert_eq!(full.len(), PALETTE_LUT_LEN);

    // One entry short and one entry long. Either would paint colours the sender
    // did not send, so neither may be accepted.
    for len in [PALETTE_LUT_LEN - 1, PALETTE_LUT_LEN + 1] {
        let mut lut = full.clone();
        lut.resize(len, serde_json::json!(0));
        doc["lut"] = serde_json::Value::Array(lut);
        assert!(
            serde_json::from_value::<Palette>(doc.clone()).is_err(),
            "a {len}-byte lookup table deserialised"
        );
    }
}

/// The doc on [`Palette::normalise`] promises a GPU host that computing the
/// same two lines in `f32` costs it at most one lookup-table entry. That is a
/// tolerance a shader is checked against, so it is pinned here rather than
/// asserted in prose: the CPU painter is the oracle, and this is how far the
/// oracle allows the shader to be.
#[test]
fn the_f32_normalise_stays_within_one_entry_of_the_painted_index() {
    let pal = Palette::build(&colormaps()[0], false, 0.0, 1.0, ScaleMode::Linear);
    let mut differed = 0u64;
    let mut s: u64 = 0x9E3779B97F4A7C15;
    const N: u64 = 4_000_000;
    for _ in 0..N {
        s = s
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let v = ((s >> 11) as f64) / ((1u64 << 53) as f64);
        let painted = pal.index(v).expect("a finite value in range has a colour");
        let shader = (pal.normalise(v).expect("likewise") * 255.0).round() as i32;
        let delta = (shader - painted as i32).abs();
        assert!(delta <= 1, "v={v}: shader {shader} vs painted {painted}");
        differed += u64::from(delta != 0);
    }
    // Both bounds matter. A rate of zero would mean the two paths had silently
    // become one and the promise no longer needs stating; a large one would
    // mean `normalise` is not the same rule at all.
    assert!(
        differed > 0 && differed < N / 10_000,
        "{differed} of {N} positions differed ({:.2e} per position)",
        differed as f64 / N as f64
    );
}

/// The dimensions reach the painter from a file's grid description, and on
/// wasm32 — the host `Palette` exists for — `usize` is 32 bits, so their
/// product need not fit. Sizing the buffer must fail closed; wrapping would
/// allocate a short one and then index off the end of it.
#[test]
fn a_grid_too_large_to_address_paints_nothing() {
    let pal = Palette::build(&colormaps()[0], false, 0.0, 1.0, ScaleMode::Linear);
    // Unaddressable on any target: the byte count passes `u64`.
    let mut cases = vec![(u32::MAX, u32::MAX), (0, 10), (10, 0)];
    // Addressable on a 64-bit host, so only a 32-bit one refuses it — which is
    // the case that matters, since wasm32 is where this runs.
    if usize::BITS == 32 {
        cases.push((u32::MAX, 5));
        cases.push((70_000, 70_000));
    }
    for (w, h) in cases {
        assert!(
            pal.paint(&[], None, w, h, false).is_empty(),
            "{w}x{h} produced a buffer"
        );
    }
}

/// `normalise` is within one lookup entry of `index` because the arithmetic is
/// still `f64` and only the result is narrowed. A shader that does the
/// subtraction and division in `f32` has no such bound — it loses the *domain*
/// once the `f32` gap at `t0` passes one lookup step. The doc on `normalise`
/// tells a GPU host to rebase (`v - t0`) rather than upload raw values against
/// raw bounds; this is the measurement that instruction rests on, kept so it is
/// not folklore.
#[test]
fn f32_arithmetic_loses_the_domain_where_the_docs_say_it_does() {
    // (t0, t1, whether f32 arithmetic should still hold to one entry)
    let cases = [
        (0.0f64, 1.0f64, true),
        (250.0, 320.0, true),         // temperature, K
        (95_000.0, 105_000.0, true),  // surface pressure, Pa
        (5.0e4, 5.0e4 + 100.0, true), // geopotential, m²/s², ordinary range
        (5.0e4, 5.0e4 + 0.01, false), // …under a tight manual range
        (1.0e7, 1.0e7 + 1.0, false),
    ];
    for (t0, t1, expect_tight) in cases {
        let pal = Palette::build(&colormaps()[0], false, t0, t1, ScaleMode::Linear);
        let mut worst = 0i32;
        let mut s: u64 = 0x9E3779B97F4A7C15;
        for _ in 0..200_000 {
            s = s
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let u = ((s >> 11) as f64) / ((1u64 << 53) as f64);
            let v = t0 + u * (t1 - t0);
            let painted = i32::from(pal.index(v).expect("in-range value has a colour"));
            // Exactly what a naive shader computes: everything in f32.
            let t = ((v as f32) - (t0 as f32)) / ((t1 as f32) - (t0 as f32));
            let shader = (t.clamp(0.0, 1.0) * 255.0).round() as i32;
            worst = worst.max((shader - painted).abs());
        }
        // The threshold the doc states: one lookup step against the f32 gap.
        let resolvable = (t1 - t0) >= 255.0 * t0.abs() * 2f64.powi(-23);
        assert_eq!(
            resolvable, expect_tight,
            "domain [{t0}, {t1}] classified wrong by the stated rule"
        );
        if expect_tight {
            assert!(
                worst <= 1,
                "[{t0}, {t1}]: f32 arithmetic was {worst} entries out"
            );
        } else {
            assert!(
                worst > 1,
                "[{t0}, {t1}]: f32 arithmetic held after all — restate the rule"
            );
        }
    }
}
