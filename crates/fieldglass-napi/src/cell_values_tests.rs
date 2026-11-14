//! `cell_values` rejoins a field's values and mask without a silent arm (#1806).
//!
//! `fieldglass::Values` is `#[non_exhaustive]`, so a `match` on it in this
//! crate needs a wildcard arm. That arm once returned an empty `Vec` in a
//! release build, caught only by a `debug_assert`. No width exists today that
//! could reach it, so no run can show the failure; the guard reads the source,
//! as `no_unsafe_tests` does, and holds the function to `Values::get`.

use crate::cell_values;
use fieldglass::Values;

#[test]
fn both_widths_rejoin_with_the_mask() {
    let mask = [1, 0, 1];
    assert_eq!(
        cell_values(&Values::F64(vec![1.5, 9.0, -2.0]), &mask),
        vec![Some(1.5), None, Some(-2.0)]
    );
    assert_eq!(
        cell_values(&Values::F32(vec![0.5, 9.0, 2.0]), &mask),
        vec![Some(0.5), None, Some(2.0)]
    );
}

#[test]
fn a_mask_longer_than_the_values_stops_with_the_values() {
    assert_eq!(
        cell_values(&Values::F64(vec![1.0]), &[1, 1, 1]),
        vec![Some(1.0)]
    );
}

/// The text of `fn cell_values`, up to its closing brace at column 0.
fn cell_values_source() -> String {
    let src = include_str!("lib.rs");
    let start = src
        .find("\nfn cell_values(")
        .expect("lib.rs defines cell_values");
    let body = &src[start + 1..];
    let end = body.find("\n}\n").expect("cell_values ends at column 0");
    body[..end].to_owned()
}

#[test]
fn the_width_dispatch_is_values_get_not_a_local_match() {
    let body = cell_values_source();
    // Prose in comments names these on purpose, so only code is read.
    let code: String = body
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        code.contains("values.get("),
        "cell_values reads through Values::get"
    );
    for needle in [
        "Values::",
        "match ",
        "debug_assert",
        "Vec::new()",
        "other =>",
        "_ =>",
    ] {
        assert!(
            !code.contains(needle),
            "cell_values holds `{needle}`, a local width arm that can be silent (#1806)"
        );
    }
}
