# Releasing Fieldglass

Operational checklist for cutting a Fieldglass release. The conceptual model
(release-candidate branches, prep PRs, master promotion) lives in
[CONTRIBUTING.md § Pull request workflow](CONTRIBUTING.md#pull-request-workflow);
this doc is the *how*, not the *why*.

The Marketplace pre-release / stable convention applies: odd minors
(`0.1.x`, `0.3.x`, …) ship to the pre-release channel; stable releases jump
to the next even minor (`0.2.0`, `0.4.0`, …). All examples below use
`0.1.2` — substitute the version you're cutting.

## Roles

- **Release branch** — `release/X.Y.Z`. Feature PRs target this branch
  during the release cycle (see CONTRIBUTING.md).
- **Prep branch** — `release/X.Y.Z-prep`. A short-lived branch off the
  release branch that bumps versions and promotes the CHANGELOG.
- **Master** — the publish reference. The `vX.Y.Z` tag is placed on the
  master commit that merges `release/X.Y.Z` in.

## 1 — Prep PR

When the release branch contains everything you want to ship:

```sh
git fetch origin
git switch -c release/X.Y.Z-prep origin/release/X.Y.Z
```

Bump versions in lockstep:

| File | What |
|---|---|
| `Cargo.toml` (workspace) | `[workspace.package].version` → new version |
| `crates/fieldglass-{grib1,grib2,napi,netcdf}/Cargo.toml` | internal `version = "=X.Y.Z"` pins to match |
| `extension/package.json` | `version` field |
| `Cargo.lock` | `cargo check --workspace` to refresh |
| `extension/package-lock.json` | `cd extension && npm install --package-lock-only` to refresh |

Promote the CHANGELOG: rename `## [Unreleased]` to `## [X.Y.Z] — YYYY-MM-DD`
(today's date), update the `[Unreleased]` / `[X.Y.Z]` link references at the
bottom of the file, and review entries one more time for accuracy. The
`## [X.Y.Z]` section becomes the GitHub Release body verbatim (the publish
workflow extracts it by heading — see §4), so make sure it reads as user-facing
release notes.

Reconcile the README with what shipped: walk the entries you just promoted and
update any capability statement they contradict — the feature matrix, the
`GRIB2 …` / **Known limitations** bullets, the per-crate table, and the packing
tables. The README ships inside the `.vsix` and drives the Marketplace listing,
so a stale capability list goes out to users.

Run the local gates before pushing:

```sh
cargo test --workspace
cd extension && npm test     # needs xvfb-run -a on headless boxes
```

Open the prep PR against the release branch:

```sh
gh pr create --base release/X.Y.Z --title "release: prep X.Y.Z"
```

CI must be green before moving on. Merge.

## 2 — Pre-deploy verification

After the prep PR merges into `release/X.Y.Z`:

- [ ] **CI green on `release/X.Y.Z`** — `gh run list --branch release/X.Y.Z --limit 5`. All of `ci.yml`, `coverage.yml`, `semgrep.yml`, `codeql.yml` should pass on the merged tip.
- [ ] **Release-workflow dry-run** — manually trigger `release.yml` against `release/X.Y.Z`. This builds the full six-target `.vsix` matrix without publishing (the publish job is gated on `refs/tags/v*`):

  ```sh
  gh workflow run release.yml --ref release/X.Y.Z
  gh run list --workflow=release.yml --limit 3
  ```

  Wait for completion (typically ~5 min). The six native builds + six `.vsix` packages should all be green; the "Publish to Marketplace + GitHub Release" job should appear with a dash (skipped) — that's the gate working as designed.

- [ ] **Manual smoke test in a dev host (F5)** — open one fixture from each format and exercise the user-facing path:
  - GRIB1: render a temperature message from a multi-message file; toggle projection picker (Source / Equirectangular) and resampling (Nearest / Bilinear); confirm the canvas paints and the caption reads correctly on both picker positions.
  - GRIB2: open a simple-packed fixture (`regular_latlon_surface.grib2`); confirm message table populates and Render works.
  - NetCDF: open a classic `.nc` (`netcdf_classic_dummy.nc`); confirm the dataset-metadata view renders dimensions, attributes, and variables.

  The integration tests cover the wire path, but a visual sanity check is the last guard against regressions that only manifest in the UI (CSS, picker wiring, colorbar).

- [ ] **Marketplace screenshot fresh** — if the render UI changed materially, refresh `extension/media/screenshot.png` so the Marketplace listing reflects the shipping version.

## 3 — Promote to master

When the dry-run is green and the smoke test passes:

```sh
gh pr create --base master --head release/X.Y.Z \
  --title "release: X.Y.Z" \
  --body "Promotes release/X.Y.Z to master; see CHANGELOG.md for the entry."
```

This is the single merge that brings the release branch's commits onto
master. CI runs on the PR as usual. Merge it.

## 4 — Tag and publish

After the master merge lands:

```sh
git fetch origin
git switch master
git pull --ff-only
git tag -a vX.Y.Z -m "vX.Y.Z"
git push origin vX.Y.Z
```

The tag push triggers `release.yml`'s publish path:

- builds all six native targets
- packages six platform-specific `.vsix` files
- publishes to the VS Code Marketplace
- creates the GitHub Release with the `.vsix` files attached and the release
  notes taken from this version's `## [X.Y.Z]` section of CHANGELOG.md (the
  workflow's *Extract release notes* step pulls that section by heading — not
  GitHub's auto-generated commit list)

Watch the run:

```sh
gh run list --workflow=release.yml --limit 1
gh run watch
```

## 5 — Post-release verification

- [ ] **GitHub Release created** at `https://github.com/D0ubleD0uble/fieldglass/releases/tag/vX.Y.Z` with six `.vsix` attachments.
- [ ] **CHANGELOG link refs resolve** — `[X.Y.Z]: …/compare/v{prev}...vX.Y.Z` should be live now that the tag exists.
- [ ] **Marketplace listing updated** at `https://marketplace.visualstudio.com/items?itemName=fieldglass.fieldglass` — the new version number, screenshot, and README all reflect what shipped.
- [ ] **Install from Marketplace and round-trip** a real file from each format in a clean VS Code install. The full chain — Marketplace → `.vsix` selection by platform → activation → file open → render — is something only a real install can validate.
- [ ] **Issues that closed in this release** — review the CHANGELOG's `Closes #N` references and confirm those issues are closed (the Marketplace can't see GitHub issue state; this is a manual cleanup).

## When things break

- **Dry-run native build fails on one target** — usually a toolchain drift (windows-arm64 has been the recurring culprit). Fix in a new PR on the release branch; rerun the dry-run; do not tag until it's green.
- **Tag pushed but publish fails partway** — the GitHub Release will be missing assets. Re-run the failed job from the Actions UI; the workflow is idempotent for the platform builds.
- **A regression slips past CI** — if it's caught after publish but before users adopt, the cleanest fix is a hotfix release (`vX.Y.Z+1`) from a fresh prep PR. Don't retag.
- **Cutting an even-minor *stable* release** — `release.yml` is wired for the pre-release channel only: it hardcodes `vsce package --pre-release`, `vsce publish --pre-release`, and `prerelease: true` on the GitHub Release. The stable jump (`0.2.0`, `0.4.0`, …) needs those gated on the minor's parity **before** tagging — otherwise an even-minor tag still publishes to the pre-release channel. Don't tag a stable minor until the workflow is updated.

## What lives where

| Layer | Doc |
|---|---|
| Branch model + PR rules | [CONTRIBUTING.md](CONTRIBUTING.md) |
| What's shipping in each version | [CHANGELOG.md](CHANGELOG.md) |
| Release procedure (this doc) | RELEASING.md |
| Build/publish automation | [`.github/workflows/release.yml`](.github/workflows/release.yml) |
| Security disclosure | [SECURITY.md](SECURITY.md) |
