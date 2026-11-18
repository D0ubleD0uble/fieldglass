//! The README "GRIB2 packing modes" table is the one place that states which
//! §5 templates decode (#1878), and it ships to the Marketplace. Nothing else
//! ties it to the decoder, so a template that starts or stops decoding could
//! leave it wrong. This reads the table and the "every registered template"
//! sentence above it and holds them to the templates `fieldglass-grib2`'s
//! §5 parser registers.
//!
//! The README is embedded with `include_str!`, so the test needs no
//! filesystem under `wasm32-wasip1`. It lives in this crate because the
//! umbrella's `tests/` is excluded from its crates.io package; the README sits
//! outside every published crate, so a test in `fieldglass-grib2` could not
//! build from its `.crate`.

#![cfg(feature = "grib2")]

use std::collections::BTreeSet;

use fieldglass_grib2::{DataRepresentationTemplate, parse_data_representation};

const README: &str = include_str!("../../../README.md");

/// Local-use templates the parser registers that the table deliberately omits:
/// they are not in Code Table 5.0 and decode through the image codec of the
/// registered template they predate (5.40000 -> 5.40, 5.40010 -> 5.41). The
/// table's own scope is Code Table 5.0 plus the 5.50001/5.50002 rows.
const UNLISTED_LOCAL: &[u16] = &[40000, 40010];

/// Whether the §5 parser models template `n`: anything but `Unsupported`. A
/// zero-filled payload is enough, because the template number is matched
/// before the payload is read; a registered template may then return `Ok` or a
/// parse error for the degenerate body, and both count as registered.
fn registered(n: u16) -> bool {
    let mut section = vec![0u8; 11 + 128];
    let len = section.len() as u32;
    section[0..4].copy_from_slice(&len.to_be_bytes());
    section[4] = 5;
    section[9..11].copy_from_slice(&n.to_be_bytes());
    !matches!(
        parse_data_representation(&section).map(|s| s.template),
        Ok(DataRepresentationTemplate::Unsupported(_))
    )
}

/// The "GRIB2 packing modes" section: from its heading to the `<details>`
/// block that follows the table.
fn section() -> &'static str {
    let start = README
        .find("### GRIB2 packing modes")
        .expect("the README has a GRIB2 packing modes section");
    let rest = &README[start..];
    let end = rest.find("\n<details>").unwrap_or(rest.len());
    &rest[..end]
}

/// Every table row as (template number, decode cell).
fn table_rows() -> Vec<(u16, String)> {
    let rows: Vec<(u16, String)> = section()
        .lines()
        .filter_map(|line| {
            let cells: Vec<&str> = line.split('|').map(str::trim).collect();
            // `| first | second | decode |` splits to ["", first, second, decode, ""].
            let [_, first, _, decode, _] = cells[..] else {
                return None;
            };
            let number = first
                .trim_start_matches('[')
                .strip_prefix("5.")?
                .split(|c: char| !c.is_ascii_digit())
                .next()?
                .parse::<u16>()
                .ok()?;
            Some((number, decode.to_string()))
        })
        .collect();
    assert!(!rows.is_empty(), "no template rows found in the table");
    rows
}

#[test]
fn the_table_lists_exactly_the_templates_the_parser_registers() {
    let rows = table_rows();
    let listed: BTreeSet<u16> = rows.iter().map(|(n, _)| *n).collect();
    assert_eq!(listed.len(), rows.len(), "a template is listed twice");

    let known: BTreeSet<u16> = (0..=u16::MAX)
        .filter(|&n| registered(n) && !UNLISTED_LOCAL.contains(&n))
        .collect();

    let missing_from_table: Vec<_> = known.difference(&listed).collect();
    let not_registered: Vec<_> = listed.difference(&known).collect();
    assert!(
        missing_from_table.is_empty() && not_registered.is_empty(),
        "README GRIB2 packing modes table disagrees with the §5 parser: \
         registered but not in the table {missing_from_table:?}; \
         in the table but not registered {not_registered:?}"
    );

    // The exemption list must not go stale.
    for n in UNLISTED_LOCAL {
        assert!(registered(*n), "5.{n} is exempted but no longer registered");
    }
}

#[test]
fn every_row_states_a_decode_status() {
    for (n, decode) in table_rows() {
        assert!(
            decode.starts_with('\u{2705}') || decode.starts_with('\u{1F6A7}'),
            "5.{n}: the Decode cell {decode:?} is neither a check mark nor a construction sign"
        );
    }
}

#[test]
fn the_summary_sentence_claims_every_registered_template() {
    let sentence = section()
        .lines()
        .find(|l| l.contains("every registered template"))
        .expect("the sentence above the table says it decodes \"every registered template\"");
    assert!(
        sentence.contains("Code Table 5.0"),
        "the summary sentence no longer names Code Table 5.0: {sentence}"
    );
    // "Every registered template decodes" is only true while the rows agree
    // with the parser (the first test); the one partial row is the
    // coefficients-only 5.53, which the same paragraph names.
    for (n, decode) in table_rows() {
        if decode.starts_with('\u{1F6A7}') {
            assert_eq!(n, 53, "5.{n} is marked as only partly decoding");
        }
    }
}
