//! The trust boundary for the proofs over shipped code: what the verified
//! kernels may assume about the code they call but that is not itself verified.
//!
//! Everything here is **trusted**, not proved. It is kept to three statements,
//! each small enough to check by reading, and `docs/verification.md` lists them:
//!
//! - `BitReader::new` and `BitReader::read_bits` (`fieldglass-core/src/bits.rs`)
//!   behave as `msb_bits` says, stated with `assume_specification`. A Tier-0
//!   proof of `read_bits` would replace the assumption with a theorem.
//! - `f64::powi` computes *some* fixed function of its base and exponent,
//!   `powi_spec`. Nothing more is assumed about it, so what the proofs establish
//!   is which base and which exponent sign each factor is computed with.
//! - `f64` `+` and `*` never panic and are deterministic, and `u32 as f64` is
//!   the exact conversion (every `u32` is representable in an `f64`).
use crate::bits::BitReader;
use crate::FieldglassError;
use vstd::arithmetic::power2::pow2;
use vstd::prelude::*;

verus! {

#[verifier::external_type_specification]
#[verifier::external_body]
pub struct ExBitReader<'a>(BitReader<'a>);

#[verifier::external_type_specification]
#[verifier::external_body]
pub struct ExFieldglassError(FieldglassError);

/// The bytes a reader reads from.
pub uninterp spec fn reader_bytes<'a>(r: BitReader<'a>) -> Seq<u8>;

/// The reader's cursor, in bits from the first bit of `reader_bytes`.
pub uninterp spec fn reader_pos<'a>(r: BitReader<'a>) -> int;

/// Bit `i` of `bytes`, counting from the most significant bit of byte 0.
pub open spec fn bit(bytes: Seq<u8>, i: int) -> nat {
    ((bytes[i / 8] as nat) / pow2((7 - i % 8) as nat)) % 2
}

/// The `width` bits of `bytes` starting at bit `start`, read MSB-first as an
/// unsigned integer.
pub open spec fn msb_bits(bytes: Seq<u8>, start: int, width: int) -> nat
    decreases width,
{
    if width <= 0 {
        0
    } else {
        2 * msb_bits(bytes, start, width - 1) + bit(bytes, start + width - 1)
    }
}

/// When `read_bits(n)` succeeds from bit `pos` of a `len`-byte buffer: `n` fits
/// a `u32`, and either nothing is read or the read ends inside the buffer
/// (whose bit length the reader computes in a `usize`).
pub open spec fn read_fits(len: int, pos: int, n: int) -> bool {
    n <= 32 && (n == 0 || (pos + n <= len * 8 && len * 8 <= usize::MAX))
}

pub assume_specification<'a>[ BitReader::<'a>::new ](bytes: &'a [u8]) -> (r: BitReader<'a>)
    ensures
        reader_bytes(r) == bytes@,
        reader_pos(r) == 0,
;

pub assume_specification<'a>[ BitReader::<'a>::read_bits ](r: &mut BitReader<'a>, n: u8) -> (out:
    Result<u32, FieldglassError>)
    ensures
        reader_bytes(*final(r)) == reader_bytes(*old(r)),
        out is Ok <==> read_fits(reader_bytes(*old(r)).len() as int, reader_pos(*old(r)), n as int),
        out matches Ok(v) ==> v as nat == msb_bits(
            reader_bytes(*old(r)),
            reader_pos(*old(r)),
            n as int,
        ),
        out is Ok ==> reader_pos(*final(r)) == reader_pos(*old(r)) + n,
        out is Err ==> reader_pos(*final(r)) == reader_pos(*old(r)),
;

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
