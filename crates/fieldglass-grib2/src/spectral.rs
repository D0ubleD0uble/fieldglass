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
//!
//! Template 5.51 (`spectral_complex`): coefficients up to a sub-truncation
//! `(JS, KS, MS)` are stored as raw IEEE floats, and the rest are simple-packed
//! after division by a Laplacian operator `(n·(n+1))^P` — a faithful port of
//! eccodes' `DataComplexPacking::unpack_real` (see [`decode_spectral_complex`]).

use crate::drs::{SpectralComplexPackingTemplate, SpectralSimplePackingTemplate};
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

/// Read a big-endian IEEE 32-bit float from `payload` at `*offset`, advancing
/// `*offset` by four bytes. Errors rather than panics if the block is short.
fn read_f32_be(payload: &[u8], offset: &mut usize) -> Result<f64, FieldglassError> {
    let end = offset.checked_add(4).ok_or_else(|| {
        FieldglassError::Parse("spectral_complex: unpacked-block offset overflow".to_string())
    })?;
    let bytes = payload.get(*offset..end).ok_or_else(|| {
        FieldglassError::Parse(
            "spectral_complex: unpacked sub-truncation block is truncated".to_string(),
        )
    })?;
    let v = f32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as f64;
    *offset = end;
    Ok(v)
}

/// Decode a `spectral_complex` (template 5.51) data section into coefficients.
///
/// §7 has two parts: coefficients up to the triangular sub-truncation `KS` are
/// stored as raw IEEE 32-bit floats (copied through unscaled), and the rest are
/// simple-packed after division by the Laplacian operator `(n·(n+1))^P` — the
/// packed value at degree `n` is `(R + X · 2^E) · 10^-D / (n·(n+1))^P`. This is
/// a faithful port of eccodes' `DataComplexPacking::unpack_real` for GRIB2,
/// where `GRIBEXShBugPresent` is a constant `0` (so the GRIB1 last-row scaling
/// quirk is deliberately absent) and the packed imaginary part of zonal
/// wavenumber 0 is forced back to zero.
pub fn decode_spectral_complex(
    ds_payload: &[u8],
    t: &SpectralComplexPackingTemplate,
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
    if t.js != t.ks || t.js != t.ms {
        return Err(FieldglassError::UnsupportedSection(format!(
            "spectral_complex sub-truncation JS={} KS={} MS={} is not triangular",
            t.js, t.ks, t.ms
        )));
    }
    if t.bits_per_value > 32 {
        return Err(FieldglassError::Parse(format!(
            "spectral_complex: bits_per_value {} exceeds 32",
            t.bits_per_value
        )));
    }
    // The unpacked sub-truncation block is read as 4-byte IEEE floats
    // (`unpackedSubsetPrecision == 1`), the only value WMO defines and eccodes
    // supports. Reject anything else loudly rather than misreading the block at
    // the wrong width.
    if t.unpacked_subset_precision != 1 {
        return Err(FieldglassError::UnsupportedSection(format!(
            "spectral_complex: unpackedSubsetPrecision {} is unsupported (only 1 = IEEE 32-bit)",
            t.unpacked_subset_precision
        )));
    }
    let ks = t.ks as u32;
    if ks > j {
        return Err(FieldglassError::Parse(format!(
            "spectral_complex sub-truncation KS={ks} exceeds the truncation T={j}"
        )));
    }

    // Coefficient counts, checked + capped BEFORE any allocation or read so a
    // hostile J/KS cannot overflow the multiply or size a huge `Vec`.
    let n_values = triangular_value_count(j)?;
    let unpacked_values = triangular_value_count(ks)?; // (KS+1)(KS+2)
    let unpacked_bytes = unpacked_values.saturating_mul(4); // IEEE 32-bit
    let d_inv = 10f64.powi(-(t.decimal_scale_factor as i32));

    // Edge case: the sub-truncation is the whole field — everything is unpacked
    // and multiplied by the decimal scale (eccodes' `pen_j == sub_j` branch).
    if j == ks {
        let block = ds_payload.get(..unpacked_bytes).ok_or_else(|| {
            FieldglassError::Parse(format!(
                "spectral_complex: §7 holds {} bytes but the unpacked block needs {unpacked_bytes}",
                ds_payload.len()
            ))
        })?;
        let coefficients = block
            .chunks_exact(4)
            .map(|c| f32::from_be_bytes([c[0], c[1], c[2], c[3]]) as f64 * d_inv)
            .collect();
        return Ok(SpectralCoefficients {
            j,
            k,
            m,
            coefficients,
        });
    }

    // Bit budget: the unpacked block plus the packed real+imag parts.
    let packed_parts = n_values - unpacked_values;
    let available_bits = ds_payload.len().saturating_mul(8);
    let required_bits = unpacked_bytes
        .saturating_mul(8)
        .saturating_add(packed_parts.saturating_mul(t.bits_per_value as usize));
    if required_bits > available_bits {
        return Err(FieldglassError::Parse(format!(
            "spectral_complex truncation T={j}/KS={ks} needs {required_bits} bits but §7 holds only {available_bits}"
        )));
    }
    let packed_bytes = &ds_payload[unpacked_bytes..];

    let s = 2f64.powi(t.binary_scale_factor as i32);
    let reference = t.reference_value as f64;
    let p = t.laplacian_scaling_factor as f64 / 1e6;

    // Laplacian de-scaling factors `1/(n·(n+1))^P`; degree 0 has none. Sized by
    // the *initial* `maxv = J+1`, since the traversal below mutates `maxv`.
    let maxv0 = j as usize + 1;
    let mut scals = Vec::with_capacity(maxv0);
    scals.push(0.0f64);
    for n in 1..maxv0 {
        let operator = ((n * (n + 1)) as f64).powf(p);
        scals.push(if operator != 0.0 { 1.0 / operator } else { 0.0 });
    }

    // Direct port of eccodes' triangular traversal: `sub_k` shrinks the unpacked
    // run each outer step, `maxv` shrinks and `mmax` grows so `lup` (the degree
    // index into `scals`) stays in `0..=J`.
    let mut out = Vec::with_capacity(n_values);
    let mut hpos = 0usize; // byte cursor into the unpacked block at the payload start
    let mut packed = BitReader::new(packed_bytes);
    let mut sub_k: i64 = ks as i64;
    let mut maxv: i64 = j as i64 + 1;
    let mut mmax: i64 = 0;

    while maxv > 0 {
        let mut lup = mmax;
        let unpacked_count = if sub_k >= 0 { sub_k + 1 } else { 0 };
        for _ in 0..unpacked_count {
            let re = read_f32_be(ds_payload, &mut hpos)?;
            let im = read_f32_be(ds_payload, &mut hpos)?;
            out.push(re);
            out.push(im);
            lup += 1;
        }
        if sub_k >= 0 {
            sub_k -= 1;
        }
        for _ in unpacked_count..maxv {
            let scale = scals[lup as usize];
            let re = d_inv * ((packed.read_bits(t.bits_per_value)? as f64) * s + reference) * scale;
            let mut im =
                d_inv * ((packed.read_bits(t.bits_per_value)? as f64) * s + reference) * scale;
            // Zonal wavenumber 0 has no imaginary part; it is packed anyway, so
            // force it back to zero, matching eccodes.
            if mmax == 0 {
                im = 0.0;
            }
            out.push(re);
            out.push(im);
            lup += 1;
        }
        maxv -= 1;
        mmax += 1;
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

    fn complex_template(
        r: f32,
        e: i16,
        d: i16,
        bits: u8,
        laplacian: i32,
        ks: u16,
    ) -> SpectralComplexPackingTemplate {
        SpectralComplexPackingTemplate {
            reference_value: r,
            binary_scale_factor: e,
            decimal_scale_factor: d,
            bits_per_value: bits,
            laplacian_scaling_factor: laplacian,
            js: ks,
            ks,
            ms: ks,
            ts: (ks as u32 + 1) * (ks as u32 + 2),
            unpacked_subset_precision: 1,
        }
    }

    /// Big-endian IEEE-f32 bytes for a slice of values.
    fn f32_be(values: &[f32]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_be_bytes()).collect()
    }

    #[test]
    fn complex_fully_unpacked_edge_case() {
        // J == KS: the whole field is the unpacked IEEE block, scaled by 10^-D.
        // J=1 → (1+1)(1+2) = 6 values.
        let t = complex_template(0.0, 0, 1, 16, 2_000_000, 1); // D=1 → ×0.1
        let block = f32_be(&[10.0, 20.0, 30.0, 40.0, 50.0, 60.0]);
        let c = decode_spectral_complex(&block, &t, 1, 1, 1).expect("decode");
        assert_eq!(c.coefficients.len(), 6);
        for (got, want) in c.coefficients.iter().zip([1.0, 2.0, 3.0, 4.0, 5.0, 6.0]) {
            assert!((got - want).abs() < 1e-5, "{got} vs {want}");
        }
    }

    #[test]
    fn complex_rejects_non_triangular_and_out_of_range() {
        let t = complex_template(0.0, 0, 0, 16, 2_000_000, 4);
        // Non-triangular main truncation.
        assert!(decode_spectral_complex(&[0u8; 64], &t, 5, 5, 4).is_err());
        // KS > J.
        let big_ks = complex_template(0.0, 0, 0, 16, 2_000_000, 10);
        assert!(decode_spectral_complex(&[0u8; 64], &big_ks, 3, 3, 3).is_err());
        // bits > 32.
        let wide = complex_template(0.0, 0, 0, 33, 2_000_000, 1);
        assert!(decode_spectral_complex(&[0u8; 64], &wide, 3, 3, 3).is_err());
        // Non-triangular sub-truncation.
        let mut bad_sub = complex_template(0.0, 0, 0, 16, 2_000_000, 2);
        bad_sub.ms = 3;
        assert!(decode_spectral_complex(&[0u8; 64], &bad_sub, 5, 5, 5).is_err());
    }

    #[test]
    fn complex_rejects_short_section() {
        // J=4, KS=1: needs the unpacked block plus packed pairs; give nothing.
        let t = complex_template(0.0, 0, 0, 16, 2_000_000, 1);
        assert!(decode_spectral_complex(&[0u8; 4], &t, 4, 4, 4).is_err());
    }

    #[test]
    fn complex_rejects_unsupported_unpacked_precision() {
        // Only IEEE 32-bit (precision 1) unpacked floats are read; a message
        // declaring 64-bit must fail loudly rather than misread the block.
        let mut t = complex_template(0.0, 0, 0, 16, 2_000_000, 1);
        t.unpacked_subset_precision = 2;
        let err = decode_spectral_complex(&[0u8; 64], &t, 3, 3, 3).expect_err("must reject");
        assert!(
            format!("{err:?}").contains("unpackedSubsetPrecision"),
            "error names the unsupported precision, got: {err:?}"
        );
    }
}
