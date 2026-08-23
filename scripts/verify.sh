#!/usr/bin/env bash
# Run Verus over the verification crate (issue #197).
#
# Verus is not a cargo dependency and is not installed by `cargo`: it ships as a
# prebuilt release containing its own `z3` and `vstd`, and it requires a
# specific rustup toolchain. Everything it needs is pinned here so a local run
# and a CI run verify the same thing.
#
#   scripts/verify.sh            # verify everything
#   scripts/verify.sh --install  # fetch the pinned Verus, then verify
#
# See docs/verification.md.
set -euo pipefail

# Pinned together, and only ever bumped together: `vstd` on crates.io is
# versioned by release date and is guaranteed to work only with the matching
# Verus. The vstd pin lives in crates/fieldglass-verify/Cargo.toml.
VERUS_VERSION="0.2026.08.15.7d4628a"
VERUS_TOOLCHAIN="1.97.1-x86_64-unknown-linux-gnu"

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
verify_crate="${repo_root}/crates/fieldglass-verify"
install_root="${VERUS_HOME:-${HOME}/.local/share/verus}/${VERUS_VERSION}"

install_verus() {
  local url="https://github.com/verus-lang/verus/releases/download/release/${VERUS_VERSION}/verus-${VERUS_VERSION}-x86-linux.zip"
  local tmp
  tmp="$(mktemp -d)"
  trap 'rm -rf "${tmp}"' RETURN

  echo "Fetching Verus ${VERUS_VERSION}…"
  curl -fsSL "${url}" -o "${tmp}/verus.zip"
  unzip -q "${tmp}/verus.zip" -d "${tmp}"
  mkdir -p "$(dirname "${install_root}")"
  rm -rf "${install_root}"
  mv "${tmp}/verus-x86-linux" "${install_root}"

  # `verus` resolves its bundled vstd and z3 from the `verus-root` marker beside
  # the real binary, so link the directory rather than the executables.
  ln -sfn "${install_root}" "$(dirname "${install_root}")/current"
  echo "Installed to ${install_root}"
}

if [[ "${1:-}" == "--install" ]]; then
  install_verus
  shift
fi

if ! command -v cargo-verus >/dev/null 2>&1 && [[ ! -x "${install_root}/cargo-verus" ]]; then
  echo "error: cargo-verus not found." >&2
  echo "       Run 'scripts/verify.sh --install', or see docs/verification.md." >&2
  exit 1
fi

# Prefer the pinned install over whatever is on PATH, so a stray newer Verus
# cannot silently verify against a different vstd than the one pinned here.
if [[ -x "${install_root}/cargo-verus" ]]; then
  PATH="${install_root}:${PATH}"
  export PATH
fi

if ! rustup toolchain list 2>/dev/null | grep -q "^${VERUS_TOOLCHAIN%%-*}"; then
  echo "note: Verus ${VERUS_VERSION} wants rustup toolchain ${VERUS_TOOLCHAIN}." >&2
  echo "      Install it with: rustup toolchain install ${VERUS_TOOLCHAIN%%-x86_64*}" >&2
fi

echo "Verifying ${verify_crate} with Verus ${VERUS_VERSION}…"
cd "${verify_crate}"

# Cargo caches a successful verification, so a second run prints nothing at all
# and still exits 0 — a gate that passes without having checked anything.
# Discard just our crate's artifacts so it is always re-verified; `vstd` stays
# cached, which is where all the time goes (0.6s versus 30s). Then insist the
# output proves the run happened.
cargo clean -p fieldglass-verify >/dev/null 2>&1 || true

set +e
output="$(cargo verus verify "$@" 2>&1)"
status=$?
set -e
printf '%s\n' "${output}"

if [[ ${status} -ne 0 ]]; then
  exit "${status}"
fi

if ! grep -q 'verification results' <<<"${output}"; then
  echo >&2
  echo "error: Verus reported no verification results, so nothing was checked." >&2
  echo "       A silent pass here is worse than a failure — treat it as one." >&2
  exit 1
fi
