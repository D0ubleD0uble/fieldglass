use crate::metadata::{GridDefinition, Metadata};

/// Implemented by each format crate's top-level reader
pub trait FormatReader {
    fn format_name() -> String;
    fn message_count() -> i32;
    fn message(index: i32) -> Metadata;
}

/// Implemented by each format's message type
pub trait DataMessage {
    fn metadata() -> Metadata;
    fn grid() -> GridDefinition;
    /// Decode the actual grid values — lazy, only called on demand
    fn decode_field() -> Vec<f64>;
}
