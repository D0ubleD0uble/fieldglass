#[cfg(feature = "fs")]
use std::fs::File;
#[cfg(feature = "fs")]
use std::io::Read;

use crate::bytes::{ByteSource, read_up_to};

/// The HDF5 signature, `\x89HDF\r\n\x1a\n`, which opens a NetCDF-4 file's
/// superblock.
pub const HDF5_SIGNATURE: [u8; 8] = [0x89, b'H', b'D', b'F', b'\r', b'\n', 0x1a, b'\n'];

/// Where [`find_hdf5_signature`] looks: byte 0, then 512 and each power of two
/// after it, through 16384.
///
/// An HDF5 file may begin with a userblock, a power of two from 512 bytes up,
/// and the superblock then follows it (#936). The specification places no top
/// on its size and libhdf5 searches to the end of the file; a bounded list
/// keeps detection a handful of small reads over any source. A userblock larger
/// than 16 KiB is rare enough that such a file reads as not HDF5.
pub const HDF5_SIGNATURE_OFFSETS: [u64; 7] = [0, 512, 1024, 2048, 4096, 8192, 16384];

/// How many leading bytes [`detect_from_bytes`] can look at: through the HDF5
/// signature at the last offset [`HDF5_SIGNATURE_OFFSETS`] names. A host that
/// detects from a prefix of the file passes at least this much, or a NetCDF-4
/// file with a userblock reads as unknown.
pub const DETECT_WINDOW: usize = 16384 + HDF5_SIGNATURE.len();

/// The offset of the HDF5 signature in `source`, if it is at one of
/// [`HDF5_SIGNATURE_OFFSETS`].
///
/// The offset is the superblock's: the userblock's size, and the base every
/// other address in the file is relative to. A source that ends, or fails to
/// read, before the next offset has no signature past it.
pub fn find_hdf5_signature<S: ByteSource + ?Sized>(source: &S) -> Option<u64> {
    for off in HDF5_SIGNATURE_OFFSETS {
        let at = read_up_to(source, off, HDF5_SIGNATURE.len()).ok()?;
        if at.len() < HDF5_SIGNATURE.len() {
            return None;
        }
        if at[..] == HDF5_SIGNATURE {
            return Some(off);
        }
    }
    None
}

/// What the leading bytes of a file turned out to be.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Format {
    /// WMO FM 92 GRIB edition 1.
    Grib1,
    /// WMO FM 92 GRIB edition 2.
    Grib2,
    /// NetCDF, either the classic layout or NetCDF-4 / HDF5.
    NetCdf,
    /// None of the above — the caller has no reader for these bytes.
    Unknown,
}

/// Detect format from the first bytes of a file.
/// Returns `Unknown` if the bytes don't match any known magic sequence.
///
/// GRIB and classic NetCDF are told by their first eight bytes. An HDF5 file
/// may put its signature after a userblock, as far as [`DETECT_WINDOW`] in, so
/// a prefix shorter than that misses one that does.
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
    // NetCDF-4 / HDF5: "\x89HDF\r\n\x1a\n", at byte 0 or after a userblock.
    if find_hdf5_signature(bytes).is_some() {
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
    if let Ok(f) = File::open(file_path) {
        // `take` + `read_to_end` rather than one `read`, which may return
        // fewer bytes than the file holds.
        let mut buf = Vec::with_capacity(DETECT_WINDOW);
        if f.take(DETECT_WINDOW as u64).read_to_end(&mut buf).is_ok() {
            match detect_from_bytes(&buf) {
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

    /// `len` zero bytes with the HDF5 signature at `at`.
    fn signature_at(at: usize, len: usize) -> Vec<u8> {
        let mut bytes = vec![0u8; len];
        bytes[at..at + HDF5_SIGNATURE.len()].copy_from_slice(&HDF5_SIGNATURE);
        bytes
    }

    #[test]
    fn an_hdf5_signature_after_a_userblock_is_netcdf() {
        for at in HDF5_SIGNATURE_OFFSETS {
            let at = at as usize;
            let bytes = signature_at(at, at + 64);
            assert_eq!(find_hdf5_signature(&bytes[..]), Some(at as u64));
            assert!(matches!(detect_from_bytes(&bytes), Format::NetCdf), "{at}");
        }
        // The last offset is inside the window a host is told to pass.
        assert_eq!(DETECT_WINDOW, 16384 + 8);
        let bytes = signature_at(16384, DETECT_WINDOW);
        assert!(matches!(detect_from_bytes(&bytes), Format::NetCdf));
    }

    #[test]
    fn an_hdf5_signature_off_the_searched_offsets_is_unknown() {
        // Past the last offset searched, between two of them, and cut off by
        // the end of the buffer.
        for (at, len) in [(32768, 32768 + 64), (256, 1024), (100, 200)] {
            let bytes = signature_at(at, len);
            assert_eq!(find_hdf5_signature(&bytes[..]), None, "{at}");
            assert!(matches!(detect_from_bytes(&bytes), Format::Unknown), "{at}");
        }
        let bytes = signature_at(512, 1024);
        assert!(matches!(detect_from_bytes(&bytes[..515]), Format::Unknown));
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
    fn a_file_with_a_userblock_is_read_far_enough_to_find_its_signature() {
        // Named `.bin` so only the bytes can say NetCDF.
        let mut bytes = vec![0u8; 2048 + 64];
        bytes[2048..2056].copy_from_slice(&HDF5_SIGNATURE);
        let file = temp_file(".bin", &bytes);
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
