pub enum Format {
    Grib1,
    Grib2,
    NetCdf,
    Unknown,
}

/// Detect format from file magic bytes and extension
pub fn detect_format(file_path: String) -> Format {
    todo!("Implement detect_format")
}
