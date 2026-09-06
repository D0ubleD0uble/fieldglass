//! The rule that decides whether a label of ours still says what the authority
//! says, shared by the two table gates that need it.
//!
//! `wmo_code_tables.rs` and `wmo_parameter_tables.rs` both compare a curated
//! label against an authority's text, and both accept a list of reviewed
//! divergences whose wording is deliberately not the authority's. For those,
//! pinning the authority's exact text catches a *reassigned* code; this is the
//! other half, on our own side. It lived twice, once in each file, and the two
//! copies were identical — which is how a weakness fixed in one would have
//! survived in the other. It lives here now.
//!
//! What it has to separate is a **rewrite** from a **different entry**. The
//! accepted labels are heavy rewrites: "Oblate spheroid (WGS84)" for a
//! 60-character geodetic definition, "Significant height of combined
//! wind+swell" for eccodes' spelt-out version. So it cannot ask for much. What
//! it must not do is what it did until #655 — accept two labels **swapped
//! inside one table**, which is invisible to every other check in both files:
//! the strings all still exist, are still distinct, and the authority's
//! recorded wording is untouched.
//!
//! Three rules, in [`is_a_recognisable_rewrite`] and [`keeps_wmos_word_order`]:
//!
//! 1. Some substantial word of ours appears in the authority's text. Every
//!    defect found in #415 shared no word at all, and this is what caught them.
//! 2. Every **number** we state is a number they state. A rewrite may drop a
//!    number; it may not keep a different one. This separates §3.2 code 0's
//!    6 367 470 m sphere from code 6's 6 371 229 m one — both otherwise reduce
//!    to "spherical" and "radius" — and separates the three aerosol optical
//!    thicknesses at 0.635, 0.810 and 1.640 µm, whose labels are identical
//!    except for the wavelength.
//! 3. The words of ours that survive appear in **their order**. This separates
//!    §4.10 code 4, "Difference (end minus start)", from code 8, "Difference
//!    (start minus end)": opposite quantities, so a swap is a sign inversion in
//!    a displayed statistic. Neither states a number and `end` is too short to
//!    be a word here, so order is the only signal left.
//!
//! Rule 3 is separate from the other two because a handful of rewrites
//! legitimately reorder — English fronts a noun where "Level of X" trails it —
//! and each caller records those rather than folding the exemption in here.

/// Labels compare on letters and digits only: the sources differ freely on
/// hyphens, case and spacing (`Dew-point` / `dewpoint`) without disagreeing
/// about what a code names.
pub(crate) fn normalize(s: &str) -> String {
    s.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// `s` with the spaces WMO puts *inside* a number removed.
///
/// WMO groups digits — `radius = 6 367 470.0 m` — so splitting on
/// non-alphanumerics leaves `6`, `367`, `470`, three fragments too short for
/// any word filter to keep. The effect was that no radius, axis or datum
/// number in Table 3.2 was ever compared, and the numbers are the only thing
/// separating one earth shape from another. Joined, the radius is a single
/// seven-character token.
fn join_digit_groups(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        if !chars[i].is_whitespace() {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        let mut end = i;
        while end < chars.len() && chars[end].is_whitespace() {
            end += 1;
        }
        let between_digits = out.chars().next_back().is_some_and(|c| c.is_ascii_digit())
            && chars.get(end).is_some_and(|c| c.is_ascii_digit());
        if !between_digits {
            out.extend(&chars[i..end]);
        }
        i = end;
    }
    out
}

/// The words of `s` long enough to carry meaning, in the order it writes them.
fn substantial_words(s: &str) -> Vec<String> {
    join_digit_groups(s)
        .split(|c: char| !c.is_ascii_alphanumeric())
        .map(normalize)
        .filter(|w| w.len() >= 5)
        .collect()
}

/// The numbers `s` states, as digit runs of two or more.
///
/// Kept below the word floor on purpose. A number is an identifier, not prose:
/// `1965` is what makes Table 3.2 code 2 the IAU spheroid and nothing else, and
/// four characters of it are as decisive as forty of wording.
fn numbers_in(s: &str) -> Vec<String> {
    join_digit_groups(s)
        .split(|c: char| !c.is_ascii_alphanumeric())
        .map(normalize)
        .filter(|w| w.len() >= 2 && w.bytes().all(|b| b.is_ascii_digit()))
        .collect()
}

/// Rules 1 and 2: `ours` keeps a word of `theirs`, and states no number
/// `theirs` does not.
pub(crate) fn is_a_recognisable_rewrite(ours: &str, theirs: &str) -> bool {
    let haystack = normalize(&join_digit_groups(theirs));
    substantial_words(ours)
        .iter()
        .any(|w| haystack.contains(w.as_str()))
        && numbers_in(ours)
            .iter()
            .all(|n| haystack.contains(n.as_str()))
}

/// Rule 3: the words of `ours` that survive into `theirs` appear in its order.
///
/// Words of ours that appear nowhere in their text are skipped rather than
/// failed, which is what lets this survive a rewrite: "Difference (end minus
/// start)" holds against "Difference (value at the end of time range minus
/// value at the beginning)", where `start` is simply absent.
pub(crate) fn keeps_wmos_word_order(ours: &str, theirs: &str) -> bool {
    let haystack = normalize(&join_digit_groups(theirs));
    // `normalize` leaves ASCII alphanumerics only, so byte offsets are char
    // offsets and slicing at a match cannot split a character.
    let mut cursor = 0usize;
    for word in substantial_words(ours) {
        if !haystack.contains(word.as_str()) {
            continue;
        }
        match haystack[cursor..].find(word.as_str()) {
            // Advance past the match's first character rather than its whole
            // length, so two of our words may overlap in their text.
            Some(at) => cursor += at + 1,
            None => return false,
        }
    }
    true
}
