//! Centre-local GRIB2 parameter tables (#439).
//!
//! WMO reserves discipline, category and parameter-number codes 192-254 for
//! local use, so the same triple means different things depending on who wrote
//! the file. Resolving one needs the originating centre, which is what
//! [`Originator`] carries in from §1.
//!
//! #439 landed this registry empty so the generators behind it would be pure
//! data changes — a generated module and one `match` arm here, with no
//! call-site churn and nothing to re-review about dispatch. ECMWF (#424),
//! DWD/ICON (#425) and NCEP (#426) have all arrived.
//!
//! # What reaches this seam, and what does not
//!
//! Surveying the eccodes local concepts before building this seam turned up a
//! split that matters for what plugs in here. These are the numbers
//! `tools/gen_localconcepts_tables.py` reports at eccodes 2.34.1 — concept
//! blocks in the first two columns, distinct triples in the rest, because a
//! triple several blocks claim is one triple and several blocks:
//!
//! | Centre | blocks | outside 192-254 | emitted | §4-keyed | ambiguous | placeholder |
//! |---|---|---|---|---|---|---|
//! | ECMWF (#424) | 3,601 | 63 | 2,826 | 48 | 6 | 642 |
//! | DWD/ICON (#425) | 1,704 | 983 | 213 | 100 | 50 | 254 |
//!
//! NCEP is not in that table because it does not come from eccodes: wgrib2's
//! `gribtable.dat` carries 479 routable entries where eccodes' `kwbc` concepts
//! carry 313, a strict superset, with NCEP's own uppercase abbreviations. See
//! `tools/gen_ncep_tables.py`.
//!
//! ECMWF fits this seam almost whole. DWD does not, and the reason is worth
//! keeping: most of its table sits on *standard* codes, which the ≥192 rule
//! never routes to a centre, and 50 of its remaining triples are claimed by
//! several blocks at once — `(0, 0, 0)` alone has 17 candidates, separated by
//! `typeOfFirstFixedSurface`, `scaledValueOfFirstFixedSurface`,
//! `typeOfStatisticalProcessing` and `typeOfGeneratingProcess`. Resolving those
//! needs §4 context this signature does not carry, which is why [`Originator`]
//! is a struct: adding a field is not a breaking change for what already fits.
//!
//! So the DWD table names the genuinely local part of ICON — tendencies,
//! sub-grid diagnostics, `FRESHSNW`, `CLCT_MOD` — and leaves `T_2M`,
//! `TOT_PREC`, `PMSL` and the rest to the WMO master set, which already names
//! them correctly. The generator drops exactly what it cannot key, so the table
//! is silent wherever eccodes is silent rather than guessing; see
//! `tools/gen_localconcepts_tables.py`.

use crate::tables::Originator;

/// A centre with a local parameter table.
///
/// [`LOCAL_TABLE_CENTRES`] and the `match` inside [`lookup`] are generated from
/// one list by [`local_table_registry!`], so a centre cannot appear in one and
/// not the other. That is the point (#476): the unit sweep in
/// `tests/unit_notation.rs` has to visit every centre to pin every unit string,
/// and when it kept its own list, adding a table without adding it there left
/// that centre's whole unit vocabulary silently unpinned.
///
/// A macro rather than a runtime registry of function pointers, because the
/// indirect call that shape needs cannot be inlined: measured on the sweep, it
/// took the unit-notation test from 0.4 s to 12.6 s, and every local-code
/// lookup in production would have paid the same.
///
/// The guarantee is exact rather than absolute, and worth stating as such: a
/// centre reached *through this module* cannot be missing from the registry.
/// A table wired straight into `tables.rs` would bypass both. Nothing does
/// today — `tables_local::lookup` is the sole caller of every `tables_*::lookup`
/// — and the generated tables are `pub(crate)`, so it would take a deliberate
/// second dispatch path to break.
#[derive(Debug, Clone, Copy)]
pub struct LocalTableCentre {
    /// WMO Common Code Table C-11 code. Sub-centres share it — NBM is
    /// sub-centre 14 under NCEP — which is why nothing here keys on one.
    pub centre: u16,
    /// The local table versions this centre gates entries on, beyond the
    /// ungated ones every version sees. Empty for a centre that gates nothing,
    /// which is the common case. A caller that wants to reach every entry
    /// visits version 0 plus each of these.
    ///
    /// Generated from the emitted rows, not hand-kept, so it cannot drift from
    /// the table it describes.
    pub gated_versions: &'static [u8],
}

/// Declare the centre tables once, and get the registry and the dispatch.
macro_rules! local_table_registry {
    ($( $(#[$doc:meta])* $centre:literal => $lookup:expr, $versions:expr );* $(;)?) => {
        /// Every centre with a local parameter table. Adding one is a row in
        /// the macro invocation below — there is nowhere else to add it.
        pub const LOCAL_TABLE_CENTRES: &[LocalTableCentre] = &[
            $( LocalTableCentre { centre: $centre, gated_versions: $versions }, )*
        ];

        /// Look up a centre-local parameter, or `None` when the centre has no
        /// table or no entry for the triple.
        ///
        /// Returns the same `(short_name, long_name, units)` shape as the
        /// master lookup, so a local entry can supply the abbreviation WMO does
        /// not publish.
        pub(crate) fn lookup(
            originator: Originator,
            discipline: u8,
            category: u8,
            number: u8,
        ) -> Option<(&'static str, &'static str, &'static str)> {
            match originator.centre {
                $( $centre => ($lookup)(
                    originator.local_tables_version,
                    discipline,
                    category,
                    number,
                ), )*
                _ => None,
            }
        }
    };
}

local_table_registry! {
    /// ECMWF (#424).
    98 => crate::tables_ecmf::lookup, crate::tables_ecmf::GATED_VERSIONS;
    /// Offenbach (RSMC) - DWD, eccodes concept directory `edzw` (#425).
    78 => crate::tables_edzw::lookup, crate::tables_edzw::GATED_VERSIONS;
    /// US National Weather Service, NCEP (#426). Its table takes no version:
    /// wgrib2 records one for its own bookkeeping, eccodes' concepts gate none
    /// of them, and every NCEP sample in the tree declares 1 anyway.
    7 => |_version, discipline, category, number| {
        crate::tables_ncep::lookup(discipline, category, number)
    }, crate::tables_ncep::GATED_VERSIONS;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The registry and the dispatch agree, which the macro makes true by
    /// construction — asserted anyway, because "by construction" is what every
    /// enumeration that later went stale was also said to be.
    #[test]
    fn every_registered_centre_actually_resolves_something() {
        assert!(
            LOCAL_TABLE_CENTRES.len() >= 3,
            "the registry is empty or truncated, so this proves nothing"
        );
        for table in LOCAL_TABLE_CENTRES {
            let versions: Vec<u8> = std::iter::once(0)
                .chain(table.gated_versions.iter().copied())
                .collect();
            // Sweep discipline 0 whole rather than only numbers at or above
            // 192: a triple is local if *any* component is, and ECMWF's
            // version-228 entries sit at `(0, 254, 134)` — local by category,
            // with a number well below the threshold. A probe that assumed the
            // number carried it missed them, which is how this test found its
            // own blind spot before it found anyone else's.
            let resolved = versions.iter().any(|&version| {
                let originator = Originator::new(table.centre, 0, version);
                (0u8..=255).any(|category| {
                    (0u8..=255).any(|number| lookup(originator, 0, category, number).is_some())
                })
            });
            assert!(
                resolved,
                "centre {} is registered but resolves nothing in local code space",
                table.centre
            );
        }
    }

    /// A centre's gated versions really are gated: each names at least one
    /// entry version 0 cannot reach. An empty list is fine — most centres gate
    /// nothing — but a listed version that adds nothing is a stale entry.
    #[test]
    fn gated_versions_reach_entries_version_zero_cannot() {
        for table in LOCAL_TABLE_CENTRES {
            for &version in table.gated_versions {
                let ungated = Originator::new(table.centre, 0, 0);
                let gated = Originator::new(table.centre, 0, version);
                let differs = (0u8..=255).any(|category| {
                    (0u8..=255).any(|number| {
                        lookup(gated, 0, category, number) != lookup(ungated, 0, category, number)
                    })
                });
                assert!(
                    differs,
                    "centre {} lists version {version} as gated, but it reaches \
                     nothing version 0 does not",
                    table.centre
                );
            }
        }
    }
}
