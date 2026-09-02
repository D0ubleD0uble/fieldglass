# 0008 — CodeQL analyses production code only: test code is not extracted

**Status:** Accepted (2026-09-02). Answers the triage of the 72 Rust alerts
raised on master on 2026-08-29. Supersedes an earlier draft of this record that
proposed filtering the SARIF after analysis; that draft rested on a factual
claim that turned out to be wrong, and the correction is recorded below rather
than erased.

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
code flows in the analysis SARIF trace every one of them to an identifier
containing `latitude` or `longitude`:

| taint source | alerts |
| --- | --- |
| `meta.lambert_azimuthal_central_longitude` | 35 |
| `(p.standard_parallel, p.central_longitude)`, one line in `projection.rs` | 22 |
| `gaussian_latitudes(...)` | 6 |
| `normalise_longitude(...)` | 5 |
| `mercator_latitude(...)` | 1 |

That is not a coincidence of naming. CodeQL's shared sensitive-data heuristic
(`SensitiveDataHeuristics.qll`, used by every language) lists `latitude` and
`longitude` under *"geographic location — where the user is (or was)"*: it is a
PII detector, and in a grid decoder every coordinate trips it. The sink is
`core::panicking::assert_failed` / `panic_fmt`, which `log.model.yml` models as
a logging sink, so an `assert!` with a message is a log write. Both ends of the
taint pair are wrong for this codebase.

**The 3** are on the `#[napi]` attribute of `Grib1Handle`, `Grib2Handle` and
`NetcdfHandle`. napi-rs's generated code does `let mut p = null_mut();
napi_unwrap(env, val, &mut p); &mut *p` inside an `unsafe fn`. CodeQL models
`null_mut` as producing an invalid pointer and has no model saying
`napi_unwrap` fills its out-parameter, so the dereference is reported. The
alert is true to the model and false in fact. CodeQL attributes macro-expanded
code to the attribute line because it keeps only original-source locations
(github/codeql#20659, "no plans" to change). `crates/fieldglass-napi/src/lib.rs`
contains zero hand-written `unsafe`.

So all 72 are false positives. Both rules self-report high precision, which is
a statement about the corpus the rule was tuned against, not about this one.

## What the queries ask for

A rule asking for a real improvement should be obeyed rather than worked
around. That is what happened with Semgrep's `temp-dir` (#526) and
`dynamic-urllib` (#528), where the remediation the rule wanted was better than
the code it flagged, and taking it removed the suppression too. So the first
question is what these two would have us do.

**`rust/cleartext-logging`** recommends:

> Do not log sensitive data. If it is necessary to log sensitive data, encrypt
> it before logging.

Its worked example contrasts `"User password changed to {password}"` with
`"User password changed"` — remove the value from the message. Applied here
that means turning

```rust
assert!((lats[0] - 87.863_798_839).abs() < 1e-6, "{}", lats[0]);
```

into an assertion with no message, across sixty-nine sites whose message exists
to report which latitude was wrong. The remediation is a regression. The other
route — renaming so no identifier contains the word — would rename a public
`core` function and a napi field that reaches the JavaScript API, to dodge a
PII heuristic in code that is *about* latitude. Also a regression.

**`rust/access-invalid-pointer`** recommends:

> When dereferencing a pointer in `unsafe` code, take care that the pointer is
> valid and points to the intended data.

with fixes framed as rearranging the dereference or rewriting in safe Rust.
Neither is available: the `unsafe` is `#[napi]`'s output, and acting on it
means dropping napi-rs.

So one remediation is harmful and the other impossible. That is the reason to
look for a configuration answer rather than a code one.

## Why the ordinary levers do not apply

Each of these is the first thing one would reach for. Recording why, because
the next person will reach for them in the same order.

**Inline suppression.** `// codeql[rule-id]` is inert in Rust. Every other
supported language has an `AlertSuppression.ql`; Rust does not
(github/codeql#21637; a PR, #21638, has been open since April 2026 with no
approval). And it would be a suppression, which this project prefers to avoid.

**Excluding test paths.** `paths-ignore` matches files. Rust unit tests live in
`#[cfg(test)]` modules *inside* the `src/` files they test, so no path selects
them. Sufficient for `tests/` directories, insufficient for the sixty-two
alerts inside `src/`.

**Narrowing the query suite.** The workflow requests `security-extended`, which
looks like the culprit. `rust/cleartext-logging` is in `rust-code-scanning.qls`,
the default suite. Dropping back would keep all 69.

**Dismissing the alerts.** Recorded per location, in GitHub's database rather
than in the repository. It does not survive lines moving, and this codebase
moves lines.

**Excluding the rule outright.** `query-filters` would clear all 69 in three
lines, and is the wrong answer for a specific reason: [ADR-0005](0005-byte-access-and-the-remote-seam.md)
commits to a remote byte-access seam, and some of the data sources already
identified for it are authenticated. This project is on a path towards holding
credentials. The query that watches for logging them should not be switched off
in the years before there are any.

**Filtering the SARIF after analysis.** The earlier draft of this record
proposed dropping `cleartext-logging` results found inside `#[cfg(test)]`
before upload, on the claim that CodeQL "has no `#[cfg(test)]` awareness of
its own". That claim was wrong, and the mechanism it led to is a suppression —
editing the evidence before the auditor sees it — which is exactly what the
project had just finished removing from Semgrep.

## The lever that does apply

CodeQL's Rust extractor evaluates `#[cfg(...)]` gates *at extraction*, against
a cfg set in which `test` is enabled by default. The extractor option
`cargo_cfg_overrides=-test` disables it, and a gated item then is not emitted
at all — no AST, no dataflow nodes, nothing in the SARIF. The QL library has no
test-code concept, and the maintainers say one is not possible on that side;
but the extractor does, and they say so in the very issue the earlier draft
cited:

> You can however exclude code under a `cfg(test)` module or block while
> extracting. You can do so by setting
> `CODEQL_EXTRACTOR_RUST_OPTION_CARGO_CFG_OVERRIDES=-test` in the environment
> — github/codeql#20771, maintainer reply

CodeQL itself changed its mind on this default once: PR #17937 turned test
extraction off, and PR #18347 turned it back on while naming `-test` as the
intended opt-out for unit tests and noting that restricting the security
queries to non-test code "will need further work on the QL side". That work has
not landed.

This is also what every other CodeQL language already does. Most classify test
files and exclude them from security alerts by default; Rust has no classifier
yet. Setting `-test` is the missing default, not a departure from one.

**Validated locally** with CLI 2.26.4 — the version that produced the 72 — on
this workspace, running the two queries against a database built each way:

| database | `cleartext-logging` | `access-invalid-pointer` |
| --- | --- | --- |
| default | 69 | 3 |
| `cargo_cfg_overrides=-test` | **0** | 3 |

The override removes exactly the 69 and nothing else. The seven alerts in
`crates/*/tests/*.rs` went with them, so `paths-ignore` is not needed
alongside.

## Decision

**1. Test code is not extracted.** The CodeQL workflow sets
`CODEQL_EXTRACTOR_RUST_OPTION_CARGO_CFG_OVERRIDES: "-test"` in the job
environment. It is an environment variable because the action has no input for
extractor options and the config file cannot carry them. The JavaScript job
ignores it.

**2. `rust/cleartext-logging` stays enabled**, and now sees production code
only. A logged credential in `src/` still fires. That is the whole point of
choosing this shape over excluding the rule.

**3. Nothing is filtered after analysis.** The SARIF step that drops
in-source-suppressed Semgrep results (#524) stays, because it drops only what
a `nosemgrep` has already silenced; after #528 there are none, so it is dormant.
No such step is added for CodeQL. What CodeQL reports is what appears.

**4. The three `access-invalid-pointer` alerts are dismissed as false
positives, with the reason recorded** — napi-rs's generated glue, which CodeQL
cannot see through because it has no model for `napi_unwrap`'s out-parameter.
Dismissal is the tool's own record of a reviewed finding, it is reversible, and
the three are anchored to struct attributes that rarely move. It is the one
place this record accepts a suppression, because no code change and no
configuration removes them: the only extractor lever, `proc_macro_server=none`,
disables every proc-macro expansion including serde's, which is not worth three
alerts. If github/codeql#21638 merges, inline suppression at the site is
preferred and these dismissals should be replaced by it.

Done on 2026-09-02: alerts #57, #58 and #59, reason "false positive", each
carrying this comment (the field allows 280 characters, so it points here for
the rest):

> napi-rs generated FFI glue: null_mut() -> napi_unwrap(&mut p) fills p;
> status checked before deref. CodeQL has no model for napi_unwrap out-param,
> so flags the deref. Crate has zero hand-written unsafe; Rust has no inline
> suppression. Full reasoning: docs/decisions/0008.

What the generated code does, so the comment can be checked against it: make
a null pointer; pass it by `&mut` to `napi_unwrap`, which fills it with the
address of the wrapped object; `check_status!` on the call's result, returning
early on failure; a type-tag check; then the dereference. The pointer is read
only after Node has reported success, and the same expansion exists in every
`#[napi]` struct of every napi-rs project.

If one of the three structs moves, GitHub may raise the alert again at the new
line. The answer is the same dismissal with the same comment; nothing about the
code will have changed.

**5. The CodeQL bundle is not pinned.** A new bundle bringing a batch of
findings is triage, and triage is the cost of a scanner that improves.

**6. Nothing is reported upstream.** Two reports were drafted — that
`latitude|longitude` in the shared sensitive-data heuristic classifies every
coordinate in geoscience code as user PII, and that `access-invalid-pointer`
has no model for FFI out-parameter initialisers such as `napi_unwrap` — and
the maintainer decided on 2026-09-02 not to file them. The analysis stays in
this record so the option is open later, and so the next reader does not have
to rediscover why the alerts fire; the decision is that the workaround above is
sufficient for this project and the upstream conversation is not one it needs
to carry.

**7. CodeQL does not gate the build yet.** Semgrep does, as of #521. Making
CodeQL match is the natural end state and is deliberately not decided here — it
should be its own record, taken once the count is genuinely zero.

## Consequences

The Security tab becomes a list of things to act on, which is the only form in
which it is worth anything. Seventy-two false positives do not merely waste
triage time; they teach every contributor that the tab is noise, and that
lesson outlasts the alerts.

Test code is now outside CodeQL's view for *every* query, not just the two
that fired. That is the same trade every other CodeQL language makes by
default, and the trade PR #18347 named when it made `-test` the opt-out. A
security defect that exists only in a test would not be reported. Tests are not
shipped.

The 69 closed as fixed on the first analysis of `master` after the change
(`896a7a5`, 2026-09-02: 72 results on the commit before it, 3 on it). The
three were dismissed the same day. The Security tab reads zero.

The earlier draft's `#[cfg(test)]` parser is not built. The measurement that
justified worrying about it — whether production code ever follows a test
module in this codebase — was taken anyway: it does not, in any of the fifty
files that have one. That is recorded here in case the question comes up again,
not because anything now depends on it.

A future bundle can bring a new batch, and the answer will be the same
sequence: measure where they fall, read what the query asks for, and prefer a
code change that makes the finding not exist over any mechanism that hides it.

## References

- [Rust extractor options](https://github.com/github/codeql/blob/main/rust/codeql-extractor.yml)
  and [`config.rs`](https://github.com/github/codeql/blob/main/rust/extractor/src/config.rs)
  — `cargo_cfg_overrides`, and the `test` cfg enabled by default.
- [github/codeql#18347](https://github.com/github/codeql/pull/18347) — reinstated
  test extraction, named `-test` as the opt-out, and recorded that QL-side test
  filtering is future work. [#17937](https://github.com/github/codeql/pull/17937)
  is the change it reversed.
- [github/codeql#20771](https://github.com/github/codeql/issues/20771) — the
  maintainer reply giving the `-test` override, and confirming attribute token
  trees are not extracted so nothing can be done on the QL side.
- [github/codeql#21637](https://github.com/github/codeql/issues/21637) and
  [#21638](https://github.com/github/codeql/pull/21638) — Rust inline
  suppression: requested, and a stalled PR.
- [github/codeql#20659](https://github.com/github/codeql/issues/20659) —
  macro-expanded code is attributed to its original-source location; no plans
  to change.
- [`SensitiveDataHeuristics.qll`](https://github.com/github/codeql/blob/main/shared/concepts/codeql/concepts/internal/SensitiveDataHeuristics.qll)
  — `latitude|longitude` in `maybePrivate`.
- [`log.model.yml`](https://github.com/github/codeql/blob/main/rust/ql/lib/codeql/rust/frameworks/log.model.yml)
  — `assert_failed` / `panic_fmt` as logging sinks.
- [`rust/cleartext-logging`](https://codeql.github.com/codeql-query-help/rust/rust-cleartext-logging/)
  and [`rust/access-invalid-pointer`](https://codeql.github.com/codeql-query-help/rust/rust-access-invalid-pointer/)
  query help — suite membership and the recommendations quoted above.
- [napi-rs `struct.rs` codegen](https://github.com/napi-rs/napi-rs/blob/main/crates/backend/src/codegen/struct.rs)
  — the `null_mut` → `napi_unwrap` → deref sequence the three alerts describe.
- [#521](https://github.com/D0ubleD0uble/fieldglass/pull/521),
  [#524](https://github.com/D0ubleD0uble/fieldglass/pull/524),
  [#526](https://github.com/D0ubleD0uble/fieldglass/pull/526),
  [#528](https://github.com/D0ubleD0uble/fieldglass/pull/528) — the Semgrep
  sequence this record follows: silence at source, then remove the trigger so
  there is nothing to silence.
- [ADR-0005](0005-byte-access-and-the-remote-seam.md) — the remote seam that
  makes a credential surface foreseeable, and so the reason the rule stays on.
