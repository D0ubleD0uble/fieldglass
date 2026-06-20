# 0001 — GRIB2 compressed-packing codecs

**Status:** Accepted (2026-06-20). Resolves the [#111](https://github.com/D0ubleD0uble/fieldglass/issues/111) spike.

**Amended (2026-06-19):** The 5.42 decoder is **`rust-aec`**, not `oxiarc-szip`. While
implementing [#117](https://github.com/D0ubleD0uble/fieldglass/issues/117), `oxiarc-szip`
v0.3.3 failed the committed eccodes oracle — it decodes its own round-trips but is not
byte-compatible with real libaec streams. `rust-aec` (pure-Rust, purpose-built for GRIB2
5.42) decodes the fixture byte-for-byte against the oracle. See the 5.42 section below.

## Context

GRIB2 data-representation templates **5.40 (JPEG 2000)**, **5.41 (PNG)**, and
**5.42 (CCSDS / AEC)** wrap the packed integer grid in a third-party codec.
Unlike the bit-packed templates (5.0 / 5.2 / 5.3 / 5.4), decoding them needs a
real decompressor, and that choice ripples into our shipping artifact: the
extension bundles a prebuilt native addon for **six targets** —

- `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`
- `x86_64-apple-darwin`, `aarch64-apple-darwin`
- `x86_64-pc-windows-msvc`, `aarch64-pc-windows-msvc`

— so any decoder we add has to cross-compile to all of them. `aarch64-pc-windows-msvc`
(windows-arm64) has been the recurring toolchain pain in `release.yml`.

The spike asked four questions: codec per template (pure-Rust vs C binding),
licensing, cross-compilation, and fixture availability.

## What actually decides it

**Not licensing.** Every candidate is already on the `deny.toml` allowlist:
`png` (MIT/Apache), `rust-aec` (MIT), OpenJPEG (BSD-2-Clause), and libaec
(BSD-2-Clause). Licensing rules nothing out.

**Cross-compilation does.** A pure-Rust decoder cross-compiles to all six
targets with no C toolchain. A C binding (`openjpeg-sys`, `libaec-sys`)
re-introduces a C build per target and brings back the windows-arm64 problem.
So the rule is: **prefer pure-Rust; take a C dependency only when there is no
viable pure-Rust decoder and the packing is worth the cross-compile cost.**

**Fixtures are already in hand.** `tests/fixtures/jpeg2000_regular_latlon.grib2`
and `ccsds_regular_latlon.grib2`, each with an eccodes `.eccodes.ref.json`
snapshot and a `_expected.json` decode oracle, were committed with the PNG
corpus. Both templates can be cross-validated against eccodes the moment a
decoder lands. (Provenance in `crates/fieldglass-grib2/tests/fixtures/NOTICE.md`.)

## Decision

| Template | Codec | Crate | Cross-compile | Outcome |
| --- | --- | --- | --- | --- |
| **5.41 PNG** | pure-Rust | [`png`](https://crates.io/crates/png) | clean, no C | **Shipped** ([#118](https://github.com/D0ubleD0uble/fieldglass/issues/118)) |
| **5.42 CCSDS / AEC** | pure-Rust | [`rust-aec`](https://crates.io/crates/rust-aec) | clean, no C | **Shipped** ([#117](https://github.com/D0ubleD0uble/fieldglass/issues/117)) |
| **5.40 JPEG 2000** | none viable in pure Rust | — | windows-arm64 risk | **Defer** ([#116](https://github.com/D0ubleD0uble/fieldglass/issues/116)) |

### 5.41 PNG — done

The pure-Rust [`png`](https://crates.io/crates/png) crate decodes the PNG image
in §7; the simple-packing `R` / `E` / `D` transform then applies. Shipped in
#118.

### 5.42 CCSDS / AEC — pure-Rust (`rust-aec`)

The original spike picked [`oxiarc-szip`](https://crates.io/crates/oxiarc-szip)
on the strength of its "libaec-compatible" claim. Implementing #117 disproved
it: `oxiarc-szip` v0.3.3 decodes its own encoder's round-trips but **does not
decode a real libaec/eccodes stream** — against the committed
`ccsds_regular_latlon.grib2` oracle it disagrees from the very first reference
sample. Its test suite is all self round-trips, which never exercise libaec
compatibility. (This is exactly why the spike's "validate against the committed
oracle" gate existed.)

[`rust-aec`](https://crates.io/crates/rust-aec) (MIT) is a pure-Rust
CCSDS-121.0-B-3 AEC decoder **purpose-built for GRIB2 template 5.42**, created
to avoid native-libaec build friction. It decodes the committed fixture
**byte-for-byte against the eccodes oracle** (count, min/max/mean, and anchored
samples). Its only dependency is `bitflags` (already in the lock), and it
cross-compiles to all six targets with no C, preserving the C-free `.vsix`.

It is young (v0.1.1, single maintainer), so #117 ships it behind three
guardrails: the version is pinned exactly and `cargo deny check` stays in the
gate; the AEC payload decode is kept self-contained so it stays swappable (or
vendorable — ~1,700 LOC, MIT); and any decoder error is surfaced as
`UnsupportedSection` so an untrusted file degrades gracefully rather than
crashing the addon. The committed eccodes oracle test is the standing
correctness backstop.

`libaec-sys` (BSD-2, C) remains the documented fallback **only** if `rust-aec`
later proves insufficient on other models (e.g. 24-bit/3-byte, signed, or
non-preprocessed streams) — that would reintroduce the windows-arm64
cross-compile cost, so it is a last resort, not the default.

### 5.40 JPEG 2000 — deferred

There is no production-ready pure-Rust JPEG 2000 decoder. The `jpeg2000` crate
is stale 2019 OpenJPEG *bindings* (C++), not pure Rust, and pure-Rust codestream
(ISO 15444-1 Annex A) support never matured. The only realistic path today is an
OpenJPEG C binding (`openjpeg-sys` / `jpeg2k`), which the reference `grib` crate
uses by default — and that is precisely the C-on-six-targets situation we want
to avoid, windows-arm64 included.

JPEG 2000 is common (HRRR, MRMS, some NAM / GEFS), so this is a real gap, not a
dismissal. But it is the one template that would break the pure-Rust,
no-native-dep property of the bundled `.vsix`. **Defer #116** until either a
pure-Rust J2K decoder becomes viable, or we decide the packing is worth a
dedicated cross-compile effort. The committed fixture + oracle are ready for
whenever that happens.

## Consequences

- #111 is resolved; the per-template implementation issues (#116 / #117 / #118)
  already exist, so no new issues are needed.
- #117 proceeds on a pure-Rust basis; #116 is parked with a clear reason rather
  than left ambiguously "blocked on research".
- The decode-decoupled design holds: both 5.42 (when it lands) and 5.40 (if it
  ever does) feed the same `Vec<Option<f64>>` + grid geometry, so neither needs
  any projection, overlay, or render change.

## References

- Spike: [#111](https://github.com/D0ubleD0uble/fieldglass/issues/111). Same
  shape as the projection-library question in
  [#45](https://github.com/D0ubleD0uble/fieldglass/issues/45) (resolved by
  hand-rolling rather than taking a C/PROJ dependency — the same instinct
  applied here).
- `rust-aec` (adopted for 5.42): <https://crates.io/crates/rust-aec>
- `oxiarc-szip` (rejected — not libaec-compatible, see 5.42 section):
  <https://crates.io/crates/oxiarc-szip>
- Reference Rust GRIB crate (uses `openjpeg-sys` + `libaec-sys`): <https://docs.rs/grib/>
