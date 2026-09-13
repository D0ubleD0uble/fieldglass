//! The CPT parser against colour tables people actually distribute (#236).
//!
//! Four files from GMT's `share/cpt`, all MIT-licensed (provenance in
//! `tests/fixtures/cpt/NOTICE.md`), chosen for the shapes a CPT takes:
//!
//! - `batlow.cpt` — continuous: 255 linear slices through 256 colours, with
//!   `B`, `F` and `N`.
//! - `balance.cpt` — banded: 256 flat slices, each one colour, across -1..1.
//! - `vik.cpt` — continuous with a hinge at zero, so its slice boundaries do
//!   *not* fall on the lookup table's 256 sample points.
//! - `batlowS.cpt` — categorical, one key and colour per line, which is refused.
//!
//! The oracle is the file itself, read by the few lines of [`slices`] below
//! rather than by the parser under test: for the two tables whose boundaries
//! line up with the lookup table, entry `i` must be exactly the colour the file
//! writes at boundary `i`; for `vik`, exactly the linear interpolation between
//! the colours the file writes either side.

#![cfg(feature = "render")]

use std::path::Path;

use fieldglass_core::cpt::{ColorTable, parse_cpt};

fn text(name: &str) -> String {
    std::fs::read_to_string(Path::new("tests/fixtures/cpt").join(name))
        .unwrap_or_else(|e| panic!("read {name}: {e}"))
}

fn table(name: &str) -> ColorTable {
    parse_cpt(&text(name)).unwrap_or_else(|e| panic!("{name}: {e}"))
}

/// A file's slices as `(z0, rgb0, z1, rgb1)`, for the `z0 r/g/b z1 r/g/b`
/// layout every fixture here uses.
fn slices(name: &str) -> Vec<(f64, [f64; 3], f64, [f64; 3])> {
    let rgb = |field: &str| -> [f64; 3] {
        let parts: Vec<f64> = field
            .split('/')
            .map(|c| c.parse().expect("channel"))
            .collect();
        [parts[0], parts[1], parts[2]]
    };
    text(name)
        .lines()
        .filter(|l| !l.starts_with('#') && !l.trim().is_empty())
        .filter(|l| !matches!(l.split_whitespace().next(), Some("B" | "F" | "N")))
        .map(|l| {
            let f: Vec<&str> = l.split_whitespace().collect();
            assert_eq!(f.len(), 4, "{name}: {l}");
            (
                f[0].parse().expect("z0"),
                rgb(f[1]),
                f[2].parse().expect("z1"),
                rgb(f[3]),
            )
        })
        .collect()
}

fn entry(lut: &[u8; 768], i: usize) -> [u8; 3] {
    [lut[i * 3], lut[i * 3 + 1], lut[i * 3 + 2]]
}

fn bytes(rgb: [f64; 3]) -> [u8; 3] {
    rgb.map(|c| c as u8)
}

#[test]
fn a_continuous_table_puts_each_written_colour_on_its_own_entry() {
    let parsed = table("batlow.cpt");
    let written = slices("batlow.cpt");
    assert_eq!(parsed.slice_count(), 255);
    assert_eq!(parsed.z_range(), (0.0, 1.0));
    let lut = parsed.lut();
    // 255 slices through 256 colours at i/255, so entry i is the colour written
    // at the start of slice i, and the last entry the end of the last slice.
    for (i, slice) in written.iter().enumerate() {
        assert_eq!(entry(&lut, i), bytes(slice.1), "entry {i}");
    }
    assert_eq!(entry(&lut, 255), bytes(written[254].3));

    assert_eq!(parsed.background(), Some([1, 25, 89]));
    assert_eq!(parsed.foreground(), Some([250, 204, 250]));
    assert_eq!(parsed.nan(), Some([255, 255, 255]));
}

#[test]
fn a_banded_table_puts_each_band_on_its_own_entry() {
    let parsed = table("balance.cpt");
    let written = slices("balance.cpt");
    assert_eq!(parsed.slice_count(), 256);
    // Every slice is one flat colour: this is a banded table.
    assert!(written.iter().all(|s| s.1 == s.3));
    let lut = parsed.lut();
    // Band k spans [-1 + k/128, -1 + (k+1)/128); entry i samples -1 + 2i/255,
    // which lands in band i. So the 256 bands fill the 256 entries one each, in
    // order, with no band dropped or repeated.
    for (i, band) in written.iter().enumerate() {
        assert_eq!(entry(&lut, i), bytes(band.1), "entry {i}");
    }
}

#[test]
fn a_hinged_table_interpolates_between_the_colours_either_side() {
    let parsed = table("vik.cpt");
    let written = slices("vik.cpt");
    assert_eq!(parsed.z_range(), (-1.0, 1.0));
    let lut = parsed.lut();
    for i in 0..256 {
        let z = if i == 255 {
            1.0
        } else {
            -1.0 + 2.0 * i as f64 / 255.0
        };
        let (z0, c0, z1, c1) = *written
            .iter()
            .rev()
            .find(|s| s.0 <= z)
            .expect("inside the table");
        let t = ((z - z0) / (z1 - z0)).clamp(0.0, 1.0);
        let want: [u8; 3] =
            std::array::from_fn(|ch| (c0[ch] + (c1[ch] - c0[ch]) * t).round() as u8);
        assert_eq!(entry(&lut, i), want, "entry {i} at z = {z}");
    }
}

#[test]
fn every_table_paints_as_the_lookup_table_it_compiles_to() {
    for name in ["batlow.cpt", "balance.cpt", "vik.cpt"] {
        let parsed = table(name);
        let colormap = parsed.to_colormap(name, name);
        assert_eq!(colormap.lut(false), parsed.lut(), "{name}");
        assert_eq!(colormap.name(), name);
    }
}

#[test]
fn a_categorical_table_is_refused_on_its_first_entry() {
    let err = parse_cpt(&text("batlowS.cpt")).expect_err("categorical");
    let first_entry = text("batlowS.cpt")
        .lines()
        .position(|l| !l.starts_with('#') && !l.trim().is_empty())
        .expect("an entry")
        + 1;
    assert_eq!(err.line(), Some(first_entry), "{err}");
    assert!(err.message().contains("categorical"), "{err}");
}

/// Damage a real file the ways a download or an edit does, and each is an
/// error naming a line rather than a panic or a quietly different table.
#[test]
fn a_damaged_real_file_is_an_error_not_a_crash() {
    let original = text("batlow.cpt");
    let lines: Vec<&str> = original.lines().collect();
    let data_start = lines
        .iter()
        .position(|l| !l.starts_with('#'))
        .expect("data");

    // A slice missing from the middle leaves a gap.
    let mut dropped = lines.clone();
    dropped.remove(data_start + 100);
    let err = parse_cpt(&dropped.join("\n")).expect_err("gap");
    assert_eq!(err.line(), Some(data_start + 101), "{err}");
    assert!(err.message().contains("gap"), "{err}");

    // Cut off inside a colour, leaving `130/1`. (A cut that happens to land on a
    // field boundary can leave a valid table: `0.498039 129/130/50 0.501961 130`
    // is a slice ending in grey level 130. Nothing in the format can tell that
    // from a file written that way.)
    let cut = &original[..original.find("0.501961").expect("a mid-table value") + 14];
    assert!(cut.ends_with("130/1"), "{:?}", &cut[cut.len() - 20..]);
    let err = parse_cpt(cut).expect_err("truncated");
    assert!(err.line().is_some(), "{err}");

    // Every prefix of the file either parses or is refused; none panics.
    for end in (0..original.len()).step_by(97) {
        let _ = parse_cpt(&original[..end]);
    }
}
