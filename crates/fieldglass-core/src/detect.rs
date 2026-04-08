pub enum Format {
    Grib1,
    Grib2,
    NetCdf,
    Unknown,
}

/// Detect format from file extension.
/// Magic-byte detection will be added once IS/PDS parsing is implemented.
pub fn detect_format(file_path: &str) -> Format {
    let lower = file_path.to_lowercase();
    if lower.ends_with(".grb")
        || lower.ends_with(".grib")
        || lower.ends_with(".grib1")
        || lower.ends_with(".grb1")
    {
        return Format::Grib1;
    }
    if lower.ends_with(".grb2") || lower.ends_with(".grib2") {
        return Format::Grib2;
    }
    if lower.ends_with(".nc") || lower.ends_with(".nc4") || lower.ends_with(".netcdf") {
        return Format::NetCdf;
    }
    Format::Unknown
}
