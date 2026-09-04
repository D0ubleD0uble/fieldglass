/// Everything the parsing and decode surface can fail with.
#[derive(Debug, thiserror::Error)]
pub enum FieldglassError {
    /// Reading the file failed.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// The bytes are the right format but a section did not parse.
    #[error("parse error: {0}")]
    Parse(String),
    /// The leading bytes are not the magic this reader expects.
    #[error("invalid magic bytes")]
    InvalidMagic,
    /// The container is not one this build can open.
    #[error("unsupported format")]
    UnsupportedFormat,
    /// The section parsed, but names a template or packing this build does not
    /// decode.
    #[error("unsupported section: {0}")]
    UnsupportedSection(String),
    /// An index outside the collection it addresses.
    #[error("index out of range")]
    OutOfRange,
}
