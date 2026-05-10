use fieldglass_core::FieldglassError;

pub struct Grib2Reader {
    pub file_path: String,
}

/// Returns `UnsupportedFormat` until GRIB2 parsing lands. Public callers
/// (the napi worker) must not panic the host process — keep this as a
/// `Result` rather than `todo!()`.
pub fn open(_file_path: String) -> Result<Grib2Reader, FieldglassError> {
    Err(FieldglassError::UnsupportedFormat)
}
