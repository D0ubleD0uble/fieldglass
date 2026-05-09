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

1. Fork the repo and create a feature branch from `master` (`git switch -c my-feature`).
2. Make your change. The local pre-commit hook runs `cargo fmt`, `cargo clippy -- -D warnings`, `tsc --noEmit`, plus file-hygiene polish on every commit. The pre-push hook runs `cargo test --workspace`, `cargo deny check`, `npm audit`, and a `semgrep` SAST scan.
3. Update [CHANGELOG.md](CHANGELOG.md) under the `## [Unreleased]` heading.
4. Open the PR. CI runs the same checks; required statuses are `Lint + test via pre-commit`, `Build extension`, `Analyze (rust)`, `Analyze (javascript-typescript)`, and `Semgrep SAST`.

A few project-specific patterns worth knowing:

- `crates/fieldglass-core` must remain free of format-specific imports and free of `napi` types. Format crates depend on it; it depends on nothing else.
- WMO ON388 lookup tables live in `crates/fieldglass-grib1/src/tables.rs` and are the single source of truth for parameter / centre / level-type names. Add to the tables rather than hardcoding strings at the napi or TypeScript layer.
- The data flow for adding a new metadata field is: parse it in the relevant section module → expose it on the section struct → populate `MessageMeta` in `crates/fieldglass-napi/src/lib.rs` → add the camelCase field on the `MessageMeta` interface in `extension/src/provider.ts` → render it in the webview table. napi-rs auto-converts `snake_case` Rust field names to `camelCase` in the generated TS bindings.

## Code of conduct

This project follows the [Contributor Covenant](CODE_OF_CONDUCT.md). Be kind; assume good faith; report problems privately to the maintainer.

## Licensing

Fieldglass is dual-licensed under MIT or Apache-2.0. By submitting a contribution you agree it's licensed under both, per the language in the README.
