#![forbid(unsafe_code)]

pub mod bds;
pub mod bms;
pub mod gds;
pub mod is;
pub mod packing;
pub mod pds;
pub mod predefined;
pub mod reader;
pub mod tables;
pub mod tables_cct;
mod tables_ecmwf;

pub use bds::{
    BDS_DATA_OFFSET, BdsHeader, ComplexExtendedHeader, SphericalExtendedHeader, parse_bds_header,
};
pub use bms::Bitmap;
pub use gds::{GridDescription, SphericalHarmonicGrid};
// Re-exported from `fieldglass_core`, where GRIB2's reduced-grid decode (#503)
// also needs it. The path stays here because callers of this crate have it.
pub use fieldglass_core::expand_reduced_to_regular;
pub use is::IndicatorSection;
pub use packing::spherical::SpectralCoefficients;
pub use pds::ProductDefinition;
pub use predefined::predefined_grid;
pub use reader::{
    Grib1Message, Grib1Reader, MAX_GRID_POINTS, MatrixField, forecast_display, forecast_hours,
    level_type_str, level_unit, level_value, level_value_str, reference_time,
};
