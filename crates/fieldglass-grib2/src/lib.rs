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
