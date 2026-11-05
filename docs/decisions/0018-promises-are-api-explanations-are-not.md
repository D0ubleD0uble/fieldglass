# 0018 — Strict semver: promises are API, explanations are not

**Status:** Accepted (2026-11-04). #1682.

## Context

Fieldglass publishes eight library crates, the `@fieldglass/wasm` npm package
and the VS Code extension, all at one version. RELEASING.md already said
that a 0.x patch does not break the Rust API. Nothing checked it.

During the 0.6.1 cycle, master picked up breaking changes that no one had
marked as breaking. A person found them, not a tool:

- **#1679.** `fieldglass::conformance` gained `Op::AxisValues` in the middle of
  an enum that 0.6.0 published as exhaustive (#1575), and six of its types
  became `#[non_exhaustive]` (#1666). `cargo semver-checks` 0.51.0 against
  v0.6.0 with `--all-features` reports both. With default features every crate
  passed, because `conformance` is off by default.
- **#1680.** The v0.6.0 docs of `forecast_hours` in both GRIB crates made
  promises master no longer kept: GRIB1 "convert the PDS time unit + P1 to a
  number of forecast hours", and GRIB2 `None` for the calendar units. No
  signature changed, so no tool could see it.
- **#1681.** The v0.6.0 docs of `UnsupportedVariable` promised "only a dataset
  that fails with `FieldglassError::UnsupportedSection` lands here". On master
  a variable with one unread attribute moved there instead.
- Two more of the same kind turned up while those were fixed:
  `list_attributes` failing the whole call where 0.6.0 returned what it read
  (#1689), and `MessageInfo.forecast_hours` filled from a different number than
  0.6.0 used (#1694).

The owner's decision, 2026-11-04: "we do need to be strict on it this time".
"Distinguish between 'promises' and 'explanations of current behavior'. True
promises count as API. Explanations can be changed to match current behavior."
The semver check runs on every PR.

The [Cargo SemVer guide](https://doc.rust-lang.org/cargo/reference/semver.html)
sets the vocabulary. On 0.y.z, cargo treats a change in y as major and a
change in z as minor ([change categories](https://doc.rust-lang.org/cargo/reference/semver.html#change-categories)),
so a 0.6.x patch may only make what the guide calls minor changes. Both
#1679 changes are major there: [adding a variant to an exhaustive enum](https://doc.rust-lang.org/cargo/reference/semver.html#enum-variant-new)
and [adding `#[non_exhaustive]` to an existing type](https://doc.rust-lang.org/cargo/reference/semver.html#attr-adding-non-exhaustive).
[Adding a new item](https://doc.rust-lang.org/cargo/reference/semver.html#item-new)
and [deprecating one](https://doc.rust-lang.org/cargo/reference/semver.html#new-lints)
are minor. For behavior, the guide says it is "usually a judgment call by the
project maintainers" ([SemVer compatibility](https://doc.rust-lang.org/cargo/reference/semver.html#semver-compatibility)).
This record makes that call once, so each PR doesn't have to.

## Decision

### 1. What is public API

- Every `pub` item in the published crates, under every feature. No feature
  is outside the promise, `conformance` included.
- A **promise** in a doc comment is API. It is what a caller may rely on: a
  return contract, the `None` or error conditions, "only X lands here", a
  format documented as stable.
- An **explanation** of current behavior is not API. That covers how something
  works today, display wording, and an example of rendered text. A PR may
  update an explanation to match the code. In a crate README, which is also
  the crate-root rustdoc of grib1, grib2, netcdf and zarr, draft the change
  for the owner instead, since only the owner edits those.
- When unsure, ask: would a reasonable caller write code that depends on this
  sentence? If yes, it is a promise.
- A documented rendering of human-readable text is an explanation unless the
  doc says callers may parse it. That covers error and refusal messages and
  display labels. In 0.6.0, `ZarrStore::unfetched_for` said its list was
  "worded for a refusal", with each key in backticks. It now writes `"key"`
  with `{:?}` escaping (#1648). That doc described display wording, so it was
  an explanation and the change was allowed.
- But a field doc that names a fallback value a caller can detect is a
  promise (owner, 2026-11-04, #1700). In 0.6.0, `MessageInfo.originating_centre`
  was documented as named from the table "or `Centre <n>` when the table has
  no entry", and `sub_centre` the same way with `Sub-centre <n>`. The
  sub-center has no code field, so matching that prefix is the only way a
  caller can tell an unnamed sub-center. Master had changed both to the
  American spelling (#1611). #1700 is the issue that restores the 0.6.0 text
  for 0.6.1, and the change waits for 0.7.0 (#1684).
- Labeling display text "not a stable format", as #1684 plans, makes the
  status plain. It does not make unlabeled display text a promise today.
- A validator may, in a patch, start refusing input that is wrong per the
  formats it checks, but never input that is correct. The CHANGELOG entry
  says what it now refuses. The case is `verify_message` (npm
  `verifyMessage`): after #1304 and #1553 it refuses an ECMWF record whose
  `type` disagrees with the product template, and a wgrib2 `ENS=+n` line on
  another center's ensemble member whose type is coded missing (255), which
  0.6.0 did not compare. Each describes a different
  message from the one fetched (owner decision, 2026-11-04, on #1682).
- npm `@fieldglass/wasm`: an output type may gain fields. An input type may
  gain only optional fields. Nothing is removed, renamed or narrowed.
- The conformance suite follows from that. `conformance::compare` requires an
  object to have exactly the recorded key set, so the suite's key set grows
  whenever an output type gains a field in a compatible release. Since v0.6.0
  the recorded cases gained `originatingCentreCode` and `offsetHours` in 12
  `*/message` cases, `coordinateCalendar` and `coordinateVertical` in 4
  `decode_line` cases, and `attributesNotRead` in 3 `*/variables` cases. A
  host that replays the shipped suite through its own DTO layer must forward
  new fields to pass. That is allowed, and it is what the suite is for.
- Extension: no setting key or command id is renamed or removed in a patch.
- The one exception to all of the above: a memory-safety fix, or a fix for a
  panic on valid input, may break. Its CHANGELOG entry says so.

### 2. Prevention when an issue is planned

Triage asks whether a change would break or remove an existing `pub` item,
change a promise, or remove, rename or narrow something in a serialized
format. If it would, the issue gets the `breaking` label and goes only in a
minor milestone. Adding an optional output field, as #1696 did, is none of
these. A `breaking` PR stays open, unmerged, until the current patch
is tagged. Master is trunk-based, so it must always be releasable as the next
patch.

### 3. Detection on every PR

The *Public API* CI job (`.github/workflows/semver.yml`, #1670, #1685) runs
`cargo semver-checks --workspace --baseline-rev <last tag> --release-type
<next release>` once with default features and once with all features. A
major-level finding fails it. There is no exception clause; the one baseline
override, for the fix of a shipped break, is in §5. Since 2026-11-04 the job
("Public API vs last tag") is a required check on `master`.

Two facts about the tool shape the job. `--release-type` is required: without
it the tool derives the release type from the version number, and with the
workspace version unchanged between releases that does not give the strict
check (#1670). And the tool applies `--release-type` literally, while cargo
reads 0.x one step up. So `tools/semver-release-type` holds the Fieldglass
bump, and while the workspace is 0.x the job passes it one step up: `patch`
runs as `--release-type minor`, which lets a deprecation through and fails a
break, and `minor` runs as `major`. From 1.0 the value passes through
unchanged. A PR that raises the setting fails unless it carries the `breaking`
label. #1685 recorded both directions: unmapped `patch` failed on a throwaway
`#[deprecated]`, and mapped `minor` passed it while still failing on the
#1679 breaks.

The tool cannot see a promise in a doc comment or a serialized format, so
review also asks the promise question of every changed doc comment.

### 4. A break found on master before release

- (a) Make it compatible. Restore the promise, and put the new behavior in a
  new function, field or type. Deprecating the old item is allowed in a patch.
- (b) If that isn't possible, revert it and re-file the change in the next
  minor milestone with `breaking`.
- Never ship it under a written exception, and never bump the version to cover
  it.

The 0.6.1 fixes are the worked examples of (a):

- #1679 (PR #1686) removed the new variant and the attributes, and kept the
  cases that needed the variant running on a crate-private path.
- #1680 (PR #1691) restored both `forecast_hours` functions and added
  `offset_hours` beside them.
- #1681 (PR #1688) kept the variable listed and reported the lost attribute
  through `attributes_not_read`, which #1604 had added.
- #1689 (PR #1693) made `list_attributes` return what it reads again and added
  `list_attributes_with_unread` for the rest.
- #1694 (PR #1696) put `MessageInfo.forecast_hours` back on its 0.6.0 values
  and added an optional `MessageInfo.offset_hours` beside it.

Each PR ran `cargo semver-checks` against v0.6.0. The GRIB ones also compared
output over the committed fixtures before and after.

### 5. A break that already shipped

- Publish a patch that restores compatibility first.
- Then yank the broken crate versions (`cargo yank`) and run `npm deprecate`
  on the npm version. Yanking before a compatible release exists strands
  users.
- The fixed release's CHANGELOG entry says which version broke what.
- The change is redone in the next minor.

The job measures against the latest tag, which is now the broken release.
Restoring a doc promise, or a fix that only adds API, passes it as usual. The
restoring patch fails it only when undoing the break is itself a break against
the broken tag, such as removing an enum variant the broken release added. The
owner's decision for that case (2026-11-04, on #1682): for that one release, a
one-line `tools/semver-baseline` file names the last good tag, and the job
measures against it instead. The file is removed once the new tag exists, in
the post-release steps, because the latest tag stays the broken one until
then. The check stays required, and nobody merges past it by hand. Only the
owner adds the file. That file and the job's support for it are not built
yet; #1699 tracks them.

RELEASING.md "Breaking changes" has the steps.

### 6. Design defaults

- New public types are `#[non_exhaustive]` from the start. The umbrella's
  `crates/fieldglass/tests/api_rules.rs` enforces it for its API types; in the
  other crates it is a review item.
- Prefer private fields with a constructor, and sealed traits.
- Doc comments say plainly which sentences are promises.

## Consequences

- A patch is cheap to take. A downstream crate can depend on `0.6` and pick up
  every patch without reading its notes.
- Some fixes cost more. A changed meaning becomes a new item beside the old
  one, and the old one waits for the next minor to go. #1684 is the 0.7.0 end
  of the `forecast_hours` case, and #1683 re-adds the conformance changes.
- Work that breaks waits. A `breaking` PR can sit open for most of a patch
  cycle, and may need a rebase when it lands.
- An agent or a host author needs this record to tell a promise from an
  explanation, so the rule is also in `.claude/rules/conventions.md` under
  Versioning.
- An out-of-tree host that runs the conformance suite must forward new output
  fields before it upgrades. The module doc of
  `crates/fieldglass/src/conformance.rs` says so.

## When to revisit

At 1.0, when `--release-type` stops being mapped and the extension and npm
rules may want their own checks. Or if the per-crate versioning the
conventions name for that time arrives first, since the job assumes one
workspace version.

## References

- [The Cargo SemVer guide](https://doc.rust-lang.org/cargo/reference/semver.html)
- #1682 (this policy), #1670 and #1685 (the CI job)
- The 0.6.1 case: #1679, #1680, #1681, #1689 and #1694, fixed by PRs #1686,
  #1691, #1688, #1693 and #1696
- #1683 and #1684, the 0.7.0 counterparts
