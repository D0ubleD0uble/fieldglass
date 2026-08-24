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

/// Look up a centre-local parameter, or `None` when the centre has no table or
/// no entry for the triple.
///
/// Returns the same `(short_name, long_name, units)` shape as the master
/// lookup, so a local entry can supply the abbreviation WMO does not publish.
pub(crate) fn lookup(
    originator: Originator,
    discipline: u8,
    category: u8,
    number: u8,
) -> Option<(&'static str, &'static str, &'static str)> {
    match originator.centre {
        CENTRE_ECMWF => crate::tables_ecmf::lookup(
            originator.local_tables_version,
            discipline,
            category,
            number,
        ),
        CENTRE_DWD => crate::tables_edzw::lookup(
            originator.local_tables_version,
            discipline,
            category,
            number,
        ),
        // NCEP's table takes no version: wgrib2 records one for its own
        // bookkeeping, eccodes' concepts gate none of them, and every NCEP
        // sample in the tree declares 1 anyway.
        CENTRE_NCEP => crate::tables_ncep::lookup(discipline, category, number),
        _ => None,
    }
}

/// WMO Common Code Table C-11 code for ECMWF.
const CENTRE_ECMWF: u16 = 98;

/// WMO Common Code Table C-11 code for Offenbach (RSMC) - DWD, whose eccodes
/// concept directory is `edzw`.
const CENTRE_DWD: u16 = 78;

/// WMO Common Code Table C-11 code for the US National Weather Service, NCEP.
/// Its sub-centres — NBM is 14, and there are a dozen more — share it, which is
/// why nothing here keys on the sub-centre.
const CENTRE_NCEP: u16 = 7;
