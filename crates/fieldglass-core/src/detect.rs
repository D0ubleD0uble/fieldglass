use std::fs::File;
use std::io::Read;

pub enum Format {
    Grib1,
    Grib2,
    NetCdf,
    Unknown,
}

/// Detect format from the first bytes of a file.
/// Returns `Unknown` if the bytes don't match any known magic sequence.
pub fn detect_from_bytes(bytes: &[u8]) -> Format {
    // GRIB: first 4 bytes are ASCII "GRIB"; edition is at byte offset 7.
    if bytes.len() >= 8 && &bytes[0..4] == b"GRIB" {
        return match bytes[7] {
            1 => Format::Grib1,
            2 => Format::Grib2,
            _ => Format::Unknown,
        };
    }
    // NetCDF classic / 64-bit offset / CDF-5: "CDF\x01", "CDF\x02", "CDF\x05"
    if bytes.len() >= 4 && &bytes[0..3] == b"CDF" && matches!(bytes[3], 1 | 2 | 5) {
        return Format::NetCdf;
    }
    // NetCDF-4 / HDF5: "\x89HDF\r\n\x1a\n"
    if bytes.len() >= 8 && &bytes[0..8] == b"\x89HDF\r\n\x1a\n" {
        return Format::NetCdf;
    }
    Format::Unknown
}

/// Detect format from a file path.
/// Tries magic bytes first; falls back to file extension if the file cannot be
/// read or the bytes don't match a known signature.
pub fn detect_format(file_path: &str) -> Format {
    if let Ok(mut f) = File::open(file_path) {
        let mut buf = [0u8; 8];
        if let Ok(n) = f.read(&mut buf) {
            match detect_from_bytes(&buf[..n]) {
                Format::Unknown => {}
                fmt => return fmt,
            }
        }
    }
    detect_format_from_extension(file_path)
}

fn detect_format_from_extension(file_path: &str) -> Format {
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
