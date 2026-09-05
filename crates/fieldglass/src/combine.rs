//! Two fields, combined element by element — the difference map and its
//! siblings (#239, moved here by #579).
//!
//! Combine is the one operation on this surface that takes *two* fields, and
//! the only one whose precondition is about the pair rather than about either
//! half: the two rasters have to align cell for cell, or the arithmetic
//! subtracts one place from another. That precondition is [`aligned`], and it
//! is the whole reason this is not simply [`fieldglass_core::combine_fields`]
//! re-exported.
//!
//! # Why the gate compares the geometry and not a flat key
//!
//! `fieldglass-napi` used to compare a 49-field borrowed view of its own
//! `MessageMeta` — every family's parameters at once, `Option` for the ones
//! that did not apply. That key answered "are these two DTOs the same" and was
//! asked "are these two grids the same", which are different questions in both
//! directions: it refused two fields whose *irrelevant* slots differed (a
//! lat/lon grid carrying a stray Lambert spacing), and it accepted two
//! curvilinear grids whose corner coordinates agreed while their cell arrays
//! did not. [`GridGeometry`] is the model — one variant per family, carrying
//! exactly what that family defines — so `==` on it is the question that was
//! being asked. #464 named this as the replacement; this is it.
//!
//! Three things travel beside the geometry and are compared too:
//!
//! * `ni` and `nj`, because [`GridGeometry::Unsupported`] carries no dimensions
//!   and a synthesised raster's shape is not the one its family states. That is
//!   also why [`Source`] states them separately in the first place.
//! * [`crate::api::Scan`], because two grids that are identical apart from the
//!   direction their rows were stored in hold their cells the other way up.

use fieldglass_core::{GridGeometry, combine_cell, combine_fields};

use crate::api::{CombineOpInfo, Field, Stats, Values};
use crate::error::Error;
use crate::render::Source;

/// The op vocabulary, re-exported: `core` owns it, and a host that offers a
/// Compare picker builds it from [`combine_ops`] rather than restating the
/// five tags (#342).
pub use fieldglass_core::CombineOp;

/// Every field-combine operation, in menu order, as a host's picker wants it.
///
/// One list, so the browser host's dropdown and the VS Code panel's cannot
/// drift apart — and so an op added to [`CombineOp::ALL`] appears in both
/// without either being edited.
#[must_use]
pub fn combine_ops() -> Vec<CombineOpInfo> {
    CombineOp::ALL
        .iter()
        .map(|op| CombineOpInfo {
            value: op.as_str().to_string(),
            label: op.label().to_string(),
        })
        .collect()
}

/// A wire tag from a host's picker, as an op — or the refusal, naming the tags
/// this build knows.
///
/// Here rather than in each host, because "unknown combine op" is exactly the
/// kind of message two bindings word differently and then answer differently
/// (#342). The list in the message is built from [`CombineOp::ALL`], so an op
/// added to the enum appears in the refusal as well as in [`combine_ops`].
///
/// # Errors
///
/// [`Error::InvalidOption`] for a tag [`CombineOp::from_wire`] does not know.
pub fn op_from_wire(tag: &str) -> Result<CombineOp, Error> {
    CombineOp::from_wire(tag).ok_or_else(|| Error::InvalidOption {
        detail: format!("unknown combine op {tag:?} (expected {})", known_tags()),
    })
}

/// `"a", "b", "c", or "d"` over every tag in [`CombineOp::ALL`].
fn known_tags() -> String {
    let quoted: Vec<String> = CombineOp::ALL
        .iter()
        .map(|op| format!("{:?}", op.as_str()))
        .collect();
    match quoted.split_last() {
        // One op, or none, has no list to punctuate.
        None => String::new(),
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{}, or {last}", rest.join(", ")),
    }
}

/// Whether two sources hold their cells in the same places, and if not, which
/// property says so.
///
/// A refusal is [`Error::Unsupported`] rather than [`Error::InvalidOption`]:
/// nothing the caller passed is out of range: these are two perfectly good
/// fields that this operation is not defined over.
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
    // A source whose geometry did not resolve is **not** refused out of hand.
    // That is the point of `Source::geometry` being a `Result`: the refusal
    // travels with the source and each operation decides whether it needs a
    // geometry. Combine does not — it is an element-wise walk of two rasters —
    // and the source projection paints such a raster as stored, so two of the
    // same shape whose placement is unknown do combine, index for index, which
    // is what a source-view difference of them means. A §3.20 stating `Dx = 0`
    // and a NetCDF slice with no coordinate arrays are both in the corpus and
    // both combine today.
    //
    // They have to be unplaceable for the *same* reason, though: `Error` is
    // `PartialEq`, and the refusal names the field that was missing, so a grid
    // with no spacing and one with no first latitude are still refused against
    // each other — as is a placeable grid against an unplaceable one.
    match (&a.geometry, &b.geometry) {
        (Ok(ga), Ok(gb)) if ga != gb => Err(mismatch("grid", &describe(ga), &describe(gb))),
        (Err(ea), Err(eb)) if ea != eb => Err(mismatch("grid", &unplaceable(ea), &unplaceable(eb))),
        (Ok(g), Err(e)) => Err(mismatch("grid", &describe(g), &unplaceable(e))),
        (Err(e), Ok(g)) => Err(mismatch("grid", &unplaceable(e), &describe(g))),
        _ => Ok(()),
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
fn describe(g: &GridGeometry) -> String {
    let mut out = g.label().to_string();
    if let Some((ni, nj)) = g.dims() {
        out.push_str(&format!(" {ni}x{nj}"));
    }
    if let Some(a) = g.plane_affine() {
        let units = match a.units {
            fieldglass_core::PlaneUnits::Degrees => "deg",
            fieldglass_core::PlaneUnits::Metres => "m",
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

/// Combine two aligned rasters into the engine's own cell shape.
///
/// The low seam, for a host that already holds decoded `Vec<Option<f64>>`
/// buffers and its own resolved [`Source`] for each — `fieldglass-napi` does,
/// and passing through [`Field`] there would round its values through
/// [`Values`]'s narrowing rule and move the pixels it has a golden for.
/// [`crate::Session::combine`] is the same operation over the API's own DTO.
///
/// A cell is present in the output only where it is present in both inputs and
/// the result is finite; see [`combine_fields`] for the rule and why the
/// ratio's divide-by-zero falls out as missing.
///
/// # Errors
///
/// [`Error::Unsupported`] when [`aligned`] refuses the pair.
pub fn combine_values(
    a: &Source<'_>,
    values_a: &[Option<f64>],
    b: &Source<'_>,
    values_b: &[Option<f64>],
    op: CombineOp,
) -> Result<Vec<Option<f64>>, Error> {
    aligned(a, b)?;
    Ok(combine_fields(values_a, values_b, op))
}

/// [`crate::Session::combine`]'s body, over the API DTO.
pub(crate) fn combine_api_fields(a: &Field, b: &Field, op: CombineOp) -> Result<Field, Error> {
    // The same gate the low seam runs, asked through the same function: a
    // `Source` borrowed off each field's own georef. Two gates would be two
    // answers the day one of them learned about a new family.
    let (sa, sb) = (source_of(a), source_of(b));
    aligned(&sa, &sb)?;

    let (va, vb) = (a.values.to_f64(), b.values.to_f64());
    let mut values = Vec::with_capacity(a.mask.len());
    let mut mask = Vec::with_capacity(a.mask.len());
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    let mut valid_count = 0u32;
    for k in 0..a.mask.len() {
        let present = |m: &[u8], v: &[f64]| -> Option<f64> {
            (m.get(k).copied().unwrap_or(0) == 1)
                .then(|| v.get(k).copied())
                .flatten()
        };
        // `combine_fields`' own rule, asked cell by cell rather than over two
        // materialised `Vec<Option<f64>>`s — see `combine_cell` for the memory
        // this saves and for why the rule is not restated here.
        match combine_cell(present(&a.mask, &va), present(&b.mask, &vb), op) {
            Some(v) => {
                values.push(v);
                mask.push(1);
                min = min.min(v);
                max = max.max(v);
                valid_count += 1;
            }
            None => {
                values.push(0.0);
                mask.push(0);
            }
        }
    }

    Ok(Field {
        // `Dtype::Auto`, the rule `Session::decode` uses: narrow to `f32` only
        // where every present value survives the round trip. A difference of
        // two `f32`-representable fields usually is one; a ratio usually is
        // not, and saying so is the point of the rule.
        values: Values::build(values, &mask, crate::api::Dtype::Auto),
        mask,
        ni: a.ni,
        nj: a.nj,
        georef: a.georef.clone(),
        stats: Stats {
            min: (valid_count > 0).then_some(min),
            max: (valid_count > 0).then_some(max),
            valid_count,
        },
        // Field A's, verbatim. The combined field's *caption* is `A − B` over
        // A's parameter, which is the host's to compose and the VS Code panel
        // already does; a units algebra here would have to answer what `A / B`
        // of two different parameters is measured in, and answering it wrongly
        // is worse than not answering.
        parameter: a.parameter.clone(),
        units: a.units.clone(),
    })
}

/// A [`Source`] borrowed off a field's own georef.
fn source_of(f: &Field) -> Source<'_> {
    Source {
        geometry: Ok(&f.georef.geometry),
        ni: f.ni,
        nj: f.nj,
        scan: f.georef.scan,
        family: &f.georef.kind,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{Georef, Scan};
    use fieldglass_core::{LambertParams, LatLonParams};

    fn latlon(ni: u32, nj: u32) -> GridGeometry {
        GridGeometry::LatLon(LatLonParams {
            ni,
            nj,
            lat_first: 40.0,
            lon_first: 0.0,
            lat_last: 20.0,
            lon_last: 20.0,
        })
    }

    fn source<'a>(
        g: &'a GridGeometry,
        ni: u32,
        nj: u32,
        scan: Scan,
        family: &'a str,
    ) -> Source<'a> {
        Source {
            geometry: Ok(g),
            ni,
            nj,
            scan,
            family,
        }
    }

    fn field(g: &GridGeometry, values: Vec<f64>, mask: Vec<u8>) -> Field {
        let (ni, nj) = g.dims().expect("a raster family");
        let present: Vec<f64> = values
            .iter()
            .zip(&mask)
            .filter(|&(_, &m)| m == 1)
            .map(|(&v, _)| v)
            .collect();
        Field {
            values: Values::F64(values),
            mask,
            ni,
            nj,
            georef: Georef::from_geometry(g, Scan::north_down()),
            stats: Stats {
                min: present.iter().copied().reduce(f64::min),
                max: present.iter().copied().reduce(f64::max),
                valid_count: present.len() as u32,
            },
            parameter: "Temperature".to_string(),
            units: "K".to_string(),
        }
    }

    /// The vocabulary a host builds its picker from is the enum's, in order.
    #[test]
    fn the_op_vocabulary_is_cores_own_list() {
        let ops = combine_ops();
        assert_eq!(ops.len(), CombineOp::ALL.len());
        for (info, op) in ops.iter().zip(CombineOp::ALL) {
            assert_eq!(info.value, op.as_str());
            assert_eq!(info.label, op.label());
            assert_eq!(CombineOp::from_wire(&info.value), Some(op));
        }
    }

    /// Every tag a picker can send parses, and one it cannot is refused with a
    /// message that names all five — the message both hosts now report.
    #[test]
    fn an_unknown_op_is_refused_by_name() {
        for op in CombineOp::ALL {
            assert_eq!(op_from_wire(op.as_str()), Ok(op));
        }
        let e = op_from_wire("product").expect_err("not an op");
        assert_eq!(e.code(), "invalid_option");
        assert_eq!(
            e.message(),
            "unknown combine op \"product\" (expected \"a_minus_b\", \"b_minus_a\", \
             \"a_plus_b\", \"mean\", or \"ratio\")",
            "the wording the VS Code panel has shown since #239, built from \
             CombineOp::ALL so it cannot fall behind the enum"
        );
        assert!(op_from_wire("").is_err());
    }

    /// The gate accepts what aligns and names what does not — one case per
    /// property it compares, because a gate that answered "different grids" to
    /// everything would pass a test that only checked it refused.
    #[test]
    fn the_gate_names_the_property_that_differs() {
        let g = latlon(8, 4);
        let scan = Scan::north_down();
        let a = source(&g, 8, 4, scan, "latlon");
        assert!(aligned(&a, &source(&g, 8, 4, scan, "latlon")).is_ok());

        // Same family, different parameters.
        let wider = latlon(9, 4);
        let e = aligned(&a, &source(&wider, 9, 4, scan, "latlon")).expect_err("different Ni");
        assert!(e.message().contains("raster shape"), "{}", e.message());

        // Same dimensions, different family.
        let lambert = GridGeometry::Lambert(LambertParams {
            earth_radius_m: 6_371_229.0,
            ni: 8,
            nj: 4,
            lat_first: 20.0,
            lon_first: -120.0,
            lad: 25.0,
            lov: -95.0,
            dx_metres: 12_000.0,
            dy_metres: 12_000.0,
            latin1: 25.0,
            latin2: 25.0,
        });
        let e = aligned(&a, &source(&lambert, 8, 4, scan, "lambert")).expect_err("family differs");
        assert!(
            e.message().contains("their grid differs"),
            "{}",
            e.message()
        );
        // Named the way a user reads it, not as a Rust struct literal: the
        // family, the raster shape, and where the raster sits — in degrees for
        // a geographic family and in projection metres for a planar one.
        assert!(
            e.message().contains("A: latlon 8x4 from (0, 40) deg by ("),
            "{}",
            e.message()
        );
        assert!(
            e.message().contains("B: lambert 8x4 from (") && e.message().contains(") m by ("),
            "{}",
            e.message()
        );

        // Same family and dimensions, one row order reversed. The values would
        // combine upside down, so this must be refused rather than accepted on
        // the strength of the geometry alone.
        let flipped = Scan::new(false, true, false);
        let e = aligned(&a, &source(&g, 8, 4, flipped, "latlon")).expect_err("scan differs");
        assert!(e.message().contains("scan order"), "{}", e.message());

        // Same family, same shape, one parameter apart. The old flat key
        // compared every family's slots at once and would have accepted this
        // pair only because both carried the same `None`s; the geometry does
        // the comparison the operation actually needs.
        let moved = GridGeometry::LatLon(LatLonParams {
            lat_first: 41.0,
            ..latlon_params(&g)
        });
        let e = aligned(&a, &source(&moved, 8, 4, scan, "latlon")).expect_err("corner differs");
        assert!(
            e.message().contains("their grid differs"),
            "{}",
            e.message()
        );

        // Two grids that state no usable geometry, for the same reason, still
        // combine: neither can be placed, both are the same shape, and the
        // source view paints each as stored. This is the arm the napi
        // characterisation golden caught — ten of its fields are §3.20 grids
        // with `Dx = 0` or HDF5 slices with no coordinate arrays, and all of
        // them differenced before #579 as well as after.
        let unplaceable = |detail: &str| Source {
            geometry: Err(Error::Unsupported {
                detail: detail.to_string(),
            }),
            ni: 8,
            nj: 4,
            scan,
            family: "latlon",
        };
        let no_spacing = unplaceable("dx is zero");
        assert!(aligned(&no_spacing, &unplaceable("dx is zero")).is_ok());

        // But unplaceable for *different* reasons is still a mismatch, and so
        // is a placeable grid against an unplaceable one — in both directions,
        // because the two are separate arms and one of them could be wrong.
        let e = aligned(&no_spacing, &unplaceable("missing latFirst"))
            .expect_err("two different refusals");
        assert!(
            e.message().contains("no usable geometry"),
            "{}",
            e.message()
        );
        assert!(aligned(&a, &no_spacing).is_err());
        assert!(aligned(&no_spacing, &a).is_err());
    }

    fn latlon_params(g: &GridGeometry) -> LatLonParams {
        match g {
            GridGeometry::LatLon(p) => *p,
            other => panic!("not a lat/lon grid: {other:?}"),
        }
    }

    /// Every op, over a field with a hole in it, through the API form.
    #[test]
    fn each_op_combines_the_api_fields_the_way_core_does() {
        let g = latlon(2, 2);
        let a = field(&g, vec![10.0, 3.0, 0.0, 0.0], vec![1, 1, 1, 0]);
        let b = field(&g, vec![4.0, 6.0, 0.0, 0.0], vec![1, 1, 0, 1]);
        for (op, want) in [
            (CombineOp::Difference, [6.0, -3.0]),
            (CombineOp::ReverseDifference, [-6.0, 3.0]),
            (CombineOp::Sum, [14.0, 9.0]),
            (CombineOp::Mean, [7.0, 4.5]),
            (CombineOp::Ratio, [2.5, 0.5]),
        ] {
            let out = combine_api_fields(&a, &b, op).expect("same grid");
            assert_eq!(out.mask, vec![1, 1, 0, 0], "{op:?}: a hole in either input");
            assert_eq!(out.values.get(0), Some(want[0]), "{op:?}");
            assert_eq!(out.values.get(1), Some(want[1]), "{op:?}");
            assert_eq!(
                (out.stats.min, out.stats.max),
                (Some(want[0].min(want[1])), Some(want[0].max(want[1]))),
                "{op:?}: the stats are the combined field's, not A's"
            );
            assert_eq!(out.stats.valid_count, 2, "{op:?}");
            assert_eq!(out.ni, 2);
            assert_eq!(out.nj, 2);
            assert_eq!(out.georef, a.georef, "{op:?}: A's placement is kept");
            assert_eq!(out.parameter, "Temperature", "{op:?}");
            assert_eq!(out.units, "K", "{op:?}");
        }
    }

    /// A masked output cell carries no value, so a field with nothing present
    /// reports no range at all rather than `±inf`.
    #[test]
    fn a_fully_masked_result_has_no_range() {
        let g = latlon(2, 1);
        let a = field(&g, vec![1.0, 2.0], vec![1, 1]);
        let b = field(&g, vec![0.0, 0.0], vec![1, 1]);
        // Every cell divides by zero, so every cell is dropped.
        let out = combine_api_fields(&a, &b, CombineOp::Ratio).expect("same grid");
        assert_eq!(out.mask, vec![0, 0]);
        assert_eq!(out.stats.valid_count, 0);
        assert_eq!((out.stats.min, out.stats.max), (None, None));
    }

    /// The low seam and the API form run the same gate, so a pair one refuses
    /// is a pair the other refuses with the same words.
    #[test]
    fn both_seams_refuse_the_same_pair_the_same_way() {
        let (small, large) = (latlon(2, 2), latlon(3, 2));
        let a = field(&small, vec![1.0; 4], vec![1; 4]);
        let b = field(&large, vec![1.0; 6], vec![1; 6]);
        let api = combine_api_fields(&a, &b, CombineOp::Difference).expect_err("different grids");

        let scan = Scan::north_down();
        let raw = combine_values(
            &source(&small, 2, 2, scan, "latlon"),
            &[Some(1.0); 4],
            &source(&large, 3, 2, scan, "latlon"),
            &[Some(1.0); 6],
            CombineOp::Difference,
        )
        .expect_err("different grids");
        assert_eq!(api, raw);
        assert_eq!(api.code(), "unsupported");
    }
}
