#[cfg(feature = "fs")]
use std::fs::File;
#[cfg(feature = "fs")]
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

    /// Write `bytes` to a uniquely named file under the temp dir and return the
    /// path. Named for the calling test so a leftover file says where it came
    /// from; the process id keeps concurrent `cargo test` runs apart.
    ///
    /// Opened with `create_new` rather than `File::create`, which is the part
    /// that matters: the temp directory is shared between users on most
    /// systems and the name here is predictable, so a plain create would follow
    /// a symlink planted at that path and write these bytes through it.
    /// `create_new` refuses an existing path — symlink included — so the worst
    /// case is a failed test rather than a clobbered file.
    fn temp_file(name: &str, bytes: &[u8]) -> String {
        // nosemgrep: rust.lang.security.temp-dir.temp-dir
        let dir = std::env::temp_dir();
        let path = dir.join(format!("fieldglass-detect-{}-{name}", std::process::id()));
        // Unlink any stale entry first — a crashed earlier run can leave one
        // behind, and a recycled pid would otherwise collide with it. Unlinking
        // a symlink removes the link and never follows it, so this cannot write
        // through one; and if something re-appears in the window before the
        // open below, create_new refuses rather than following it.
        let _ = std::fs::remove_file(&path);
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .expect("create temp file");
        f.write_all(bytes).expect("write temp file");
        path.to_str().expect("temp path is utf-8").to_string()
    }

    #[test]
    fn magic_bytes_beat_a_lying_extension() {
        // The discriminating case for the `fs` feature: without the file read,
        // this would answer Grib1 from the extension alone.
        let path = temp_file("liar.grib1", b"GRIB\0\0\0\x02");
        assert!(matches!(detect_format(&path), Format::Grib2));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn extension_is_the_fallback_when_the_bytes_say_nothing() {
        let path = temp_file("garbage.nc", b"not a data file");
        assert!(matches!(detect_format(&path), Format::NetCdf));
        let _ = std::fs::remove_file(&path);
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
