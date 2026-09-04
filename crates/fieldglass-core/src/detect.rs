#[cfg(feature = "fs")]
use std::fs::File;
#[cfg(feature = "fs")]
use std::io::Read;

#[derive(Debug)]
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
///
/// Requires the `fs` feature (default). Hosts without a filesystem should call
/// [`detect_from_bytes`] on a buffer they fetched themselves: on a target where
/// `File::open` always fails this would silently degrade to guessing from the
/// extension.
#[cfg(feature = "fs")]
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

#[cfg(feature = "fs")]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grib_edition_selects_the_format() {
        assert!(matches!(
            detect_from_bytes(b"GRIB\0\0\0\x01"),
            Format::Grib1
        ));
        assert!(matches!(
            detect_from_bytes(b"GRIB\0\0\0\x02"),
            Format::Grib2
        ));
        // Editions 0 and 3 are not registered; neither is a GRIB we can read.
        assert!(matches!(
            detect_from_bytes(b"GRIB\0\0\0\x03"),
            Format::Unknown
        ));
    }

    #[test]
    fn netcdf_magics_cover_every_container() {
        for magic in [b"CDF\x01", b"CDF\x02", b"CDF\x05"] {
            assert!(matches!(detect_from_bytes(magic), Format::NetCdf));
        }
        assert!(matches!(
            detect_from_bytes(b"\x89HDF\r\n\x1a\n"),
            Format::NetCdf
        ));
        // CDF-3 and CDF-4 were never assigned; netCDF-4 is HDF5, not a CDF.
        assert!(matches!(detect_from_bytes(b"CDF\x03"), Format::Unknown));
        assert!(matches!(detect_from_bytes(b"CDF\x04"), Format::Unknown));
    }

    #[test]
    fn short_and_unrecognised_buffers_are_unknown() {
        assert!(matches!(detect_from_bytes(b""), Format::Unknown));
        // "GRIB" alone: the edition byte at offset 7 is not there to read.
        assert!(matches!(detect_from_bytes(b"GRIB"), Format::Unknown));
        assert!(matches!(detect_from_bytes(b"CD"), Format::Unknown));
        assert!(matches!(
            detect_from_bytes(b"not a data file"),
            Format::Unknown
        ));
    }
}

#[cfg(all(test, feature = "fs"))]
mod fs_tests {
    use super::*;
    use std::io::Write;

    /// A temp file holding `bytes` and ending in `suffix`.
    ///
    /// The suffix is the point: these tests are about how the extension and the
    /// magic bytes interact, so the name has to end in a real one.
    ///
    /// `tempfile` rather than a name of our own under `std::env::temp_dir()`.
    /// That directory is shared between users, so a predictable name is a
    /// liability: another user can plant a symlink at the path we are about to
    /// write, and an ordinary create follows it. This gives a random name,
    /// `O_EXCL` creation and 0600 — nothing to guess and nothing to race.
    ///
    /// Returned by value, not as a path: the file lives exactly as long as the
    /// handle and is removed on drop, so a test cannot leak one into `/tmp` and
    /// no test needs to remember to clean up.
    fn temp_file(suffix: &str, bytes: &[u8]) -> tempfile::NamedTempFile {
        let mut file = tempfile::Builder::new()
            .prefix("fieldglass-detect-")
            .suffix(suffix)
            .tempfile()
            .expect("create temp file");
        file.write_all(bytes).expect("write temp file");
        file.flush().expect("flush temp file");
        file
    }

    /// The path of `file` as the `&str` `detect_format` takes.
    fn path_of(file: &tempfile::NamedTempFile) -> &str {
        file.path().to_str().expect("temp path is utf-8")
    }

    #[test]
    fn magic_bytes_beat_a_lying_extension() {
        // The discriminating case for the `fs` feature: without the file read,
        // this would answer Grib1 from the extension alone.
        let file = temp_file(".grib1", b"GRIB\0\0\0\x02");
        assert!(matches!(detect_format(path_of(&file)), Format::Grib2));
    }

    #[test]
    fn extension_is_the_fallback_when_the_bytes_say_nothing() {
        let file = temp_file(".nc", b"not a data file");
        assert!(matches!(detect_format(path_of(&file)), Format::NetCdf));
    }

    #[test]
    fn an_unreadable_path_still_answers_from_its_extension() {
        assert!(matches!(
            detect_format("/nonexistent/fieldglass/x.GRB2"),
            Format::Grib2
        ));
        assert!(matches!(
            detect_format("/nonexistent/fieldglass/x.grb"),
            Format::Grib1
        ));
        assert!(matches!(
            detect_format("/nonexistent/fieldglass/x.tar"),
            Format::Unknown
        ));
    }
}
