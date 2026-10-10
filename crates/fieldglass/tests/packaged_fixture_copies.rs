//! Every fixture copied into a format crate so that its published `.crate`
//! carries it (#926) is still byte-identical to the original it was copied
//! from.
//!
//! The copies exist because a published crate's tests may read only files
//! inside that crate, and the originals live in a sibling crate or in a
//! `fuzz/` directory, which is a separate package. Nothing regenerates the
//! copies, so a change to an original would leave its copy testing something
//! else without anyone noticing. This test is that notice.
//!
//! It lives here because the umbrella's integration tests are left out of its
//! package (`exclude` in its manifest): this one reads across crates, which is
//! exactly what a packaged test cannot do. Paths are relative to this crate's
//! directory, which the wasm32-wasip1 run in CI preopens along with its parent.

/// Each copy, and the original it must match, both from this crate's
/// directory.
const COPIES: [(&str, &str); 5] = [
    (
        "../fieldglass-grib1/tests/other_edition/regular_latlon_surface.grib2",
        "../fieldglass-grib2/tests/fixtures/regular_latlon_surface.grib2",
    ),
    (
        "../fieldglass-grib2/tests/other_edition/ieee32_cmc_wind.grib1",
        "../fieldglass-grib1/tests/fixtures/ieee32_cmc_wind.grib1",
    ),
    (
        "../fieldglass-grib2/tests/fuzz_seeds/jpeg2000_codestream_8192x8192_on_1x1.grib2",
        "../fieldglass-grib2/fuzz/corpus/decode/jpeg2000_codestream_8192x8192_on_1x1.grib2",
    ),
    (
        "../fieldglass-grib2/tests/fuzz_seeds/constant_field_bifourier_ellipse_wide.grib2",
        "../fieldglass-grib2/fuzz/corpus/decode/constant_field_bifourier_ellipse_wide.grib2",
    ),
    (
        "../fieldglass-netcdf/tests/fuzz_seeds/oom_large_fill_dataset.h5",
        "../fieldglass-netcdf/fuzz/corpus/parse/oom_large_fill_dataset.h5",
    ),
];

fn read(path: &str) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|e| panic!("read {path}: {e}"))
}

#[test]
fn every_packaged_copy_matches_its_original() {
    for (copy, original) in COPIES {
        let (copy_bytes, original_bytes) = (read(copy), read(original));
        assert!(!original_bytes.is_empty(), "{original} is empty");
        assert!(
            copy_bytes == original_bytes,
            "{copy} ({} bytes) has drifted from {original} ({} bytes); \
             copy the original over it again",
            copy_bytes.len(),
            original_bytes.len()
        );
    }
}
