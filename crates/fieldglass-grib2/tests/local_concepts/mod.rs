//! Shared plumbing for the centre-local parameter oracles (#424, #425).
//!
//! Each centre table generated from eccodes' `localConcepts` ships beside a
//! snapshot of what eccodes' own concept *engine* answers for the same triples,
//! collected by driving `grib_set`/`grib_get` over a committed fixture. The
//! table comes from parsing the definition *text*, so the two are independent
//! transcriptions of one source and their agreeing is the point — a regex over
//! definition files is exactly the kind of thing that silently mis-joins a
//! table, and #415 showed what an unverified table costs.
//!
//! Committed, so the suite needs no eccodes at runtime. Regenerate with
//! `python3 tools/gen_localconcepts_tables.py <centre> --oracle && cargo fmt`.

use fieldglass_grib2::{Originator, lookup_parameter};
use std::collections::BTreeMap;

/// The eccodes the repo pins. A snapshot from any other version is not
/// comparable, so it is recorded and checked rather than assumed.
const PINNED_ECCODES: &str = "2.34.1";

/// One centre's `--oracle` snapshot, parsed.
pub(crate) struct Oracle {
    /// WMO Common Code Table C-11 code the snapshot was collected under. The
    /// oracle sets §1 `centre` because that is what selects the `localConcepts`
    /// directory; if it disagreed with what `tables_local` dispatches on, the
    /// snapshot would be describing a different centre's table.
    pub centre_code: u16,
    /// `"<local tables version>/<discipline>/<category>/<number>"` -> the
    /// `(shortName, name, units)` eccodes printed, one `grib_get` pass per key
    /// so that a name or unit containing spaces stays one field.
    pub entries: BTreeMap<String, (String, String, String)>,
}

impl Oracle {
    pub(crate) fn load(json: &str) -> Self {
        let parsed: serde_json::Value = serde_json::from_str(json).expect("oracle is valid JSON");
        assert_eq!(
            parsed["eccodes"].as_str(),
            Some(PINNED_ECCODES),
            "the oracle was built with a different eccodes than the repo pins"
        );
        Self {
            centre_code: parsed["centreCode"]
                .as_u64()
                .expect("centreCode section")
                .try_into()
                .expect("a C-11 centre code fits in u16"),
            entries: parsed["resolved"]
                .as_object()
                .expect("resolved section")
                .iter()
                .map(|(k, v)| {
                    let field = |i: usize| unset(v[i].as_str().expect("string")).to_string();
                    (k.clone(), (field(0), field(1), field(2)))
                })
                .collect(),
        }
    }

    /// Every entry the table ships resolves to exactly what eccodes reports,
    /// field by field — including the stray leading and trailing spaces eight
    /// DWD names carry upstream, which is what "the table is what eccodes says"
    /// has to mean if it is to mean anything.
    ///
    /// `minimum` is what stops a snapshot that failed to load from passing
    /// vacuously; set it just under the table's real size.
    pub(crate) fn assert_every_entry_matches(&self, minimum: usize) {
        assert!(
            self.entries.len() > minimum,
            "only {} oracle entries — it is not loaded, so this proves nothing",
            self.entries.len()
        );
        for (raw, expected) in &self.entries {
            let (version, discipline, category, number) = key(raw);
            let (short, name, units) = lookup_parameter(
                Originator::new(self.centre_code, 0, version),
                discipline,
                category,
                number,
            )
            .unwrap_or_else(|| panic!("{raw} is in the eccodes oracle but does not resolve"));
            assert_eq!(
                (short, name, units),
                (
                    expected.0.as_str(),
                    expected.1.as_str(),
                    expected.2.as_str()
                ),
                "{raw} disagrees with eccodes"
            );
        }
    }

    /// eccodes resolved every triple the table ships — no entry was emitted
    /// that eccodes itself cannot reach from the centre, table version and
    /// triple alone.
    ///
    /// This is what makes the skip rule in the generator load-bearing rather
    /// than cosmetic: an entry constrained by §4 keys would show up here as
    /// `unknown`. The generator refuses to write such a snapshot; this is the
    /// check that survives a hand-edited one.
    pub(crate) fn assert_nothing_unresolved(&self) {
        let unresolved: Vec<_> = self
            .entries
            .iter()
            .filter(|(_, (short, name, units))| {
                [short, name, units].iter().any(|f| *f == "unknown")
            })
            .map(|(k, _)| k.clone())
            .collect();
        assert!(
            unresolved.is_empty(),
            "eccodes could not resolve {} shipped triples from the triple alone, so the \
             generator emitted entries it should have skipped: {unresolved:?}",
            unresolved.len()
        );
    }

    /// Placeholder names are absent by construction. `Experimental product` is
    /// 635 identical ECMWF entries with no short name and no units; showing
    /// that instead of the numeric triple would be strictly worse for the
    /// reader.
    pub(crate) fn assert_no_placeholder_names(&self) {
        for (raw, (short, name, units)) in &self.entries {
            let (_, discipline, category, number) = key(raw);
            let lowered = name.to_lowercase();
            assert!(
                !lowered.contains("experimental product")
                    && !lowered.contains("reserved")
                    && !lowered.starts_with("dummy_"),
                "{discipline}/{category}/{number} ships a placeholder name: \
                 {short:?} {name:?} {units:?}"
            );
        }
    }

    /// This centre's answer is this centre's alone, and the sub-centre does not
    /// gate it — these tables are centre-wide.
    ///
    /// Note what this does *not* assert. Another centre may define the same
    /// triple, and increasingly does as tables land: `(0, 1, 203)` is DWD's
    /// `FRESHSNW` and NCEP's `RIME`, `(0, 3, 192)` is NCEP's `MSLET` and DWD's
    /// `PP`. Two centres disagreeing about one triple is the entire reason the
    /// seam is keyed on the centre, so the property is that they get *different*
    /// answers — asserting `None` for everyone else only held while the registry
    /// was nearly empty, and would fail again with every centre added.
    pub(crate) fn assert_scoped_to_its_centre(&self, sample: (u8, u8, u8), others: &[u16]) {
        let (discipline, category, number) = sample;
        let mine = Originator::new(self.centre_code, 0, 0);
        let ours = lookup_parameter(mine, discipline, category, number);
        assert!(
            ours.is_some(),
            "the sample triple {discipline}/{category}/{number} is not in this table"
        );
        for &other in others {
            assert_ne!(
                lookup_parameter(Originator::new(other, 0, 0), discipline, category, number),
                ours,
                "centre {other} read centre {}'s answer for {discipline}/{category}/{number}",
                self.centre_code
            );
        }
        assert_eq!(
            lookup_parameter(mine, discipline, category, number),
            lookup_parameter(
                Originator::new(self.centre_code, 99, 0),
                discipline,
                category,
                number
            ),
            "the sub-centre must not gate a centre-wide table"
        );
    }
}

/// Split an oracle key back into `(version, discipline, category, number)`.
pub(crate) fn key(part: &str) -> (u8, u8, u8, u8) {
    let mut it = part
        .split('/')
        .map(|n| n.parse::<u8>().expect("numeric key part"));
    (
        it.next().unwrap(),
        it.next().unwrap(),
        it.next().unwrap(),
        it.next().unwrap(),
    )
}

/// eccodes has two spellings for "this concept leaves the field unset": ECMWF's
/// files write `~`, which `grib_get` echoes back, while DWD's write `''`, which
/// prints as nothing. The generator stores both as an empty string, so the
/// oracle side is folded to match rather than the table side being taught which
/// centre uses which marker.
fn unset(value: &str) -> &str {
    if value == "~" { "" } else { value }
}
