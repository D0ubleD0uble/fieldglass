//! Centre-local parameter dispatch (#439).
//!
//! #439 landed the seam that #424-#426 plug centre tables into, and landed it
//! empty. ECMWF (#424) and DWD (#425) have since filled it, so what is tested
//! here is the *policy*: local code space routes to the originating centre and
//! nowhere else, standard code space never moves, and a centre with no table of
//! its own resolves exactly as it did before any of this existed.
//!
//! The ordering inside `resolve_parameter` — local table consulted before the
//! master set — is asserted against a stub table in `tables.rs`'s own unit
//! tests, because a stub cannot be injected from an integration test. What the
//! two real tables give this file instead is scale: every one of the 16.7
//! million triples is swept for both of them, which is what makes "nothing
//! outside 192-254 moved" a fact rather than a spot-check.

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
/// for every triple. This is the guarantee that adding a centre table (#424,
/// #425) could not quietly change what any other file shows.
#[test]
fn a_centre_without_a_table_resolves_as_the_master_set() {
    // JMA, NASA, and one that does not exist. None has a table; ECMWF (98),
    // DWD (78) and NCEP (7) all do, and belong in the sweep below instead.
    for centre in [34u16, 173, 60_000] {
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

/// A centre table may differ from the master set **only** inside local code
/// space. One that shadowed a standard parameter would be a silent regression
/// for every file from that centre, so this sweeps the whole triple space
/// rather than spot-checking.
///
/// The floors are floors rather than exact counts: two ECMWF entries are gated
/// on table version 228 and so do not resolve at the version 1 used here, and
/// `ecmwf_local_parameters.rs` and `dwd_local_parameters.rs` pin both tables
/// entry by entry anyway. What this adds is the negative half — that nothing
/// outside 192-254 moved.
#[test]
fn a_centre_table_never_changes_a_standard_triple() {
    for (centre, name, floor) in [
        (98u16, "ECMWF", 2_500usize),
        (78, "DWD", 200),
        (7, "NCEP", 450),
    ] {
        let mut local_hits = 0usize;
        for discipline in 0..=255u8 {
            for category in 0..=255u8 {
                for number in 0..=255u8 {
                    let master = lookup_parameter(MASTER_ONLY, discipline, category, number);
                    let local = lookup_parameter(
                        Originator::new(centre, 0, 1),
                        discipline,
                        category,
                        number,
                    );
                    let local_space = [discipline, category, number]
                        .iter()
                        .any(|&c| (192..=254).contains(&c));
                    if local_space {
                        if local != master {
                            local_hits += 1;
                        }
                    } else {
                        assert_eq!(
                            local, master,
                            "{discipline}/{category}/{number} is standard code space, but \
                             {name} resolves it differently"
                        );
                    }
                }
            }
        }
        assert!(
            local_hits > floor,
            "only {local_hits} triples resolve through the {name} table — it is not wired in"
        );
    }
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
    // And an ECMWF triple means nothing to a different centre — including ones
    // that have tables of their own, which is the case a single-table registry
    // could not distinguish from "no table at all". `(192, 128, 4)` is in
    // ECMWF's local discipline space, which neither DWD nor NCEP uses at all.
    assert!(lookup_parameter(Originator::new(98, 0, 0), 192, 128, 4).is_some());
    for other in [7u16, 78, 34] {
        assert_eq!(
            lookup_parameter(Originator::new(other, 0, 0), 192, 128, 4),
            None
        );
    }
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
    // subset, so both halves of the fallback chain are covered. Its name and
    // units are WMO's; the abbreviation is joined on from wgrib2, since WMO
    // publishes none of its own (#469).
    assert_eq!(
        lookup_parameter(MASTER_ONLY, 0, 19, 1),
        Some(("ALBDO", "Albedo", "%"))
    );
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

/// Local code space is genuinely contested, which is what makes the dispatch
/// load-bearing rather than decorative.
///
/// With three centre tables in, 120 triples mean different things to different
/// centres — `(0, 0, 192)` is DWD's `DT_CON` and NCEP's `SNOHF`, `(0, 1, 195)`
/// is DWD's `CLW_CON` and NCEP's `CSNOW`. Before #426 there were far fewer, and
/// two tests had quietly baked in "at most one centre defines any triple";
/// both had to be rewritten. Asserting the contested count keeps that
/// assumption from being made a third time, and makes the number visible to
/// whoever adds the fourth table.
#[test]
fn local_code_space_is_contested_between_centres() {
    let centres = [(98u16, "ECMWF"), (78, "DWD"), (7, "NCEP")];
    let mut contested = 0usize;
    for discipline in 0..=255u8 {
        for category in 0..=255u8 {
            for number in 0..=255u8 {
                if ![discipline, category, number]
                    .iter()
                    .any(|&c| (192..=254).contains(&c))
                {
                    continue;
                }
                let answers: Vec<_> = centres
                    .iter()
                    .filter_map(|(code, _)| {
                        lookup_parameter(Originator::new(*code, 0, 1), discipline, category, number)
                    })
                    .collect();
                if answers.len() > 1 {
                    contested += 1;
                }
            }
        }
    }
    assert!(
        contested >= 100,
        "only {contested} triples are claimed by more than one centre — either a \
         table is not wired in, or local code space stopped overlapping"
    );

    // Spot-checks with the values written out, because "contested" is only
    // meaningful if the answers really are different quantities.
    for (discipline, category, number, dwd, ncep) in [
        (0u8, 0u8, 192u8, "DT_CON", "SNOHF"),
        (0, 1, 195, "CLW_CON", "CSNOW"),
        (0, 3, 192, "PP", "MSLET"),
    ] {
        let of = |centre| {
            lookup_parameter(Originator::new(centre, 0, 1), discipline, category, number)
                .unwrap_or_else(|| {
                    panic!("centre {centre} does not define {discipline}/{category}/{number}")
                })
                .0
        };
        assert_eq!(of(78), dwd);
        assert_eq!(of(7), ncep);
    }
}
