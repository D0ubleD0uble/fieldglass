# Releasing Fieldglass

Operational checklist for cutting a Fieldglass release. The conceptual model
(trunk-based development, prep PRs, tag-triggered publish) lives in
[CONTRIBUTING.md § Pull request workflow](CONTRIBUTING.md#pull-request-workflow);
this doc is the *how*, not the *why*.

The Marketplace pre-release / stable convention applies: odd minors
(`0.1.x`, `0.3.x`, …) ship to the pre-release channel; stable releases jump
to the next even minor (`0.2.0`, `0.4.0`, …). All examples below use
`0.1.2` — substitute the version you're cutting.

## Roles

- **Master** — the trunk. Feature PRs land here continuously (see
  CONTRIBUTING.md), and the `vX.Y.Z` tag is placed on a commit here.
- **Prep branch** — `release-prep/X.Y.Z`. A short-lived branch off `master`
  that bumps versions and promotes the CHANGELOG. Its merge commit on `master`
  is the commit that gets tagged.

## 1 — Prep PR

When `master` contains everything you want to ship:

```sh
git fetch origin
git switch -c release-prep/X.Y.Z origin/master
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

Open the prep PR against `master`:

```sh
gh pr create --base master --title "release: prep X.Y.Z"
```

CI must be green before moving on. Merge it, then record the merge commit SHA —
that exact commit is what you verify and tag below. `master` is a moving trunk,
so everything from here on pins to that SHA rather than to "master HEAD":

```sh
git fetch origin
RELEASE_SHA=$(git rev-parse origin/master)   # the prep merge commit
```

## 2 — Pre-deploy verification

Verify the prep merge commit (`$RELEASE_SHA`), not whatever has landed on
`master` since:

- [ ] **CI green on the prep merge** — `gh run list --branch master --limit 5` and confirm the run for `$RELEASE_SHA`. All of `ci.yml`, `coverage.yml`, `semgrep.yml`, `codeql.yml` should pass.
- [ ] **Release-workflow dry-run** — manually trigger `release.yml` against the prep merge commit. This builds the full six-target `.vsix` matrix without publishing (the publish job is gated on `refs/tags/v*`):

  ```sh
  gh workflow run release.yml --ref "$RELEASE_SHA"
  gh run list --workflow=release.yml --limit 3
  ```

  Wait for completion (typically ~5 min). The six native builds + six `.vsix` packages should all be green; the "Publish to Marketplace + GitHub Release" job should appear with a dash (skipped) — that's the gate working as designed.

- [ ] **Manual smoke test in a dev host (F5)** — open one fixture from each format and exercise the user-facing path:
  - GRIB1: render a temperature message from a multi-message file; toggle projection picker (Source / Equirectangular) and resampling (Nearest / Bilinear); confirm the canvas paints and the caption reads correctly on both picker positions.
  - GRIB2: open a simple-packed fixture (`regular_latlon_surface.grib2`); confirm message table populates and Render works.
  - NetCDF: open a classic `.nc` (`netcdf_classic_dummy.nc`); confirm the dataset-metadata view renders dimensions, attributes, and variables.

  The integration tests cover the wire path, but a visual sanity check is the last guard against regressions that only manifest in the UI (CSS, picker wiring, colorbar).

- [ ] **Marketplace screenshot fresh** — if the render UI changed materially, refresh `extension/media/screenshot.png` so the Marketplace listing reflects the shipping version.

## 3 — Tag and publish

When the dry-run is green and the smoke test passes, tag the verified prep
merge commit — `$RELEASE_SHA`, not `master` HEAD. Pinning the SHA means a
feature or Dependabot PR that landed on `master` since prep can't slip into
this release; it simply rides the next one, and the tag reflects exactly what
you verified:

```sh
git tag -a vX.Y.Z "$RELEASE_SHA" -m "vX.Y.Z"
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

## 4 — Post-release verification

- [ ] **GitHub Release created** at `https://github.com/D0ubleD0uble/fieldglass/releases/tag/vX.Y.Z` with six `.vsix` attachments.
- [ ] **CHANGELOG link refs resolve** — `[X.Y.Z]: …/compare/v{prev}...vX.Y.Z` should be live now that the tag exists.
- [ ] **Marketplace listing updated** at `https://marketplace.visualstudio.com/items?itemName=fieldglass.fieldglass` — the new version number, screenshot, and README all reflect what shipped.
- [ ] **Install from Marketplace and round-trip** a real file from each format in a clean VS Code install. The full chain — Marketplace → `.vsix` selection by platform → activation → file open → render — is something only a real install can validate.
- [ ] **Linked issues already closed** — issues with `Closes #N` in their PR auto-closed when that PR merged to `master`, so this needs no action in the normal case. Just spot-check the CHANGELOG's `Closes #N` references are in fact closed; a still-open one means its PR didn't carry the keyword.

## When things break

- **Dry-run native build fails on one target** — usually a toolchain drift (windows-arm64 has been the recurring culprit). Fix in a normal feature PR to `master`, re-prep so the fix is in the tagged commit, rerun the dry-run; do not tag until it's green.
- **Tag pushed but publish fails partway** — the GitHub Release will be missing assets. Re-run the failed job from the Actions UI; the workflow is idempotent for the platform builds.
- **A regression slips past CI** — if it's caught after publish but before users adopt, the cleanest fix is a hotfix release (`vX.Y.Z+1`): land the fix on `master` like any other PR, run a fresh prep PR, and tag the new merge commit. Don't retag.
- **Cutting an even-minor *stable* release** — `release.yml` derives the channel from the tag's minor parity automatically (its `channel` job: odd minor → pre-release, even minor → stable), and feeds that to `vsce package`, `vsce publish`, and the GitHub Release `prerelease` field. So an even-minor tag (`0.2.0`, `0.4.0`, …) publishes to the stable channel with no pre-tag workflow edit needed. Just tag the right version and the channel follows.

## What lives where

| Layer | Doc |
|---|---|
| Branch model + PR rules | [CONTRIBUTING.md](CONTRIBUTING.md) |
| What's shipping in each version | [CHANGELOG.md](CHANGELOG.md) |
| Release procedure (this doc) | RELEASING.md |
| Build/publish automation | [`.github/workflows/release.yml`](.github/workflows/release.yml) |
| Security disclosure | [SECURITY.md](SECURITY.md) |
