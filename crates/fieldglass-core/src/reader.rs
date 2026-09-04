use crate::metadata::{GridDefinition, Metadata};

/// Implemented by each format crate's top-level reader
pub trait FormatReader {
    /// The format's name, for display.
    fn format_name() -> String;
    /// How many messages the file holds.
    fn message_count() -> i32;
    /// One message's metadata, by index.
    fn message(index: i32) -> Metadata;
}

/// Implemented by each format's message type
pub trait DataMessage {
    /// The message's metadata.
    fn metadata() -> Metadata;
    /// The message's grid geometry.
    fn grid() -> GridDefinition;
    /// Decode the actual grid values — lazy, only called on demand
    fn decode_field() -> Vec<f64>;
}
