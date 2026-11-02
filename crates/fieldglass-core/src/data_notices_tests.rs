//! `DATA_NOTICES` held to the tables this crate compiles in (#1563). The other
//! half, the `NOTICE` files held to the list, is the umbrella's
//! `tests/third_party_notices.rs`, since it reads files outside this crate.

use std::collections::BTreeMap;

use crate::color_names::COLOR_NAMES;
use crate::colormap_tables::COLORMAPS;
use crate::data_notices::{DATA_NOTICES, X11_COLOR_NAMES};

/// Every table the list names, mapped to how many entries name it.
fn listed() -> BTreeMap<&'static str, usize> {
    let mut seen = BTreeMap::new();
    for notice in DATA_NOTICES {
        for &table in notice.tables {
            *seen.entry(table).or_insert(0) += 1;
        }
    }
    seen
}

#[test]
fn every_colormap_is_listed_exactly_once() {
    let listed = listed();
    for colormap in COLORMAPS {
        let name: &str = &colormap.name;
        assert_eq!(
            listed.get(name),
            Some(&1),
            "colormap `{name}` must appear in exactly one DATA_NOTICES entry",
        );
    }
}

#[test]
fn the_x11_color_names_are_listed_exactly_once() {
    assert!(!COLOR_NAMES.is_empty());
    assert_eq!(listed().get(X11_COLOR_NAMES), Some(&1));
}

#[test]
fn every_listed_table_is_listed_once_and_names_a_table() {
    for (table, count) in listed() {
        assert_eq!(count, 1, "`{table}` is in more than one DATA_NOTICES entry");
        assert!(
            table == X11_COLOR_NAMES
                || table.ends_with(".rs")
                || COLORMAPS.iter().any(|c| c.name == table),
            "DATA_NOTICES lists `{table}`, which is no bundled table",
        );
    }
}

/// An entry that owes a notice names the packages it goes in, and one that
/// owes none names none, so a package can't be listed for nothing to carry.
#[test]
fn an_entry_names_packages_exactly_when_it_owes_a_notice() {
    for notice in DATA_NOTICES {
        let owes = notice.credit.is_some() || notice.terms.is_some();
        assert_eq!(
            owes,
            !notice.packages.is_empty(),
            "{:?}: a notice owed must name its packages, and only then",
            notice.tables,
        );
        assert!(
            notice.terms.is_none() || notice.credit.is_some(),
            "{:?}: license text needs a credit line saying what it covers",
            notice.tables,
        );
    }
}
