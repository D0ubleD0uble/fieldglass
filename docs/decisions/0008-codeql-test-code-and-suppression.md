# 0008 — CodeQL results are filtered by rule, outside the tool

**Status:** Proposed (2026-08-29). Answers the triage of the 72 Rust alerts
raised on master that day. Depends on the SARIF filtering step added for
Semgrep in [#524](https://github.com/D0ubleD0uble/fieldglass/pull/524).

## Context

On 2026-08-29 the Security tab went from zero Rust alerts to 72 in one run.
Nothing in the code had changed in a way that could account for it: the run
that produced them was the merge of a pull request that added a GRIB1 test
fixture, a builder script and one test.

The analyses record what happened:

| date | commit | CodeQL | results |
| --- | --- | --- | --- |
| 2026-08-26 | `3bc41d8` | 2.26.3 | 0 |
| 2026-08-29 | `f0117a2f` | 2.26.4 | 72 |

The CodeQL *bundle* moved 2.26.3 → 2.26.4 between the two runs. The action is
pinned to a commit SHA and Dependabot keeps it current, but the bundle it
downloads is resolved at run time and is not pinned by that. So the queries can
change under a workflow that did not change, and did here.

This will happen again. It is the normal way CodeQL ships query improvements,
and the alternative — pinning the bundle — trades away the improvements.

## What the 72 actually are

Measured rather than sampled. Every alert was classified by whether its line
falls inside a `#[cfg(test)]` module or a `tests/` directory:

| | count | rule |
| --- | --- | --- |
| test code | **69** | `rust/cleartext-logging` |
| production | **3** | `rust/access-invalid-pointer` |

The split is exact: every logging alert is in test code, every pointer alert is
in production.

**The 69** are `assert!` failure messages — `assert!(cond, "{}", value)`. The
"sensitive data" they format is latitudes, temperatures, grid dimensions and
projection summaries. Fieldglass has no credential, token or PII surface for
them to expose; it reads scientific file formats.

**The 3** are on the `#[napi]` attribute of `Grib1Handle`, `Grib2Handle` and
`NetcdfHandle`. They are the FFI glue the macro generates, which necessarily
dereferences pointers handed over by V8. `crates/fieldglass-napi/src/lib.rs`
contains zero hand-written `unsafe`.

So all 72 are false positives. Both rules self-report high precision, which is
a statement about the corpus the rule was tuned against, not about this one.

## Why the ordinary remedies do not apply

Each of these is the first thing one would reach for, and each is unavailable
or wrong here. Recording why, because the next person will reach for them too.

**Inline suppression.** `// codeql[rule-id]` is inert in Rust. Every other
supported language has an `AlertSuppression.ql`; Rust does not
([codeql#21637](https://github.com/github/codeql/issues/21637)). The mechanism
this project would otherwise use — judgement recorded at the site, versioned
with the code, reviewed in the pull request that introduces it — simply does
not exist for this language.

**Excluding test paths.** `paths-ignore` matches files. Rust unit tests live in
`#[cfg(test)]` modules *inside* the `src/` files they test, because that is how
a Rust unit test reaches private items. There is no path that selects them.
CodeQL has no `#[cfg(test)]` awareness of its own
([codeql#20771](https://github.com/github/codeql/issues/20771)).

**Narrowing the query suite.** The workflow requests `security-extended`, which
looks like the culprit. It is not: `rust/cleartext-logging` is a member of
`rust-code-scanning.qls`, the default suite. Dropping back would keep all 69.

**Dismissing the alerts.** Dismissal is recorded per location, in GitHub's
database rather than in the repository. It does not survive the lines moving,
and this codebase moves lines. It also leaves no trace a reader of the source
would ever encounter.

**Excluding the rule outright.** `query-filters` would clear all 69 in three
lines, and is the wrong answer for a specific reason: [ADR-0005](0005-byte-access-and-the-remote-seam.md)
commits to a remote byte-access seam, and some of the data sources already
identified for it are authenticated. This project is on a path towards holding
credentials. Turning off the query that watches for logging them is safe today
and quietly wrong on the day that changes, with nobody watching for the
transition.

## Decision

**1. `rust/cleartext-logging` stays enabled.** The rule is not wrong about the
class of bug; it is wrong about test code in a repository with no secrets. The
first of those may stop being true.

**2. Its results are dropped from the SARIF when they fall inside a
`#[cfg(test)]` module**, in the step that already filters the Semgrep SARIF
before upload. This implements the test/production boundary CodeQL lacks,
outside CodeQL, because that is the only place it can be implemented.

The boundary must be found by real module-scope parsing. "Is there a
`#[cfg(test)]` earlier in the file" is not good enough — it silently suppresses
production code that happens to sit after a test module, which is a worse
failure than the noise it replaces, because it is invisible.

**3. The three macro-generated pointer alerts are handled narrowly**, by rule
and location, not by disabling `rust/access-invalid-pointer`. That query is
worth keeping live on a crate that crosses an FFI boundary, and the three sites
are anchored to struct attributes that rarely move.

**4. The CodeQL bundle is not pinned.** A new bundle bringing a batch of
findings is triage, and triage is the cost of a scanner that improves. Pinning
converts a recurring small cost into a growing invisible one.

**5. Filters encode rules, never instances.** A filter says "this class of
finding, in this class of location, for this reason". The moment the filter
list starts naming individual findings it has become a dismissal list kept in
the wrong place, and it should be reverted to a rule or abandoned.

**6. CodeQL does not gate the build yet.** Semgrep does, as of #521. Making
CodeQL match is the natural end state and is deliberately not decided here —
it should be its own record, taken when the alert count is genuinely zero
rather than as a rider on the change that gets it there.

## Consequences

The Security tab becomes a list of things to act on, which is the only form in
which it is worth anything. 72 false positives do not merely waste triage time;
they teach every contributor that the tab is noise, and that lesson outlasts
the alerts.

A real `cleartext-logging` finding in production code still fires. That is the
point of the shape chosen: the filter is scoped to test code, so acquiring a
credential surface does not require remembering to re-enable anything.

The project now runs two homegrown SARIF filters, one per scanner. That is a
maintenance surface that did not exist before, and it is the main cost of this
decision. They should share one implementation rather than diverge.

Filtering a SARIF is editing the evidence before the auditor sees it. This is
acceptable while the filter encodes a reviewable rule and is read in review
like any other code; it stops being acceptable the moment it accumulates
exceptions. Decision 5 exists to make that failure visible rather than gradual.

A `#[cfg(test)]` parser is a small amount of code that must be right about Rust
syntax. It is worth a test of its own, including the case it exists to avoid:
production code following a test module in the same file.

Nothing here reduces what CodeQL analyses. The queries all still run; only the
reporting of a known-inapplicable class is suppressed, and the rules array is
left intact in the SARIF so GitHub can still resolve alerts it raised earlier.

## References

- [codeql#21637](https://github.com/github/codeql/issues/21637) — Rust has no
  `AlertSuppression.ql`, so inline suppression comments do nothing.
- [codeql#20771](https://github.com/github/codeql/issues/20771) — no built-in
  way to exclude `#[cfg(test)]` code from Rust alerts.
- [`rust/cleartext-logging` query help](https://codeql.github.com/codeql-query-help/rust/rust-cleartext-logging/)
  — suite membership (`rust-code-scanning.qls`), precision and severity.
- [Customising your advanced setup for code scanning](https://docs.github.com/en/code-security/code-scanning/creating-an-advanced-setup-for-code-scanning/customizing-your-advanced-setup-for-code-scanning)
  — `query-filters` and `paths-ignore`, the two levers that do not fit here.
- [#521](https://github.com/D0ubleD0uble/fieldglass/pull/521) — Semgrep findings
  to zero; established that a suppression is silenced at the source, not in the
  Security tab.
- [#524](https://github.com/D0ubleD0uble/fieldglass/pull/524) — the SARIF
  filtering step this decision extends, and why the rules array survives it.
- [#526](https://github.com/D0ubleD0uble/fieldglass/pull/526) — the preferred
  outcome where it is available: remove the trigger rather than suppress the
  finding.
- [ADR-0005](0005-byte-access-and-the-remote-seam.md) — the remote seam that
  makes a credential surface foreseeable.
