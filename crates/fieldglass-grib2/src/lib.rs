//! GRIB edition 2 reader.
//!
//! Phase 4.0 scope: Indicator Section parsing and message enumeration only.
//! Sections 1–7 (IDS / LUS / GDS / PDS / DRS / BMS / DS) and per-section
//! decoding are tracked under separate issues.

pub mod is;
pub mod reader;
pub mod tables;

pub use is::{
    END_SECTION_LEN, GRIB2_EDITION, INDICATOR_SECTION_LEN, IndicatorSection, parse_indicator,
};
pub use reader::{Grib2Message, Grib2Reader};
pub use tables::lookup_discipline;

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
