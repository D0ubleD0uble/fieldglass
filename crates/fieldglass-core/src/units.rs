//! Display normalisation for unit strings (issue #432).
//!
//! The GRIB2 parameter table mixes two notations, and not by accident: the
//! curated entries carry typeset units (`m s⁻¹`), while the 1,387 generated from
//! the WMO master tables in #415 carry WMO's own ASCII (`m/s`, `kg m-2`). The
//! generated file is reproduced byte-for-byte from a pinned upstream tag, so it
//! is the wrong place to fix this; normalising here, where the string is about
//! to be shown, keeps that guarantee intact.
//!
//! # Why this is not a regex
//!
//! Upstream is not consistent — `m/s` and `m s-1` both appear, as do `J/kg` and
//! `J kg-1` — but it also contains strings that a structural rule mangles:
//!
//! - `CCITT IA5` is a character encoding. Any "letter followed by digit means
//!   exponent" rule renders it `CCITT IA⁵`.
//! - `m2/3 s-1` is a **fractional** exponent, m^(2/3). Treating `/` as "divide
//!   by what follows" turns it into nonsense.
//! - `(m2 s sr eV/nuc)-1` nests a solidus inside a parenthesised group.
//! - `Code table 4.253` is a cross-reference, not a unit at all.
//!
//! So the rule here is the other way round: a token is rewritten only when its
//! symbol is a **recognised unit**, and if any token in the string is not, the
//! whole string is returned untouched. Unrecognised input degrades to exactly
//! what WMO published, which is always defensible; the failure mode is a missed
//! normalisation, never a corrupted one.
//!
//! That leaves one thing to watch: a future WMO tag can introduce a unit symbol
//! this list has never seen, and it will pass through in ASCII rather than
//! failing. The snapshot in `fieldglass-grib2/tests/unit_notation.rs` is what
//! surfaces it — the set of distinct unit strings changes, the snapshot diffs,
//! and the un-normalised entry is visible in the diff for whoever bumps the
//! tag.

use std::borrow::Cow;

/// Unit symbols this module will rewrite around.
///
/// Deliberately an allow-list rather than a deny-list of prose. `IA` (from
/// `CCITT IA5`) and `table` (from `Code table 4.253`) are not units, and no
/// structural test distinguishes `IA5` from `K2` — only knowing that `K` is
/// kelvin and `IA` is not. Built from the units actually present in WMO Code
/// Table 4.2 at tag v37 plus the curated entries; anything new simply passes
/// through until it is added here.
const UNIT_SYMBOLS: &[&str] = &[
    // SI base and derived
    "m", "s", "kg", "g", "K", "Pa", "J", "W", "N", "mol", "Hz", "V", "T", "S", "A", "C", "rad",
    "sr", "Bq", "Sv", "Gy", "lm", "lx", "F", "H", "Wb", "ohm",
    // Prefixed forms that appear in the table as single symbols
    "km", "cm", "mm", "nm", "nT", "nSv", "hPa", "kPa", "MPa", "kt", // Time
    "d", "h", "min", "day", "week", "month", "year", // Meteorological and domain units
    "gpm", "dB", "DU", "TECU", "eV", "nuc", "pH", "deg", "degree", "°", "%",
    // Countable "units" that WMO writes as words but that do take exponents
    "Person", "Number", "Bites", // Spelled-out forms upstream occasionally uses
    "Joule",
];

/// Superscript digits, indexed by value.
const SUPERSCRIPT: [char; 10] = ['⁰', '¹', '²', '³', '⁴', '⁵', '⁶', '⁷', '⁸', '⁹'];
const SUPERSCRIPT_MINUS: char = '⁻';

/// Normalise a unit string for display.
///
/// Products become space-separated symbols with typeset exponents — `kg m-2` and
/// `kg/m2` both render `kg m⁻²` — and anything not recognisably a product of
/// units is returned unchanged. Already-normalised input is returned unchanged,
/// so this is idempotent and safe to apply to the curated entries too.
pub fn normalize_units(units: &str) -> Cow<'_, str> {
    if units.is_empty() || !is_normalisable(units) {
        return Cow::Borrowed(units);
    }
    let mut out = String::with_capacity(units.len() + 4);
    for (index, token) in units.split_whitespace().enumerate() {
        if index > 0 {
            out.push(' ');
        }
        match rewrite_token(token) {
            Some(rewritten) => out.push_str(&rewritten),
            // `is_normalisable` already established every token rewrites, so
            // this is unreachable; returning the input is still the right
            // answer if that ever stops being true.
            None => return Cow::Borrowed(units),
        }
    }
    if out == units {
        return Cow::Borrowed(units);
    }
    Cow::Owned(out)
}

/// Whether every token of `units` is a unit this module recognises.
///
/// All-or-nothing on purpose: a string half-rewritten is worse than one left
/// alone, because it looks deliberate.
fn is_normalisable(units: &str) -> bool {
    // Parentheses mean a grouped expression (`(m2 s sr eV/nuc)-1`) or a
    // cross-reference (`(Code table 4.201)`); neither survives token rewriting.
    if units.contains('(') || units.contains(')') {
        return false;
    }
    // Already typeset — the curated entries. Idempotent by short-circuit.
    if units.contains(SUPERSCRIPT_MINUS) || units.chars().any(|c| SUPERSCRIPT.contains(&c)) {
        return false;
    }
    // A digit on both sides of a solidus is a fractional exponent (`m2/3 s-1`),
    // not a division.
    if units
        .as_bytes()
        .windows(3)
        .any(|w| w[0].is_ascii_digit() && w[1] == b'/' && w[2].is_ascii_digit())
    {
        return false;
    }
    units.split_whitespace().all(|t| rewrite_token(t).is_some())
}

/// Rewrite one whitespace-delimited token, or `None` if it is not a unit.
fn rewrite_token(token: &str) -> Option<String> {
    // `A/B` — a solidus inside one token is division: `J/kg` is `J kg⁻¹`.
    if let Some((numerator, denominator)) = token.split_once('/') {
        // A leading solidus (`/s`, `/kg`) is a bare reciprocal.
        if numerator.is_empty() {
            let (symbol, exponent) = split_exponent(denominator)?;
            return Some(format!("{symbol}{}", superscript(-exponent)));
        }
        if denominator.contains('/') {
            return None;
        }
        let (num_symbol, num_exponent) = split_exponent(numerator)?;
        let (den_symbol, den_exponent) = split_exponent(denominator)?;
        let numerator = if num_exponent == 1 {
            num_symbol.to_string()
        } else {
            format!("{num_symbol}{}", superscript(num_exponent))
        };
        return Some(format!(
            "{numerator} {den_symbol}{}",
            superscript(-den_exponent)
        ));
    }

    let (symbol, exponent) = split_exponent(token)?;
    if exponent == 1 {
        Some(symbol.to_string())
    } else {
        Some(format!("{symbol}{}", superscript(exponent)))
    }
}

/// Split `kg-2` into `("kg", -2)`, `m2` into `("m", 2)`, `Pa` into `("Pa", 1)`.
///
/// `None` when the alphabetic part is not a recognised unit symbol, which is
/// what keeps `IA5` from becoming `IA⁵`.
fn split_exponent(token: &str) -> Option<(&str, i32)> {
    let digits_start = token.len()
        - token
            .chars()
            .rev()
            .take_while(|c| c.is_ascii_digit())
            .count();
    let (head, digits) = token.split_at(digits_start);
    let (symbol, sign) = match head.strip_suffix('-') {
        Some(symbol) if !digits.is_empty() => (symbol, -1),
        // A trailing `-` with no digits is not an exponent.
        Some(_) => return None,
        None => (head, 1),
    };
    if !UNIT_SYMBOLS.contains(&symbol) {
        return None;
    }
    let exponent = if digits.is_empty() {
        1
    } else {
        sign * digits.parse::<i32>().ok()?
    };
    Some((symbol, exponent))
}

/// Render an exponent as superscript digits. `1` renders empty, since `m¹` is
/// written `m`.
fn superscript(exponent: i32) -> String {
    if exponent == 1 {
        return String::new();
    }
    let mut out = String::new();
    if exponent < 0 {
        out.push(SUPERSCRIPT_MINUS);
    }
    for digit in exponent.unsigned_abs().to_string().bytes() {
        out.push(SUPERSCRIPT[(digit - b'0') as usize]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn products_gain_typeset_exponents() {
        assert_eq!(normalize_units("kg m-2"), "kg m⁻²");
        assert_eq!(normalize_units("W m-2 sr-1 m-1"), "W m⁻² sr⁻¹ m⁻¹");
        assert_eq!(normalize_units("m2"), "m²");
        assert_eq!(normalize_units("K2"), "K²");
    }

    #[test]
    fn a_solidus_becomes_a_negative_exponent() {
        assert_eq!(normalize_units("m/s"), "m s⁻¹");
        assert_eq!(normalize_units("J/kg"), "J kg⁻¹");
        assert_eq!(normalize_units("kg/kg"), "kg kg⁻¹");
        assert_eq!(normalize_units("m2/s"), "m² s⁻¹");
        // A leading solidus is a bare reciprocal.
        assert_eq!(normalize_units("/s"), "s⁻¹");
        assert_eq!(normalize_units("/kg"), "kg⁻¹");
    }

    /// Both spellings of the same quantity have to land on one form — that is
    /// the entire point of the exercise.
    #[test]
    fn the_two_upstream_spellings_converge() {
        for (solidus, ascii) in [("m/s", "m s-1"), ("J/kg", "J kg-1"), ("kg/kg", "kg kg-1")] {
            assert_eq!(
                normalize_units(solidus),
                normalize_units(ascii),
                "{solidus:?} and {ascii:?} must render the same"
            );
        }
    }

    /// An unrecognised symbol makes the *whole* string pass through. Rewriting
    /// the tokens it does recognise would leave a half-converted string, which
    /// reads as deliberate and is worse than leaving it alone.
    #[test]
    fn one_unknown_symbol_leaves_the_whole_string_alone() {
        // `IA` is not a unit, so `CCITT IA5` is untouched — including the
        // `CCITT` that would otherwise have been fine.
        assert_eq!(normalize_units("CCITT IA5"), "CCITT IA5");
        // Same shape, with a symbol that *is* a unit, to show the guard is the
        // allow-list and not something incidental about the string.
        assert_eq!(normalize_units("CCITT K5"), "CCITT K5");
        assert_eq!(normalize_units("m-2 zorkmid-1"), "m-2 zorkmid-1");
    }

    #[test]
    fn already_typeset_input_is_returned_unchanged() {
        for already in ["m s⁻¹", "kg m⁻² s⁻¹", "s⁻¹", "K m⁻¹"] {
            assert_eq!(normalize_units(already), already);
        }
    }

    #[test]
    fn a_fractional_exponent_is_not_a_division() {
        // m^(2/3) per second. Reading the solidus as division would produce
        // something that means nothing.
        assert_eq!(normalize_units("m2/3 s-1"), "m2/3 s-1");
    }

    #[test]
    fn grouped_expressions_pass_through() {
        for grouped in ["(m2 s sr eV/nuc)-1", "(m2 s sr)-1", "(Code table 4.201)"] {
            assert_eq!(normalize_units(grouped), grouped);
        }
    }

    #[test]
    fn a_trailing_hyphen_is_not_an_exponent() {
        assert_eq!(normalize_units("m-"), "m-");
        assert_eq!(normalize_units("kg- m"), "kg- m");
    }

    /// `normalize_units` is public and takes arbitrary text, so it has to be
    /// panic-free on input that is not a unit string at all.
    ///
    /// The specific hazard is `split_at` on a byte index: the exponent split
    /// counts trailing digits as *chars* and subtracts from a *byte* length.
    /// That is only sound because digits are ASCII, so the two agree over the
    /// digit run — which is exactly the kind of reasoning worth a test rather
    /// than a comment. The cases below put multi-byte characters immediately
    /// before a digit run, where a wrong index would land mid-character.
    #[test]
    fn arbitrary_input_does_not_panic() {
        for input in [
            "",
            " ",
            "\t",
            "m-",
            "m-0",
            "-1",
            "/",
            "//",
            "a/b/c",
            "m/",
            "m/-2",
            "m--2",
            "m2-3",
            // Multi-byte immediately before the digits.
            "°2",
            "µ2",
            "℃3",
            "日本語2",
            "🌡2",
            "\u{feff}m-2",
            "K\u{301}2",
            // Exponents at and beyond the integer range.
            "m-2147483648",
            "m2147483647",
            "m-99999999999999999999",
            "kg-000002",
        ] {
            let _ = normalize_units(input);
        }

        // Exhaustive over short strings from an alphabet chosen to hit every
        // branch: symbol, hyphen, solidus, digit, multi-byte, separator.
        let alphabet = ['m', 'k', 'g', '-', '/', '2', '°', ' '];
        for a in alphabet {
            for b in alphabet {
                for c in alphabet {
                    for d in alphabet {
                        let s: String = [a, b, c, d].iter().collect();
                        let _ = normalize_units(&s);
                    }
                }
            }
        }
    }

    #[test]
    fn borrowing_is_preserved_when_nothing_changes() {
        assert!(matches!(normalize_units("K"), Cow::Borrowed(_)));
        assert!(matches!(normalize_units("Numeric"), Cow::Borrowed(_)));
        assert!(matches!(normalize_units("kg m-2"), Cow::Owned(_)));
    }
}
