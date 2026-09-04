//! GRIB edition 1 reader.
//!
//! Full section parsing (IS, PDS, GDS, BMS, BDS) for message metadata, plus
//! value decoding for the packings that appear in the wild: simple, IEEE
//! floating point, ECMWF complex, second-order in both the classic and general
//! extended forms (with spatial differencing and boustrophedonic row order),
//! the `matrixOfValues` form, and spherical-harmonic coefficients — which
//! [`Grib1Reader::synthesize_spectral_message`] transforms back onto a lat/lon
//! grid so a spectral field can be rendered rather than only listed.
//!
//! [`GridDescription`] covers regular and rotated lat/lon, Gaussian, polar
//! stereographic, Lambert conformal, and the reduced (quasi-regular) lat/lon
//! and Gaussian layouts whose rows differ in width; a message carrying no GDS
//! at all resolves the common predefined global grids by their ON388 catalogue
//! number. Every grid reports the corners and geometry
//! `fieldglass_core::projection` needs to place it on a map.
//!
//! A reduced grid stores `sum(PL)` values, not the `Ni × Nj` its
//! [`GridDescription::dimensions`] reports. [`Grib1Reader::decode_message_raster`]
//! is the entry point that hands back the rectangle, with
//! [`GridDescription::raster_bounds`] as its extent;
//! [`Grib1Reader::decode_message_values`] is the field exactly as the message
//! stores it. Neither leaves the widening rule to the caller.
//!
//! A message routes to one of three decode methods, and which one is not a
//! property of its packing label alone —
//! [`Grib1Reader::message_kind`] is the answer. Grid policy the file states
//! obliquely (the scan direction behind unsigned `Dx`/`Dy`, the ±60° true-scale
//! parallel GRIB1 never writes down, whether a message has a raster at all)
//! lives on [`GridDescription`] and its per-family structs rather than in each
//! consumer.
//!
//! Decoders are cross-checked against eccodes: `tests/eccodes_reference.rs`
//! walks every committed fixture and compares both the metadata keys and the
//! decoded values against a pinned snapshot, so a packing bug fails the suite
//! rather than surfacing as a plausible-looking picture.

#![forbid(unsafe_code)]
/// Section 4, the Binary Data Section: its header and the value decode.
pub mod bds;
/// Section 3, the Bit Map Section: which grid points carry a value.
pub mod bms;
/// Section 2, the Grid Description Section: one struct per grid family.
pub mod gds;
pub mod geometry;
/// Section 0, the Indicator Section: message length and edition.
pub mod is;
/// The BDS packings, one module each, behind a common trait.
pub mod packing;
/// Section 1, the Product Definition Section: what the field is, and when.
pub mod pds;
pub mod predefined;
/// The message scanner and the decode entry points over a whole file.
pub mod reader;
/// WMO ON388 Table 2 parameter lookup, international and centre-local.
pub mod tables;
pub mod tables_cct;
mod tables_ecmwf;

pub use bds::{
    BDS_DATA_OFFSET, BdsHeader, ComplexExtendedHeader, SphericalExtendedHeader, parse_bds_header,
};
pub use bms::Bitmap;
pub use gds::{GridDescription, ScanningMode, SphericalHarmonicGrid};
// Re-exported from `fieldglass_core`, where GRIB2's reduced-grid decode (#503)
// also needs it. The path stays here because callers of this crate have it.
pub use fieldglass_core::expand_reduced_to_regular;
pub use is::IndicatorSection;
pub use packing::spherical::SpectralCoefficients;
pub use pds::ProductDefinition;
pub use predefined::predefined_grid;
pub use reader::{
    Grib1Message, Grib1MessageKind, Grib1Reader, MAX_GRID_POINTS, MatrixField, forecast_display,
    forecast_hours, level_type_str, level_unit, level_value, level_value_str, reference_time,
};
