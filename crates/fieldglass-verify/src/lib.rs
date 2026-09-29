//! Formally verified pieces of the decode kernel (issue #197, milestone
//! "Formal verification of the decode kernel").
//!
//! The kernel is the few hundred lines that turn untrusted bytes into numbers —
//! bit reading, scaling, spatial differencing, complex-packing group expansion —
//! where a malformed file can cause wrong values, an overflow, or a panic. Every
//! GRIB value in the project passes through it.
//!
//! # What this crate is
//!
//! The proofs, and the only crate that depends on `vstd`. It is not on the path
//! of any build that ships. See `docs/verification.md` for how to run it and
//! for the policy on what must stay verified.
//!
//! Two kinds of proof live here:
//!
//! - functions written here, such as the `bits_to_bytes` smoke test below;
//! - **kernel files** of shipped crates, included below with `#[path]`. Their
//!   specs sit in `cfg_attr(verus_keep_ghost, ...)` attributes, so the crate
//!   that ships the file compiles it as plain Rust, and this crate proves the
//!   same text. `bits_model` holds what those proofs trust rather than prove.
//!
//! `proc_macro_hygiene` is the nightly feature a kernel's loop invariant needs
//! here: Verus rewrites the `verus_spec` attribute on a `for` loop into one on
//! an expression (E0658 without it). Verus's toolchain allows the feature, and
//! the `cfg_attr` keeps it out of a stock build. The shipped crate never needs
//! it, because there the attribute is never expanded.
#![cfg_attr(verus_keep_ghost, feature(proc_macro_hygiene))]
use vstd::prelude::*;

verus! {

/// Bytes needed to hold `bits` bits — the rounding every bit reader in the
/// decode kernel does before it slices a buffer.
///
/// The smoke test for the whole setup, chosen because it is the smallest
/// function whose obvious implementation is also the one that overflows: `bits
/// + 7` wraps for the last seven values of `usize`, and the precondition is
/// what rules that out. Verus discharges both the arithmetic identity and the
/// bound that the result really does cover every bit.
pub fn bits_to_bytes(bits: usize) -> (out: usize)
    requires
        bits <= usize::MAX - 7,
    ensures
        out == (bits + 7) / 8,
        out * 8 >= bits,
{
    (bits + 7) / 8
}

} // verus!
// The proofs over shipped code (#199: `scaling`; #201: `groups`). Each kernel file is the production
// source itself, included by path, so there is one copy to keep verified.
// Those files name `crate::FieldglassError`, which this re-export provides
// here exactly as `fieldglass-core` does, `crate::bits::BitReader`, and
// `crate::bits_model`, the specifications and the trusted statements they rest
// on.
pub use fieldglass_core::FieldglassError;

// The bit reader every packed-integer kernel reads through (#771). Core ships
// this file as its private `bits::reader` and re-exports `BitReader` from
// `bits`; `bits` below does the same here, so the other kernels' paths resolve
// to the proved reader rather than to core's copy.
#[path = "../../fieldglass-core/src/bits/reader.rs"]
pub mod bit_reader;

/// The path the kernels name the reader by, as in `fieldglass-core`.
pub mod bits {
    pub use crate::bit_reader::BitReader;
}

pub mod bits_model;

#[path = "../../fieldglass-core/src/scaling.rs"]
pub mod scaling;

#[path = "../../fieldglass-core/src/spatial_diff.rs"]
pub mod spatial_diff;

// The byte shuffle of HDF5 and blosc / Zarr (#203). It names nothing from
// either crate, so it needs no re-export.
#[path = "../../fieldglass-core/src/shuffle.rs"]
pub mod shuffle;

// The group expansion of GRIB complex and second-order packing (#201).
#[path = "../../fieldglass-core/src/groups.rs"]
pub mod groups;
