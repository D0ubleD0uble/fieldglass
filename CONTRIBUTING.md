# Contributing to Fieldglass

Thanks for considering a contribution. Fieldglass is in beta — bug reports, file fixtures that don't render correctly, and pull requests are all welcome.

## Filing issues

The fastest path to a fix is a small fixture file. Use the [Bug report](.github/ISSUE_TEMPLATE/bug_report.md) template; if the file isn't shareable, describe its provenance (centre, format edition, originating model, approximate size) so we can find a stand-in.

For features, the [Feature request](.github/ISSUE_TEMPLATE/feature_request.md) template asks for the user-facing problem and the scope; please fill it in rather than describing only the implementation you have in mind.

For security issues, please **do not file a public issue**. Use GitHub's [private vulnerability reporting](https://github.com/D0ubleD0uble/fieldglass/security/advisories/new) instead.

## Development setup

```sh
git clone git@github.com:D0ubleD0uble/fieldglass.git
cd fieldglass
pipx install pre-commit                     # or pip install --user pre-commit
npm install                                  # auto-installs git hooks via the prepare step
```

Then build the native module and the extension:

```sh
cd crates/fieldglass-napi
npx napi build --platform --release --output-dir ../../extension/bin
cd ../../extension
npm install
npm run compile
```

Open the repo in VS Code and press `F5` to launch an Extension Development Host with Fieldglass loaded.

## Pull request workflow

Fieldglass uses a release-candidate branching model: feature work accumulates on a `release/X.Y.Z` branch for the next version, and that branch is promoted to `master` in a single merge at release time (alongside a `vX.Y.Z` tag). The active candidate branch is whichever `release/*` branch is currently *ahead* of `master`; check `git branch -r | grep release/` and pick the one with commits to spare.

1. Fork the repo and create a feature branch from the active release candidate, not from `master`:
   ```sh
   git fetch origin
   git switch -c my-feature origin/release/0.1.2   # substitute the current RC
   ```
2. Make your change. The local pre-commit hook runs `cargo fmt`, `cargo clippy -- -D warnings`, `tsc --noEmit`, plus file-hygiene polish on every commit. The pre-push hook runs `cargo test --workspace`, `cargo deny check`, `npm audit`, and a `semgrep` SAST scan.
3. Update [CHANGELOG.md](CHANGELOG.md) under the `## [Unreleased]` heading.
4. Open the PR **targeting the release candidate branch**, not `master`:
   ```sh
   gh pr create --base release/0.1.2
   ```
   CI runs the same checks regardless of base; required statuses are `Lint + test via pre-commit`, `Build extension`, `Analyze (rust)`, `Analyze (javascript-typescript)`, and `Semgrep SAST`.

At release time a `release/X.Y.Z-prep` branch off the RC bumps versions and promotes the `## [Unreleased]` heading; that prep PR merges into the RC, then the RC is promoted to `master` in one merge and tagged `vX.Y.Z`.

A few project-specific patterns worth knowing:

- `crates/fieldglass-core` must remain free of format-specific imports and free of `napi` types. Format crates depend on it; it depends on nothing else.
- WMO lookup tables live in `crates/fieldglass-grib1/src/tables.rs` (GRIB1, WMO ON388) and `crates/fieldglass-grib2/src/tables.rs` (GRIB2, WMO FM 92 Code Tables 0.0 / 1.x / 3.x / 4.x). They're the single source of truth for parameter / centre / level / process names — add to the tables rather than hardcoding strings at the napi or TypeScript layer.
- The data flow for adding a new metadata field is the same across editions: parse it in the relevant section module (GRIB1 `is/pds/gds/bds.rs`, GRIB2 `is/ids/lus/gds/pds/drs/bms/ds.rs`) → expose it on the section struct → populate `MessageMeta` in `crates/fieldglass-napi/src/lib.rs` (both `open_grib1` and `open_grib2` produce the same shape) → add the camelCase field on the `MessageMeta` interface in `extension/src/provider.ts` → render it in the webview table. napi-rs auto-converts `snake_case` Rust field names to `camelCase` in the generated TS bindings.

## Code of conduct

This project follows the [Contributor Covenant](CODE_OF_CONDUCT.md). Be kind; assume good faith; report problems privately to the maintainer.

## Licensing

Fieldglass is dual-licensed under MIT or Apache-2.0. By submitting a contribution you agree it's licensed under both, per the language in the README.
