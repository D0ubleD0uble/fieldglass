//! A `Field` a caller hands back with a cell count past a 32-bit `usize`
//! probes, contours and warps without panicking (#902).
//!
//! `Field` is public with public `ni`, `nj`, `values` and `mask`, so a Rust
//! caller, or anything deserialising one, can hand back any shape. The session
//! indexed it with `j·ni + i` in `usize`, which on wasm32 overflowed for a
//! 70,000 x 70,000 field: a debug panic, in release the wrong cell. These
//! tests bite under `wasm32-wasip1`, which CI's wasm32 job runs; natively they
//! pin that nothing past the four values is read.

use fieldglass::{DecodeOptions, Field, GridGeometry, Session, Values, WarpOptions};
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
