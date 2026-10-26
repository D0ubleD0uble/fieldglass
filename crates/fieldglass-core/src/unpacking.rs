//! The GRIB `R` / `E` / `D` scaling: a packed integer `X` unpacks to the value
//! `(R + X·2^E)·10^-D`, where `R` is the reference value, `E` the binary scale
//! factor and `D` the decimal scale factor.
//!
//! Every GRIB decoder that packs values as integers applies it: GRIB1 simple,
//! second-order, matrix-of-values and spherical-harmonic packing, and GRIB2
//! simple, complex, second-order, PNG, CCSDS, JPEG 2000, run-length, matrix,
//! spherical-harmonic and bi-Fourier packing. It lives here once so that a
//! direction error (`2^-E` for `2^E`, `10^D` for `10^-D`), which decodes
//! plausible wrong numbers whenever a test happens to use `D = 0`, cannot hide
//! in one copy of it.
//!
//! # Verified kernel
//!
//! This file is compiled twice. This crate compiles it as ordinary Rust; the
//! verification crate `crates/fieldglass-verify` includes the same file with
//! `#[path]` and proves it with Verus. The proofs are the
//! `cfg_attr(verus_keep_ghost, ...)` attributes and the `verus_keep_ghost`
//! items at the bottom, which a normal build never sees, so the shipped code
//! carries no Verus dependency and no runtime cost. What is proved, and what is
//! trusted rather than proved, is listed in `docs/verification.md`.
//!
//! Two rules follow from being compiled twice. The file names only items both
//! crates provide: `crate::FieldglassError`, `crate::bits::BitReader`, and,
//! under Verus only, `crate::bits_model`, the bit-reader model and the trusted
//! `f64` statements. And its
//! docs use plain backticks rather than intra-doc links, which would resolve in
//! only one of the two. `tools/check_verified_kernels.py` fails if the
//! verification crate stops including a file that carries a proof.

use crate::FieldglassError;
use crate::bits::BitReader;
#[cfg(verus_keep_ghost)]
use crate::bits_model::{msb_bits, powi_spec, reader_bytes, reader_pos};
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;
#[cfg(verus_keep_ghost)]
use vstd::std_specs::ops::{AddSpec, MulSpec};

/// The three factors of the `R` / `E` / `D` transform, computed once per
/// message by `red_scale`: the reference value `R`, `2^E`, and `10^-D`.
///
/// `apply` is the per-value transform and `constant` the value of every point
/// in a constant field.
// `verus_verify` makes the type visible to the proofs; Verus ignores a type
// outside `verus!` that does not carry it.
#[cfg_attr(verus_keep_ghost, verus_verify)]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Scaling {
    /// The reference value `R`.
    pub reference: f64,
    /// `2^E`, from the binary scale factor `E`.
    pub binary_factor: f64,
    /// `10^-D`, from the decimal scale factor `D`.
    pub decimal_factor: f64,
}

/// The factors for reference value `R`, binary scale factor `E` and decimal
/// scale factor `D`.
///
/// A free function rather than `Scaling::new`: the pinned Verus cannot attach
/// a proof to an associated function without a receiver when the proof sits
/// behind `cfg_attr`, as it has to here.
#[cfg_attr(verus_keep_ghost, verus_spec(s =>
    ensures
        s.reference == reference,
        s.binary_factor == powi_spec(2.0f64, binary_scale_factor as int),
        s.decimal_factor == powi_spec(10.0f64, -(decimal_scale_factor as int)),
))]
#[inline]
pub fn red_scale(reference: f64, binary_scale_factor: i16, decimal_scale_factor: i16) -> Scaling {
    Scaling {
        reference,
        binary_factor: binary_factor(binary_scale_factor),
        decimal_factor: decimal_factor(decimal_scale_factor),
    }
}

/// `2^E` for binary scale factor `E`.
///
/// Computed with `f64::powi`, as every decoder did before the factor was
/// shared here, so decoded values are unchanged to the bit. Proved to be
/// `powi(2, E)`: base 2, exponent `+E`.
#[cfg_attr(verus_keep_ghost, verus_spec(f =>
    ensures
        f == powi_spec(2.0f64, binary_scale_factor as int),
))]
#[inline]
pub fn binary_factor(binary_scale_factor: i16) -> f64 {
    2f64.powi(binary_scale_factor as i32)
}

/// `10^-D` for decimal scale factor `D`.
///
/// Computed with `f64::powi`, as every decoder did before the factor was
/// shared here, so decoded values are unchanged to the bit. Proved to be
/// `powi(10, -D)`: base 10, exponent `-D`.
#[cfg_attr(verus_keep_ghost, verus_spec(f =>
    ensures
        f == powi_spec(10.0f64, -(decimal_scale_factor as int)),
))]
#[inline]
pub fn decimal_factor(decimal_scale_factor: i16) -> f64 {
    10f64.powi(-(decimal_scale_factor as i32))
}

impl Scaling {
    /// The physical value of the packed integer `x`: `(R + x·2^E)·10^-D`.
    ///
    /// `x` is taken as an `f64` so one transform serves the unsigned bit-field
    /// values of simple packing and the signed reconstructed integers of
    /// complex and second-order packing alike; the caller's `as f64` is the
    /// conversion every decoder already made.
    #[cfg_attr(verus_keep_ghost, verus_spec(out =>
        ensures
            out == scaled(self, x),
    ))]
    #[inline]
    pub fn apply(&self, x: f64) -> f64 {
        scale(self.reference, x, self.binary_factor, self.decimal_factor)
    }

    /// The value of every present point in a constant field: `R·10^-D`.
    ///
    /// Not `apply(0.0)`, which adds `0·2^E`: that is `NaN` when `2^E`
    /// overflows to infinity, and it turns a reference of `-0.0` into `+0.0`.
    #[cfg_attr(verus_keep_ghost, verus_spec(out =>
        ensures
            out == self.reference.mul_spec(self.decimal_factor),
    ))]
    #[inline]
    pub fn constant(&self) -> f64 {
        scale_constant(self.reference, self.decimal_factor)
    }
}

// The arithmetic takes its operands as parameters rather than reading the
// fields of `Scaling` itself: under the pinned Verus, the trusted `f64` axiom
// the proofs rest on fires on a parameter but not on a value read out of a
// struct field.

/// `(reference + x·binary_factor)·decimal_factor`, the body of `apply`.
#[cfg_attr(verus_keep_ghost, verus_spec(out =>
    ensures
        out == reference.add_spec(x.mul_spec(binary_factor)).mul_spec(decimal_factor),
))]
#[inline]
fn scale(reference: f64, x: f64, binary_factor: f64, decimal_factor: f64) -> f64 {
    (reference + x * binary_factor) * decimal_factor
}

/// `reference·decimal_factor`, the body of `constant`.
#[cfg_attr(verus_keep_ghost, verus_spec(out =>
    ensures
        out == reference.mul_spec(decimal_factor),
))]
#[inline]
fn scale_constant(reference: f64, decimal_factor: f64) -> f64 {
    reference * decimal_factor
}

/// Read `count` simple-packed integers of `bits_per_value` bits (MSB-first,
/// back to back from the first bit of `packed`) and `apply` `scaling` to each.
///
/// Fails, rather than returning fewer values, when `packed` is too short to
/// hold `count` of them, and when `bits_per_value` is over 32, the widest
/// field the bit reader returns. A width of 0 reads no bits, so every value is
/// `apply(0.0)`; a decoder that treats that as a constant field uses
/// `Scaling::constant` instead and does not call this.
///
/// Proved (see the module docs): the result is `Ok` exactly when those two
/// conditions allow it, an `Ok` result has `count` values, and value `i` is
/// `scaling.apply` of the `i`-th `bits_per_value`-bit field of `packed`.
#[cfg_attr(verus_keep_ghost, verus_spec(r =>
    ensures
        r is Ok <==> holds_fields(packed@.len() as int, count as int, bits_per_value as int),
        r matches Ok(v) ==> v@.len() == count,
        r matches Ok(v) ==> forall|i: int|
            0 <= i < count ==> #[trigger] v@[i] == scaled(
                scaling,
                field(packed@, i, bits_per_value as int) as f64,
            ),
))]
pub fn unpack_simple(
    packed: &[u8],
    bits_per_value: u8,
    scaling: &Scaling,
    count: usize,
) -> Result<Vec<f64>, FieldglassError> {
    let mut decoded = Vec::with_capacity(count);
    unpack_simple_into(packed, bits_per_value, scaling, count, &mut decoded)?;
    Ok(decoded)
}

/// `unpack_simple`, appending to `out` rather than allocating: for a decoder
/// whose output holds other values too, such as a spectral field whose first
/// coefficient is stored apart from the packed ones.
///
/// Proved (see the module docs), as for `unpack_simple`: `Ok` exactly when the
/// fields fit, and then `out` keeps what it held and gains `count` values, the
/// `i`-th being `scaling.apply` of the `i`-th field. On an error `out` may hold
/// some of them; every caller discards it.
#[cfg_attr(verus_keep_ghost, verus_spec(r =>
    ensures
        r is Ok <==> holds_fields(packed@.len() as int, count as int, bits_per_value as int),
        r is Ok ==> final(out)@.len() == old(out)@.len() + count,
        r is Ok ==> forall|i: int|
            0 <= i < old(out)@.len() ==> #[trigger] final(out)@[i] == old(out)@[i],
        r is Ok ==> forall|i: int|
            #![trigger field(packed@, i, bits_per_value as int)]
            0 <= i < count ==> final(out)@[old(out)@.len() + i] == scaled(
                scaling,
                field(packed@, i, bits_per_value as int) as f64,
            ),
))]
pub fn unpack_simple_into(
    packed: &[u8],
    bits_per_value: u8,
    scaling: &Scaling,
    count: usize,
    out: &mut Vec<f64>,
) -> Result<(), FieldglassError> {
    let mut reader = BitReader::new(packed);
    #[cfg_attr(verus_keep_ghost, verus_spec(it =>
        invariant
            reader_bytes(reader) == packed@,
            reader_pos(reader) == it.index@ * bits_per_value,
            out@.len() == old(out)@.len() + it.index@,
            holds_fields(packed@.len() as int, it.index@ as int, bits_per_value as int),
            forall|i: int| 0 <= i < old(out)@.len() ==> #[trigger] out@[i] == old(out)@[i],
            forall|i: int|
                #![trigger field(packed@, i, bits_per_value as int)]
                0 <= i < it.index@ ==> out@[old(out)@.len() + i] == scaled(
                    scaling,
                    field(packed@, i, bits_per_value as int) as f64,
                ),
    ))]
    for _ in 0..count {
        #[cfg(verus_keep_ghost)]
        proof! { lemma_next_field(out@.len() - old(out)@.len(), count as int, bits_per_value as int); }
        let x = reader.read_bits(bits_per_value)?;
        out.push(scaling.apply(x as f64));
    }
    Ok(())
}

#[cfg(verus_keep_ghost)]
verus! {

broadcast use crate::bits_model::axioms::f64_ops_are_total;

/// `(R + x·2^E)·10^-D` over the factors in `s`, in the order `apply` evaluates
/// it: `x·2^E`, then `R +`, then `·10^-D`.
pub open spec fn scaled(s: &Scaling, x: f64) -> f64 {
    s.reference.add_spec(x.mul_spec(s.binary_factor)).mul_spec(s.decimal_factor)
}

/// Whether reading `count` fields of `width` bits from a `len`-byte buffer
/// succeeds: nothing is read, or each field fits the reader's `u32` and the
/// buffer holds them all. The last clause is the reader computing the buffer's
/// bit length in a `usize`, which no buffer that fits in memory overflows.
pub open spec fn holds_fields(len: int, count: int, width: int) -> bool {
    count == 0 || width == 0 || (width <= 32 && count * width <= len * 8 && len * 8
        <= usize::MAX)
}

/// Field `i` of a run of `width`-bit fields packed MSB-first from bit 0.
pub open spec fn field(bytes: Seq<u8>, i: int, width: int) -> u32 {
    msb_bits(bytes, i * width, width) as u32
}

/// Field `k` of `count` ends where field `k + 1` starts, and inside the first
/// `count` fields. Nonlinear, so the solver needs it stated.
pub proof fn lemma_next_field(k: int, count: int, width: int)
    requires
        0 <= k < count,
        0 <= width,
    ensures
        (k + 1) * width == k * width + width,
        (k + 1) * width <= count * width,
{
    assert((k + 1) * width == k * width + width) by (nonlinear_arith);
    assert((k + 1) * width <= count * width) by (nonlinear_arith)
        requires
            k + 1 <= count,
            0 <= width,
    ;
}

} // verus!

// Runtime checks of the same claims. The proof covers the structure; these pin
// the numbers `f64::powi` actually produces, which the proof leaves opaque.
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn factors_have_the_right_base_and_sign() {
        assert_eq!(binary_factor(3), 8.0);
        assert_eq!(binary_factor(-1), 0.5);
        assert_eq!(decimal_factor(2), 0.01);
        assert_eq!(decimal_factor(-2), 100.0);
        let s = red_scale(1.5, 3, 2);
        assert_eq!(
            (s.reference, s.binary_factor, s.decimal_factor),
            (1.5, 8.0, 0.01)
        );
    }

    #[test]
    fn apply_is_reference_plus_scaled_integer_times_ten_to_minus_d() {
        // (R + X·2^E)·10^-D with R = 1, E = 1, D = -1: (1 + 3·2)·10 = 70.
        assert_eq!(red_scale(1.0, 1, -1).apply(3.0), 70.0);
    }

    #[test]
    fn constant_is_not_apply_of_zero() {
        // 2^1024 overflows to infinity, so `apply(0.0)` adds 0·∞ = NaN.
        let s = red_scale(2.0, 1024, 0);
        assert!(s.apply(0.0).is_nan());
        assert_eq!(s.constant(), 2.0);
        // A negative-zero reference keeps its sign.
        let s = red_scale(-0.0, 0, 0);
        assert!(s.constant().is_sign_negative());
        assert!(s.apply(0.0).is_sign_positive());
    }

    #[test]
    fn unpack_reads_msb_first_fields_in_order() {
        let s = red_scale(0.0, 0, 0);
        let v = unpack_simple(&[0xAB, 0xCD], 4, &s, 4).unwrap();
        assert_eq!(v, [10.0, 11.0, 12.0, 13.0]);
        let v = unpack_simple(&[0xAB, 0xCD], 12, &s, 1).unwrap();
        assert_eq!(v, [f64::from(0xABCu16)]);
        let v = unpack_simple(&[0xAB, 0xCD], 4, &red_scale(1.0, 1, 1), 1).unwrap();
        assert_eq!(v, [(1.0 + 10.0 * 2.0) * 0.1]);
    }

    #[test]
    fn unpack_fails_when_the_fields_do_not_fit() {
        let s = red_scale(0.0, 0, 0);
        assert!(unpack_simple(&[0xAB, 0xCD], 4, &s, 5).is_err());
        assert!(unpack_simple(&[0; 8], 33, &s, 1).is_err());
        assert_eq!(unpack_simple(&[], 8, &s, 0).unwrap(), Vec::<f64>::new());
    }

    #[test]
    fn a_zero_width_reads_nothing() {
        let s = red_scale(4.0, 0, 1);
        assert_eq!(unpack_simple(&[], 0, &s, 3).unwrap(), [s.apply(0.0); 3]);
    }
}
