//! Spherical-harmonic coefficient encoding — GRIB1 BDS flag bit 0 = 1.
//!
//! Used by IFS analyses and other spectral-model output. The section holds the
//! *spectral coefficients* of the field, not values on a grid: there is nothing
//! to sample, and recovering a grid needs an inverse Legendre transform. So this
//! module decodes the coefficients and stops there, and a spectral message is
//! not routed through the scalar `decode_message_values` path — exactly as the
//! true `matrixOfValues` form isn't, and for the same reason (see
//! [`super::matrix`]). [`crate::Grib1Reader::decode_spectral_message`] is the
//! entry point.
//!
//! Two packings exist and both decode here:
//!
//! - `spectral_simple`: the whole array is simple-packed, except the real part
//!   of the `(0, 0)` coefficient — the field mean — which is lifted out into the
//!   section header as a bare IBM float so its magnitude doesn't swamp the
//!   quantisation of every other coefficient.
//! - `spectral_complex`: coefficients up to a *sub-truncation* (`n ≤ KS`) are
//!   stored as raw IBM floats, and the rest are simple-packed after division by
//!   a Laplacian operator `(n·(n+1))^P`. That flattens the steep fall-off of a
//!   spectral field's amplitude with degree, so a modest `bitsPerValue` still
//!   resolves the small high-degree coefficients.
//!
//! Ported from eccodes' `DataComplexPacking::unpack_real` and
//! `DataG1ShSimplePacking::unpack` — the code `grib_get_data` runs — and
//! validated against its output on a real T63 message.

use fieldglass_core::FieldglassError;
use fieldglass_core::bits::{BitReader, ibm_float_to_f64};

use crate::bds::{
    BdsHeader, SPECTRAL_COMPLEX_DATA_OFFSET, SPECTRAL_SIMPLE_DATA_OFFSET, SphericalExtendedHeader,
};

use super::Grib1Packing;

/// Bit width of one unpacked sub-truncation coefficient part: a 4-byte IBM
/// float. eccodes hard-codes the same 32 when it inverts the value count.
const UNPACKED_BITS: u8 = 32;

/// The decoded spherical-harmonic coefficients of one message.
///
/// `coefficients` is the flat `(real, imaginary)` interleaving eccodes reports,
/// traversed by zonal wavenumber `m` (outer) and total wavenumber `n` (inner):
///
/// ```text
/// for m in 0..=T:
///     for n in m..=T:
///         push Re(X[m][n]); push Im(X[m][n])
/// ```
///
/// so the length is `(T + 1)·(T + 2)` for a triangular truncation `T` — 4160 at
/// T63, which is what eccodes reports as `numberOfValues`.
#[derive(Debug, Clone, PartialEq)]
pub struct SpectralCoefficients {
    /// Pentagonal resolution parameters from the GDS. Triangular truncation
    /// (the only form in the wild, and the only one eccodes decodes) has
    /// `j == k == m`.
    pub j: u16,
    pub k: u16,
    pub m: u16,
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

/// Stored value count (real *and* imaginary parts) of a triangular truncation
/// `t`: `(t + 1)·(t + 2)`.
fn triangular_value_count(t: u16) -> usize {
    (usize::from(t) + 1) * (usize::from(t) + 2)
}

/// Reject a truncation the section cannot possibly hold, before anything is
/// allocated from it.
///
/// `J` is a bare `u16` from the GDS, so a corrupt or hostile message can declare
/// `J = 65535` — 4.3 billion values, which sizes a `Vec` at tens of gigabytes and
/// aborts the process long before the short data section is found to be short.
/// Counting the bits the declared layout would need and comparing them against
/// the bits actually present bounds every later allocation by the file itself.
fn check_declared_size(
    available_bits: usize,
    required_bits: usize,
    t: u16,
) -> Result<(), FieldglassError> {
    if required_bits > available_bits {
        return Err(FieldglassError::Parse(format!(
            "spectral truncation T={t} needs {required_bits} bits but the data \
             section holds only {available_bits}"
        )));
    }
    Ok(())
}

/// The scalar-grid decoder. A spectral message has no grid, so this always
/// refuses — and names the call that *does* decode it, rather than leaving the
/// caller to guess.
pub struct SphericalPacking;

impl Grib1Packing for SphericalPacking {
    fn decode(
        &self,
        _bds: &[u8],
        _header: &BdsHeader,
        _decimal_scale: i16,
        _bitmap: Option<&[bool]>,
        _expected_count: usize,
        _cols: usize,
    ) -> Result<Vec<Option<f64>>, FieldglassError> {
        Err(FieldglassError::UnsupportedSection(
            "BDS holds spherical-harmonic coefficients, which are not values on \
             a grid — decode them with `Grib1Reader::decode_spectral_message`. \
             Rendering one as a 2-D field needs an inverse Legendre transform, \
             which this crate does not do yet."
                .into(),
        ))
    }
}

/// Decode the coefficients of a spherical-harmonic BDS. `j`/`k`/`m` come from
/// the GDS; `decimal_scale` is the PDS's `D`.
pub fn decode_spectral(
    bds: &[u8],
    header: &BdsHeader,
    decimal_scale: i16,
    j: u16,
    k: u16,
    m: u16,
) -> Result<SpectralCoefficients, FieldglassError> {
    // eccodes rejects anything but a triangular truncation, and no encoder emits
    // one. A pentagonal truncation would change both the coefficient count and
    // the traversal, so refuse rather than decode it wrong.
    if j != k || j != m {
        return Err(FieldglassError::UnsupportedSection(format!(
            "spectral truncation J={j} K={k} M={m} is not triangular \
             (only J = K = M is defined for GRIB1 spectral packing)"
        )));
    }
    let spherical = header.spherical_extended.ok_or_else(|| {
        FieldglassError::Parse("spectral BDS is missing its follow-on header".to_string())
    })?;

    let coefficients = match spherical {
        SphericalExtendedHeader::Simple { real_part } => {
            decode_simple(bds, header, decimal_scale, j, real_part)?
        }
        SphericalExtendedHeader::Complex { p, js, ks, ms } => {
            decode_complex(bds, header, decimal_scale, j, p, js, ks, ms)?
        }
    };
    Ok(SpectralCoefficients {
        j,
        k,
        m,
        coefficients,
    })
}

/// `spectral_simple`: `values[0]` is the header's IBM-float real part, copied
/// through unscaled. Everything after it comes from the simple-packed stream —
/// *including* the imaginary part of `(0, 0)`, which is mathematically zero but
/// is really stored, and which eccodes does not force back to zero here (unlike
/// complex packing). Reproduce that, or the two decoders disagree.
fn decode_simple(
    bds: &[u8],
    header: &BdsHeader,
    decimal_scale: i16,
    t: u16,
    real_part: f64,
) -> Result<Vec<f64>, FieldglassError> {
    let data = bds
        .get(SPECTRAL_SIMPLE_DATA_OFFSET..)
        .ok_or_else(|| FieldglassError::Parse("spectral_simple BDS is truncated".to_string()))?;
    let n_values = triangular_value_count(t);
    // values[0] comes from the header; the other (n_values - 1) are packed.
    check_declared_size(
        data.len().saturating_mul(8),
        (n_values - 1).saturating_mul(usize::from(header.bits_per_value)),
        t,
    )?;
    let s = binary_scale(header.binary_scale_factor);
    let d = decimal_scale_factor(decimal_scale);

    let mut out = Vec::with_capacity(n_values);
    out.push(real_part);
    if header.bits_per_value == 0 {
        // A zero width means every coded value is the reference value.
        out.resize(n_values, header.reference_value * d);
        return Ok(out);
    }
    let mut reader = BitReader::new(data);
    for _ in 1..n_values {
        let raw = f64::from(reader.read_bits(header.bits_per_value)?);
        out.push((raw * s + header.reference_value) * d);
    }
    Ok(out)
}

/// `spectral_complex`: the sub-truncation triangle `{(m, n) : n ≤ KS}` is stored
/// as raw 32-bit IBM floats and the rest is simple-packed and Laplacian-scaled.
/// The two streams are read independently, front to back, woven together by one
/// triangular traversal.
#[allow(clippy::too_many_arguments)]
fn decode_complex(
    bds: &[u8],
    header: &BdsHeader,
    decimal_scale: i16,
    t: u16,
    p: i16,
    js: u8,
    ks: u8,
    ms: u8,
) -> Result<Vec<f64>, FieldglassError> {
    if js != ks || js != ms {
        return Err(FieldglassError::UnsupportedSection(format!(
            "spectral_complex sub-truncation JS={js} KS={ks} MS={ms} is not triangular"
        )));
    }
    let ks = u16::from(ks);
    if ks > t {
        return Err(FieldglassError::Parse(format!(
            "spectral_complex sub-truncation KS={ks} exceeds the truncation T={t}"
        )));
    }
    let data = bds
        .get(SPECTRAL_COMPLEX_DATA_OFFSET..)
        .ok_or_else(|| FieldglassError::Parse("spectral_complex BDS is truncated".to_string()))?;

    // The unpacked block is `(KS+1)·(KS+2)` values of four bytes each, at the
    // front of the data; the packed block follows immediately, with no
    // re-alignment. eccodes derives this offset the same way rather than read the
    // section's own `N` pointer, which it writes relative to the *message* — its
    // own source calls that out as wrong. Don't trust `N`.
    let n_values = triangular_value_count(t);
    let unpacked_values = triangular_value_count(ks);
    let unpacked_bytes = unpacked_values * (UNPACKED_BITS as usize / 8);
    // The declared truncation must fit the bytes actually present, or a hostile
    // J would size the output before the short section is ever noticed.
    check_declared_size(
        data.len().saturating_mul(8),
        unpacked_bytes.saturating_mul(8).saturating_add(
            n_values
                .saturating_sub(unpacked_values)
                .saturating_mul(usize::from(header.bits_per_value)),
        ),
        t,
    )?;
    let packed_bytes = data.get(unpacked_bytes..).ok_or_else(|| {
        FieldglassError::Parse(format!(
            "spectral_complex BDS is too short for a {unpacked_bytes}-byte sub-truncation block"
        ))
    })?;

    // Laplacian de-scaling. The encoder divided coefficient (m, n) by
    // (n·(n+1))^P, so multiply it back. Degree 0 has no Laplacian, and eccodes
    // defines its factor as 0 rather than 1 — which matters only through the
    // GRIBEX quirk below, since the packed branch never reaches degree 0.
    let p_operator = f64::from(p) / 1000.0;
    let scals: Vec<f64> = (0..=t)
        .map(|n| {
            if n == 0 {
                0.0
            } else {
                let operator = (f64::from(n) * f64::from(n + 1)).powf(p_operator);
                if operator == 0.0 { 0.0 } else { 1.0 / operator }
            }
        })
        .collect();

    let s = binary_scale(header.binary_scale_factor);
    let d = decimal_scale_factor(decimal_scale);
    let bits = header.bits_per_value;

    let mut unpacked = BitReader::new(data);
    let mut packed = BitReader::new(packed_bytes);
    let mut out = Vec::with_capacity(n_values);

    for zonal in 0..=t {
        let mut degree = zonal;
        // Coefficients of this row inside the sub-truncation: degree m..=KS.
        // None once the row itself starts above KS.
        if let Some(last) = ks.checked_sub(zonal) {
            for row in 0..=last {
                let mut re = ibm_float_to_f64(unpacked.read_bits(UNPACKED_BITS)?);
                let mut im = ibm_float_to_f64(unpacked.read_bits(UNPACKED_BITS)?);
                // The GRIBEX quirk, faithfully reproduced. The encoder scaled the
                // last coefficient of each sub-truncation row — the one at degree
                // KS — when it should not have, so the decoder scales it back.
                // eccodes carries the same branch, comment and all ("bug in ecmwf
                // data, last row is scaled but should not").
                if row == last {
                    let scale = scals[usize::from(degree)];
                    re *= scale;
                    im *= scale;
                }
                out.push(re);
                out.push(im);
                degree += 1;
            }
        }
        // The rest of the row is simple-packed and Laplacian-scaled.
        while degree <= t {
            let (raw_re, raw_im) = if bits == 0 {
                (0.0, 0.0)
            } else {
                (
                    f64::from(packed.read_bits(bits)?),
                    f64::from(packed.read_bits(bits)?),
                )
            };
            let scale = scals[usize::from(degree)];
            out.push(d * (raw_re * s + header.reference_value) * scale);
            // Zonal wavenumber 0 has no imaginary part. It is stored anyway, so
            // it decodes to quantisation noise; eccodes forces it to zero, and so
            // must we, or a real field comes back faintly complex.
            let im = d * (raw_im * s + header.reference_value) * scale;
            out.push(if zonal == 0 { 0.0 } else { im });
            degree += 1;
        }
    }
    Ok(out)
}

/// `2^E` — the binary scale factor.
fn binary_scale(e: i16) -> f64 {
    2f64.powi(i32::from(e))
}

/// `10^-D` — the decimal scale factor.
fn decimal_scale_factor(d: i16) -> f64 {
    10f64.powi(-i32::from(d))
}
