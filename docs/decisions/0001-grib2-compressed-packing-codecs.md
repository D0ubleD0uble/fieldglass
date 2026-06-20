# 0001 — GRIB2 compressed-packing codecs

**Status:** Accepted (2026-06-20). Resolves the [#111](https://github.com/D0ubleD0uble/fieldglass/issues/111) spike.

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
`png` (MIT/Apache), `oxiarc-szip` (Apache-2.0), OpenJPEG (BSD-2-Clause), and
libaec (BSD-2-Clause). Licensing rules nothing out.

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
| **5.42 CCSDS / AEC** | pure-Rust | [`oxiarc-szip`](https://crates.io/crates/oxiarc-szip) | clean, no C | **Pursue** ([#117](https://github.com/D0ubleD0uble/fieldglass/issues/117)) |
| **5.40 JPEG 2000** | none viable in pure Rust | — | windows-arm64 risk | **Defer** ([#116](https://github.com/D0ubleD0uble/fieldglass/issues/116)) |

### 5.41 PNG — done

The pure-Rust [`png`](https://crates.io/crates/png) crate decodes the PNG image
in §7; the simple-packing `R` / `E` / `D` transform then applies. Shipped in
#118.

### 5.42 CCSDS / AEC — pure-Rust

[`oxiarc-szip`](https://crates.io/crates/oxiarc-szip) (Apache-2.0, published
2026-06-06) is a pure-Rust, "libaec-compatible" AEC / SZIP implementation —
which is exactly the codec GRIB2 5.42 uses. It did not exist when the reference
Rust [`grib`](https://docs.rs/grib/) crate chose `libaec-sys`, so that crate's
C dependency is not the precedent to follow here.

Plan for #117: wire `oxiarc-szip` and validate against the committed
`ccsds_regular_latlon.grib2` eccodes oracle. **Fall back to `libaec-sys`
(BSD-2, C) only if it fails to decode the fixtures** — that fallback would
reintroduce the windows-arm64 cross-compile cost, so it is a last resort, not
the default.

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
- `oxiarc-szip`: <https://crates.io/crates/oxiarc-szip>
- Reference Rust GRIB crate (uses `openjpeg-sys` + `libaec-sys`): <https://docs.rs/grib/>
