//! The short-name column of the WMO GRIB2 master parameter table (#469).
//!
//! WMO publishes names and units for Code Table 4.2 but no abbreviations, so
//! the third column arrives from a second upstream: wgrib2's centre-0 rows,
//! generated into `tables_wmo_short.rs` by `tools/gen_wmo_short_names.py`.
//!
//! What lives here is what the *public* seam can prove — the coverage the
//! column actually reaches, and whether the abbreviations are the ones NCO
//! publishes. The integrity of the join itself (no row inventing a parameter,
//! no hand-written arm shadowing the table) needs the generated module in
//! view, so those two are unit tests beside `resolve_parameter` in `tables.rs`.
//!
//! Every triple is walked rather than sampled. These properties are about the
//! whole join, not about any triple a person would think to pick.

use fieldglass_grib2::{Originator, lookup_parameter};

/// Walk every `(discipline, category, number)` there is.
///
/// 16.7M iterations of one table lookup, which the generated `match` arms
/// compile to a search rather than a scan.
fn every_triple() -> impl Iterator<Item = (u8, u8, u8)> {
    (0..=255u8).flat_map(|d| (0..=255u8).flat_map(move |c| (0..=255u8).map(move |n| (d, c, n))))
}

/// The abbreviation shown for a master parameter, or `None` when the master
/// table does not define the triple.
///
/// `Originator::default()` is centre 0, which no local table registers for, so
/// nothing here routes through the centre-local seam — this is the master
/// table's own answer, curated arms included.
fn shown(triple: (u8, u8, u8)) -> Option<&'static str> {
    let (discipline, category, number) = triple;
    lookup_parameter(Originator::default(), discipline, category, number)
        .map(|(abbreviation, _, _)| abbreviation)
}

/// The measured state of the column, pinned so a bump on either upstream has
/// to restate it deliberately rather than drift.
///
/// Before #469 this was 41 named and 1346 blank: every abbreviation in the
/// master table was hand-written, so the column was full for the local
/// parameters and empty for 97% of the standard ones, which is backwards from
/// what a reader expects.
///
/// The 157 that remain are triples wgrib2's table does not carry; 88 of them
/// are discipline 20, which WMO added after the snapshot wgrib2 transcribed.
/// They resolve to name and units with an empty abbreviation, exactly as all
/// 1346 did before.
#[test]
fn master_parameter_coverage_is_what_the_table_claims() {
    let mut defined = 0usize;
    let mut named = 0usize;
    for triple in every_triple() {
        let Some(abbreviation) = shown(triple) else {
            continue;
        };
        defined += 1;
        if !abbreviation.is_empty() {
            named += 1;
        }
    }
    assert_eq!(defined, 1387, "WMO v37 master parameters");
    assert_eq!(named, 1230, "master parameters carrying an abbreviation");
    assert_eq!(defined - named, 157, "master parameters still without one");
}

/// Spot checks against what NCO publishes.
///
/// The exhaustive tests prove the join is *consistent*; they cannot prove it is
/// *right*, because both sides of it come from this repo. These are read off
/// NCO's own Code Table 4.2 pages — the document wgrib2's table is transcribed
/// from, and the listing a forecaster compares a decoded file against.
#[test]
fn spot_checks_match_the_published_ncep_abbreviations() {
    let cases: &[((u8, u8, u8), &str, &str)] = &[
        // 4.2-0-1: 22 is CLMR. The curated arm read CLWMR, which is ON388
        // GRIB1 parameter 153 — the same quantity under the older edition.
        ((0, 1, 22), "CLMR", "Cloud mixing ratio"),
        // 4.2-10-0: 3 is HTSGW and 5 is WVHGT. The curated arm read WVHGT at
        // 3, pairing number 5's abbreviation with number 3's name.
        (
            (10, 0, 3),
            "HTSGW",
            "Significant height of combined wind+swell",
        ),
        ((10, 0, 5), "WVHGT", "Significant height of wind waves"),
        // Uncurated entries, blank before #469, that a GFS listing prints.
        ((10, 3, 0), "WTMP", "Water temperature"),
        ((2, 0, 3), "SOILM", "Soil moisture content"),
        ((0, 3, 10), "DEN", "Density"),
        ((0, 19, 1), "ALBDO", "Albedo"),
        ((0, 6, 6), "CWAT", "Cloud water"),
        // Curated already, and unchanged: the 39 that agreed.
        ((0, 0, 0), "TMP", "Temperature"),
        ((0, 2, 2), "UGRD", "U-component of wind"),
    ];
    for &(triple, abbreviation, name) in cases {
        let (d, c, n) = triple;
        let resolved = lookup_parameter(Originator::default(), d, c, n)
            .unwrap_or_else(|| panic!("{d}/{c}/{n} resolves to nothing"));
        assert_eq!(resolved.0, abbreviation, "abbreviation for {d}/{c}/{n}");
        assert_eq!(
            resolved.1.to_lowercase(),
            name.to_lowercase(),
            "name for {d}/{c}/{n}"
        );
    }
}
