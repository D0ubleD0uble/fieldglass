//! GRIB1 BDS packing decoders.
//!
//! Each variant of GRIB1 packing lives in its own module behind the
//! [`Grib1Packing`] trait. The dispatcher [`decoder_for`] inspects the BDS
//! flag bits and returns the right decoder. Decoding today covers
//! [`simple::SimplePacking`], the full second-order family
//! ([`complex::ComplexPacking`] → [`second_order`] / [`second_order_classic`]),
//! IEEE raw floats ([`ieee::IeeePacking`]), and matrix-of-values
//! ([`matrix::MatrixPacking`]). [`spherical::SphericalPacking`] remains a stub
//! that surfaces [`FieldglassError::UnsupportedSection`] with a message naming
//! the packing mode, so users get a precise reason rather than a bare
//! "unsupported section".
//!
//! Adding a new packing means: drop a new module under `packing/`, implement
//! `Grib1Packing::decode`, and route it from `decoder_for`. No other crates
//! need to change.

use fieldglass_core::{FieldglassError, bits::BitReader};

use crate::bds::BdsHeader;

pub mod complex;
pub mod ieee;
pub mod matrix;
pub mod second_order;
pub mod second_order_classic;
pub mod simple;
pub mod spherical;

/// A BDS packing decoder. Implementors take the full Binary Data Section
/// (starting at its 3-byte length prefix), the parsed header, the PDS
/// decimal scale factor, an optional per-point bitmap (from BMS), the
/// total grid-point count from the GDS, and the grid's column count
/// `cols` (used by complex/second-order decoders to undo boustrophedonic
/// row scanning — simple-packing impls ignore it). They return one
/// `Option<f64>` per grid point — `None` for bitmap-masked points,
/// `Some(value)` otherwise.
pub trait Grib1Packing {
    fn decode(
        &self,
        bds: &[u8],
        header: &BdsHeader,
        decimal_scale: i16,
        bitmap: Option<&[bool]>,
        expected_count: usize,
        cols: usize,
    ) -> Result<Vec<Option<f64>>, FieldglassError>;
}

/// Pick the right [`Grib1Packing`] for a parsed BDS header.
///
/// Mirrors the `packingType` concept dispatch in eccodes'
/// `grib1/section.4.def`, which keys off the BDS octet-4 flag bits:
///
/// | flags (SH, complex, integer, extra) | packingType |
/// |---|---|
/// | `1, *, *, *` | spherical (`spectral_*`) |
/// | `0, 1, *, *` | complex / second-order |
/// | `0, 0, 1, 1` | `grid_ieee` (raw IEEE floats) |
/// | `0, 0, 0, 1` | `grid_simple_matrix` |
/// | `0, 0, *, 0` | `grid_simple` |
///
/// The integer-vs-matrix split matters: eccodes' concept lists `grid_ieee`
/// (which requires `integerPointValues = 1`) ahead of `grid_simple_matrix`, so
/// the extra-flags-present + integer-data combination resolves to IEEE.
/// Spherical-harmonic wins over complex when both bits are set — spherical is
/// only a stub today, so that precedence merely shapes the error message.
pub fn decoder_for(header: &BdsHeader) -> Box<dyn Grib1Packing> {
    if header.is_spherical_harmonic {
        return Box::new(spherical::SphericalPacking);
    }
    if header.is_complex_packing {
        return Box::new(complex::ComplexPacking);
    }
    if header.has_extra_flags {
        if header.is_integer_data {
            return Box::new(ieee::IeeePacking);
        }
        return Box::new(matrix::MatrixPacking);
    }
    Box::new(simple::SimplePacking)
}

/// Read `count` simple-packed integers of `bits_per_value` bits (MSB-first)
/// from `packed`, scaling each by `(R + X·2^E) / 10^D`. Shared by
/// [`simple::SimplePacking`] and the matrix-of-values body decoder — both lay
/// their values out as plain simple packing, differing only in where the data
/// slice begins. `bits_per_value` must be in `1..=32`; the caller handles the
/// `bits_per_value == 0` constant-field case before calling.
pub(crate) fn unpack_simple_values(
    packed: &[u8],
    bits_per_value: u8,
    reference: f64,
    two_pow_e: f64,
    d_scale: f64,
    count: usize,
) -> Result<Vec<f64>, FieldglassError> {
    let mut reader = BitReader::new(packed);
    let mut decoded = Vec::with_capacity(count);
    for _ in 0..count {
        let x = reader.read_bits(bits_per_value)?;
        decoded.push((reference + x as f64 * two_pow_e) * d_scale);
    }
    Ok(decoded)
}

/// Interleave `None` at bitmap-masked points: walk the per-point `bitmap`,
/// pulling the next decoded value where the bit is set and emitting `None`
/// where it is clear. With no bitmap every decoded value is `Some`. Shared by
/// the simple, IEEE, and matrix decoders.
pub(crate) fn interleave_with_bitmap(
    decoded: Vec<f64>,
    bitmap: Option<&[bool]>,
    expected_count: usize,
) -> Vec<Option<f64>> {
    match bitmap {
        None => decoded.into_iter().map(Some).collect(),
        Some(b) => {
            let mut out = Vec::with_capacity(expected_count);
            let mut iter = decoded.into_iter();
            for present in b.iter().take(expected_count) {
                out.push(if *present { iter.next() } else { None });
            }
            out
        }
    }
}

/// Number of present (non-masked) grid points: every point when there is no
/// bitmap, otherwise the count of set bits.
pub(crate) fn present_count(bitmap: Option<&[bool]>, expected_count: usize) -> usize {
    match bitmap {
        Some(b) => b.iter().filter(|p| **p).count(),
        None => expected_count,
    }
}

/// Shared reconstruction tail for every GRIB1 second-order packing. Given the
/// reconstructed integer grid `x` in storage order, scale it by
/// `(R + x·2^E) / 10^D`, undo boustrophedonic row ordering (odd rows stored
/// right-to-left) if `boustrophedonic` is set, then interleave `None` at any
/// bitmap-masked points. Boustrophedonic undo must precede the bitmap
/// interleave, because the bitmap maps the storage stream. Used by both
/// [`second_order`] and [`second_order_classic`].
pub(crate) fn finalize_second_order(
    x: Vec<i64>,
    header: &BdsHeader,
    decimal_scale: i16,
    boustrophedonic: bool,
    cols: usize,
    bitmap: Option<&[bool]>,
    expected_count: usize,
) -> Result<Vec<Option<f64>>, FieldglassError> {
    let two_pow_e = 2f64.powi(header.binary_scale_factor as i32);
    let d_scale = 10f64.powi(-(decimal_scale as i32));
    let r = header.reference_value;

    let mut scaled: Vec<f64> = x
        .iter()
        .map(|v| (r + (*v as f64) * two_pow_e) * d_scale)
        .collect();

    if boustrophedonic && cols > 0 {
        let rows = scaled.len() / cols;
        for row in (1..rows).step_by(2) {
            let start = row * cols;
            let end = start + cols;
            scaled[start..end].reverse();
        }
    }

    match bitmap {
        None => {
            if scaled.len() != expected_count {
                return Err(FieldglassError::Parse(format!(
                    "second-order decoded {} values but {} expected",
                    scaled.len(),
                    expected_count
                )));
            }
            Ok(scaled.into_iter().map(Some).collect())
        }
        Some(b) => {
            let mut out = Vec::with_capacity(expected_count);
            let mut iter = scaled.into_iter();
            for present in b.iter().take(expected_count) {
                out.push(if *present { iter.next() } else { None });
            }
            Ok(out)
        }
    }
}
