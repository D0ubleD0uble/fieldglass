//! Reduced-resolution decode: the display raster and the options that ask for
//! one (#463).
//!
//! A zoomed-out map samples a grid far more coarsely than the grid is stored,
//! and JPEG 2000 (§5.40) is the one packing that can answer coarsely without
//! decoding the whole field first: its codestream is a wavelet pyramid, so
//! discarding the finest levels skips most of the entropy decode, which is
//! where the time goes. On the committed RAP fixture that is 39.9 ms → 10.5 ms
//! → 2.8 ms at reduction 0 / 1 / 2, natively in release — the numbers
//! `examples/bench_reduce.rs` prints.
//!
//! Two rules govern everything here.
//!
//! **A coarse field is not the message's field.** Its values are wavelet
//! low-pass averages that exist in no GRIB message, so they may be *rendered*
//! and must not be probed, exported, contoured or reduced to statistics. That
//! is why [`DisplayRaster`] is its own type rather than a `Vec<Option<f64>>`:
//! nothing that takes a decoded field accepts one, and
//! [`DisplayRaster::exact_values`] hands the slice over only when the reduction
//! was zero and the values therefore *are* the message's. The same shape #633
//! settled for an unresolved parameter — a type-checked absence rather than a
//! sentinel a caller has to recognise.
//!
//! **A coarse field is not on the message's grid either.** It is
//! `ceil(ni / 2^r) × ceil(nj / 2^r)` points at `2^r` times the spacing, so
//! pairing it with the message's own GDS would draw the field at `1/2^r` of its
//! true size. [`DisplayRaster::geometry`] is the derived one, from
//! [`GridGeometry::subsampled`], and it is the only geometry that goes with
//! these values.

use fieldglass_core::{FieldglassError, GridGeometry};

/// How a decode should be shaped (#463).
///
/// `#[non_exhaustive]`: this crate is published, and the pyramid is not the
/// last thing a decode might be asked to do. Build one with [`Self::new`] or
/// from [`Default`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct DecodeOptions {
    /// How many of the finest wavelet levels to discard: `0` decodes the
    /// message's own field, `1` a half-size one, `2` a quarter-size one.
    ///
    /// Only §5.40 (JPEG 2000) honours a non-zero value — it is the only packing
    /// with a pyramid to climb. Every other template refuses one rather than
    /// decoding in full and pretending, so a caller always knows which field it
    /// got. See [`crate::Grib2Reader::decode_message_raster_with`] for the
    /// other four refusals.
    pub resolution_reduction: u8,
}

impl DecodeOptions {
    /// The one field this type has.
    ///
    /// A constructor rather than a struct literal because the type is
    /// `#[non_exhaustive]`: a caller outside this crate cannot write one.
    #[must_use]
    pub fn new(resolution_reduction: u8) -> Self {
        Self {
            resolution_reduction,
        }
    }
}

/// A raster to draw, and the geometry that places it — the result of
/// [`Grib2Reader::decode_message_raster_with`](crate::Grib2Reader::decode_message_raster_with).
///
/// At [`resolution_reduction`](Self::resolution_reduction) zero this is exactly
/// what [`Grib2Reader::decode_message_raster`](crate::Grib2Reader::decode_message_raster)
/// returns, carried beside the message's own geometry. Above zero the values
/// are a wavelet low-pass of the field: right to look at, wrong to measure.
///
/// The type is what enforces that. There is no conversion from a
/// `DisplayRaster` to a decoded field, so an operation that takes one —
/// `probe`, a CSV export, contouring, statistics — cannot be handed a coarse
/// raster at all:
///
/// ```compile_fail
/// # use fieldglass_grib2::{DecodeOptions, Grib2Reader};
/// fn stats(values: &[Option<f64>]) -> usize { values.len() }
/// # fn demo(reader: &Grib2Reader) {
/// let coarse = reader
///     .decode_message_raster_with(0, DecodeOptions::new(1))
///     .expect("a 5.40 message");
/// // A `DisplayRaster` is not a decoded field, and this does not compile.
/// stats(&coarse);
/// # }
/// ```
///
/// [`exact_values`](Self::exact_values) is the one way through, and it answers
/// `None` for a coarse raster.
#[derive(Debug, Clone, PartialEq)]
pub struct DisplayRaster {
    values: Vec<Option<f64>>,
    ni: u32,
    nj: u32,
    geometry: GridGeometry,
    resolution_reduction: u8,
}

impl DisplayRaster {
    /// Build one. Crate-internal: the reader is the only thing that knows
    /// whether the values and the geometry belong together, which is the whole
    /// invariant.
    pub(crate) fn new(
        values: Vec<Option<f64>>,
        ni: u32,
        nj: u32,
        geometry: GridGeometry,
        resolution_reduction: u8,
    ) -> Result<Self, FieldglassError> {
        // `checked_mul`, not `*`: both call sites bound the product before they
        // get here, but this is the one place that establishes the type's
        // invariant, and a product that wrapped would *satisfy* the length
        // check rather than fail it. `usize` is 32 bits on `wasm32`, where
        // `ni · nj` for a plausible-looking `ni`/`nj` pair is much closer to
        // the ceiling than it looks on a 64-bit host.
        let expected = (ni as usize).checked_mul(nj as usize).ok_or_else(|| {
            FieldglassError::Parse(format!("display raster {ni}×{nj} overflows usize"))
        })?;
        if values.len() != expected {
            return Err(FieldglassError::Parse(format!(
                "display raster is {ni}×{nj} = {expected} points but {} values were decoded",
                values.len()
            )));
        }
        Ok(Self {
            values,
            ni,
            nj,
            geometry,
            resolution_reduction,
        })
    }

    /// The values to draw, `ni · nj` of them in row-major order, `None` where
    /// the field has no value.
    ///
    /// Named for what they are for. At a non-zero reduction these are wavelet
    /// averages, so a number read out of here is not a number the message
    /// carries — use [`exact_values`](Self::exact_values) when that matters.
    #[must_use]
    pub fn display_values(&self) -> &[Option<f64>] {
        &self.values
    }

    /// The values, but only when they are the message's own — `None` at any
    /// non-zero [`resolution_reduction`](Self::resolution_reduction).
    ///
    /// This is the seam between "draw it" and "measure it". A probe, a CSV
    /// export, a contour set and a statistic all answer *about the data*, so
    /// they need the field the message stores; giving them a low-pass would
    /// report a value that is nowhere in the file, with nothing in the answer
    /// to say so.
    #[must_use]
    pub fn exact_values(&self) -> Option<&[Option<f64>]> {
        (self.resolution_reduction == 0).then_some(self.values.as_slice())
    }

    /// Raster columns.
    #[must_use]
    pub fn ni(&self) -> u32 {
        self.ni
    }

    /// Raster rows.
    #[must_use]
    pub fn nj(&self) -> u32 {
        self.nj
    }

    /// Where these values sit on the Earth — **derived** at a non-zero
    /// reduction, and never the message's own GDS. See the module docs.
    #[must_use]
    pub fn geometry(&self) -> &GridGeometry {
        &self.geometry
    }

    /// How many wavelet levels were discarded to produce this raster. Zero
    /// means the values are the message's.
    #[must_use]
    pub fn resolution_reduction(&self) -> u8 {
        self.resolution_reduction
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fieldglass_core::LatLonParams;

    fn geometry(ni: u32, nj: u32) -> GridGeometry {
        GridGeometry::LatLon(LatLonParams {
            ni,
            nj,
            lat_first: 90.0,
            lon_first: 0.0,
            lat_last: -90.0,
            lon_last: 350.0,
        })
    }

    /// The invariant the type exists for: values and shape agree. A decoder
    /// that returned a raster of the wrong length would otherwise hand a
    /// caller an `ni × nj` promise it cannot index by.
    #[test]
    fn a_raster_that_is_not_its_own_shape_is_refused() {
        let err = DisplayRaster::new(vec![Some(1.0); 5], 2, 3, geometry(2, 3), 0)
            .expect_err("five values are not a 2×3 raster");
        let text = err.to_string();
        assert!(text.contains("2×3") && text.contains("5 values"), "{text}");
        assert!(DisplayRaster::new(vec![Some(1.0); 6], 2, 3, geometry(2, 3), 0).is_ok());
    }

    /// The shape is checked with `checked_mul`, so a product that does not fit
    /// the platform's `usize` is refused rather than wrapping into a length
    /// that happens to match. On `wasm32` the ceiling is 2^32, not 2^64.
    #[test]
    fn a_shape_that_overflows_the_pointer_width_is_refused() {
        let (ni, nj) = (u32::MAX, u32::MAX);
        let overflows = (ni as usize).checked_mul(nj as usize).is_none();
        let built = DisplayRaster::new(Vec::new(), ni, nj, geometry(2, 3), 0);
        if overflows {
            let err = built.expect_err("the product does not fit this usize");
            assert!(err.to_string().contains("overflows usize"), "{err}");
        } else {
            // 64-bit: the product fits, so the length check is what refuses.
            let err = built.expect_err("no values for a 2^64-point raster");
            assert!(err.to_string().contains("0 values were decoded"), "{err}");
        }
    }

    /// `exact_values` is the seam, so it is checked on both sides of the one
    /// condition it turns on.
    #[test]
    fn only_an_unreduced_raster_hands_over_its_values() {
        let full = DisplayRaster::new(vec![Some(1.0); 6], 2, 3, geometry(2, 3), 0).expect("built");
        assert_eq!(full.exact_values(), Some(full.display_values()));
        let coarse =
            DisplayRaster::new(vec![Some(1.0); 6], 2, 3, geometry(2, 3), 1).expect("built");
        assert_eq!(coarse.exact_values(), None);
        assert_eq!(coarse.display_values().len(), 6);
    }

    /// The options type is `#[non_exhaustive]`, so a caller outside this crate
    /// reaches every field through the constructor or `Default`.
    #[test]
    fn the_options_default_to_the_message_field() {
        assert_eq!(DecodeOptions::default().resolution_reduction, 0);
        assert_eq!(DecodeOptions::new(3).resolution_reduction, 3);
    }
}
