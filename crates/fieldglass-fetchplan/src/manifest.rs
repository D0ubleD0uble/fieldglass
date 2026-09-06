//! The seam every dialect implements, and the query it answers.

use crate::level::LevelSpec;
use crate::plan::{Expect, ParameterId, PlanItem};

/// Resolve a sidecar's own parameter vocabulary to WMO codes.
///
/// A trait rather than a table because the table is in `fieldglass-grib2`, and
/// a planner that depended on it would drag the GRIB2 decoder and its four
/// codecs into a host that only wanted to know which bytes to ask for. The
/// `fieldglass` umbrella implements this over those tables; this crate stays
/// syntax.
///
/// `level` is passed as well as `abbrev` because NCEP's abbreviations are not
/// unique on their own: `TMP` is (0, 0, 0) wherever it appears, but the
/// local-table parameters a centre defines are disambiguated by the surface
/// they are published on, and a resolver that only saw the short name could not
/// tell them apart.
pub trait ParameterResolver {
    /// The parameter these codes name, or `None` when no table in this build
    /// resolves it.
    fn resolve(&self, abbrev: &str, level: &str) -> Option<ParameterId>;
}

/// A resolver that resolves nothing.
///
/// What a caller passes when its query is purely syntactic — matching the
/// sidecar's own `TMP` / `2 m above ground` and never a WMO triple. Exists so
/// that path needs no table and no umbrella, which is what makes this crate
/// testable on its own.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoResolver;

impl ParameterResolver for NoResolver {
    fn resolve(&self, _abbrev: &str, _level: &str) -> Option<ParameterId> {
        None
    }
}

/// What to select out of a manifest.
///
/// Every stated constraint must hold; an unstated one matches anything, so
/// `Query::default()` selects every record. The two halves are deliberately
/// separate:
///
/// * [`abbreviation`](Self::abbreviation) and [`level_text`](Self::level_text)
///   match the sidecar's **own** words, which is what a user pasting a line out
///   of a `.idx` has.
/// * [`parameter`](Self::parameter) and [`level`](Self::level) match the
///   *meaning*, through a [`ParameterResolver`] and the level grammar, which is
///   what a stored request has — and is the only form that matches on NCEP and
///   ECMWF sources alike.
///
/// **Ambiguity is never resolved by guessing.** A query that matches four
/// records returns four items, each carrying the qualifiers that distinguish it
/// (`ENS=+1`, `prob <304.8`), and the caller picks. Silently taking the first
/// would hand a user one ensemble member and call it the forecast.
#[derive(Debug, Clone, Default, PartialEq)]
#[non_exhaustive]
pub struct Query {
    /// The sidecar's own short name, matched case-insensitively and in full.
    pub abbreviation: Option<String>,
    /// WMO codes, matched against whatever the record resolves to.
    pub parameter: Option<ParameterId>,
    /// The sidecar's own level string, matched case-insensitively and in full.
    pub level_text: Option<String>,
    /// A level, matched through [`LevelSpec::matches`] — so a query naming only
    /// a surface matches every value on it.
    pub level: Option<LevelSpec>,
    /// The sidecar's own forecast field, matched case-insensitively and in
    /// full: `anl`, `6 hour fcst`.
    pub forecast: Option<String>,
    /// Qualifiers that must **all** appear on the record, each matched
    /// case-insensitively and in full against one of the record's own:
    /// `ENS=+1`, `probability forecast`.
    pub qualifiers: Vec<String>,
    /// Require the record to carry **no** qualifiers at all.
    ///
    /// The other half of narrowing an ambiguous request, and not reachable
    /// through [`qualifiers`](Self::qualifiers), which can only add
    /// requirements. NBM publishes a plain deterministic field beside its
    /// probabilistic ones under the same abbreviation and level — a bare
    /// `CEIL:cloud ceiling:1 hour fcst:` next to five `prob <…` records — and
    /// the deterministic one is distinguished precisely by having nothing after
    /// the forecast field. Without this it is the one record in an ambiguous
    /// set that cannot be asked for, which would leave a caller taking the
    /// first match and being right by accident.
    ///
    /// Setting this *and* [`qualifiers`](Self::qualifiers) is a contradiction
    /// and matches nothing, which is the honest answer rather than a panic.
    pub unqualified: bool,
}

impl Query {
    /// A query for one short name, as the sidecar spells it.
    pub fn abbreviation(abbrev: impl Into<String>) -> Self {
        Self {
            abbreviation: Some(abbrev.into()),
            ..Self::default()
        }
    }

    /// A query for one WMO parameter, which resolves on either dialect.
    pub fn parameter(parameter: ParameterId) -> Self {
        Self {
            parameter: Some(parameter),
            ..Self::default()
        }
    }

    /// Narrow to a level.
    #[must_use]
    pub fn at_level(mut self, level: LevelSpec) -> Self {
        self.level = Some(level);
        self
    }

    /// Narrow to the sidecar's own level wording.
    #[must_use]
    pub fn at_level_text(mut self, level: impl Into<String>) -> Self {
        self.level_text = Some(level.into());
        self
    }

    /// Narrow to a forecast field, as the sidecar words it.
    #[must_use]
    pub fn with_forecast(mut self, forecast: impl Into<String>) -> Self {
        self.forecast = Some(forecast.into());
        self
    }

    /// Require a qualifier — an ensemble member, a probability threshold.
    #[must_use]
    pub fn with_qualifier(mut self, qualifier: impl Into<String>) -> Self {
        self.qualifiers.push(qualifier.into());
        self
    }

    /// Require the record to carry no qualifiers, which is how the plain
    /// deterministic field is picked out from the probabilistic ones published
    /// beside it. See [`Query::unqualified`].
    #[must_use]
    pub fn unqualified(mut self) -> Self {
        self.unqualified = true;
        self
    }

    /// Whether one record satisfies every stated constraint.
    ///
    /// `resolved` is what the [`ParameterResolver`] made of the record, passed
    /// in rather than looked up here so a manifest resolves each record once
    /// per `select` rather than once per constraint.
    pub(crate) fn matches(&self, expect: &Expect, resolved: Option<ParameterId>) -> bool {
        if let Some(want) = &self.abbreviation
            && !expect
                .abbreviation
                .as_deref()
                .is_some_and(|have| have.eq_ignore_ascii_case(want))
        {
            return false;
        }
        if let Some(want) = self.parameter
            && resolved != Some(want)
        {
            return false;
        }
        if let Some(want) = &self.level_text
            && !expect
                .level
                .as_deref()
                .is_some_and(|have| have.eq_ignore_ascii_case(want))
        {
            return false;
        }
        if let Some(want) = &self.level
            && !expect
                .level_spec
                .as_ref()
                .is_some_and(|have| want.matches(have))
        {
            return false;
        }
        if let Some(want) = &self.forecast
            && !expect
                .forecast
                .as_deref()
                .is_some_and(|have| have.eq_ignore_ascii_case(want))
        {
            return false;
        }
        if self.unqualified && !expect.qualifiers.is_empty() {
            return false;
        }
        self.qualifiers.iter().all(|want| {
            expect
                .qualifiers
                .iter()
                .any(|have| have.eq_ignore_ascii_case(want))
        })
    }
}

/// One cloud-native manifest, read.
///
/// The dialects differ in grammar and in what they promise, not in what they
/// are for: each says where the bytes of each field are, in one object, and
/// each can be asked which of those fields a request wants.
pub trait Manifest {
    /// The object key these records address, exactly as the caller supplied it.
    fn key(&self) -> &str;

    /// Every record, in the order the manifest wrote them.
    ///
    /// One item per **record**, so a message holding several fields appears once
    /// per field, each with its own
    /// [`sub_index`](crate::PlanItem::sub_index) and the same range. Use
    /// [`messages`](Self::messages) for one item per distinct byte range.
    fn items(&self) -> Vec<PlanItem>;

    /// The records a query selects, in manifest order.
    ///
    /// Every match is returned. See [`Query`] on why an ambiguous request is
    /// not narrowed here.
    fn select(&self, query: &Query, resolver: &dyn ParameterResolver) -> Vec<PlanItem>;

    /// One item per distinct byte range, dropping the sub-message siblings.
    ///
    /// The list a host fetches when it wants whole messages: strictly ascending
    /// and non-overlapping, which [`items`](Self::items) is not, because two
    /// sub-messages of one message carry the identical range.
    ///
    /// A provided method rather than a per-dialect one: "collapse the records
    /// that share a range" is a property of the plan, not of the grammar it was
    /// read from, and a dialect that implemented it itself would be a place for
    /// the two to disagree.
    fn messages(&self) -> Vec<PlanItem> {
        let mut out: Vec<PlanItem> = Vec::new();
        for mut item in self.items() {
            if out.last().is_some_and(|prev| prev.range == item.range) {
                continue;
            }
            // The kept representative addresses the whole message, so it must
            // not claim to be field 1 of it.
            item.sub_index = None;
            out.push(item);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::level::{Surface, parse_ncep_level};
    use crate::plan::PlanRange;

    fn expect(abbrev: &str, level: &str, forecast: &str, quals: &[&str]) -> Expect {
        Expect {
            abbreviation: Some(abbrev.to_string()),
            level: Some(level.to_string()),
            level_spec: Some(parse_ncep_level(level)),
            forecast: Some(forecast.to_string()),
            qualifiers: quals.iter().map(|q| (*q).to_string()).collect(),
            ..Expect::default()
        }
    }

    /// The empty query is the identity: it selects everything. A `matches` that
    /// defaulted a constraint to "no" instead of "any" would make
    /// `Query::default()` select nothing, which is the same code path as a
    /// query whose one constraint failed.
    #[test]
    fn an_empty_query_matches_every_record() {
        assert!(Query::default().matches(&expect("TMP", "surface", "anl", &[]), None));
        assert!(Query::default().matches(&Expect::default(), None));
    }

    /// Verbatim matching is case-insensitive but not partial: `TM` must not
    /// select `TMP`, or a query for a short name would sweep up its longer
    /// neighbours.
    #[test]
    fn a_verbatim_match_is_whole_and_case_insensitive() {
        let record = expect("TMP", "2 m above ground", "anl", &[]);
        assert!(Query::abbreviation("tmp").matches(&record, None));
        assert!(!Query::abbreviation("TM").matches(&record, None));
        assert!(!Query::abbreviation("TMPX").matches(&record, None));
    }

    /// Every stated constraint has to hold, not just one.
    #[test]
    fn constraints_are_conjunctive() {
        let record = expect("TMP", "2 m above ground", "anl", &[]);
        assert!(
            Query::abbreviation("TMP")
                .at_level_text("2 m above ground")
                .matches(&record, None)
        );
        assert!(
            !Query::abbreviation("TMP")
                .at_level_text("surface")
                .matches(&record, None)
        );
        assert!(
            !Query::abbreviation("TMP")
                .with_forecast("6 hour fcst")
                .matches(&record, None)
        );
    }

    /// The deterministic member of an ambiguous set carries no qualifiers, and
    /// is otherwise unaskable-for: `qualifiers` can only add requirements.
    #[test]
    fn an_unqualified_query_picks_the_record_with_nothing_after_the_forecast() {
        let plain = expect("CEIL", "cloud ceiling", "1 hour fcst", &[]);
        let probabilistic = expect(
            "CEIL",
            "cloud ceiling",
            "1 hour fcst",
            &["prob <304.8", "probability forecast"],
        );
        let q = Query::abbreviation("CEIL").unqualified();
        assert!(q.matches(&plain, None));
        assert!(!q.matches(&probabilistic, None));

        // Asking for both is a contradiction, and answers nothing rather than
        // quietly preferring one of the two constraints.
        let contradiction = Query::abbreviation("CEIL")
            .unqualified()
            .with_qualifier("prob <304.8");
        assert!(!contradiction.matches(&plain, None));
        assert!(!contradiction.matches(&probabilistic, None));
    }

    /// A qualifier query requires every named qualifier to be present, and
    /// tolerates the record carrying more.
    #[test]
    fn qualifiers_are_required_not_exclusive() {
        let record = expect(
            "CEIL",
            "cloud ceiling",
            "1 hour fcst",
            &["prob <304.8", "prob fcst 3/7", "probability forecast"],
        );
        assert!(
            Query::abbreviation("CEIL")
                .with_qualifier("probability forecast")
                .matches(&record, None)
        );
        assert!(
            Query::abbreviation("CEIL")
                .with_qualifier("prob <304.8")
                .with_qualifier("probability forecast")
                .matches(&record, None)
        );
        assert!(
            !Query::abbreviation("CEIL")
                .with_qualifier("prob <609.6")
                .matches(&record, None)
        );
    }

    /// A semantic level query matches through the grammar, so "any isobaric
    /// level" selects a record the verbatim form would have missed.
    #[test]
    fn a_semantic_level_query_goes_through_the_grammar() {
        let record = expect("TMP", "850 mb", "anl", &[]);
        assert!(
            Query::abbreviation("TMP")
                .at_level(LevelSpec::named(Surface::Isobaric))
                .matches(&record, None)
        );
        assert!(
            !Query::abbreviation("TMP")
                .at_level(LevelSpec::at(Surface::Isobaric, 500.0))
                .matches(&record, None)
        );
    }

    /// A parameter query is satisfied by what the resolver made of the record,
    /// and refuses a record the resolver could not place — rather than falling
    /// through to "matches anything".
    #[test]
    fn a_parameter_query_needs_the_resolver_to_have_succeeded() {
        let tmp = ParameterId {
            discipline: 0,
            category: 0,
            number: 0,
        };
        let record = expect("TMP", "surface", "anl", &[]);
        assert!(Query::parameter(tmp).matches(&record, Some(tmp)));
        assert!(!Query::parameter(tmp).matches(&record, None));
    }

    struct Two;
    impl Manifest for Two {
        fn key(&self) -> &str {
            "obj"
        }
        fn items(&self) -> Vec<PlanItem> {
            let range = PlanRange::Exact {
                offset: 10,
                length: 5,
            };
            vec![
                PlanItem {
                    key: "obj".into(),
                    range,
                    sub_index: Some(1),
                    expect: expect("UGRD", "10 m above ground", "anl", &[]),
                },
                PlanItem {
                    key: "obj".into(),
                    range,
                    sub_index: Some(2),
                    expect: expect("VGRD", "10 m above ground", "anl", &[]),
                },
                PlanItem {
                    key: "obj".into(),
                    range: PlanRange::OpenEnded { offset: 15 },
                    sub_index: None,
                    expect: expect("TMP", "surface", "anl", &[]),
                },
            ]
        }
        fn select(&self, _q: &Query, _r: &dyn ParameterResolver) -> Vec<PlanItem> {
            unimplemented!("not exercised by this test")
        }
    }

    /// `messages()` collapses the sub-message siblings onto one fetch, and the
    /// representative must not claim to be sub-message 1 — a host handed
    /// `sub_index: Some(1)` for a range covering both fields would decode the
    /// wrong half.
    #[test]
    fn messages_collapses_shared_ranges_and_drops_the_sub_index() {
        let items = Two.messages();
        assert_eq!(items.len(), 2);
        assert_eq!(
            items[0].range,
            PlanRange::Exact {
                offset: 10,
                length: 5
            }
        );
        assert_eq!(items[0].sub_index, None);
        assert_eq!(items[1].range, PlanRange::OpenEnded { offset: 15 });
    }
}
