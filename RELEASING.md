# Releasing Fieldglass

Operational checklist for cutting a Fieldglass release. The conceptual model
(trunk-based development, prep PRs, tag-triggered publish) lives in
[CONTRIBUTING.md § Pull request workflow](CONTRIBUTING.md#pull-request-workflow);
this doc is the *how*, not the *why*.

Versioning is plain semver, and **every tag is a stable release**. Pre-1.0, a
minor bump (`0.3.0` → `0.4.0`) may break the Rust API; a patch (`0.3.0` →
`0.3.1`) does not. The extension and the eight library crates share the version.

There is no pre-release channel. Fieldglass used one through `0.1.x` — under the
Marketplace's odd/even-minor convention, where the minor digit encodes the
channel — and retired it at `0.2.0`. Encoding the channel in the version number
makes the minor digit meaningless to semver, which matters now that the crates
publish to crates.io, where cargo reads it strictly. If a soak build is ever
wanted, `vsce publish --pre-release` is a per-publish flag; nothing in the
version scheme has to change to use it.

All examples below use `0.1.2` — substitute the version you're cutting.

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
| `crates/fieldglass{,-grib1,-grib2,-netcdf,-zarr,-fetchplan,-napi,-wasm}/Cargo.toml` | internal `version = "=X.Y.Z"` pins to match (`-grib2` and `-netcdf` pin `fieldglass-aec` as well as `fieldglass-core`; `fieldglass` pins every crate it can take; `-napi` and `-wasm` pin `fieldglass`). `grep -rn '"=<old>"' --include=Cargo.toml crates` finds them all |
| `extension/package.json` | `version` field |
| `Cargo.lock` | `cargo check --workspace` to refresh |
| `crates/fieldglass-{aec,grib1,grib2,netcdf,zarr,fetchplan}/fuzz/Cargo.lock` | refresh each of the six — the fuzz crates are excluded from the workspace, so `cargo check --workspace` does **not** touch their locks, yet each lock still records the resolved `fieldglass-*` version. Run `cargo update -w` in each `fuzz/` dir (or `cargo check`) so the committed locks aren't left on the old version. Forgetting is caught by the `nested-lockfiles` pre-commit hook, which refuses the bump commit itself; if hooks are bypassed it fails the **Nested lockfiles in sync** job and blocks the fuzz jobs until fixed. |
| `crates/fieldglass-verify/Cargo.lock` | the same shape and the same hook, but nothing to do at release time: `fieldglass-verify` is its own workspace and depends on `vstd` alone, so no `fieldglass-*` version reaches its lock. Listed because the hook covers it and this table is where a maintainer looks. |
| `extension/package-lock.json` | `cd extension && npm install --package-lock-only` to refresh |

Two versions in the tree deliberately **do not** move with the release, and
both look like omissions if you are sweeping for the old number:

- `crates/fieldglass-napi/package.json` — the napi crate's npm manifest, on its
  own `0.1.0`. It is build tooling for `napi build`, never published (the crate
  carries `publish = false`), and nothing reads its version. Bumping it is
  harmless but meaningless; leaving it alone is correct.
- `rust-j2k = "=X.Y.Z"` in the workspace `Cargo.toml` — an **external**
  dependency that happens to be pinned exactly, not one of ours. It moves only
  when that crate is upgraded, and when it does, the fuzz lockfiles have to
  move with it (see the row above; that skew was #398).

Promote the CHANGELOG: rename `## [Unreleased]` to `## [X.Y.Z] — YYYY-MM-DD`
(today's date), fix the link references at the bottom of the file, and review
entries one more time for accuracy. Both link edits are easy to forget — do
**both**:

- repoint `[Unreleased]` to compare from the version you're cutting:
  `compare/vX.Y.Z...HEAD`;
- add a new `[X.Y.Z]:` line for the released version:
  `compare/v{prev}...vX.Y.Z`.

A missing `[X.Y.Z]:` line renders the heading as a dead link, and leaving
`[Unreleased]` on the previous base makes its diff span two releases. The
`## [X.Y.Z]` section becomes the GitHub Release body verbatim (the publish
workflow extracts it by heading; see §3), so make sure it reads as user-facing
release notes. A release body must be under 125,000 characters; the workflow's
Release action cuts a longer one short without a warning. Check the
promoted section with:

```sh
python3 tools/release_notes.py --tag vX.Y.Z
```

It prints the section's length and fails if the section is missing, empty or
too long, or if `X.Y.Z` is not the workspace version.

Reconcile the README with what shipped: walk the entries you just promoted and
update any capability statement they contradict — the feature matrix, the
`GRIB2 …` / **Known limitations** bullets, the per-crate table, and the packing
tables. The README ships inside the `.vsix` and drives the Marketplace listing,
so a stale capability list goes out to users.

Re-record the browser bundle-size table in `crates/fieldglass-wasm/README.md`.
Between releases a PR re-records it only once CI reports drift above 1.0%, so
smaller changes add up over a cycle, and this is where they are written down.
Once the prep PR is open (below), open the *Bundle-size gate* step of its
`ci.yml` `wasm` job, and copy each build's measured `.wasm` and gzipped bytes
from the run summary into the table, with a paragraph above it saying what the
cycle added. Use CI's figures, not a local build's: CI builds on the toolchain
the release ships with. Push that as a commit on the prep PR; its next run
then reports 0.0%.

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
- [ ] **Release-workflow dry-run** — manually trigger `release.yml` against the prep merge commit. This builds the full six-target `.vsix` matrix without publishing (the publish job is gated on `refs/tags/v*`). A manual run takes a branch or tag name, not a SHA, so point a throwaway branch at the commit first; running it on `master` would build whatever has landed since prep:

  ```sh
  git push origin "$RELEASE_SHA:refs/heads/dry-run/vX.Y.Z"
  gh workflow run release.yml --ref dry-run/vX.Y.Z
  gh run list --workflow=release.yml --event workflow_dispatch --limit 3 --json databaseId,headBranch,headSha,status,conclusion
  ```

  The run's `headSha` must be `$RELEASE_SHA`. Delete the branch after the tag (`git push origin --delete dry-run/vX.Y.Z`).

  Wait for completion (typically ~5 min). The six native builds + six `.vsix` packages should all be green, and so should "Check release notes": with no tag it checks the workspace version's CHANGELOG section, which must exist, and, if it has content, `## [Unreleased]`, against GitHub's 125,000-character limit. Every publishing job waits for it, so on a tag a bad section stops the release before anything goes out. The "Publish to Marketplace + GitHub Release" job should appear with a dash (skipped) — that's the gate working as designed.

- [ ] **Manual pass over what changed** — [RELEASE-TEST-PLAN.md](RELEASE-TEST-PLAN.md)
  is the per-cycle plan: it is rewritten each release to cover everything that
  landed since the last tag, grouped so each file is opened once. Work it top
  to bottom and record the outcome. The smoke test below is the floor — the
  minimum for a release whose diff is small — not a substitute for the plan
  when the cycle shipped user-facing features.

- [ ] **Manual smoke test in a dev host (F5)** — open one fixture from each format and exercise the user-facing path:
  - GRIB1: render a temperature message from a multi-message file; toggle projection picker (Source / Equirectangular) and resampling (Nearest / Bilinear); confirm the canvas paints and the caption reads correctly on both picker positions.
  - GRIB2: open a simple-packed fixture (`regular_latlon_surface.grib2`); confirm message table populates and Render works.
  - NetCDF: open a classic `.nc` (`netcdf_classic_dummy.nc`); confirm the dataset-metadata view renders dimensions, attributes, and variables.

  The integration tests cover the wire path, but a visual sanity check is the last guard against regressions that only manifest in the UI (CSS, picker wiring, colorbar).

- [ ] **npm trusted publisher, when one is due** — at the first release after
  `@fieldglass/wasm`'s bootstrap, create it now, no more than 2 days before the
  tag (§3 *npm*). At the bootstrap release itself, publish the package by hand
  from this dry run's artefact instead.

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
- extracts and checks this version's release notes (the *Check release notes*
  job); every publish below waits for it
- publishes to the VS Code Marketplace
- publishes the eight library crates to crates.io, on a **stable tag only** (see
  below)
- publishes `@fieldglass/wasm` to npm, also tag-only (see below)
- creates the GitHub Release with the `.vsix` files attached and the release
  notes taken from this version's `## [X.Y.Z]` section of CHANGELOG.md (the
  *Check release notes* job pulls that section by heading — not GitHub's
  auto-generated commit list)

### crates.io

The eight library crates publish to crates.io from the `publish-crates` job, in
dependency order: `fieldglass-aec`, `fieldglass-core`, `-grib1`, `-grib2`,
`-netcdf`, `-zarr`, `-fetchplan`, and the `fieldglass` umbrella last.
`fieldglass-napi` and `fieldglass-wasm` do not: each carries `publish = false`,
since each is a host binding, not a library anyone should depend on (the browser
build reaches users through npm instead; see below).

**Every tag publishes.** A `workflow_dispatch` dry run has no tag, so it skips
this job — the dry run remains free of side effects.

**Every stable release publishes all eight crates, whether or not they changed.**
Each crate pins the workspace crates it takes with `=` (the format crates pin
core, `-grib2` and `-netcdf` also pin `fieldglass-aec`, `-fetchplan` pins
`-zarr`, and `fieldglass` pins them all), so their manifests change with every
version bump by construction. That lockstep is deliberate while the API is
pre-1.0; it is not worth the bookkeeping to publish them independently.

**Auth is Trusted Publishing** (OIDC): the job exchanges a GitHub identity token
for a short-lived registry token, so there is no long-lived crates.io secret in
the repo's settings.

**Re-running a failed release is safe.** `cargo publish` errors if a version is
already on crates.io, so the job checks the sparse index first and skips any
crate whose version is already out. A run that died halfway through can simply be
re-run from the Actions UI.

#### First publish: a one-time manual bootstrap

crates.io only lets you configure Trusted Publishing for a crate **that already
exists**, so the very first publish of each crate cannot come from the workflow.
Once, from a maintainer machine, with a scoped API token:

```sh
# Core first: the format crates pin it with `=` and cannot even be packaged
# until it is in the index.
cargo publish -p fieldglass-core
cargo publish -p fieldglass-grib1
cargo publish -p fieldglass-grib2
cargo publish -p fieldglass-netcdf
```

Then, on crates.io, add a Trusted Publishing entry for **each of the four
crates**: repository `D0ubleD0uble/fieldglass`, workflow `release.yml`. After
that the workflow takes over and the token can be revoked.

Until that bootstrap happens, a stable tag's `publish-crates` job will fail on
the first `cargo publish` — nothing else in the release is affected, since the
Marketplace publish and the GitHub Release are separate jobs.

**`fieldglass-aec` needs the same bootstrap once** (ADR-0012 decision 10). The
four crates above were bootstrapped at 0.3.0. `fieldglass-aec` joined the loop
with #762, so at the first stable tag after that merge, before or after the
workflow first fails on it:

```sh
cargo publish -p fieldglass-aec     # with a scoped API token
```

then add its Trusted Publishing entry (repository `D0ubleD0uble/fieldglass`,
workflow `release.yml`) and re-run the `publish-crates` job. It is first in the
loop, so a missed bootstrap fails the job before any other crate goes out, and
the re-run skips whatever is already published.

**`fieldglass-zarr`, `fieldglass-fetchplan` and `fieldglass` need it once too**
(#851). They join the end of the loop at 0.6.0, in that order. Each pins the
ones before it with `=`, so publish them in that order, once the five crates
ahead of them in the loop are on the index at the same version:

```sh
cargo publish -p fieldglass-zarr        # API token with the publish-new scope
cargo publish -p fieldglass-fetchplan
cargo publish -p fieldglass
```

then add a Trusted Publishing entry for each, as above. The simplest order is to
let the tag's `publish-crates` job publish the first five crates and fail on
`fieldglass-zarr`, run the three commands from a clean checkout of the tag, add the
entries, and re-run the job, which then skips all eight. Unlike `fieldglass-aec`,
a missed bootstrap here fails the job *after* those five are out, so the
failure is partial; the re-run is still safe.

At 0.6.0 this falls in the same release as `fieldglass-aec`'s first publish
(#861), so expect two stops: the first run fails on `fieldglass-aec` before
anything goes out; after its bootstrap the re-run publishes the next four and
fails on `fieldglass-zarr`; after the three commands above a final re-run skips
all eight. `fieldglass-aec` pins nothing, so it can also be published by hand
before the tag, which saves the first stop.

To check all eight package and build before a release, without uploading
anything, run `cargo publish --workspace --dry-run`. Cargo packages every
publishable member and verifies each against the others' packages rather than
crates.io, so it works before any of the three new crates exists there. Run it
on the prep branch after the version bump, when the new version is not on
crates.io yet; that is the case it was checked on (#851).

That check builds only each library. A published crate's tests also have to
build from the `.crate`, which carries only the files under its own directory:
not another crate's fixtures, not its `fuzz/` directory (a separate package),
and no path-only dev-dependency, which cargo strips. So a test reads only files
inside its own crate, and a fixture another crate also needs is copied, not
reached for (#926). `fieldglass` is the exception: its integration tests need
the whole repository and are left out of its package by `exclude`.

The `packaged-crate-tests` job in `ci.yml` checks this on every pull request. It
runs `tools/check_packaged_crate_tests.py`, which packages the publishable
crates, unpacks each outside the repository, points every sibling at its
unpacked copy (so it never tests against a version on crates.io), and runs
`cargo test` there. Run the same script locally with `--allow-dirty` to
reproduce a failure. Nothing extra is needed before a release.

### npm

`@fieldglass/wasm` is the browser build, published from the `publish-wasm-npm`
job. It is one package, built with `wasm-bindgen --target web` and `wasm-opt
-Oz`; `crates/fieldglass-wasm/pack.sh` assembles it and `npm pack`s it. The
Node addon (`fieldglass-napi`) has nothing to do with it and takes no wasm
dependency.

**The version is never typed.** `pack.sh` reads it from the workspace
`Cargo.toml`, and the job then checks it against the tag. The committed
`crates/fieldglass-wasm/npm/package.json` carries a `0.0.0` placeholder, which
`pack.sh` refuses to publish — so there is no second place to bump at release
time. It is not in the version table in §1 for that reason.

**Every tag publishes (except the bootstrap release, below); a
`workflow_dispatch` dry run builds the tarball and does not publish it.** The
dry run still runs the full package check — it installs the tarball into a
throwaway project and decodes a GRIB2 and a NetCDF fixture through it — and
uploads the `.tgz` as a run artefact. Every pull request runs
that same check in `ci.yml`, so a broken package fails long before a tag.

**Auth is Trusted Publishing** (OIDC), the same pattern as crates.io: the job
holds `id-token: write`, npm exchanges the identity token for a short-lived one,
and no npm secret is stored in the repo. npm generates the provenance
attestation automatically from that token — `--provenance` is neither passed nor
needed. This requires **npm 11.5.1 or later**, which is newer than the npm that
ships with Node 22, so the job upgrades npm explicitly before publishing.

**Re-running a failed release is safe.** The job asks `npm view` whether the
version is already published and skips if it is.

#### First publish: a one-time manual bootstrap

npm only lets you add a trusted publisher to a package **that already exists**
("Package must exist", in the [`npm trust`
reference](https://docs.npmjs.com/cli/v11/commands/npm-trust)), so the first
publish of `@fieldglass/wasm` cannot come from the workflow. It is the real
release, published by hand from the tarball the workflow built and the dry run
checked. There is no placeholder version (#853).

At the first release that ships the package (0.6.0):

1. **Account.** Turn on two-factor authentication on the npm account (avatar →
   *Account* → *Two-Factor Authentication*). npm requires it to publish and to
   manage trusted publishers.
2. **Scope.** Create the free npm organization `fieldglass` (avatar → *Add
   Organization*, the unlimited-public-packages plan), which is the
   `@fieldglass` scope. If the name is taken, stop: the package has to be
   renamed in `crates/fieldglass-wasm/npm/package.json`, the READMEs,
   `release.yml`, this file and the CHANGELOG before it can ship.
3. **Publish**, once every §2 item has passed and immediately before pushing
   the tag. A published npm version cannot be replaced, so a smoke-test failure
   found after this step would leave npm's `X.Y.Z` different from every other
   channel's. Take the tarball from §2's dry run, and check that the run built
   `$RELEASE_SHA`. `pack.sh` took the version from `Cargo.toml`, so it is
   already `X.Y.Z`, and `package.json` marks it public:

   First find the run and check its commit. Go on only if this prints `same
   commit`:

   ```sh
   gh run list --workflow=release.yml --event workflow_dispatch --limit 3 --json databaseId,headBranch,headSha,conclusion
   [ "$(gh run view <dry-run id> --json headSha -q .headSha)" = "$RELEASE_SHA" ] && echo "same commit"
   ```

   Then download, log in, and publish, one command at a time (`npm login` and
   `npm publish` both prompt):

   ```sh
   gh run download <dry-run id> -n fieldglass-wasm-npm -D /tmp/fieldglass-npm
   npm login                                                   # the account that owns the fieldglass org
   npm whoami
   npm publish /tmp/fieldglass-npm/fieldglass-wasm-X.Y.Z.tgz   # asks for the 2FA code
   npm view @fieldglass/wasm version                           # X.Y.Z
   ```

4. **Tag** as usual. The tag's `publish-wasm-npm` job finds `X.Y.Z` with
   `npm view` and skips the publish, so it needs no trusted publisher yet.

This first version has no npm provenance attestation, because it was not
published from a workflow. Every later one does.

**The trusted publisher is created just before the next release, not now.** A
new trusted-publisher configuration expires if it has not published within
**2 days** ([npm docs](https://docs.npmjs.com/trusted-publishers)), so one made
right after the bootstrap would be dead by the next tag. At the next release,
no more than 2 days before pushing the tag, open the package on npmjs.com →
*Settings* → *Trusted publishing* → *GitHub Actions*, and enter:

- organization or user `D0ubleD0uble`, repository `fieldglass`;
- workflow filename `release.yml`;
- no environment;
- **allow `npm publish`**. Configurations created after 2026-09-03 allow only
  `npm stage publish` unless this is ticked. A staged version is not public
  until a maintainer approves it, so the job would succeed with nothing
  installable, and its `npm view` re-run guard cannot rely on seeing it.

npm does not check the configuration when you save it, so a typo shows up only
as an authentication error at publish time. If the tag's run is more than
2 days away after all, delete the configuration and create it again later; an
expired one cannot be edited.

Watch the run:

```sh
gh run list --workflow=release.yml --limit 1
gh run watch
```

## 4 — Post-release verification

- [ ] **GitHub Release created** at `https://github.com/D0ubleD0uble/fieldglass/releases/tag/vX.Y.Z` with six `.vsix` attachments.
- [ ] **CHANGELOG link refs resolve** — `[X.Y.Z]: …/compare/v{prev}...vX.Y.Z` should be live now that the tag exists.
- [ ] **crates.io shows the new version** for all eight library crates — `cargo info fieldglass-core` should report `X.Y.Z`, and likewise for `-grib1`, `-grib2`, `-netcdf`, `fieldglass-aec`, `fieldglass-zarr`, `fieldglass-fetchplan` and `fieldglass`. `cargo info fieldglass-aec` and the last three are the ones that catch a missed first publish (see *crates.io → First publish* above).
- [ ] **npm shows the new version** — `npm view @fieldglass/wasm version` should report `X.Y.Z`. If the trusted publisher is configured for staged publishing, the version sits unapproved until a maintainer approves it from the Versions tab, and `npm view` will not report it until then.
- [ ] **Marketplace listing updated** at `https://marketplace.visualstudio.com/items?itemName=fieldglass.fieldglass` — the new version number, screenshot, and README all reflect what shipped.
- [ ] **Install from Marketplace and round-trip** a real file from each format in a clean VS Code install. The full chain — Marketplace → `.vsix` selection by platform → activation → file open → render — is something only a real install can validate.
- [ ] **Reset the manual test plan** — [RELEASE-TEST-PLAN.md](RELEASE-TEST-PLAN.md)
  describes the cycle that just shipped, so it is stale the moment the tag
  lands. Rewrite it against the new baseline as the next cycle's features
  merge, rather than leaving the previous release's plan in place to be
  mistaken for current.
- [ ] **Linked issues already closed** — issues with `Closes #N` in their PR auto-closed when that PR merged to `master`, so this needs no action in the normal case. Just spot-check the CHANGELOG's `Closes #N` references are in fact closed; a still-open one means its PR didn't carry the keyword.

## When things break

- **Dry-run native build fails on one target** — usually a toolchain drift (windows-arm64 has been the recurring culprit). Fix in a normal feature PR to `master`, re-prep so the fix is in the tagged commit, rerun the dry-run; do not tag until it's green.
- **Release-notes check fails on the tag** — nothing has been published: every publishing job waits for this check. The error says whether the `## [X.Y.Z]` section is missing, empty, 125,000 characters or longer, or for a version other than the workspace's. A re-run cannot help, because the tag still points at the same CHANGELOG. Fix the section in a PR; then, since no channel has the version yet, delete the tag (`git push origin --delete vX.Y.Z` and `git tag -d vX.Y.Z`) and tag the new merge commit once its dry run passes. A dry run (§2) runs the same check, so this should not reach a tag.
- **Tag pushed but the Marketplace publish or the GitHub Release fails** — re-run the failed job from the Actions UI. `vsce publish --skip-duplicate` skips each `.vsix` whose version and platform are already on the Marketplace, so a re-run publishes only what is missing and then creates the Release. Creating the Release again updates it and replaces its assets rather than failing.
- **crates.io publish fails partway** — say core went out and `-grib1` failed. Re-run the job: it checks the index and skips what is already published, so it picks up where it stopped. A version that went out *wrongly* cannot be replaced, only yanked (`cargo yank -p <crate> --version X.Y.Z`), and yanking does not free the version number — the fix ships as the next patch.
- **npm publish fails with an authentication error** — the trusted publisher for `@fieldglass/wasm` is missing, expired (it must publish within 2 days of being created), or configured for a different workflow file. The tarball is still attached to the run as an artefact, so a maintainer can publish it by hand if the release cannot wait; create a fresh configuration before the next tag.
- **npm publish succeeds but `npm i @fieldglass/wasm` cannot find the version** — the trusted publisher was created without *allow `npm publish`*, so the version landed staged. Approve it from the package's Versions tab on npmjs.com, then recreate the configuration with direct publishing allowed.
- **crates.io publish fails on the very first stable release** — most likely the Trusted Publishing bootstrap above hasn't been done. The rest of the release (Marketplace, GitHub Release) is unaffected; do the manual bootstrap and re-run the job.
- **A regression slips past CI** — if it's caught after publish but before users adopt, the cleanest fix is a hotfix release (`vX.Y.Z+1`): land the fix on `master` like any other PR, run a fresh prep PR, and tag the new merge commit. Don't retag.
- **Wanting a soak build before a stable one** — there is no pre-release channel any more, so this is a manual step: publish one version with `vsce publish --pre-release` (the flag is per-publish, not a property of the version), let it soak, then publish the *next* version without it. A version number can only be published once, so the promoted build needs its own number. For a one-off, handing out the `.vsix` from the release workflow's artifacts is usually simpler.

## What lives where

| Layer | Doc |
|---|---|
| Branch model + PR rules | [CONTRIBUTING.md](CONTRIBUTING.md) |
| What's shipping in each version | [CHANGELOG.md](CHANGELOG.md) |
| Release procedure (this doc) | RELEASING.md |
| Build/publish automation | [`.github/workflows/release.yml`](.github/workflows/release.yml) |
| Security disclosure | [SECURITY.md](SECURITY.md) |
