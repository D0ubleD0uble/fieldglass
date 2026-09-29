//! The trust boundary for the proofs over shipped code: what the verified
//! kernels may assume about the code they call but that is not itself verified.
//!
//! Everything asserted here is **trusted**, not proved. It is kept to two
//! statements, each small enough to check by reading, and
//! `docs/verification.md` lists them:
//!
//! - `f64::powi` computes *some* fixed function of its base and exponent,
//!   `powi_spec`. Nothing more is assumed about it, so what the proofs establish
//!   is which base and which exponent sign each factor is computed with.
//! - `f64` `+` and `*` never panic and are deterministic, and `u32 as f64` is
//!   the exact conversion (every `u32` is representable in an `f64`).
//!
//! The bit reader was trusted here too until #771 proved it. Its model
//! (`msb_bits` and the rest) now lives beside the proof, in the reader's kernel
//! file, and is re-exported below so the other kernels keep naming it from
//! here. Those are definitions, not assumptions.
use crate::FieldglassError;
use vstd::prelude::*;

pub use crate::bit_reader::{align8, bit, msb_bits, read_fits, reader_bytes, reader_pos};

verus! {

#[verifier::external_type_specification]
#[verifier::external_body]
pub struct ExFieldglassError(FieldglassError);

/// Whatever `f64::powi(base, exp)` computes. Deliberately uninterpreted.
pub uninterp spec fn powi_spec(base: f64, exp: int) -> f64;

pub assume_specification[ f64::powi ](x: f64, n: i32) -> (r: f64)
    ensures
        r == powi_spec(x, n as int),
;

} // verus!
/// The `f64` axioms. Each kernel file enables them for itself with
/// `broadcast use crate::bits_model::axioms::f64_ops_are_total;`, so a proof
/// that leans on them says so where it is written. A file of their own keeps
/// the most sweeping part of the trusted base short enough to audit at a
/// glance.
pub mod axioms;
