//! GRIB edition 2 reader.
//!
//! Full §0–§7 parsing for the message metadata, plus value decoding for **every
//! registered §5 Data Representation template** (Code Table 5.0), in pure Rust
//! with no external libraries and no build flags. The scalar packings
//! (one value per grid point) decode through
//! [`Grib2Reader::decode_message_values`]: simple (5.0), complex with and
//! without spatial differencing (5.2 / 5.3), IEEE floating point (5.4),
//! JPEG 2000 (5.40), PNG (5.41), CCSDS / AEC (5.42), simple + logarithmic
//! pre-processing (5.61), run-length (5.200), second-order (5.50001 / 5.50002),
//! and the flat form of matrix-of-values (5.1). The non-scalar packings have
//! their own entry points: spherical-harmonic spectral (§3.50 + 5.50 / 5.51) via
//! [`Grib2Reader::decode_spectral_message`] — and
//! [`Grib2Reader::synthesize_spectral_message`] runs the inverse transform back
//! to a grid; bi-Fourier spectral (5.53) via
//! [`Grib2Reader::decode_bifourier_message`]; and the true per-point matrix
//! (5.1, `matrixBitmapsPresent = 1`) via
//! [`Grib2Reader::decode_matrix_message`]. The pre-standard local image
//! templates 5.40000 / 5.40010 decode too (the latter eccodes cannot). Value
//! decoders are cross-checked against eccodes; for the handful eccodes cannot
//! handle, against the definitive spec and independent implementations.
//!
//! The error type every `Result` here returns ([`FieldglassError`]) and the
//! typed grid value [`GridGeometry`] converts into are re-exported from
//! `fieldglass-core`, so this crate can be the only Fieldglass dependency in a
//! consumer's manifest; the shared WMO sub-centre lookup is re-exported the
//! same way, from [`tables_cct`].
//!
//! Those entry points all return the field **as the message stores it**. A
//! reduced Gaussian grid stores `sum(PL)` values, not the `Ni × Nj` its
//! [`GridDefinitionSection::dimensions`] reports, so
//! [`Grib2Reader::decode_message_raster`] is the entry point that hands back
//! the rectangle, with [`GridDefinitionSection::raster_bounds`] as its extent.
//! Neither leaves the widening rule to the caller.

#![forbid(unsafe_code)]
pub mod bms;
pub mod drs;
pub mod ds;
pub mod gds;
pub mod geometry;
pub mod ids;
/// §0, the Indicator Section: discipline, edition and total length.
pub mod is;
pub mod lus;
pub mod matrix;
pub mod pds;
/// The message scanner and the decode entry points over a whole file.
pub mod reader;
pub mod section;
pub mod spectral;
pub mod tables;
pub mod tables_cct;
mod tables_ecmf;
mod tables_edzw;
mod tables_local;
mod tables_ncep;
/// Generated WMO master code tables (Table 4.2 / 4.4 / 4.5). Private: callers
/// go through [`tables`], which layers the curated entries over these.
mod tables_wmo;
mod tables_wmo_short;

pub use bms::{
    BMS_INDICATOR_NONE, BMS_INDICATOR_PRESENT, BMS_INDICATOR_PREVIOUS, BMS_SECTION_NUMBER,
    BitMapSection, parse_bit_map,
};
pub use drs::{
    DRS_SECTION_NUMBER, DataRepresentationSection, DataRepresentationTemplate, IeeePackingTemplate,
    SimplePackingTemplate, SpectralComplexPackingTemplate, SpectralSimplePackingTemplate,
    parse_data_representation,
};
pub use ds::{DS_SECTION_NUMBER, decode_values};
// The `fieldglass_core` types this crate's own signatures name (#537), so a
// consumer needs no direct dependency on `fieldglass-core` — and cannot
// accidentally take one without `default-features = false`, which would
// re-enable `render` and `fs` across the whole dependency graph. The three
// parameter structs are here because three §3 templates hand one back by
// value: `LambertAzimuthalTemplate::projection_params`,
// `TransverseMercatorTemplate::projection_params` and
// `SpaceViewTemplate::scan_grid`. The other families reach a consumer through
// [`GridGeometry`], whose payload types are core's own API rather than this
// crate's, and are not re-exported here.
pub use fieldglass_core::{
    FieldglassError, GeostationaryParams, GridGeometry, LambertAzimuthalParams,
    TransverseMercatorParams,
};
pub use gds::{
    GDS_SECTION_NUMBER, GaussianTemplate, GridDefinitionSection, GridTemplate, LambertTemplate,
    LatLonTemplate, SCAN_ALTERNATE_ROWS, SCAN_J_CONSECUTIVE, SpaceViewTemplate,
    SphericalHarmonicTemplate, parse_grid_definition, undo_alternate_reduced_rows,
    undo_alternate_rows,
};
pub use ids::{IDS_MIN_LEN, IDS_SECTION_NUMBER, IdentificationSection, parse_identification};
pub use is::{
    END_SECTION_LEN, GRIB2_EDITION, INDICATOR_SECTION_LEN, IndicatorSection, parse_indicator,
};
pub use lus::{LUS_SECTION_NUMBER, LocalUseSection, parse_local_use};
pub use pds::{
    FixedSurface, HorizontalProductCommon, PDS_SECTION_NUMBER, ProductDefinitionSection,
    ProductTemplate, StatisticalProcessing, Template40, Template48, Template411, TimeRangeSpec,
    parse_product_definition,
};
pub use reader::{Grib2Message, Grib2Reader, MatrixField};
pub use section::{SECTION_HEADER_LEN, SectionHeader, parse_section_header};
pub use spectral::{SpectralCoefficients, decode_spectral_complex, decode_spectral_simple};
pub use tables::{
    Originator, lookup_data_type, lookup_discipline, lookup_earth_shape, lookup_ensemble_type,
    lookup_fixed_surface, lookup_generating_process_type, lookup_grid_template, lookup_parameter,
    lookup_production_status, lookup_reference_time_significance, lookup_statistical_process,
    lookup_time_range_unit,
};
/// Originating centres come from the generated CCT table (#440), not the
/// hand-written `tables` module.
pub use tables_cct::lookup_centre;
pub use tables_local::{LOCAL_TABLE_CENTRES, LocalTableCentre};
