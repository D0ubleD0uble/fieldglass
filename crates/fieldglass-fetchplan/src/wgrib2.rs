//! The wgrib2 `.idx` sidecar: NOAA's convention, and the one kerchunk and
//! VirtualiZarr consume.
//!
//! A few kilobytes of text beside a multi-gigabyte object, one line per field:
//!
//! ```text
//! 1:0:d=2026090400:PRMSL:mean sea level:anl:
//! 2:875084:d=2026090400:CLMR:1 hybrid level:anl:
//! ```
//!
//! Colon-separated, six fields and then however many qualifiers the product
//! adds. What it states is the **offset** of every message and the length of
//! none: a message ends where the next one begins, and the last one ends at the
//! end of the object — which is why [`PlanRange::OpenEnded`] exists.
//!
//! Three shapes in real sidecars drive the parsing, and each is covered by a
//! committed fixture:
//!
//! * **Sub-messages.** A record number `13.1` means field 1 of message 13, and
//!   every record of message 13 carries message 13's offset. NAM and RAP files
//!   pair `UGRD`/`VGRD` this way. They are one fetch and two fields.
//! * **Qualifiers.** GEFS appends `ENS=+1`; NBM appends a probability
//!   threshold, a member count and `probability forecast`. The vocabulary is
//!   per-product, so they are carried as an ordered list rather than modelled.
//! * **Numeric parameters.** For a parameter its own tables do not name, wgrib2
//!   writes `var discipline=0 center=7 local_table=1 parmcat=16 parm=201` in
//!   place of the short name — the one case where the sidecar is *more*
//!   precise than an abbreviation, and worth reading rather than treating as an
//!   opaque string.

use crate::error::{Dialect, FetchPlanError};
use crate::level::parse_ncep_level;
use crate::manifest::{Manifest, ParameterResolver, Query};
use crate::plan::{Expect, ParameterId, PlanItem, PlanRange};

/// The dialect tag every error from this module carries.
const DIALECT: Dialect = Dialect::Wgrib2Idx;

/// The fields the grammar requires: record, offset, `d=`, name, level,
/// forecast.
const REQUIRED_FIELDS: usize = 6;

/// A parsed wgrib2 `.idx`.
#[derive(Debug, Clone)]
pub struct Wgrib2Idx {
    key: String,
    items: Vec<PlanItem>,
}

/// One line, before the ranges are worked out.
///
/// Ranges cannot be assigned while reading, because a record's end is the
/// *next* record's offset — so the lines are collected first and the arithmetic
/// runs over the whole list.
#[derive(Debug)]
struct Record {
    offset: u64,
    sub_index: Option<u32>,
    expect: Expect,
}

impl Wgrib2Idx {
    /// Parse a sidecar for the object at `key`.
    ///
    /// `key` is whatever the caller calls the object — a bucket key, a URL, a
    /// path. This crate never derives one (dropping a `.idx` suffix would be a
    /// guess about a naming convention) and hard-codes no bucket.
    ///
    /// Blank lines are skipped; anything else must parse. A sidecar is machine
    /// output, so a line that does not fit the grammar means the file is not
    /// the index it was taken for, and reading on would produce ranges built
    /// from whatever did parse.
    pub fn parse(key: impl Into<String>, text: &str) -> Result<Self, FetchPlanError> {
        let key = key.into();
        let mut records: Vec<Record> = Vec::new();

        for (n, line) in text.lines().enumerate() {
            let line_no = n + 1;
            let line = line.trim_end_matches(['\r', '\n']);
            if line.trim().is_empty() {
                continue;
            }
            let record = parse_line(line_no, line)?;
            if let Some(previous) = records.last()
                && record.offset < previous.offset
            {
                return Err(FetchPlanError::DescendingOffset {
                    line: line_no,
                    offset: record.offset,
                    previous_offset: previous.offset,
                });
            }
            records.push(record);
        }

        Ok(Self {
            items: to_items(&key, &records),
            key,
        })
    }
}

/// Turn parsed records into plan items, resolving each message's end.
///
/// A message runs from its offset to the offset of the next record that has a
/// *different* offset. Records sharing an offset are sub-messages of one
/// message and all get that message's range; the final group has no successor
/// and is open-ended.
fn to_items(key: &str, records: &[Record]) -> Vec<PlanItem> {
    records
        .iter()
        .enumerate()
        .map(|(i, record)| {
            let next = records[i + 1..]
                .iter()
                .find(|later| later.offset != record.offset)
                .map(|later| later.offset);
            let range = match next {
                // `saturating_sub` cannot fire: offsets are checked to be
                // non-descending on the way in, and `next` is by construction
                // a *different* offset, so it is strictly greater. It is here
                // so a future edit to that invariant degrades to a zero-length
                // range rather than to a panic in a host.
                Some(end) => PlanRange::Exact {
                    offset: record.offset,
                    length: end.saturating_sub(record.offset),
                },
                None => PlanRange::OpenEnded {
                    offset: record.offset,
                },
            };
            PlanItem {
                key: key.to_string(),
                range,
                sub_index: record.sub_index,
                expect: record.expect.clone(),
            }
        })
        .collect()
}

/// Parse one line into a record.
fn parse_line(line_no: usize, line: &str) -> Result<Record, FetchPlanError> {
    let fields: Vec<&str> = line.split(':').collect();
    if fields.len() < REQUIRED_FIELDS {
        return Err(FetchPlanError::ShortRecord {
            dialect: DIALECT,
            line: line_no,
            expected: REQUIRED_FIELDS,
            found: fields.len(),
        });
    }

    let sub_index = parse_record_number(line_no, fields[0])?;
    let offset = fields[1]
        .parse::<u64>()
        .map_err(|_| FetchPlanError::NotAnInteger {
            dialect: DIALECT,
            line: line_no,
            field: "offset",
            value: fields[1].to_string(),
        })?;
    let reference_time =
        fields[2]
            .strip_prefix("d=")
            .ok_or_else(|| FetchPlanError::MissingDatePrefix {
                line: line_no,
                value: fields[2].to_string(),
            })?;

    let name = fields[3];
    let level = fields[4];
    let forecast = fields[5];
    // Everything past the forecast is a qualifier. wgrib2 terminates most lines
    // with a colon, which `split` turns into a trailing empty field; GEFS's
    // `ENS=+1` lines have no terminator. Dropping empties covers both without
    // the parser having to know which product it is reading.
    let qualifiers: Vec<String> = fields[REQUIRED_FIELDS..]
        .iter()
        .filter(|f| !f.trim().is_empty())
        .map(|f| (*f).to_string())
        .collect();

    Ok(Record {
        offset,
        sub_index,
        expect: Expect {
            abbreviation: Some(name.to_string()),
            parameter: parse_numeric_parameter(name),
            level: Some(level.to_string()),
            level_spec: Some(parse_ncep_level(level)),
            forecast: Some(forecast.to_string()),
            reference_time: Some(reference_time.to_string()),
            // A `.idx` states no length, ever. Left `None` rather than derived
            // from the next offset, because the two are not the same claim: the
            // gap is what the *sidecar's arithmetic* implies, and `total_length`
            // is what the manifest *said*. Verification compares the message's
            // own §0 length against the fetch either way.
            total_length: None,
            qualifiers,
        },
    })
}

/// Read a `n` or `n.m` record number, returning the sub-message index.
///
/// The message number itself is not kept: it is the record's position in the
/// file, which the plan already expresses as an offset, and a sidecar that
/// disagreed with itself about it would not change which bytes to fetch.
fn parse_record_number(line_no: usize, field: &str) -> Result<Option<u32>, FetchPlanError> {
    let bad = || FetchPlanError::BadRecordNumber {
        line: line_no,
        value: field.to_string(),
    };
    match field.split_once('.') {
        None => {
            field.parse::<u32>().map_err(|_| bad())?;
            Ok(None)
        }
        Some((message, sub)) => {
            message.parse::<u32>().map_err(|_| bad())?;
            let sub = sub.parse::<u32>().map_err(|_| bad())?;
            Ok(Some(sub))
        }
    }
}

/// Read wgrib2's numeric fallback name into WMO codes.
///
/// `var discipline=0 center=7 local_table=1 parmcat=16 parm=201` — what wgrib2
/// writes when its own tables do not name the parameter. `center` and
/// `local_table` are read past rather than kept: [`ParameterId`] is the WMO
/// triple, and a local table's identity belongs with the abbreviation, which is
/// carried verbatim alongside.
///
/// Returns `None` for an ordinary short name, and for a malformed `var ` line —
/// a partial parse would be worse than none, since it would claim a parameter
/// identity the sidecar did not state.
fn parse_numeric_parameter(name: &str) -> Option<ParameterId> {
    let rest = name.strip_prefix("var ")?;
    let field = |key: &str| -> Option<u8> {
        rest.split_whitespace()
            .find_map(|token| token.strip_prefix(key))
            .and_then(|v| v.parse::<u8>().ok())
    };
    Some(ParameterId {
        discipline: field("discipline=")?,
        category: field("parmcat=")?,
        number: field("parm=")?,
    })
}

impl Manifest for Wgrib2Idx {
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
                // The line's own codes win over the resolver's: wgrib2 wrote
                // them because its tables could not name the parameter, so a
                // resolver asked for the same abbreviation would fail too.
                let resolved = item.expect.parameter.or_else(|| {
                    resolver.resolve(
                        item.expect.abbreviation.as_deref().unwrap_or_default(),
                        item.expect.level.as_deref().unwrap_or_default(),
                    )
                });
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

    const THREE: &str = "\
1:0:d=2026090400:PRMSL:mean sea level:anl:
2:875084:d=2026090400:CLMR:1 hybrid level:anl:
3:985989:d=2026090400:TMP:2 m above ground:anl:
";

    /// Each message runs to the next offset, and the last one is open-ended
    /// because a `.idx` does not state the object's size.
    #[test]
    fn a_message_ends_where_the_next_one_begins() {
        let idx = Wgrib2Idx::parse("gfs.grib2", THREE).unwrap();
        let items = idx.items();
        assert_eq!(
            items[0].range,
            PlanRange::Exact {
                offset: 0,
                length: 875_084
            }
        );
        assert_eq!(
            items[1].range,
            PlanRange::Exact {
                offset: 875_084,
                length: 110_905
            }
        );
        assert_eq!(items[2].range, PlanRange::OpenEnded { offset: 985_989 });
        assert_eq!(idx.key(), "gfs.grib2");
    }

    /// The `n.m` shape: one message, two fields, one fetch. Both records carry
    /// the message's range and differ only in `sub_index` — a parser that
    /// treated them as two messages would compute a zero-length range for the
    /// first and hand the host nothing.
    #[test]
    fn sub_messages_share_one_range_and_differ_only_by_index() {
        let text = "\
12:1744810:d=2026090400:SPFH:1 hybrid level:anl:
13.1:2090377:d=2026090400:UGRD:1 hybrid level:anl:
13.2:2090377:d=2026090400:VGRD:1 hybrid level:anl:
14:2507827:d=2026090400:VVEL:1 hybrid level:anl:
";
        let items = Wgrib2Idx::parse("nam.grib2", text).unwrap().items();
        assert_eq!(items.len(), 4);
        let expected = PlanRange::Exact {
            offset: 2_090_377,
            length: 2_507_827 - 2_090_377,
        };
        assert_eq!(items[1].range, expected);
        assert_eq!(items[2].range, expected);
        assert_eq!(items[1].sub_index, Some(1));
        assert_eq!(items[2].sub_index, Some(2));
        assert_eq!(items[0].sub_index, None);

        // …and the first message's range ends where the *pair* begins, not
        // where the second sub-record does.
        assert_eq!(
            items[0].range,
            PlanRange::Exact {
                offset: 1_744_810,
                length: 2_090_377 - 1_744_810
            }
        );
    }

    /// A trailing group of sub-messages has no successor, so every one of them
    /// is open-ended — not just the last.
    #[test]
    fn a_trailing_sub_message_group_is_all_open_ended() {
        let text = "\
1:0:d=2026090400:TMP:surface:anl:
2.1:100:d=2026090400:UGRD:10 m above ground:anl:
2.2:100:d=2026090400:VGRD:10 m above ground:anl:
";
        let items = Wgrib2Idx::parse("o", text).unwrap().items();
        assert_eq!(items[1].range, PlanRange::OpenEnded { offset: 100 });
        assert_eq!(items[2].range, PlanRange::OpenEnded { offset: 100 });
    }

    /// GEFS's member tag and NBM's probability fields are qualifiers, kept in
    /// order. The trailing colon most products write must not become an empty
    /// one.
    #[test]
    fn qualifiers_are_kept_in_order_without_the_empty_terminator() {
        let text = "\
1:0:d=2026090400:HGT:10 mb:anl:ENS=+1
2:100:d=2026090400:CEIL:cloud ceiling:1 hour fcst:prob <304.8:prob fcst 3/7:probability forecast
3:200:d=2026090400:TMP:surface:anl:
";
        let items = Wgrib2Idx::parse("o", text).unwrap().items();
        assert_eq!(items[0].expect.qualifiers, ["ENS=+1"]);
        assert_eq!(
            items[1].expect.qualifiers,
            ["prob <304.8", "prob fcst 3/7", "probability forecast"]
        );
        assert!(items[2].expect.qualifiers.is_empty());
    }

    /// wgrib2's numeric fallback is read into codes rather than left opaque,
    /// and the abbreviation is still carried verbatim beside it.
    #[test]
    fn the_numeric_fallback_name_yields_wmo_codes() {
        let text = "3:705047:d=2026090400:var discipline=0 center=7 local_table=1 parmcat=16 \
                    parm=201:entire atmosphere:anl:\n";
        let items = Wgrib2Idx::parse("hrrr.grib2", text).unwrap().items();
        assert_eq!(
            items[0].expect.parameter,
            Some(ParameterId {
                discipline: 0,
                category: 16,
                number: 201
            })
        );
        assert!(
            items[0]
                .expect
                .abbreviation
                .as_deref()
                .unwrap()
                .starts_with("var discipline=")
        );
    }

    /// An ordinary short name yields no codes here — that is the resolver's
    /// job, and inventing a triple would be a guess.
    #[test]
    fn an_ordinary_name_carries_no_codes_of_its_own() {
        let items = Wgrib2Idx::parse("o", THREE).unwrap().items();
        assert!(items.iter().all(|i| i.expect.parameter.is_none()));
    }

    /// Selection with no resolver is the purely syntactic path, and it is
    /// enough for the vocabulary a user pastes out of a sidecar.
    #[test]
    fn selection_needs_no_resolver_for_the_sidecars_own_words() {
        let idx = Wgrib2Idx::parse("gfs.grib2", THREE).unwrap();
        let hits = idx.select(
            &Query::abbreviation("TMP").at_level_text("2 m above ground"),
            &NoResolver,
        );
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].range, PlanRange::OpenEnded { offset: 985_989 });

        // …and the semantic level form selects the same record.
        let hits = idx.select(
            &Query::abbreviation("TMP").at_level(LevelSpec::at(Surface::HeightAboveGround, 2.0)),
            &NoResolver,
        );
        assert_eq!(hits.len(), 1);
    }

    /// A resolver places the ordinary names, so a stored WMO request selects
    /// without the caller knowing NCEP's spelling.
    #[test]
    fn a_resolver_lets_a_wmo_request_select_by_codes() {
        struct Ncep;
        impl ParameterResolver for Ncep {
            fn resolve(&self, abbrev: &str, _level: &str) -> Option<ParameterId> {
                (abbrev == "TMP").then_some(ParameterId {
                    discipline: 0,
                    category: 0,
                    number: 0,
                })
            }
        }
        let idx = Wgrib2Idx::parse("gfs.grib2", THREE).unwrap();
        let hits = idx.select(
            &Query::parameter(ParameterId {
                discipline: 0,
                category: 0,
                number: 0,
            }),
            &Ncep,
        );
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].expect.abbreviation.as_deref(), Some("TMP"));
    }

    /// Blank lines are noise; a malformed line is not. A sidecar is machine
    /// output, so a line that does not fit means the file is not what it was
    /// taken for — and carrying on would build ranges out of whatever parsed.
    #[test]
    fn blank_lines_are_skipped_and_malformed_ones_are_refused() {
        let with_blanks =
            "\n1:0:d=2026090400:TMP:surface:anl:\n\n2:10:d=2026090400:RH:surface:anl:\n\n";
        assert_eq!(Wgrib2Idx::parse("o", with_blanks).unwrap().items().len(), 2);

        assert_eq!(
            Wgrib2Idx::parse("o", "1:0:d=2026090400:TMP\n").unwrap_err(),
            FetchPlanError::ShortRecord {
                dialect: Dialect::Wgrib2Idx,
                line: 1,
                expected: 6,
                found: 4
            }
        );
        assert_eq!(
            Wgrib2Idx::parse("o", "1:notanumber:d=2026090400:TMP:surface:anl:\n").unwrap_err(),
            FetchPlanError::NotAnInteger {
                dialect: Dialect::Wgrib2Idx,
                line: 1,
                field: "offset",
                value: "notanumber".to_string()
            }
        );
        assert_eq!(
            Wgrib2Idx::parse("o", "1:0:2026090400:TMP:surface:anl:\n").unwrap_err(),
            FetchPlanError::MissingDatePrefix {
                line: 1,
                value: "2026090400".to_string()
            }
        );
        assert_eq!(
            Wgrib2Idx::parse("o", "x.y:0:d=2026090400:TMP:surface:anl:\n").unwrap_err(),
            FetchPlanError::BadRecordNumber {
                line: 1,
                value: "x.y".to_string()
            }
        );
    }

    /// Offsets that run backwards are refused rather than turned into a
    /// negative length. The line number reported is the offending one, counting
    /// blank lines, so it is the number an editor shows.
    #[test]
    fn descending_offsets_are_refused_at_the_line_that_descends() {
        let text = "1:0:d=2026090400:TMP:surface:anl:\n\n2:500:d=2026090400:RH:surface:anl:\n3:100:d=2026090400:UGRD:surface:anl:\n";
        assert_eq!(
            Wgrib2Idx::parse("o", text).unwrap_err(),
            FetchPlanError::DescendingOffset {
                line: 4,
                offset: 100,
                previous_offset: 500
            }
        );
    }

    /// `\r\n` line endings survive a trip through a Windows editor or an S3
    /// console, and the reference time is the last field on the line most often
    /// — so a stray `\r` would end up inside a qualifier.
    #[test]
    fn carriage_returns_do_not_leak_into_fields() {
        let text =
            "1:0:d=2026090400:TMP:surface:anl:\r\n2:10:d=2026090400:RH:surface:anl:ENS=+1\r\n";
        let items = Wgrib2Idx::parse("o", text).unwrap().items();
        assert_eq!(items[0].expect.forecast.as_deref(), Some("anl"));
        assert_eq!(items[1].expect.qualifiers, ["ENS=+1"]);
    }
}
