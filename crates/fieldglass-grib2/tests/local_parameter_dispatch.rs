//! Centre-local parameter dispatch (#439).
//!
//! #439 lands the seam that #424-#426 plug centre tables into, and lands it
//! empty: `tables_local::lookup` answers `None` for everything, so resolution
//! today is exactly what it was before. That makes the seam the hard thing to
//! test — a registry with no entries proves nothing about ordering.
//!
//! So the policy is tested two ways. The properties that hold *because the
//! registry is empty* are asserted here directly. The ordering that only shows
//! up once a centre table exists is asserted against a stub table through
//! `resolve_parameter`, the same function `lookup_parameter` calls, in
//! `tables.rs`'s own unit tests — a stub cannot be injected from an
//! integration test, and testing the ordering through a private seam is worth
//! more than not testing it at all.

use fieldglass_grib2::{Grib2Reader, Originator, lookup_parameter};

/// A centre with no local table of its own, for the checks where the
/// originating centre is not what is under test.
const MASTER_ONLY: Originator = Originator {
    centre: 0,
    sub_centre: 0,
    local_tables_version: 0,
};

/// The property the whole design rests on: WMO's master table and local code
/// space do not overlap, so routing local codes to a centre first can never
/// shadow a standard parameter.
///
/// This is a fact about the upstream tables, not about our code, which is
/// exactly why it is asserted rather than assumed — a future WMO tag that
/// assigned something at 192+ would silently change what dispatch means.
#[test]
fn the_master_table_never_defines_a_local_use_code() {
    let mut master = 0usize;
    for discipline in 0..=255u8 {
        for category in 0..=255u8 {
            for number in 0..=255u8 {
                let Some((_, name, _)) =
                    lookup_parameter(MASTER_ONLY, discipline, category, number)
                else {
                    continue;
                };
                master += 1;
                assert!(
                    discipline < 192 && category < 192 && number < 192,
                    "the master set defines {discipline}/{category}/{number} ({name:?}), \
                     which is local code space — local dispatch would now shadow it"
                );
            }
        }
    }
    assert!(
        master > 1_000,
        "only {master} parameters resolve — the master table is not loaded, so this \
         proves nothing"
    );
}

/// A centre with no table of its own resolves exactly as the master set does,
/// for every triple. This is the guarantee that adding ECMWF (#424) could not
/// quietly change what any other file shows.
#[test]
fn a_centre_without_a_table_resolves_as_the_master_set() {
    // NCEP, DWD, JMA, NASA, and one that does not exist. None has a table yet.
    for centre in [7u16, 78, 34, 173, 60_000] {
        for discipline in [0u8, 1, 2, 3, 10, 192, 255] {
            for category in [0u8, 1, 2, 3, 191, 192, 255] {
                for number in [0u8, 1, 8, 191, 192, 254, 255] {
                    let baseline = lookup_parameter(MASTER_ONLY, discipline, category, number);
                    for local_tables_version in [0u8, 1, 228] {
                        assert_eq!(
                            lookup_parameter(
                                Originator::new(centre, 0, local_tables_version),
                                discipline,
                                category,
                                number
                            ),
                            baseline,
                            "{discipline}/{category}/{number} resolved differently for \
                             centre {centre}, which has no local table"
                        );
                    }
                }
            }
        }
    }
}

/// ECMWF may differ from the master set **only** inside local code space. A
/// centre table that shadowed a standard parameter would be a silent
/// regression for every ECMWF file, so this sweeps the whole triple space
/// rather than spot-checking.
#[test]
fn the_ecmwf_table_never_changes_a_standard_triple() {
    let mut local_hits = 0usize;
    for discipline in 0..=255u8 {
        for category in 0..=255u8 {
            for number in 0..=255u8 {
                let master = lookup_parameter(MASTER_ONLY, discipline, category, number);
                let ecmwf =
                    lookup_parameter(Originator::new(98, 0, 1), discipline, category, number);
                let local_space = [discipline, category, number]
                    .iter()
                    .any(|&c| (192..=254).contains(&c));
                if local_space {
                    if ecmwf != master {
                        local_hits += 1;
                    }
                } else {
                    assert_eq!(
                        ecmwf, master,
                        "{discipline}/{category}/{number} is standard code space, but ECMWF \
                         resolves it differently"
                    );
                }
            }
        }
    }
    assert!(
        local_hits > 3_000,
        "only {local_hits} triples resolve through the ECMWF table — it is not wired in"
    );
}

/// Local code space still resolves to nothing for a centre with no table, so a
/// file from one renders its numeric triple rather than picking up another
/// centre's meaning.
#[test]
fn local_use_codes_resolve_to_nothing_without_a_centre_table() {
    for discipline in [192u8, 200, 254] {
        assert_eq!(lookup_parameter(MASTER_ONLY, discipline, 0, 0), None);
    }
    for category in [192u8, 200, 254] {
        assert_eq!(lookup_parameter(MASTER_ONLY, 0, category, 0), None);
    }
    for number in [192u8, 200, 254] {
        assert_eq!(lookup_parameter(MASTER_ONLY, 0, 0, number), None);
    }
    // And an ECMWF triple means nothing to a different centre.
    assert!(lookup_parameter(Originator::new(98, 0, 0), 192, 128, 4).is_some());
    assert_eq!(
        lookup_parameter(Originator::new(7, 0, 0), 192, 128, 4),
        None
    );
}

/// A standard triple still hits the master set, which is the other half of the
/// acceptance criterion.
#[test]
fn standard_codes_still_resolve_against_the_master_set() {
    assert_eq!(
        lookup_parameter(Originator::new(7, 4, 0), 0, 0, 0),
        Some(("TMP", "Temperature", "K"))
    );
    assert_eq!(
        lookup_parameter(Originator::new(98, 0, 0), 0, 2, 2),
        Some(("UGRD", "U-component of wind", "m s⁻¹"))
    );
    // And one that comes from the generated WMO table rather than the curated
    // subset, so both halves of the fallback chain are covered.
    let (abbreviation, _, _) = lookup_parameter(MASTER_ONLY, 0, 19, 1).expect("WMO master entry");
    assert_eq!(abbreviation, "", "WMO publishes no short names");
}

/// The originating centre reaches the lookup from §1 of a real message, not
/// just from a hand-built `Originator`. This is the thread the issue asks for,
/// and the part a unit test on `tables.rs` cannot reach.
#[test]
fn a_real_message_carries_its_originator_to_the_lookup() {
    const FIXTURE: &[u8] = include_bytes!("fixtures/reduced_gaussian_pressure_level.grib2");
    let reader = Grib2Reader::from_bytes(FIXTURE.to_vec()).expect("read fixture");
    let msg = &reader.messages[0];

    let originator = msg.ids.originator();
    assert_eq!(originator.centre, msg.ids.centre);
    assert_eq!(originator.sub_centre, msg.ids.sub_centre);
    assert_eq!(
        originator,
        Originator::new(98, 0, 0),
        "the fixture is ECMWF"
    );

    // §1 is the only place the centre comes from, so a message resolving its
    // own parameter must go through it.
    let common = msg.pds.common().expect("template 4.0");
    assert!(
        lookup_parameter(
            originator,
            msg.is.discipline,
            common.parameter_category,
            common.parameter_number,
        )
        .is_some(),
        "the fixture's own parameter should resolve"
    );
}

/// 255 is "missing", not local use, in all three of discipline, category and
/// number. It must not route to a centre table: a file setting it is declining
/// to say what the parameter is, and a local entry there would put a confident
/// name on an absent value.
///
/// While the registry is empty this cannot be observed through
/// `lookup_parameter` — both paths answer `None` — so it is pinned as
/// behaviour of the boundary instead, and as a note for whoever adds the first
/// centre table.
#[test]
fn the_missing_sentinel_is_not_local_use() {
    for (discipline, category, number) in [(255u8, 0u8, 0u8), (0, 255, 0), (0, 0, 255)] {
        assert_eq!(
            lookup_parameter(Originator::new(7, 0, 0), discipline, category, number),
            None,
            "{discipline}/{category}/{number} must not resolve"
        );
    }
    // 254 is the top of local use and 192 the bottom; both are local, and
    // neither is the sentinel.
    assert_eq!(
        lookup_parameter(MASTER_ONLY, 0, 0, 191),
        None,
        "191 is master space, undefined"
    );
    assert_eq!(
        lookup_parameter(MASTER_ONLY, 0, 0, 192),
        None,
        "192 is local space, unregistered"
    );
}
