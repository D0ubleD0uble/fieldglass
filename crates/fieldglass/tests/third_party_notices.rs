//! The packages' `NOTICE` files carry every notice the third-party data in
//! them owes (#1563).
//!
//! The list is `fieldglass-core`'s `src/data_notices.rs`, kept beside the
//! colormap tables and held to the registry by that crate's own tests. This
//! half reads the `NOTICE` files, which sit outside every published crate, so
//! it lives here: the umbrella's `tests/` is excluded from its crates.io
//! package (`exclude` in `Cargo.toml`), and `fieldglass-core`'s published
//! tests stay buildable from the tarball.

#[path = "../../fieldglass-core/src/data_notices.rs"]
mod data_notices;

use data_notices::{DATA_NOTICES, EXTENSION, NPM};

/// Every package that ships the tables, by the path `DATA_NOTICES` names it
/// with, and the `NOTICE` it carries.
const NOTICES: &[(&str, &str)] = &[
    (NPM, include_str!("../../fieldglass-wasm/npm/NOTICE")),
    (EXTENSION, include_str!("../../../extension/NOTICE")),
];

/// Collapse every run of whitespace to one space, so a credit wrapped to a
/// file's line width still matches the list's one-line form.
fn words(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn notice_of(package: &str) -> &'static str {
    NOTICES
        .iter()
        .find(|(path, _)| *path == package)
        .map(|(_, text)| *text)
        .unwrap_or_else(|| panic!("DATA_NOTICES names `{package}`, which has no NOTICE here"))
}

/// A credit line is compared up to line wrapping; a license text byte for
/// byte, since its terms ask for it as written.
#[test]
fn every_package_notice_carries_its_credits_and_license_texts() {
    for notice in DATA_NOTICES {
        for &package in notice.packages {
            let text = notice_of(package);
            if let Some(credit) = notice.credit {
                assert!(
                    words(text).contains(&words(credit)),
                    "{package} is missing the credit for {:?}:\n{credit}",
                    notice.tables,
                );
            }
            if let Some(terms) = notice.terms {
                assert!(
                    text.contains(terms),
                    "{package} does not carry the license text for {:?} verbatim:\n{terms}",
                    notice.tables,
                );
            }
        }
    }
}

/// A generated table the list names still exists, so a renamed or removed
/// table can't leave a stale entry behind. Relative to `crates/`, which the
/// wasm32 test runner preopens as `..`.
#[test]
fn every_listed_table_file_exists() {
    for notice in DATA_NOTICES {
        for table in notice.tables.iter().filter(|t| t.ends_with(".rs")) {
            let path = std::path::Path::new("..").join(table);
            assert!(
                path.is_file(),
                "DATA_NOTICES lists {table}, which is not a file"
            );
        }
    }
}
