#![forbid(unsafe_code)]

pub mod bits;
pub mod detect;
pub mod error;
pub mod metadata;
pub mod reader;

pub use detect::Format;
pub use detect::detect_format;
pub use detect::detect_from_bytes;
pub use error::FieldglassError;
pub use metadata::GridDefinition;
pub use metadata::Level;
pub use metadata::Metadata;
pub use metadata::Parameter;
pub use reader::DataMessage;
pub use reader::FormatReader;
