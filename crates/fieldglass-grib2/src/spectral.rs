//! GRIB2 spherical-harmonic (spectral) coefficient decode.
//!
//! A spectral message (§3 template 3.50 + §5 template 5.50) carries the
//! spherical-harmonic coefficients of a field, not values on a grid, so it
//! decodes to coefficients rather than through the scalar
//! [`decode_values`](crate::ds::decode_values) path — mirroring the GRIB1
//! reader. Recovering a lat/lon field needs an inverse spherical-harmonic
//! transform, which is not implemented yet.
//!
//! Template 5.50 (`spectral_simple`): the real part of the `(0, 0)` coefficient
//! is stored out of band in §5 ([`SpectralSimplePackingTemplate::real_part_of_00`])
//! and copied through unscaled; every other coefficient part is simple-packed
//! in §7 with the usual `value = (R + X · 2^E) · 10^-D` transform. This matches
//! eccodes' `data_g2shsimple_packing` over `data_g2simple_packing`.

use crate::drs::SpectralSimplePackingTemplate;
use fieldglass_core::{FieldglassError, bits::BitReader};

/// The decoded spherical-harmonic coefficients of one message.
///
/// `coefficients` is the flat `(real, imaginary)` interleaving eccodes reports,
/// traversed by zonal wavenumber `m` (outer) and total wavenumber `n` (inner),
/// so its length is `(T + 1)·(T + 2)` for a triangular truncation `T` — 4160 at
/// T63, which is what eccodes reports as `numberOfValues`.
#[derive(Debug, Clone, PartialEq)]
pub struct SpectralCoefficients {
    /// Pentagonal resolution parameters from §3.50. Triangular truncation (the
    /// only form eccodes decodes) has `j == k == m`.
    pub j: u32,
    pub k: u32,
    pub m: u32,
    /// `(real, imaginary)` pairs — see the type docs for the traversal.
    pub coefficients: Vec<f64>,
}

impl SpectralCoefficients {
    /// Number of *complex* coefficients — half the stored value count.
    pub fn len(&self) -> usize {
        self.coefficients.len() / 2
    }

    /// Whether the message carries no coefficients at all.
    pub fn is_empty(&self) -> bool {
        self.coefficients.is_empty()
    }
}

/// Upper bound on the coefficient count `(J + 1)·(J + 2)` the decoder will
/// allocate for. `J` is a bare `u32` from §3.50, so a hostile message can
/// declare an enormous truncation; this caps the allocation the same way the
/// scalar reader caps grid-point counts (the largest operational spectral
/// truncation, ~T3999, is four orders of magnitude below this). 200 M `f64`
/// is ~1.6 GB — the same envelope a constant gridded field already accepts.
const MAX_SPECTRAL_VALUES: u64 = 200_000_000;

/// Stored value count (real *and* imaginary parts) of a triangular truncation
/// `t`: `(t + 1)·(t + 2)`, computed with overflow checking and bounded by
/// [`MAX_SPECTRAL_VALUES`] so a corrupt `J` cannot overflow the multiply or
/// size an allocation past the cap.
fn triangular_value_count(t: u32) -> Result<usize, FieldglassError> {
    let t = t as u64;
    (t + 1)
        .checked_mul(t + 2)
        .filter(|&n| n <= MAX_SPECTRAL_VALUES)
        .map(|n| n as usize)
        .ok_or_else(|| {
            FieldglassError::Parse(format!(
                "spectral truncation T={t} declares more than {MAX_SPECTRAL_VALUES} coefficients"
            ))
        })
}

/// Decode a `spectral_simple` (template 5.50) data section into coefficients.
///
/// `ds_payload` is §7 with its section header already stripped. `j`/`k`/`m`
/// come from §3.50; only a triangular truncation (`j == k == m`) is defined for
/// the coefficient count and traversal, so anything else is refused rather than
/// decoded wrong.
pub fn decode_spectral_simple(
    ds_payload: &[u8],
    t: &SpectralSimplePackingTemplate,
    j: u32,
    k: u32,
    m: u32,
) -> Result<SpectralCoefficients, FieldglassError> {
    if j != k || j != m {
        return Err(FieldglassError::UnsupportedSection(format!(
            "spectral truncation J={j} K={k} M={m} is not triangular \
             (only J = K = M is defined for spherical-harmonic packing)"
        )));
    }
    if t.bits_per_value > 32 {
        return Err(FieldglassError::Parse(format!(
            "spectral_simple: bits_per_value {} exceeds 32",
            t.bits_per_value
        )));
    }

    // Compute the coefficient count (checked + capped) BEFORE any allocation:
    // `J` is attacker-controlled, so the multiply or a huge `Vec` must not
    // panic or OOM. For a non-constant field, bound it more tightly still by
    // the bits §7 actually holds, so a corrupt truncation can neither over-read
    // nor over-allocate. The GRIB1 reader guards in this same order.
    let n_values = triangular_value_count(j)?;
    // values[0] is the out-of-band real part of (0, 0); the other
    // (n_values - 1) are packed in §7.
    let n_packed = n_values - 1;
    if t.bits_per_value != 0 {
        let available_bits = ds_payload.len().saturating_mul(8);
        let required_bits = n_packed.saturating_mul(t.bits_per_value as usize);
        if required_bits > available_bits {
            return Err(FieldglassError::Parse(format!(
                "spectral_simple truncation T={j} needs {required_bits} bits but §7 holds only {available_bits}"
            )));
        }
    }

    let r = t.reference_value as f64;
    let two_pow_e = 2f64.powi(t.binary_scale_factor as i32);
    let d_inv = 10f64.powi(-(t.decimal_scale_factor as i32));

    let mut out = Vec::with_capacity(n_values);
    out.push(t.real_part_of_00 as f64);

    // A zero bit-width means every packed coefficient equals the reference
    // value R · 10^-D (the simple-packing constant-field case). The cap in
    // `triangular_value_count` bounds this allocation, since an empty §7 gives
    // no bit budget to check against.
    if t.bits_per_value == 0 {
        out.resize(n_values, r * d_inv);
        return Ok(SpectralCoefficients {
            j,
            k,
            m,
            coefficients: out,
        });
    }

    let mut reader = BitReader::new(ds_payload);
    for _ in 0..n_packed {
        let x = reader.read_bits(t.bits_per_value)? as f64;
        out.push((r + x * two_pow_e) * d_inv);
    }

    Ok(SpectralCoefficients {
        j,
        k,
        m,
        coefficients: out,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn template(r: f32, e: i16, d: i16, bits: u8, real00: f32) -> SpectralSimplePackingTemplate {
        SpectralSimplePackingTemplate {
            reference_value: r,
            binary_scale_factor: e,
            decimal_scale_factor: d,
            bits_per_value: bits,
            real_part_of_00: real00,
        }
    }

    /// MSB-first bit-pack `values`, each `bits` wide.
    fn pack_bits(values: &[u32], bits: u8) -> Vec<u8> {
        let mut out = vec![0u8; (values.len() * bits as usize).div_ceil(8)];
        let mut bit = 0usize;
        for &v in values {
            for i in (0..bits).rev() {
                out[bit / 8] |= (((v >> i) & 1) as u8) << (7 - (bit % 8));
                bit += 1;
            }
        }
        out
    }

    #[test]
    fn decodes_triangular_truncation() {
        // J=1 → (1+1)(1+2) = 6 values: real00 out of band + 5 simple-packed.
        // R=0, E=0, D=0 → value = X. real00 copied unscaled.
        let t = template(0.0, 0, 0, 8, 42.0);
        let packed = pack_bits(&[1, 2, 3, 4, 5], 8);
        let c = decode_spectral_simple(&packed, &t, 1, 1, 1).expect("decode");
        assert_eq!((c.j, c.k, c.m), (1, 1, 1));
        assert_eq!(c.coefficients, vec![42.0, 1.0, 2.0, 3.0, 4.0, 5.0]);
        assert_eq!(c.len(), 3); // 3 complex coefficients
        assert!(!c.is_empty());
    }

    #[test]
    fn applies_reference_and_scale() {
        // R=10, E=1, D=1 → value = (10 + X·2)·10^-1.
        let t = template(10.0, 1, 1, 8, 5.0);
        // J=0 → (0+1)(0+2) = 2 values: real00 + 1 packed, so one coded value.
        let packed = pack_bits(&[5], 8);
        let c = decode_spectral_simple(&packed, &t, 0, 0, 0).expect("decode");
        assert_eq!(c.coefficients.len(), 2);
        assert!((c.coefficients[0] - 5.0).abs() < 1e-9); // real00 unscaled
        assert!((c.coefficients[1] - 2.0).abs() < 1e-9); // (10 + 5·2)·0.1 = 2.0
    }

    #[test]
    fn constant_field_when_bits_zero() {
        // bits==0 → every packed value equals R·10^-D; real00 still leads.
        let t = template(3.0, 0, 0, 0, 9.0);
        let c = decode_spectral_simple(&[], &t, 1, 1, 1).expect("decode");
        assert_eq!(c.coefficients, vec![9.0, 3.0, 3.0, 3.0, 3.0, 3.0]);
    }

    #[test]
    fn rejects_non_triangular_truncation() {
        let t = template(0.0, 0, 0, 8, 0.0);
        assert!(decode_spectral_simple(&[0u8; 8], &t, 63, 63, 62).is_err());
        assert!(decode_spectral_simple(&[0u8; 8], &t, 63, 62, 63).is_err());
    }

    #[test]
    fn rejects_bits_over_32() {
        let t = template(0.0, 0, 0, 33, 0.0);
        assert!(decode_spectral_simple(&[0u8; 64], &t, 1, 1, 1).is_err());
    }

    #[test]
    fn rejects_short_data_section() {
        // J=2 → (3)(4)=12 values, 11 packed × 8 bits = 88 bits = 11 bytes,
        // but only 4 bytes provided.
        let t = template(0.0, 0, 0, 8, 0.0);
        assert!(decode_spectral_simple(&[0u8; 4], &t, 2, 2, 2).is_err());
    }

    #[test]
    fn rejects_hostile_truncation_without_allocating() {
        // A huge J must be rejected before allocating — both the overflow-prone
        // multiply (J near u32::MAX) and a constant-field (bits == 0) blow-up
        // that carries no §7 bit budget to bound it.
        let big = template(0.0, 0, 0, 8, 0.0);
        assert!(decode_spectral_simple(&[0u8; 8], &big, u32::MAX, u32::MAX, u32::MAX).is_err());

        let constant = template(3.0, 0, 0, 0, 9.0); // bits == 0
        assert!(decode_spectral_simple(&[], &constant, 500_000, 500_000, 500_000).is_err());
    }
}
