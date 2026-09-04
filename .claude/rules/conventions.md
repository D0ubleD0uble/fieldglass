# Project conventions

Committed so they reach every session — local, Claude Code on the web, the
GitHub Action, and routines. (User-scoped `~/.claude` memory does **not** carry
over to those, so anything an automated session must follow belongs here.)

Keep this file free of personal data: no names, email addresses, or machine
paths.

## Branching & pull requests
- Trunk-based. Branch off `master` and open PRs against it
  (`gh pr create --base master`). A release is the tagged SHA of the prep merge.
- Cloud / routine sessions may only push to the working branch (or a
  `claude/`-prefixed one); that is compatible with the above.
- Put `Closes #N` in the PR body so merging auto-closes the issue. Don't close
  issues by hand.
- Don't run `gh issue create` without explicit per-issue approval — draft the
  title and body inline for review first.

## Commits
- Conventional Commits: `feat:`, `fix:`, `docs:`, `chore:`, `ci:`, etc.
- Never write personal data (emails, names, private paths) into tracked files;
  use the repo's configured noreply git identity.

## Versioning
- One workspace version, bumped in lockstep, with `=` pins between the
  workspace crates. The set on crates.io is exactly the set the test suite
  ran; cargo never resolves a combination that was not tested. Publishing an
  unchanged crate is free, so a crate gets a new number even when nothing in
  it changed.
- Revisit at 1.0, or earlier if `core` stabilises while the storage-convention
  crates (`fetchplan`, `zarr`) still track moving specs, or if a crate gains
  consumers that do not take the rest. Then per-crate versions with
  `release-plz` or `cargo-release`; not before.

## Quality gates (must pass before opening a PR)
Enforced by pre-commit (commit stage) and pre-push:
- `cargo fmt --all -- --check`
- `cargo clippy --all-targets --workspace -- -D warnings`
- `cargo test --workspace`
- `cargo deny check`
- semgrep (ERROR severity)

Coverage: Codecov patch target is 70%. Exclude generated / stub / FFI files in
`codecov.yml` rather than writing token tests for them.

The lint levels themselves live in `[workspace.lints]` in the root manifest,
inherited by every member with `lints.workspace = true`. Add lints there, not as
a crate-root `#![deny(...)]`, so `cargo build`, rust-analyzer and a crates.io
reader see the same bar the hook does. A member that omits the inheritance line
gets no diagnostic from cargo, so `tools/check_workspace_lints.py` (pre-commit)
asserts it. The same checker holds the list of crates allowed a crate-root
`#![allow(missing_docs)]`, which is the one-line way to opt a whole crate out of
the standard. That list is empty — every public item is documented — so adding
such an attribute means editing the checker, where a reviewer sees it.

**Extension (Electron) tests are off by default in the working loop.**
`@vscode/test-electron` (`npm test` in `extension/`) needs a display and is
heavy, and cloud sandboxes can't run it. CI is the backstop that runs it; the
default local/cloud gate is the Rust gates above plus `tsc --noEmit` and eslint.

## Docs voice (user-facing: README, CHANGELOG, package.json description)
- These ship to the VS Code Marketplace and release notes. Write plainly and
  succinctly; avoid AI-isms, em-dash pile-ups, and internal jargon.
- Edit within the existing structure. In CHANGELOG, add under `## [Unreleased]`
  only; never rewrite already-released sections.

## GRIB2 packing modes table = single source of truth
The README **GRIB2 packing modes** table — plus the "every registered template"
summary sentence in the paragraph directly above it — are the only places that
state GRIB2 §5 decode coverage. When a template starts or stops decoding, update
**only** that table row, that summary sentence, and `CHANGELOG.md [Unreleased]`.
Every other README mention of §5 packing is deliberately coverage-agnostic and
points at the table; an HTML comment by the table records this rule.

## Read the architecture docs before implementing, not at review time

`docs/architecture/` describes the workspace as it is (drift-guarded against
the source). `docs/architecture/planned/` describes it after the open
milestones close. When your issue belongs to one of those milestones, the
planned doc is a **specification**, not background — read the one for what you
are building, during planning:

- `02-trait-seams.md` — method surfaces. It names the methods a planned type
  is expected to have.
- `01-crates.md` — crate boundaries, dependency edges, feature forwarding.
- `03-composition.md` — message parts and the host boundary.
- `04-hosts.md` — what each host does around the Rust.

Names in `planned/` are provisional and the issue wins if they disagree (see
its README), so check both — and prefer a name the codebase already uses over
either.

**Before adding a helper to `core`, search for the capability by concept, not
by the name you would have chosen.** The reusable version is often a *provided
method on a trait* rather than an inherent method on the concrete type, so
looking at the type finds nothing; the `projection/` modules and the napi
crate carry thousands of lines between them. A duplicate there is usually worse than the original,
which has already absorbed the edge cases (poles inside the domain, points off
a projection disc, antimeridian arcs) that a fresh one rediscovers one bug at
a time.

## Decode is decoupled from rendering
- A new decode path (a GRIB1 packing, a GRIB2 §5 template, a NetCDF variable)
  needs **no** changes to projection, overlays, or manual bounds. Those run on
  the decoded `Vec<Option<f64>>` field and the grid geometry (GDS), not on the
  packing. Reprojection eligibility keys on grid type and spacing only.
- Exception: outputs that aren't one scalar per grid point (e.g. the GRIB1 true
  `matrixOfValues` form) use their own path, not `decode_message_values`.

## eccodes validation
- GRIB decoders are cross-checked against eccodes (pinned to 2.34.1). The test
  suite needs **no** eccodes at runtime — it uses committed fixtures and
  `.eccodes.ref.json` snapshots, which carry both the metadata keys and a
  value-level block (count, masked count, statistics, sampled points). eccodes is only needed to (re)generate fixtures,
  snapshots, or tables.
- eccodes can decode more packings than it can encode. For the ones it can't
  encode, hand-build a minimal fixture and use eccodes' *decode* as the oracle.
  Record every fixture's provenance in `tests/fixtures/NOTICE.md`.
- The 2.34.1 pin can itself be the bug: when matching a decode fix eccodes
  shipped *after* 2.34.1 (e.g. ECC-2095 in 2.42.0), the pinned version is not a
  valid value oracle for that case — generate the fixture's value oracle with a
  newer eccodes (the `eccodes` PyPI wheel works) and record the version and why
  in `NOTICE.md`. Metadata `.eccodes.ref.json` snapshots stay on the pin.

## Generated data & lookup tables
- Some data (e.g. ECMWF GRIB1 parameter tables) is generated by scripts under
  `tools/`. Regenerate via the script rather than editing the output by hand,
  and keep generated files out of patch coverage in `codecov.yml`.
- Extend WMO ON388 / FM 92 lookup tables in the Rust `tables.rs` files, not at
  the napi or TypeScript layer. napi-rs converts `snake_case` Rust fields to
  `camelCase` automatically.

## Parallel subagents
- When delegating to parallel subagents, use harness-owned worktrees (the Agent
  `isolation: "worktree"` option). Pre-staged `git worktree add` paths are
  rejected by the subagent sandbox.
