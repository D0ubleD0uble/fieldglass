//! Reads GRIB edition 2 weather files in pure Rust: every message's metadata,
//! its values for every registered packing, and where its grid sits on the
//! Earth.
//!
//! A GRIB2 message describes itself in nine numbered sections (0 to 8, the
//! last being the end marker), and this crate reads all of them for the
//! metadata. It decodes the values of every packing in WMO's register, with no
//! external libraries and no build flags. The GRIB2 packing modes table shows
//! each one and how it's checked:
//! <https://github.com/D0ubleD0uble/fieldglass#grib2-packing-modes>
//!
//! Most packings store one value per grid point, and they all decode through
//! [`Grib2Reader::decode_message_values`]. The rest have their own entry
//! points. A spherical-harmonic, or spectral, field stores waves around the
//! globe instead of grid values, and decodes to its coefficients through
//! [`Grib2Reader::decode_spectral_message`].
//! [`Grib2Reader::synthesize_spectral_global`] turns it back into the shared
//! latitude and longitude grid of [`fieldglass_core::global_grid`], and
//! [`Grib2Reader::synthesize_spectral_message`] takes a grid of your own. A
//! bi-Fourier field, the limited-area spectral form, decodes through
//! [`Grib2Reader::decode_bifourier_message`], and a field with a small matrix
//! at every point decodes through [`Grib2Reader::decode_matrix_message`].
//!
//! The error type every `Result` here returns ([`FieldglassError`]) and the
//! grid type [`GridGeometry`] converts into are re-exported from
//! `fieldglass-core`, so this crate can be the only Fieldglass dependency in
//! your manifest. So are the synthesis grid [`GlobalGrid`] and the shared WMO
//! sub-center lookup (from [`tables_cct`]).
//!
//! Those entry points all return the field as the message stores it, and two
//! layouts don't store the rectangle you'd expect. A reduced Gaussian grid
//! stores `sum(PL)` values, not the `Ni × Nj` its
//! [`GridDefinitionSection::dimensions`] reports, and a `j`-consecutive grid
//! (§3 Flag Table 3.4 bit 3) stores columns rather than rows.
//! [`Grib2Reader::decode_message_raster`] hands back the rectangle either way,
//! with [`GridDefinitionSection::raster_bounds`] as its extent, so the caller
//! never has to widen or turn the grid.
//!
//! [`Grib2Reader::decode_message_raster_with`] is the third of that family,
//! and the one that can answer coarsely. JPEG 2000 stores a field as a stack
//! of levels, each half the size of the one below, so a zoomed-out display can
//! have `ceil(Ni / 2^r) x ceil(Nj / 2^r)` points for a fraction of the work. It
//! hands back a [`ReducedRaster`], whose grid is derived rather than the
//! message's own. Its values are averages no GRIB message contains, so they're
//! for drawing, never for measuring. See [`reduced_resolution`].

#![forbid(unsafe_code)]
pub mod bms;
pub mod display;
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
pub mod reduced_resolution;
pub mod section;
pub mod spectral;
pub mod tables;
pub mod tables_cct;
pub mod tables_constituent;
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
pub use display::{forecast_display, forecast_hours, level_type_str, level_value_str};
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
// `StoredRuns` is here because `undo_second_order_boustrophedonic` takes the
// grid's stored run layout — uniform for a rectangle, ragged for a reduced
// grid (#605).
// `CornerPair` is here because `GridDefinitionSection::bounds` and
// `raster_bounds` hand one back, so reading a grid's declared corners means
// naming it (#553).
// `GlobalGrid` is the grid `synthesize_spectral_global` hands back beside the
// synthesized field, so reading either means naming it (#546);
// `SynthesizedField` is the pair `synthesize_message_global` hands back, for the
// same reason (#580). `SpectralTruncation` is what `synthesis_truncation`
// answers, the label a band-limited map carries (#637). `ByteSource` is what `Grib2Reader` reads through and
// `ByteRange` is how a message records its sections, so a caller bringing its
// own source, or reading a message's ranges, names both (#697).
// `GridFingerprint` is what `GridDefinitionSection::fingerprint` holds (#962).
pub use fieldglass_core::sht::SpectralTruncation;
pub use fieldglass_core::{
    ByteRange, ByteSource, CornerPair, FieldglassError, GeostationaryParams, GlobalGrid,
    GridFingerprint, GridGeometry, LambertAzimuthalParams, StoredRuns, SynthesizedField,
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
    ConstituentTemplate, EnsembleBlock, FixedSurface, HorizontalProductCommon, PDS_SECTION_NUMBER,
    ProductDefinitionSection, ProductTemplate, ScaledLimit, SharedLayoutTemplate,
    StatisticalProcessing, Template40, Template48, Template411, TimeRangeSpec, TypedInterval,
    parse_product_definition,
};
pub use reader::{Grib2Message, Grib2Reader, MatrixField};
pub use reduced_resolution::{DecodeOptions, ReducedRaster};
pub use section::{SECTION_HEADER_LEN, SectionHeader, parse_section_header};
pub use spectral::{SpectralCoefficients, decode_spectral_complex, decode_spectral_simple};
pub use tables::{
    Originator, lookup_data_type, lookup_discipline, lookup_earth_shape, lookup_ensemble_type,
    lookup_fixed_surface, lookup_generating_process_type, lookup_grid_template, lookup_parameter,
    lookup_production_status, lookup_reference_time_significance, lookup_statistical_process,
    lookup_time_range_unit, unresolved_parameter,
};
/// Originating centres come from the generated CCT table (#440), not the
/// hand-written `tables` module.
pub use tables_cct::lookup_centre;
pub use tables_local::{LOCAL_TABLE_CENTRES, LocalTableCentre};

/// Compiles and runs the README's usage snippet as a doc test, so the crate's
/// crates.io front page cannot drift from the API it describes (#539).
///
/// `#[cfg(doctest)]` is what keeps this out of every other build: rustdoc sets
/// it when it collects doc tests and nothing else does, so the type itself is
/// never compiled into the library.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeSnippet;
