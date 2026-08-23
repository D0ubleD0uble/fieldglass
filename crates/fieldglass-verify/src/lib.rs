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
//! A **bootstrap**: the toolchain wiring plus one trivial proof, so that later
//! issues (#199–#204) add proofs rather than infrastructure. It is not on the
//! path of any build that ships. See `docs/verification.md` for how to run it
//! and for the policy on what must stay verified.
//!
//! # The open question this bootstrap does *not* settle
//!
//! The functions proved here are currently written here. Proving the *shipped*
//! `fieldglass-core` functions instead means either giving that crate a `vstd`
//! dependency — transparent to a stock build and to the six-target
//! cross-compile, both measured, but a date-stamped pre-1.0 dependency on a
//! published crate — or moving the kernel itself into a crate like this one and
//! having core depend on it. That is a real design decision with a cost either
//! way, and #199 makes it with a real proof in hand rather than being
//! pre-empted here.
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
