//! Shared test support: the FNV fold the goldens hash with, and the libm
//! fingerprint that says whether a bit-exact recording may be asserted here.
//!
//! Two goldens need this, in two different crates:
//!
//! * `fieldglass-core`'s `planar_inverse_golden.rs`, which records the exact
//!   `f64` bits of every planar inverse;
//! * `fieldglass-napi`'s `characterisation` module, which records the RGBA
//!   bytes of every render, and the probe, contour and CSV output beside them.
//!
//! Both stand on the same rule, so they stand on the same constant.
//! `docs/decisions/0009-cross-target-floating-point-agreement.md`
//! is the record: cross-target agreement is a tolerance, bit-identical results
//! are a property of one libm, and a bit-exact assertion is gated on
//! recognising that libm rather than on a target triple. One copy of
//! `REFERENCE_LIBM` means re-recording it after a glibc change is one edit, and
//! means `the_reference_toolchain_is_the_recorded_libm` — which asserts the
//! fingerprint outright on x86_64 glibc, so the gate cannot be edited into
//! never matching — keeps both goldens honest instead of one.
//!
//! `fieldglass-napi` reaches this file by `#[path]` rather than by a
//! dependency: it is a cdylib whose tests are unit tests, so there is no
//! integration-test directory of its own to put it in, and a test-only helper
//! is not worth widening `fieldglass-core`'s published surface for.

// Each consumer compiles its own copy and uses a subset: the planar golden
// never asks for the fingerprint's hex form, the characterisation golden never
// folds a `GridIndex`. Denied warnings would make that a build failure in
// whichever crate uses less.
#![allow(dead_code)]

/// FNV-1a over `bytes`, folded into `h`.
///
/// Not a cryptographic hash and not meant to be one: it is a stable, dependency
/// free way to reduce a megabyte of RGBA — or 300,000 `f64` bits — to one
/// number a golden can carry. What matters is that it is deterministic and that
/// every byte reaches it.
pub(crate) fn fnv(h: &mut u64, bytes: &[u8]) {
    for &b in bytes {
        *h ^= b as u64;
        *h = h.wrapping_mul(0x100000001b3);
    }
}

/// The FNV-1a offset basis — where a fold starts.
pub(crate) const FNV_OFFSET: u64 = 0xcbf29ce484222325;

/// A fold over the exact libm surface the projections stand on, at fixed
/// inputs, so a target can say *which* libm it has rather than being guessed at
/// from its triple.
///
/// `target_arch = "wasm32"` would be the obvious gate and it is the wrong one
/// twice over: it assumes every other target agrees with the recording (musl
/// and macOS need not, and neither need a future glibc), and it says nothing
/// about which library is actually underneath. This asks.
///
/// Eleven of these fourteen already disagree between glibc 2.39 and the `libm`
/// Rust links on `wasm32`, so the fold separates them with room to spare; `ln`,
/// `exp` and `sqrt` agree today and are folded in anyway, for the next libm.
pub(crate) fn libm_fingerprint() -> u64 {
    let mut h: u64 = FNV_OFFSET;
    let fs: [fn(f64) -> f64; 14] = [
        |t| (t * 7.3 + 1e-3).ln(),
        |t| (t * 3.1 - 1.5).exp(),
        |t| (t * 2.0 + 0.5).powf(1.7 + t),
        |t| (t * 1.4).tan(),
        |t| (t * 9.0 - 4.5).atan(),
        |t| (t - 0.5).atan2(t * 2.0 - 1.3),
        |t| (t * 6.0).sin(),
        |t| (t * 6.0).cos(),
        |t| (t * 0.001 - 0.5).asin(),
        |t| (t * 2.0 - 1.0).sinh(),
        |t| (t * 2.0 - 1.0).cosh(),
        |t| (t - 0.4).hypot(t * 3.0 + 0.1),
        |t| (t * 5.0).sqrt(),
        |t| (t * 2.0 - 1.0).tanh(),
    ];
    for f in fs {
        for k in 0..2000i64 {
            fnv(&mut h, &f(k as f64 * 0.001 + 1e-9).to_bits().to_le_bytes());
        }
    }
    h
}

/// The fingerprint of the libm the bit-exact columns were recorded against:
/// x86_64 Linux glibc 2.39, which is what CI's `ubuntu-latest` runners and the
/// pre-commit hooks run.
pub(crate) const REFERENCE_LIBM: u64 = 0x0951d9bc6bf359e4;

/// Whether this target's libm is the one the bit-exact columns were recorded
/// against.
pub(crate) fn is_reference_libm() -> bool {
    libm_fingerprint() == REFERENCE_LIBM
}
