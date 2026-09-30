//! Whether two fields line up cell for cell, the precondition of every
//! operation that pairs two value arrays element by element.
//!
//! Two operations ask it: combining two fields (`combine`, behind `analysis`)
//! and drawing vector arrows from a u and a v field
//! (`render::vector_polylines`, behind `render`). Each pairs cell `k` of one
//! array with cell `k` of the other, so each goes wrong in the same way when
//! the two rasters do not hold the same places, and both ask the same function
//! (#793). It lives here rather than in `combine` so that a build with only
//! `render` still has it.
//!
//! The `combine` module docs say why the gate compares the geometry rather
//! than a flat key, and which properties travel beside it.

use fieldglass_core::GridGeometry;

use crate::error::Error;
use crate::render::Source;

/// Whether two sources hold their cells in the same places, and if not, which
/// property says so.
///
/// A refusal is [`Error::Unsupported`] rather than [`Error::InvalidOption`]:
/// nothing the caller passed is out of range: these are two perfectly good
/// fields that this operation is not defined over.
///
/// The scan compared is each source's decoded one ([`Source::scan`]), so a
/// message stored column-major, which the reader transposes, lines up with a
/// row-major message on the same grid (#792).
///
/// # Errors
///
/// [`Error::Unsupported`] when the raster shape, the scan order or the grid
/// differ, naming which.
pub fn aligned(a: &Source<'_>, b: &Source<'_>) -> Result<(), Error> {
    if (a.ni, a.nj) != (b.ni, b.nj) {
        return Err(mismatch(
            "raster shape",
            &format!("{}x{}", a.ni, a.nj),
            &format!("{}x{}", b.ni, b.nj),
        ));
    }
    if a.scan != b.scan {
        return Err(mismatch(
            "scan order",
            &scan_label(a.scan),
            &scan_label(b.scan),
        ));
    }
    // A source whose geometry did not resolve is refused, whichever side it is
    // on and whatever the other side is. Combining is an element-wise walk that
    // needs no geometry of its own, but "these two rasters hold their cells in
    // the same places" is a claim about geometry, and a host that could not
    // state one cannot make it.
    //
    // Until #574 two unresolved sources combined when they had failed for the
    // *same* reason. That arm existed for one host: `fieldglass-napi` rebuilt
    // each geometry out of a flat DTO, its rebuilder refused grids
    // `GridGeometry::from` accepts, and the characterisation golden differenced
    // those grids. It was weaker than the key it replaced — a refusal names the
    // field, not its value, so two §3.20 grids both stating `Dx = 0` over
    // different places read alike. The addon now carries the geometry the
    // readers build, so every host hands over an `Ok` here and the arm had no
    // caller left. A raster no coordinates place is `Ok(Unsupported)`, which
    // compares by its label and still combines with itself.
    match (&a.geometry, &b.geometry) {
        (Ok(ga), Ok(gb)) if same_grid(ga, gb) => Ok(()),
        (Ok(ga), Ok(gb)) => Err(mismatch("grid", &describe(ga), &describe(gb))),
        (ga, gb) => Err(mismatch("grid", &side(ga), &side(gb))),
    }
}

/// One side of a grid mismatch, placed or not.
fn side(geometry: &Result<&GridGeometry, Error>) -> String {
    match geometry {
        Ok(g) => describe(g),
        Err(e) => unplaceable(e),
    }
}

/// Whether two geometries place their cells identically.
///
/// `==` for every family but one. A [`GridGeometry::Lookup`] is a list of cell
/// centres, and two things about its derived `PartialEq` make it the wrong
/// question here:
///
/// * **A centre the source left as a fill value is stored as `NaN`** so the
///   indices stay aligned (see `SpatialIndex`), and `NaN != NaN` — so such an
///   index does not equal *itself* rebuilt from the same file. A swath granule
///   marks the fields of view that saw no Earth, so this is the ordinary case
///   for one, not a corrupt one, and it would have refused every difference map
///   over such a granule including a field against itself. No committed
///   curvilinear fixture has a fill-valued coordinate, which is why the
///   characterisation golden does not see it.
/// * **It is `O(n)` and reads about 28 MB at a million cells**, which
///   `SpatialIndex`'s own documentation says does not belong on a per-repaint
///   path — and a combined probe runs this on every mouse move.
///
/// [`SpatialIndex::fingerprint`](fieldglass_core::SpatialIndex::fingerprint)
/// answers both: it is the key that type nominates for exactly this, and it
/// normalises `NaN` so an excluded cell hashes consistently. The pointer check
/// in front of it is the common case — a host caching one index per coordinate
/// pair hands the same borrow twice — and makes it free there.
pub(crate) fn same_grid(a: &GridGeometry, b: &GridGeometry) -> bool {
    if std::ptr::eq(a, b) {
        return true;
    }
    match (a, b) {
        (GridGeometry::Lookup(x), GridGeometry::Lookup(y)) => x.fingerprint() == y.fingerprint(),
        _ => a == b,
    }
}

/// How a grid that would not resolve is named in a refusal.
fn unplaceable(e: &Error) -> String {
    format!("a grid that states no usable geometry ({})", e.message())
}

/// The refusal, in one spelling so both halves of the gate read alike.
fn mismatch(property: &str, a: &str, b: &str) -> Error {
    Error::Unsupported {
        detail: format!(
            "the two fields are on different grids and cannot be combined: their \
             {property} differs (A: {a}, B: {b})"
        ),
    }
}

/// How a geometry is named in a refusal: its family, its raster shape, and
/// where it puts that raster.
///
/// Written out rather than `{g:?}` for two reasons, one of them measured. The
/// derived `Debug` is a Rust struct literal — `Lambert(LambertParams {
/// earth_radius_m: 6371229.0, .. })` — and this string reaches a VS Code error
/// toast and a browser console. It also drags `Debug` for eleven parameter
/// structs into the wasm bundle, which is 5,853 bytes of `-Oz` output measured
/// against binaryen 132 for a diagnostic nobody reads in that form.
///
/// The affine is what separates two grids of the same family in practice: it is
/// the origin and the signed step, so a moved grid and a coarser one both show
/// here. Two grids differing only in a projection constant — one cone's
/// standard parallel against another's — describe alike, and the refusal then
/// says only that their grids differ, which is true and is the case a `Debug`
/// dump would have served better. `plane_affine` is a cheap accessor, unlike
/// `lonlat_bbox`, which walks a projected perimeter 512 times an edge and has
/// no business on an error path.
///
/// A rotated grid's corners are the one pair here that are degrees of
/// something other than longitude and latitude, so they are labelled for the
/// frame they are in. Two COSMO grids differing only in their declared pole
/// then still describe alike — the accepted case above — but the string no
/// longer reads as a geographic position the field is nowhere near.
pub(crate) fn describe(g: &GridGeometry) -> String {
    let mut out = g.label().to_string();
    if let Some((ni, nj)) = g.dims() {
        out.push_str(&format!(" {ni}x{nj}"));
    }
    if let Some(a) = g.plane_affine() {
        let units = match (a.units, g) {
            (fieldglass_core::PlaneUnits::Degrees, GridGeometry::RotatedLatLon(_)) => {
                "deg (rotated frame)"
            }
            (fieldglass_core::PlaneUnits::Degrees, _) => "deg",
            (fieldglass_core::PlaneUnits::Metres, _) => "m",
        };
        out.push_str(&format!(" from ({}, {}) {units}", a.x0, a.y0));
        match (a.dx, a.dy) {
            (Some(dx), Some(dy)) => out.push_str(&format!(" by ({dx}, {dy})")),
            (Some(dx), None) => out.push_str(&format!(" by ({dx}, -)")),
            (None, Some(dy)) => out.push_str(&format!(" by (-, {dy})")),
            (None, None) => {}
        }
    }
    out
}

/// A scan order, as the three flags a host reads off a `Georef`.
fn scan_label(s: crate::api::Scan) -> String {
    format!(
        "iNegative={} jPositive={} jConsecutive={}",
        s.i_negative, s.j_positive, s.j_consecutive
    )
}
