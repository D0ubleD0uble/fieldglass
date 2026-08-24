//! Centre-local GRIB2 parameter tables (#439).
//!
//! WMO reserves discipline, category and parameter-number codes 192-254 for
//! local use, so the same triple means different things depending on who wrote
//! the file. Resolving one needs the originating centre, which is what
//! [`Originator`] carries in from §1.
//!
//! #439 landed this registry empty so the generators behind it would be pure
//! data changes — a generated module and one `match` arm here, with no
//! call-site churn and nothing to re-review about dispatch. ECMWF (#424) is
//! the first to arrive; DWD/ICON (#425) and NCEP (#426) follow.
//!
//! # What the centre tables actually look like
//!
//! Surveying the eccodes local concepts before building this seam turned up a
//! split that matters for what plugs in here:
//!
//! | Centre | local entries | keyed by the triple alone? |
//! |---|---|---|
//! | ECMWF (#424) | 3,320 of 3,360 sit at 192+ | yes |
//! | NCEP (#426) | 314 of 319 sit at 192+ | yes |
//! | DWD/ICON (#425) | 1,069 of 1,827 sit **below** 192 | **no** |
//!
//! ECMWF and NCEP fit this seam as it stands. DWD does not, twice over: most
//! of its table sits on standard codes this seam never routes here, and 158 of
//! its triples map to more than one parameter — `(0, 0, 0)` alone has 17
//! candidates, separated by `typeOfFirstFixedSurface`,
//! `scaledValueOfFirstFixedSurface`, `typeOfStatisticalProcessing` and
//! `typeOfGeneratingProcess`. Resolving those needs §4 context this signature
//! does not carry, which is why [`Originator`] is a struct: adding a field is
//! not a breaking change for the centres that do fit.
//!
//! ECMWF has the same shape in miniature — 54 of its 3,522 routable triples
//! constrain §4 keys or are claimed by several blocks — and the generator drops
//! exactly those. The result is a table that is silent wherever eccodes is
//! silent, rather than one that guesses; see `tools/gen_localconcepts_tables.py`.

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
        _ => None,
    }
}

/// WMO Common Code Table C-11 code for ECMWF.
const CENTRE_ECMWF: u16 = 98;
