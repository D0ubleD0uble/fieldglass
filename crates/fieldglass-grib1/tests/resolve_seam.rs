//! The resolve seam (#580), GRIB1 half: which messages carry no raster of their
//! own, what grid each is put on, and that asking costs nothing when the answer
//! is "this one has a raster".
//!
//! GRIB1 has one such family, spherical harmonics — GRIB2 adds HEALPix — so the
//! seam is the same shape with one arm. The synthesised values are checked
//! against ECMWF's definitive formula in `spectral_render.rs`; what is pinned
//! here is the *dispatch*.

use fieldglass_grib1::{GlobalGrid, Grib1Reader};

const SPECTRAL_SIMPLE_T63: &[u8] = include_bytes!("fixtures/spectral_simple_t63.grib1");
const SPECTRAL_COMPLEX_T63: &[u8] = include_bytes!("fixtures/spectral_complex_t63.grib1");
const LATLON: &[u8] = include_bytes!("fixtures/j_consecutive_latlon.grib1");

fn reader(bytes: &[u8]) -> Grib1Reader {
    Grib1Reader::from_bytes(bytes.to_vec()).expect("parse")
}

/// Both GRIB1 spectral packings resolve, and onto the same pinned 0.5° grid:
/// the grid comes from the truncation in §2, not from how §4 packed the
/// coefficients.
#[test]
fn a_spectral_message_resolves_onto_the_pinned_global_grid() {
    for (name, bytes) in [
        ("spectral_simple", SPECTRAL_SIMPLE_T63),
        ("spectral_complex", SPECTRAL_COMPLEX_T63),
    ] {
        let reader = reader(bytes);
        assert_eq!(reader.synthesis_grid(0), Some(GlobalGrid::FINEST), "{name}");

        let (grid, values) = reader
            .synthesize_message_global(0)
            .unwrap_or_else(|e| panic!("{name} synthesises: {e}"))
            .expect("and is a synthesis family");
        assert_eq!(grid, GlobalGrid::FINEST, "{name}");
        assert_eq!(values.len(), grid.len(), "{name}");
        assert!(
            values.iter().all(Option::is_some),
            "{name}: a synthesised spectral field has no absent cells"
        );

        // The same values the explicit call produces, wrapped one per cell —
        // the resolve seam is a dispatch over the existing transform, not a
        // second one.
        let explicit = reader
            .synthesize_spectral_global(0)
            .unwrap_or_else(|e| panic!("{name}: the explicit call: {e}"));
        assert_eq!(grid, explicit.0, "{name}");
        assert_eq!(
            values,
            explicit.1.into_iter().map(Some).collect::<Vec<_>>(),
            "{name}"
        );
    }
}

/// An ordinary raster message answers `None` without decoding anything, and an
/// index the file does not hold answers `None` rather than erroring — the
/// caller's own range check is the one that reports it.
#[test]
fn a_raster_message_and_an_absent_one_both_decline() {
    let reader = reader(LATLON);
    assert_eq!(reader.synthesis_grid(0), None);
    assert_eq!(reader.synthesize_message_global(0).expect("no error"), None);

    assert_eq!(reader.synthesis_grid(9_999), None);
    assert_eq!(
        reader.synthesize_message_global(9_999).expect("no error"),
        None
    );
}
