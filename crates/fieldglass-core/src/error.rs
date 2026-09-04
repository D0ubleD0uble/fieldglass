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
    /// The call does not apply to the layout the reader opened — asking a
    /// classic NetCDF file for its NetCDF-4 / HDF5 metadata, say. Nothing
    /// failed to decode; the question was put to the wrong file, which is why
    /// this is not [`Self::Parse`]. A caller that matched on the layout first
    /// cannot produce it.
    #[error("not applicable to this file's layout: {0}")]
    WrongLayout(String),
    /// An index outside the collection it addresses.
    #[error("index out of range")]
    OutOfRange,
}
