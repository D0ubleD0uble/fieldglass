//! Where the unit tests stand when they borrow a format crate's fixtures.
//!
//! A few unit tests here read a fixture from `fieldglass-grib1`, `-grib2` or
//! `-netcdf`, because what they check is private to this crate (the placement
//! memo's keying, the planar geolocation helpers) and only real bytes make it
//! worth checking. Those files belong to other packages, so the published
//! `fieldglass` crate cannot carry them (#926). Such a test asks
//! [`outside_workspace`] first and returns early when it is true.
//!
//! The question is answered by a file, not a guess. `cargo package` writes the
//! original manifest into every `.crate` as `Cargo.toml.orig`, and nothing in
//! the repository has one, so in the workspace these tests always run: the read
//! either succeeds or panics, and never skips.

/// Whether the tests are running from an unpacked `.crate` rather than from
/// the workspace. Relative, like every fixture path here: cargo runs a test
/// from its package's directory, and the wasm32-wasip1 run in CI preopens only
/// that directory and its parent.
pub(crate) fn outside_workspace() -> bool {
    let packaged = std::path::Path::new("Cargo.toml.orig").is_file();
    if packaged {
        eprintln!("skipped: this test reads a sibling crate's fixture, which the .crate lacks");
    }
    packaged
}

/// The bytes of a sibling crate's fixture, by its path from this crate's
/// directory. Only called after [`outside_workspace`] has said no.
pub(crate) fn sibling(path: &str) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|e| panic!("read {path}: {e}"))
}
