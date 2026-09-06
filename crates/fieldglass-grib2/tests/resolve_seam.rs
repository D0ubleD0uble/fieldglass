//! The resolve seam (#580): which messages carry no raster of their own, what
//! grid each is put on, and that asking costs nothing when the answer is "this
//! one has a raster".
//!
//! Two families answer here — §3.50 spherical-harmonic and §3.150 HEALPix — and
//! they reach their grid by different rules, so a build that used one rule for
//! both would be wrong on the other. §3.61/62/63 bi-Fourier is rasterless too
//! and deliberately declines: recovering its grid needs an inverse bi-Fourier
//! transform this build does not have.
//!
//! The values themselves are checked against their own oracles elsewhere
//! (`spectral_render.rs` against ECMWF's definitive formula, `healpix_grid.rs`
//! against eccodes' pixel centres). What is pinned here is the *dispatch*.

use fieldglass_grib2::{GlobalGrid, Grib2Reader};

const SPECTRAL_T63: &[u8] = include_bytes!("fixtures/spectral_simple_t63.grib2");
const HEALPIX_N4_RING: &[u8] = include_bytes!("fixtures/healpix_n4_ring.grib2");
const HEALPIX_N2_NESTED: &[u8] = include_bytes!("fixtures/healpix_n2_nested.grib2");
const BIFOURIER: &[u8] = include_bytes!("fixtures/bifourier_ellipse_ieee32.grib2");
const LATLON: &[u8] = include_bytes!("fixtures/regular_latlon_surface.grib2");

fn reader(bytes: &[u8]) -> Grib2Reader {
    Grib2Reader::from_bytes(bytes.to_vec()).expect("parse")
}

/// A spectral message lands on the pinned 0.5° grid, and the paired call agrees
/// with the one that reads the GDS alone.
#[test]
fn a_spectral_message_resolves_onto_the_pinned_global_grid() {
    let reader = reader(SPECTRAL_T63);
    assert_eq!(reader.synthesis_grid(0), Some(GlobalGrid::FINEST));

    let (grid, values) = reader
        .synthesize_message_global(0)
        .expect("a spectral message synthesises")
        .expect("and is a synthesis family");
    assert_eq!(grid, GlobalGrid::FINEST);
    assert_eq!(values.len(), grid.len());
    assert!(
        values.iter().all(Option::is_some),
        "a synthesised spectral field has no absent cells: the coefficients \
         evaluate everywhere"
    );

    // The same values the explicit call produces, wrapped one per cell — the
    // resolve seam is a dispatch over the existing transform, not a second one.
    let explicit = reader
        .synthesize_spectral_global(0)
        .expect("the explicit call");
    assert_eq!(grid, explicit.0);
    assert_eq!(values, explicit.1.into_iter().map(Some).collect::<Vec<_>>());
}

/// HEALPix reaches its grid by the *pixel-scale* rule, not the spectral one, so
/// the two subjects disagree about size — which is the whole reason both are
/// worth pinning.
#[test]
fn a_healpix_message_resolves_onto_a_grid_sized_from_nside() {
    for (bytes, nside, dims) in [
        (HEALPIX_N4_RING, 4u32, (26, 14)),
        (HEALPIX_N2_NESTED, 2, (14, 8)),
    ] {
        let reader = reader(bytes);
        let grid = reader.synthesis_grid(0).expect("a HEALPix synthesis grid");
        assert_eq!(grid.dims(), dims, "Nside {nside}");
        assert_ne!(
            grid,
            GlobalGrid::FINEST,
            "Nside {nside} must not land on the spectral grid: a build that \
             used one rule for both families would pass every other assertion"
        );

        let (paired, values) = reader
            .synthesize_message_global(0)
            .expect("a HEALPix message resamples")
            .expect("and is a synthesis family");
        assert_eq!(paired, grid);
        assert_eq!(values.len(), grid.len());

        // The same resample `core` performs on the decoded pixels, so the seam
        // is a dispatch rather than a second implementation of the rule.
        let pixels = reader.decode_message_values(0).expect("pixels decode");
        let (core_grid, core_values) =
            fieldglass_core::healpix::resample_to_global(nside, reader_is_nested(&reader), &pixels)
                .expect("core resamples the same pixels");
        assert_eq!((core_grid, core_values), (grid, values));
    }
}

/// `nested` as the message states it, read back rather than assumed from the
/// fixture's name.
fn reader_is_nested(reader: &Grib2Reader) -> bool {
    match reader.messages[0].gds.template {
        fieldglass_grib2::GridTemplate::Healpix(t) => t.nested,
        ref other => panic!("not a HEALPix message: {other:?}"),
    }
}

/// Bi-Fourier has no raster and is still not a synthesis family: there is no
/// inverse bi-Fourier transform here, so it declines rather than producing a
/// grid it cannot fill.
#[test]
fn a_bifourier_message_is_not_a_synthesis_family() {
    let reader = reader(BIFOURIER);
    assert_eq!(reader.synthesis_grid(0), None);
    assert_eq!(
        reader.synthesize_message_global(0).expect("no error"),
        None,
        "declining must be `Ok(None)`, not an error: the caller falls through \
         to the raster path, which is where the refusal belongs"
    );
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
