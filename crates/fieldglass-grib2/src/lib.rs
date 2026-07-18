//! GRIB edition 2 reader.
//!
//! Current scope: full §0–§7 parsing for the message metadata, plus value
//! decoding for **simple packing** (DRS template 5.0), **complex packing**
//! (5.2 / 5.3), **IEEE floating point** (5.4), **JPEG 2000 packing** (5.40),
//! **PNG packing** (5.41), and **CCSDS / AEC packing** (5.42) — every §5
//! template eccodes can encode. Templates outside that set parse to the section
//! level but `decode_message_values` returns
//! [`fieldglass_core::FieldglassError::UnsupportedSection`].

#![forbid(unsafe_code)]

pub mod bms;
pub mod drs;
pub mod ds;
pub mod gds;
pub mod ids;
pub mod is;
pub mod lus;
pub mod pds;
pub mod reader;
pub mod section;
pub mod tables;

pub use bms::{
    BMS_INDICATOR_NONE, BMS_INDICATOR_PRESENT, BMS_INDICATOR_PREVIOUS, BMS_SECTION_NUMBER,
    BitMapSection, parse_bit_map,
};
pub use drs::{
    DRS_SECTION_NUMBER, DataRepresentationSection, DataRepresentationTemplate, IeeePackingTemplate,
    SimplePackingTemplate, parse_data_representation,
};
pub use ds::{DS_SECTION_NUMBER, decode_values};
pub use gds::{
    GDS_SECTION_NUMBER, GaussianTemplate, GridDefinitionSection, GridTemplate, LambertTemplate,
    LatLonTemplate, SCAN_ALTERNATE_ROWS, SCAN_J_CONSECUTIVE, SpaceViewTemplate,
    parse_grid_definition, undo_alternate_rows,
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
pub use reader::{Grib2Message, Grib2Reader};
pub use section::{SECTION_HEADER_LEN, SectionHeader, parse_section_header};
pub use tables::{
    lookup_centre, lookup_data_type, lookup_discipline, lookup_earth_shape, lookup_ensemble_type,
    lookup_fixed_surface, lookup_generating_process_type, lookup_grid_template, lookup_parameter,
    lookup_production_status, lookup_reference_time_significance, lookup_statistical_process,
    lookup_time_range_unit,
};

use fieldglass_core::{DataMessage, FormatReader, GridDefinition, Level, Metadata, Parameter};

/// Static `FormatReader` implementation. The trait shape in `fieldglass-core`
/// uses self-less signatures pending a refactor; the impl is intentionally
/// minimal until that lands. Construct a reader via [`Grib2Reader::from_bytes`]
/// for real usage.
impl FormatReader for Grib2Reader {
    fn format_name() -> String {
        "grib2".to_string()
    }

    fn message_count() -> i32 {
        0
    }

    fn message(_index: i32) -> Metadata {
        Metadata {
            parameter: Parameter {
                name: String::new(),
                abbreviation: String::new(),
                units: String::new(),
                id: 0,
            },
            level: Level {
                level_type: String::new(),
                value: 0.0,
                units: String::new(),
            },
            reference_time: String::new(),
            forecast_hours: 0,
            originating_centre: String::new(),
            grid: None,
        }
    }
}

impl DataMessage for Grib2Message {
    fn metadata() -> Metadata {
        Metadata {
            parameter: Parameter {
                name: String::new(),
                abbreviation: String::new(),
                units: String::new(),
                id: 0,
            },
            level: Level {
                level_type: String::new(),
                value: 0.0,
                units: String::new(),
            },
            reference_time: String::new(),
            forecast_hours: 0,
            originating_centre: String::new(),
            grid: None,
        }
    }

    fn grid() -> GridDefinition {
        GridDefinition {
            grid_type: String::new(),
            ni: 0,
            nj: 0,
            lat_first: 0.0,
            lon_first: 0.0,
            lat_last: 0.0,
            lon_last: 0.0,
            di: 0.0,
            dj: 0.0,
        }
    }

    fn decode_field() -> Vec<f64> {
        Vec::new()
    }
}

#[cfg(test)]
mod trait_impl_smoke_tests {
    //! Smoke coverage for the pre-refactor stub trait impls above.
    //!
    //! `FormatReader` / `DataMessage` in `fieldglass-core` still use
    //! self-less signatures (a tracked refactor); these tests pin the
    //! stub contract so anything other than "named, returns defaults,
    //! does not panic" surfaces immediately when the trait shape changes.
    use super::*;

    #[test]
    fn format_reader_stub_returns_default_shape() {
        assert_eq!(<Grib2Reader as FormatReader>::format_name(), "grib2");
        assert_eq!(<Grib2Reader as FormatReader>::message_count(), 0);
        let meta = <Grib2Reader as FormatReader>::message(0);
        assert_eq!(meta.parameter.id, 0);
        assert!(meta.parameter.name.is_empty());
        assert!(meta.level.level_type.is_empty());
        assert_eq!(meta.forecast_hours, 0);
        assert!(meta.grid.is_none());
    }

    #[test]
    fn data_message_stub_returns_default_shape() {
        let meta = <Grib2Message as DataMessage>::metadata();
        assert!(meta.parameter.name.is_empty());
        assert_eq!(meta.parameter.id, 0);
        let grid = <Grib2Message as DataMessage>::grid();
        assert!(grid.grid_type.is_empty());
        assert_eq!(grid.ni, 0);
        assert_eq!(grid.nj, 0);
        assert_eq!(
            <Grib2Message as DataMessage>::decode_field(),
            Vec::<f64>::new()
        );
    }
}
