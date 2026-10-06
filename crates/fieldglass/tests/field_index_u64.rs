//! A `Field` a caller hands back with a cell count past a 32-bit `usize`
//! probes, contours and warps without panicking (#902).
//!
//! `Field` is public with public `ni`, `nj`, `values` and `mask`, so a Rust
//! caller, or anything deserialising one, can hand back any shape. The session
//! indexed it with `j·ni + i` in `usize`, which on wasm32 overflowed for a
//! 70,000 x 70,000 field: a debug panic, in release the wrong cell. These
//! tests bite under `wasm32-wasip1`, which CI's wasm32 job runs; natively they
//! pin that nothing past the four values is read.
//!
//! An output that is the field's own shape, a CSV export, a source-target
//! paint, a CPU render or a warp with no named size, is refused instead
//! (#913): it would size a 70,000 x 70,000 buffer from four values, a
//! capacity-overflow panic on wasm32 and a many-gigabyte allocation natively.

use fieldglass::{
    DecodeOptions, Error, Field, GridGeometry, PaletteOptions, RenderOptions, Session, Values,
    WarpOptions,
};
use fieldglass_core::LatLonParams;

const FIXTURE: &str = "../fieldglass-grib2/tests/fixtures/regular_latlon_surface.grib2";
const SIDE: u32 = 70_000;

/// A real decoded field, resized to a global `SIDE x SIDE` lat/lon grid that
/// holds four values.
fn huge_field(session: &Session) -> Field {
    let mut field = session
        .decode(0, &DecodeOptions::default())
        .expect("decodes");
    field.ni = SIDE;
    field.nj = SIDE;
    field.georef.geometry = GridGeometry::LatLon(LatLonParams {
        ni: SIDE,
        nj: SIDE,
        lat_first: 89.0,
        lon_first: -179.0,
        lat_last: -89.0,
        lon_last: 179.0,
    });
    field.values = Values::F32(vec![1.0; 4]);
    field.mask = vec![1; 4];
    field
}

#[test]
fn a_huge_field_probes_contours_and_warps() {
    let session = Session::open(std::fs::read(FIXTURE).expect("fixture")).expect("opens");
    let field = huge_field(&session);

    // Near the last row, where `j·ni` passes `u32::MAX`.
    let probe = session.probe(&field, -88.9, 0.0).expect("on the grid");
    assert_eq!(probe.value, None, "past the four values the field holds");

    let lines = session.contours(&field, &[0.5]).expect("contours");
    assert!(
        lines.iter().all(|l| l.segments.is_empty()),
        "nothing to march"
    );

    let mut options = WarpOptions::new(false);
    options.width = Some(64);
    options.height = Some(32);
    let warped = session.warp(&field, &options).expect("warps");
    let present = warped.mask.iter().filter(|&&m| m != 0).count();
    assert!(present <= 4, "only cells the field holds: {present}");
}

/// Assert `result` is the shape refusal, naming the stated grid.
fn refused<T: std::fmt::Debug>(result: Result<T, Error>, what: &str) {
    match result {
        Err(Error::InvalidOption { detail }) => {
            assert!(detail.contains("70000×70000"), "{what}: {detail}");
        }
        other => panic!("{what}: expected a shape refusal, got {other:?}"),
    }
}

#[test]
fn an_output_of_a_huge_fields_own_shape_is_refused() {
    let session = Session::open(std::fs::read(FIXTURE).expect("fixture")).expect("opens");
    let field = huge_field(&session);
    let values = vec![Some(1.0); 4];

    for format in ["matrix", "long"] {
        refused(session.field_csv(&field.source(), &values, format), format);
    }
    refused(
        session.project(&field.source(), &values, &RenderOptions::default()),
        "source target",
    );
    // A warp target with no named size takes its raster from the grid too;
    // the azimuthal and world targets do even with one.
    for projection in ["equirectangular", "orthographic", "mollweide"] {
        refused(
            session.project(
                &field.source(),
                &values,
                &RenderOptions::new(projection, "nearest"),
            ),
            projection,
        );
    }
    refused(
        session.render(&field, &PaletteOptions::default(), false),
        "render",
    );
    refused(
        session.warp(&field, &WarpOptions::new(false)),
        "warp to its own shape",
    );
}

#[test]
fn a_grid_whose_cell_count_overflows_is_refused() {
    let session = Session::open(std::fs::read(FIXTURE).expect("fixture")).expect("opens");
    let mut field = huge_field(&session);
    // `u32::MAX²` overflows a 32-bit `usize`, and a 64-bit one only just
    // holds it; either way four values are not one per cell.
    field.ni = u32::MAX;
    field.nj = u32::MAX;
    let values = vec![Some(1.0); 4];
    let mut source = field.source();
    source.ni = u32::MAX;
    source.nj = u32::MAX;
    for format in ["matrix", "long"] {
        refused_overflow(session.field_csv(&source, &values, format), format);
    }
    refused_overflow(
        session.project(&source, &values, &RenderOptions::default()),
        "source target",
    );
    refused_overflow(session.warp(&field, &WarpOptions::new(false)), "warp");
}

/// The shape refusal for a `u32::MAX × u32::MAX` grid, whose cell count is
/// "too many" where it overflows `usize` and a number where it does not.
fn refused_overflow<T: std::fmt::Debug>(result: Result<T, Error>, what: &str) {
    match result {
        Err(Error::InvalidOption { detail }) => assert!(
            detail.starts_with("a 4294967295×4294967295 grid has ")
                && detail.ends_with(" cells, and 4 values were given"),
            "{what}: {detail}"
        ),
        other => panic!("{what}: expected a shape refusal, got {other:?}"),
    }
}

/// A field that holds every value its grid states can still ask a world
/// target for a raster `max(ni, nj)²` wide, past what wasm32 can allocate:
/// refused rather than a capacity-overflow abort (#913). A 64-bit host can
/// allocate it, so this runs only where `usize` is 32 bits.
#[cfg(target_pointer_width = "32")]
#[test]
fn a_derived_raster_past_the_address_space_is_refused() {
    let session = Session::open(std::fs::read(FIXTURE).expect("fixture")).expect("opens");
    let mut field = huge_field(&session);
    field.ni = 30_000;
    field.nj = 2;
    field.georef.geometry = GridGeometry::LatLon(LatLonParams {
        ni: 30_000,
        nj: 2,
        lat_first: 1.0,
        lon_first: -179.0,
        lat_last: -1.0,
        lon_last: 179.0,
    });
    let values = vec![Some(1.0); 60_000];
    match session.project(
        &field.source(),
        &values,
        &RenderOptions::new("mollweide", "nearest"),
    ) {
        Err(Error::InvalidOption { detail }) => {
            assert!(
                detail.contains("larger than this target can allocate"),
                "{detail}"
            );
        }
        other => panic!("expected a raster-size refusal, got {other:?}"),
    }
}

#[test]
fn a_field_holding_its_shape_still_exports_and_paints() {
    let session = Session::open(std::fs::read(FIXTURE).expect("fixture")).expect("opens");
    let field = session
        .decode(0, &DecodeOptions::default())
        .expect("decodes");
    let values: Vec<Option<f64>> = (0..field.values.len())
        .map(|k| (field.mask[k] == 1).then(|| field.values.get(k)).flatten())
        .collect();
    let csv = session
        .field_csv(&field.source(), &values, "matrix")
        .expect("exports");
    assert_eq!(csv.lines().count(), field.nj as usize);
    let projected = session
        .project(&field.source(), &values, &RenderOptions::default())
        .expect("paints");
    assert_eq!(projected.values.len(), values.len());
    session
        .render(&field, &PaletteOptions::default(), false)
        .expect("renders");
    let warped = session
        .warp(&field, &WarpOptions::new(false))
        .expect("warps");
    assert_eq!(warped.mask.len(), values.len());
}
