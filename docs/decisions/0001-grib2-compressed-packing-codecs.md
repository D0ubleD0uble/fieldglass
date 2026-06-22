# 0001 — GRIB2 compressed-packing codecs

**Status:** Accepted (2026-06-20). Resolves the [#111](https://github.com/D0ubleD0uble/fieldglass/issues/111) spike.

**Amended (2026-06-19):** The 5.42 decoder is **`rust-aec`**, not `oxiarc-szip`. While
implementing [#117](https://github.com/D0ubleD0uble/fieldglass/issues/117), `oxiarc-szip`
v0.3.3 failed the committed eccodes oracle — it decodes its own round-trips but is not
byte-compatible with real libaec streams. `rust-aec` (pure-Rust, purpose-built for GRIB2
5.42) decodes the fixture byte-for-byte against the oracle. See the 5.42 section below.

**Amended (2026-06-21):** 5.40 is no longer deferred. The pure-Rust **`rust-j2k`** JPEG
2000 decoder (purpose-built GRIB2-decode-first) reached the same bar — it decodes the
committed fixture byte-for-byte against the eccodes oracle with no C dependency — so
[#116](https://github.com/D0ubleD0uble/fieldglass/issues/116) shipped on the same pure-Rust
basis as 5.41 / 5.42. See the 5.40 section below.

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
| **5.40 JPEG 2000** | pure-Rust | [`rust-j2k`](https://crates.io/crates/rust-j2k) | clean, no C | **Shipped** ([#116](https://github.com/D0ubleD0uble/fieldglass/issues/116)) |

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

### 5.40 JPEG 2000 — pure-Rust (`rust-j2k`)

**Originally deferred (2025).** At the spike there was no production-ready
pure-Rust JPEG 2000 decoder. The `jpeg2000` crate was stale 2019 OpenJPEG
*bindings* (C++), not pure Rust, and pure-Rust codestream (ISO 15444-1 Annex A)
support had never matured. The only realistic path was an OpenJPEG C binding
(`openjpeg-sys` / `jpeg2k`), which the reference `grib` crate uses by default —
precisely the C-on-six-targets situation (windows-arm64 included) we want to
avoid. JPEG 2000 is common (HRRR, MRMS, some NAM / GEFS), so this was a real gap;
#116 was deferred until a pure-Rust J2K decoder became viable, with the committed
fixture + oracle held ready.

**Shipped (2026-06).** [`rust-j2k`](https://crates.io/crates/rust-j2k)
(MIT / Apache-2.0) is a pure-Rust JPEG 2000 decoder **purpose-built
GRIB2-decode-first**: it decodes a single-component integer codestream (Annex A,
no JP2 boxes) over both the reversible 5/3 and irreversible 9/7 wavelet paths —
exactly the slice `grid_jpeg` produces. Against the committed
`jpeg2000_regular_latlon.grib2` fixture it decodes **byte-for-byte to the eccodes
oracle** (count, min/max/mean, and anchored samples). It has no C dependency, so
it cross-compiles to all six targets and preserves the C-free `.vsix` this ADR
set out to protect.

Like `rust-aec`, it is young (v0.1.0, single maintainer), so #116 ships it behind
the same guardrails: the version is pinned exactly and `cargo deny check` stays
in the gate; the codec call is kept self-contained in `decode_jpeg2000_packing`
so it stays swappable; and any decoder error is surfaced as `UnsupportedSection`
so an untrusted file degrades gracefully rather than crashing the addon. The
committed eccodes oracle test is the standing correctness backstop.

`openjpeg-sys` (C) remains the documented fallback **only** if `rust-j2k` later
proves insufficient on other models — that would reintroduce the windows-arm64
cross-compile cost, so it is a last resort, not the default.

## Consequences

- #111 is resolved; the per-template implementation issues (#116 / #117 / #118)
  already exist, so no new issues are needed.
- #117 and #116 both proceed on a pure-Rust basis; neither needed the C-binding
  fallback that would have reintroduced the windows-arm64 cross-compile cost.
- The decode-decoupled design holds: 5.40 / 5.41 / 5.42 all feed the same
  `Vec<Option<f64>>` + grid geometry, so none needed any projection, overlay, or
  render change.

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
