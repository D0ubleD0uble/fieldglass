#[derive(Debug, thiserror::Error)]
pub enum FieldglassError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("parse error: {0}")]
    Parse(String),
    #[error("invalid magic bytes")]
    InvalidMagic,
    #[error("unsupported format")]
    UnsupportedFormat,
    #[error("unsupported section")]
    UnsupportedSection,
    #[error("index out of range")]
    OutOfRange,
}
