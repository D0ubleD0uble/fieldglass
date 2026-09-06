//! The ECMWF `.index` sidecar: JSON lines, one object per message.
//!
//! ```text
//! {"domain": "g", "date": "20260904", "time": "0000", "levtype": "sfc",
//!  "step": "0", "param": "2t", "_offset": 224, "_length": 896851}
//! ```
//!
//! The same job as a wgrib2 `.idx` in a different grammar, with one substantive
//! difference: **it states a length**, so every range is exact and the last
//! record is not open-ended. ECMWF also serves `Content-Length` and
//! `Content-Range` cross-origin, so a browser reading this dialect never has to
//! close a range against a size it had to guess.
//!
//! The MARS keys (`class`, `stream`, `type`, `expver`, `domain`, `number`, …)
//! are carried through as qualifiers in `key=value` form rather than modelled.
//! They are the vocabulary that distinguishes an ensemble member or a
//! reforecast from the control, and a query matches them the same way it
//! matches an NCEP `ENS=+1`.

use crate::error::{Dialect, FetchPlanError};
use crate::level::parse_ecmwf_level;
use crate::manifest::{Manifest, ParameterResolver, Query};
use crate::plan::{Expect, PlanItem, PlanRange};

/// The dialect tag every error from this module carries.
const DIALECT: Dialect = Dialect::EcmwfIndex;

/// Keys that become structured fields rather than qualifiers.
///
/// Everything not named here is passed through as a `key=value` qualifier, so a
/// key ECMWF adds later reaches a caller instead of being dropped by a parser
/// that had never heard of it.
const STRUCTURED: &[&str] = &[
    "_offset", "_length", "param", "levtype", "levelist", "step", "date", "time",
];

/// A parsed ECMWF `.index`.
#[derive(Debug, Clone)]
pub struct EcmwfIndex {
    key: String,
    items: Vec<PlanItem>,
}

impl EcmwfIndex {
    /// Parse a sidecar for the object at `key`.
    ///
    /// As with [`Wgrib2Idx`](crate::Wgrib2Idx), `key` is whatever the caller
    /// calls the object; this crate derives none and hard-codes no host.
    pub fn parse(key: impl Into<String>, text: &str) -> Result<Self, FetchPlanError> {
        let key = key.into();
        let mut items = Vec::new();

        for (n, line) in text.lines().enumerate() {
            let line_no = n + 1;
            if line.trim().is_empty() {
                continue;
            }
            let record: serde_json::Map<String, serde_json::Value> = serde_json::from_str(line)
                .map_err(|e| FetchPlanError::Json {
                    line: line_no,
                    detail: e.to_string(),
                })?;
            items.push(to_item(&key, line_no, &record)?);
        }

        Ok(Self { items, key })
    }
}

/// Read one JSON object into a plan item.
fn to_item(
    key: &str,
    line_no: usize,
    record: &serde_json::Map<String, serde_json::Value>,
) -> Result<PlanItem, FetchPlanError> {
    let offset = integer(line_no, record, "_offset")?;
    let length = integer(line_no, record, "_length")?;

    let levtype = string(record, "levtype");
    let levelist = string(record, "levelist");
    // A level string in ECMWF's own words, for the verbatim half of a query.
    // `pl` + `500` reads as `pl 500` rather than as `500 hPa`, because this
    // field is what the *manifest* said and not a translation of it; the
    // translation is `level_spec` beside it.
    let level = levtype.map(|t| match levelist {
        Some(l) => format!("{t} {l}"),
        None => t.to_string(),
    });

    // `date` and `time` are separate keys; joined here into the same ten-digit
    // form a wgrib2 `d=` field carries, so one query can check either dialect's
    // reference time. `time` is `"0000"`, i.e. HHMM, so only its first two
    // digits are the hour.
    let reference_time = match (string(record, "date"), string(record, "time")) {
        (Some(d), Some(t)) => Some(format!("{d}{}", &t[..t.len().min(2)])),
        (Some(d), None) => Some(d.to_string()),
        _ => None,
    };

    let mut qualifiers: Vec<String> = record
        .iter()
        .filter(|(k, _)| !STRUCTURED.contains(&k.as_str()))
        .map(|(k, v)| match v {
            // A string value is written bare (`type=fc`); anything else keeps
            // its JSON rendering, which is lossless and still comparable.
            serde_json::Value::String(s) => format!("{k}={s}"),
            other => format!("{k}={other}"),
        })
        .collect();
    // `serde_json::Map` preserves insertion order only with the `preserve_order`
    // feature, which is not enabled; sorting makes the list deterministic so a
    // qualifier list is stable across runs and comparable in a test.
    qualifiers.sort();

    Ok(PlanItem {
        key: key.to_string(),
        range: PlanRange::Exact { offset, length },
        // The dialect has no sub-message form: each record is one message, and
        // `_offset` is unique per record.
        sub_index: None,
        expect: Expect {
            abbreviation: string(record, "param").map(str::to_string),
            // ECMWF's short names are its own; resolving them to WMO codes is
            // the resolver's job, exactly as for an NCEP abbreviation.
            parameter: None,
            level,
            level_spec: levtype.map(|t| parse_ecmwf_level(t, levelist)),
            forecast: string(record, "step").map(str::to_string),
            reference_time,
            total_length: Some(length),
            qualifiers,
        },
    })
}

/// Read a required non-negative integer.
///
/// ECMWF writes `_offset` and `_length` as JSON numbers, but a sidecar that
/// wrote them as strings would be a perfectly ordinary thing for a producer to
/// change, so both are accepted. A negative one is refused rather than cast: a
/// `u64` cast of `-1` is `u64::MAX`, which becomes a range no server can serve
/// and an error a long way from its cause.
fn integer(
    line_no: usize,
    record: &serde_json::Map<String, serde_json::Value>,
    key: &'static str,
) -> Result<u64, FetchPlanError> {
    let value = record.get(key).ok_or(FetchPlanError::MissingKey {
        dialect: DIALECT,
        line: line_no,
        key,
    })?;
    let as_i64 = match value {
        serde_json::Value::Number(n) => n.as_i64(),
        serde_json::Value::String(s) => s.trim().parse::<i64>().ok(),
        _ => None,
    };
    match as_i64 {
        Some(n) if n >= 0 => Ok(n as u64),
        Some(n) => Err(FetchPlanError::NegativeOffset {
            dialect: DIALECT,
            line: line_no,
            field: key,
            value: n,
        }),
        None => Err(FetchPlanError::NotAnInteger {
            dialect: DIALECT,
            line: line_no,
            field: key,
            value: value.to_string(),
        }),
    }
}

/// Read an optional string-valued key.
fn string<'a>(
    record: &'a serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Option<&'a str> {
    record.get(key).and_then(serde_json::Value::as_str)
}

impl Manifest for EcmwfIndex {
    fn key(&self) -> &str {
        &self.key
    }

    fn items(&self) -> Vec<PlanItem> {
        self.items.clone()
    }

    fn select(&self, query: &Query, resolver: &dyn ParameterResolver) -> Vec<PlanItem> {
        self.items
            .iter()
            .filter(|item| {
                let resolved = resolver.resolve(
                    item.expect.abbreviation.as_deref().unwrap_or_default(),
                    item.expect.level.as_deref().unwrap_or_default(),
                );
                query.matches(&item.expect, resolved)
            })
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::level::{LevelSpec, Surface};
    use crate::manifest::NoResolver;

    const TWO: &str = r#"{"domain": "g", "date": "20260904", "time": "0000", "expver": "0001", "class": "od", "type": "fc", "stream": "oper", "levtype": "sfc", "step": "0", "param": "2t", "_offset": 0, "_length": 224}
{"domain": "g", "date": "20260904", "time": "0000", "expver": "0001", "class": "od", "type": "fc", "stream": "oper", "levtype": "pl", "levelist": "500", "step": "6", "param": "t", "_offset": 224, "_length": 896851}
"#;

    /// Every range is exact, including the last: the dialect states a length,
    /// so nothing here is open-ended.
    #[test]
    fn every_range_is_exact_because_the_dialect_states_a_length() {
        let idx = EcmwfIndex::parse("ifs.grib2", TWO).unwrap();
        let items = idx.items();
        assert_eq!(
            items[0].range,
            PlanRange::Exact {
                offset: 0,
                length: 224
            }
        );
        assert_eq!(
            items[1].range,
            PlanRange::Exact {
                offset: 224,
                length: 896_851
            }
        );
        assert!(items.iter().all(|i| i.range.length().is_some()));
        assert_eq!(items[0].expect.total_length, Some(224));
    }

    /// `levtype` + `levelist` reach the same surfaces an NCEP level string
    /// does, which is what lets one stored request match on either source.
    #[test]
    fn the_level_pair_becomes_the_shared_vocabulary() {
        let items = EcmwfIndex::parse("o", TWO).unwrap().items();
        assert_eq!(
            items[0].expect.level_spec,
            Some(LevelSpec::named(Surface::Surface))
        );
        assert_eq!(
            items[1].expect.level_spec,
            Some(LevelSpec::at(Surface::Isobaric, 500.0))
        );
        // …and the verbatim field is still ECMWF's own words.
        assert_eq!(items[1].expect.level.as_deref(), Some("pl 500"));
    }

    /// `date` and `time` join into the ten-digit form a wgrib2 `d=` carries,
    /// so a reference-time check does not have to know which dialect it read.
    #[test]
    fn the_reference_time_matches_the_other_dialects_shape() {
        let items = EcmwfIndex::parse("o", TWO).unwrap().items();
        assert_eq!(
            items[0].expect.reference_time.as_deref(),
            Some("2026090400")
        );
    }

    /// The MARS keys survive as qualifiers, sorted so the list is stable, and
    /// the structured ones do not appear twice.
    #[test]
    fn mars_keys_pass_through_as_sorted_qualifiers() {
        let items = EcmwfIndex::parse("o", TWO).unwrap().items();
        assert_eq!(
            items[0].expect.qualifiers,
            [
                "class=od",
                "domain=g",
                "expver=0001",
                "stream=oper",
                "type=fc"
            ]
        );
        assert!(
            !items[0]
                .expect
                .qualifiers
                .iter()
                .any(|q| q.starts_with("param=") || q.starts_with("_offset="))
        );
    }

    /// A key this parser has never seen still reaches the caller, rather than
    /// being dropped by the parser that did not recognise it.
    #[test]
    fn an_unknown_key_is_carried_not_dropped() {
        let line = r#"{"param":"2t","levtype":"sfc","_offset":0,"_length":10,"number":"3","quantile":"5:10"}"#;
        let items = EcmwfIndex::parse("o", line).unwrap().items();
        assert!(items[0].expect.qualifiers.contains(&"number=3".to_string()));
        assert!(
            items[0]
                .expect
                .qualifiers
                .contains(&"quantile=5:10".to_string())
        );
    }

    /// Selection matches ECMWF's own short names, and the semantic level form
    /// works across both dialects.
    #[test]
    fn selection_matches_ecmwfs_own_vocabulary() {
        let idx = EcmwfIndex::parse("o", TWO).unwrap();
        assert_eq!(idx.select(&Query::abbreviation("2t"), &NoResolver).len(), 1);
        let hits = idx.select(
            &Query::abbreviation("t").at_level(LevelSpec::at(Surface::Isobaric, 500.0)),
            &NoResolver,
        );
        assert_eq!(hits.len(), 1);
        assert_eq!(
            hits[0].range,
            PlanRange::Exact {
                offset: 224,
                length: 896_851
            }
        );
    }

    /// A producer that wrote the offsets as strings is still readable; a
    /// negative one is refused rather than cast into `u64::MAX`.
    #[test]
    fn offsets_may_be_strings_but_must_not_be_negative() {
        let as_strings = r#"{"param":"2t","_offset":"100","_length":"20"}"#;
        assert_eq!(
            EcmwfIndex::parse("o", as_strings).unwrap().items()[0].range,
            PlanRange::Exact {
                offset: 100,
                length: 20
            }
        );
        assert_eq!(
            EcmwfIndex::parse("o", r#"{"param":"2t","_offset":-1,"_length":20}"#).unwrap_err(),
            FetchPlanError::NegativeOffset {
                dialect: Dialect::EcmwfIndex,
                line: 1,
                field: "_offset",
                value: -1
            }
        );
    }

    /// A missing required key names the key, and malformed JSON names the line.
    #[test]
    fn a_missing_key_and_bad_json_are_both_located() {
        assert_eq!(
            EcmwfIndex::parse("o", r#"{"param":"2t","_length":20}"#).unwrap_err(),
            FetchPlanError::MissingKey {
                dialect: Dialect::EcmwfIndex,
                line: 1,
                key: "_offset"
            }
        );
        let err = EcmwfIndex::parse("o", "{\n{not json\n").unwrap_err();
        assert!(
            matches!(err, FetchPlanError::Json { line: 1, .. }),
            "{err:?}"
        );
    }

    /// A record with no `levtype` at all has no level to match on, and must not
    /// acquire a fabricated one.
    #[test]
    fn a_record_without_a_level_has_none() {
        let line = r#"{"param":"2t","_offset":0,"_length":10}"#;
        let items = EcmwfIndex::parse("o", line).unwrap().items();
        assert_eq!(items[0].expect.level, None);
        assert_eq!(items[0].expect.level_spec, None);
    }
}
