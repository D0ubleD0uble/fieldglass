//! The value traits this crate's public types promise, pinned by use (#556).
//!
//! `missing_debug_implementations` in `[workspace.lints]` holds `Debug` for the
//! whole workspace. Nothing holds the rest: rustc has no lint for a missing
//! `Clone` or `PartialEq`, and `missing_copy_implementations` would argue with
//! the one place here that declines `Copy` on purpose. So a dropped derive is
//! caught by this file or not at all.
//!
//! The bounds below are the assertion — a removed derive is a compile error in
//! the test target, not a runtime failure — and the behavioural cases beneath
//! them exist because a bound proves the trait is implemented and says nothing
//! about it being right.

use std::collections::HashMap;

use fieldglass_core::detect::Format;

fn assert_copy<T: Copy>() {}
fn assert_clone<T: Clone>() {}
fn assert_partial_eq<T: PartialEq>() {}
fn assert_eq_hash<T: Eq + std::hash::Hash>() {}

#[test]
fn detect_format_is_a_plain_value() {
    assert_copy::<Format>();
    assert_eq_hash::<Format>();
}

#[test]
fn the_projectors_are_comparable_values() {
    use fieldglass_core::projection::{
        GeostationaryProjector, LambertAzimuthalProjector, LambertProjector, PolarStereoProjector,
        RotatedLatLonProjector, TransverseMercatorProjector,
    };
    // Small parameter bundles, so they copy.
    assert_copy::<GeostationaryProjector>();
    assert_copy::<LambertProjector>();
    assert_copy::<LambertAzimuthalProjector>();
    assert_copy::<PolarStereoProjector>();
    assert_copy::<RotatedLatLonProjector>();
    assert_copy::<TransverseMercatorProjector>();
    assert_partial_eq::<LambertProjector>();

    // `GaussianProjector` is the exception: it carries the computed latitude
    // row, so it clones rather than copies. Asserting the *absence* of `Copy`
    // is not something a bound can do, so this states the half that holds.
    assert_clone::<fieldglass_core::projection::GaussianProjector>();
    assert_partial_eq::<fieldglass_core::projection::GaussianProjector>();
}

#[test]
fn a_bit_reader_clones_its_position_rather_than_copying_it() {
    use fieldglass_core::bits::BitReader;
    assert_clone::<BitReader<'_>>();

    // The reason it is `Clone` and not `Copy`: a saved cursor has to be an
    // explicit act, and reading from the clone must not move the original.
    let bytes = [0b1010_1100u8, 0b0011_0101];
    let mut reader = BitReader::new(&bytes);
    let _ = reader.read_bits(4);
    let mut saved = reader.clone();
    // Unwrapped rather than compared as `Result`: `FieldglassError` declines
    // `PartialEq` because its `Io` variant holds a `std::io::Error`, so no
    // `Result` out of this crate is `assert_eq!`-able. That is a real cost of
    // the decision, and forced by the payload rather than chosen.
    let from_original = reader.read_bits(4).expect("four more bits");
    let from_clone = saved.read_bits(4).expect("four more bits");
    assert_eq!(
        from_original, from_clone,
        "a clone resumes from where the original stood"
    );
}

#[test]
fn a_format_can_key_a_map() {
    // What `Eq + Hash` is actually for: a host grouping by container.
    let mut seen: HashMap<Format, u32> = HashMap::new();
    *seen.entry(Format::Grib2).or_default() += 1;
    *seen.entry(Format::Grib2).or_default() += 1;
    *seen.entry(Format::NetCdf).or_default() += 1;
    assert_eq!(seen[&Format::Grib2], 2);
    assert_eq!(seen[&Format::NetCdf], 1);
    assert_eq!(seen.len(), 2, "equal variants collapse to one key");
    assert_ne!(Format::Grib1, Format::Grib2);
}
