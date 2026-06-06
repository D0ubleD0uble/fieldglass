#!/usr/bin/env bash
#
# Setup script for Claude Code on the web (and routines / cloud runners).
#
# This is NOT auto-discovered from the repo. Paste its contents into the cloud
# Environment's "Setup script" field (claude.ai/code → environment settings).
# It runs as root on Ubuntu 24.04 before Claude launches, and its output is
# cached (~7 days), so keep total runtime under ~5 minutes.
#
# The base image already provides rustc+cargo, Node 22+npm, and Python+pip, and
# the default "Trusted" network level already allows crates.io, npm, PyPI,
# GitHub, and Ubuntu apt. This script only adds the quality-gate tools the
# repo's pre-commit / pre-push hooks need but the base image doesn't guarantee.
#
# Non-critical installs use `|| true` so an intermittent failure doesn't block
# the session from starting.

set -u

# Rust lint/format components — pre-commit runs `cargo fmt --check` and
# `cargo clippy --all-targets -- -D warnings`.
rustup component add clippy rustfmt || true

# Supply-chain audit used by the pre-push hook (`cargo deny check`).
cargo install cargo-deny --locked || true

# The repo's npm `prepare` step runs `pre-commit install`, after which every
# commit runs fmt / clippy / deny / semgrep / shellcheck / actionlint /
# gitleaks. Those hook tools auto-download from GitHub and PyPI (both allowed).
pip install --quiet pre-commit || true

# OPTIONAL — uncomment only for tasks that (re)generate or validate GRIB
# fixtures, eccodes oracles, or parameter tables. The test suite itself runs
# WITHOUT eccodes (it uses committed .eccodes.ref.json snapshots). Note: apt
# installs Ubuntu's eccodes, not the pinned 2.34.1, so byte-identical fixture
# reproduction is not guaranteed.
# apt-get update && apt-get install -y libeccodes-tools || true

# OPTIONAL — pre-warm dependency caches so sessions start faster (cached to
# disk). `npm ci` runs the repo's `prepare` hook, which needs pre-commit
# installed above, so keep this after it.
# cargo fetch --locked || true
# (cd extension && npm ci) || true

exit 0
