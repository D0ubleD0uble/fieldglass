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
//! did not. [`GridGeometry`](fieldglass_core::GridGeometry) is the model —
//! one variant per family, carrying exactly what that family defines — so `==`
//! on it is the question that was being asked. #464 named this as the replacement; this is it.
//!
//! Three things travel beside the geometry and are compared too:
//!
//! * `ni` and `nj`, because
//!   [`GridGeometry::Unsupported`](fieldglass_core::GridGeometry::Unsupported)
//!   carries no dimensions and a synthesised raster's shape is not the one its
//!   family states. That is
//!   also why [`Source`] states them separately in the first place.
//! * [`crate::api::Scan`], because two grids that are identical apart from the
//!   direction their rows were stored in hold their cells the other way up.
//!   It is the decoded raster's scan, so two messages that stored the same
//!   grid in different orders (one column-major, which the reader transposes)
//!   compare equal: their values line up (#792).

use fieldglass_core::combine_fields;

use crate::api::{CombineOpInfo, Field, SpectralTruncation, Stats, Values};
use crate::error::Error;
use crate::render::Source;

/// The alignment gate, which vector arrows ask too and so lives beside both
/// (#793). Re-exported so it keeps the path it has always had.
pub use crate::align::aligned;

/// The op vocabulary, re-exported: `core` owns it, and a host that offers a
/// Compare picker builds it from [`combine_ops`] rather than restating the
/// five tags (#342).
pub use fieldglass_core::CombineOp;

/// One cell of a combine: `op` over two values, absent when either is. A host
/// that combines values it read itself — a probe that swaps a band-limited
/// map's value for the file's full-detail one (#637) — applies the same rule
/// [`combine_values`] applies to every cell.
pub use fieldglass_core::combine_cell;

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
///
/// Punctuated by position rather than by a slice pattern on purpose: a
/// `split_last` form needs an empty arm and a one-element arm that
/// `[CombineOp; 5]` can never reach, and an arm no test can reach is an arm
/// nobody can say is right.
fn known_tags() -> String {
    let n = CombineOp::ALL.len();
    CombineOp::ALL
        .iter()
        .enumerate()
        .map(|(i, op)| {
            let separator = if i == 0 {
                ""
            } else if i + 1 == n {
                ", or "
            } else {
                ", "
            };
            format!("{separator}{:?}", op.as_str())
        })
        .collect()
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

/// The band-limit label a combine of two fields carries (#637).
///
/// Either operand band-limited makes the result band-limited, so it carries a
/// label whenever one does: the one that removed more, since that is the one
/// a host has to warn about. Two aligned operands were synthesised onto the
/// same grid and share `truncated_to`, so that is the larger `declared`; on a
/// tie, `a`'s. A host that combines two
/// fields itself — a render or a probe of a difference map — labels the result
/// with this, so the label agrees with the one [`crate::Session::combine`]
/// puts on [`Field::truncation`].
#[must_use]
pub fn combine_truncation(
    a: Option<SpectralTruncation>,
    b: Option<SpectralTruncation>,
) -> Option<SpectralTruncation> {
    match (a, b) {
        (Some(x), Some(y)) => Some(if y.declared > x.declared { y } else { x }),
        (x, y) => x.or(y),
    }
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
        truncation: combine_truncation(a.truncation.clone(), b.truncation.clone()),
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
///
/// `family` is [`Georef::label`], not `kind`: [`Source::family`] is documented
/// as the decoder's own name *because* the geometry collapses a reduced grid
/// onto its regular sibling, and `kind` is exactly that collapsed name. Before
/// #645 the umbrella had nothing else to offer here — `label` fell through to
/// `kind` for every modelled family — so this read the only string there was.
/// Now it does not, and a `Source` built here captions and refuses with the
/// family the file declared, as `fieldglass-napi`'s does from the same
/// `Georef::label`.
fn source_of(f: &Field) -> Source<'_> {
    f.source()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::align::{describe, same_grid};
    use crate::api::{Georef, Scan};
    use fieldglass_core::{GridGeometry, LambertParams, LatLonParams};

    /// Either side's label survives a combine, and of two the larger
    /// declaration wins whichever side it is on (#637, #814).
    #[test]
    fn a_combine_keeps_the_label_that_removed_more() {
        let t = |declared, truncated_to| {
            Some(SpectralTruncation {
                declared,
                truncated_to,
            })
        };
        assert_eq!(combine_truncation(None, None), None);
        assert_eq!(combine_truncation(t(383, 359), None), t(383, 359));
        assert_eq!(combine_truncation(None, t(383, 359)), t(383, 359));
        assert_eq!(combine_truncation(t(383, 359), t(7999, 359)), t(7999, 359));
        assert_eq!(combine_truncation(t(7999, 359), t(383, 359)), t(7999, 359));
    }

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

    /// `source_of` reads the family the **message** declared, not the one the
    /// geometry collapsed to.
    ///
    /// Nothing observes it today — `aligned`, the only caller, compares the
    /// geometry, the raster shape and the scan and never reads `family` — so
    /// this is the gate that keeps it right until something does. The day the
    /// umbrella routes a caption or a reprojection refusal through `source_of`,
    /// a reduced Gaussian field would otherwise say `gaussian` where
    /// `fieldglass-napi` says `reduced_gaussian`, which is the divergence #645
    /// exists to remove.
    #[test]
    fn a_source_off_a_field_names_the_family_the_file_declared() {
        let geometry = latlon(8, 4);
        let mut georef = Georef::from_declared(&geometry, Scan::north_down(), "reduced_gaussian");
        assert_eq!(georef.kind, "latlon", "the collapsed family");
        let field = |georef: Georef| Field {
            values: crate::api::Values::F64(vec![0.0; 32]),
            mask: vec![1; 32],
            ni: 8,
            nj: 4,
            georef,
            truncation: None,
            stats: crate::api::Stats {
                min: Some(0.0),
                max: Some(0.0),
                valid_count: 32,
            },
            parameter: None,
            units: None,
        };
        assert_eq!(source_of(&field(georef.clone())).family, "reduced_gaussian");

        // And it is the label rather than a constant: a field whose declared
        // family is its geometry's still reports that one.
        georef.label = "latlon".to_string();
        assert_eq!(source_of(&field(georef)).family, "latlon");
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
            points_per_row: None,
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
            truncation: None,
            stats: Stats {
                min: present.iter().copied().reduce(f64::min),
                max: present.iter().copied().reduce(f64::max),
                valid_count: present.len() as u32,
            },
            parameter: Some("Temperature".to_string()),
            units: Some("K".to_string()),
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

        // A source whose geometry did not resolve is refused against anything,
        // itself included: "same cells in the same places" is a claim about
        // geometry, and a host that could not state one cannot make it. Until
        // #574 two refusals of the same wording combined, which read two §3.20
        // grids stating `Dx = 0` over different places as one grid.
        let unplaceable = |detail: &str| Source {
            geometry: Err(Error::Unsupported {
                detail: detail.to_string(),
            }),
            ni: 8,
            nj: 4,
            scan,
            family: "latlon",
            points_per_row: None,
        };
        let no_spacing = unplaceable("dx is zero");
        let e = aligned(&no_spacing, &unplaceable("dx is zero"))
            .expect_err("two refusals of one wording are still two unknown grids");
        assert!(
            e.message().contains("no usable geometry (dx is zero)"),
            "{}",
            e.message()
        );
        assert!(aligned(&no_spacing, &unplaceable("missing latFirst")).is_err());
        // And a placeable grid against an unplaceable one, in both directions.
        assert!(aligned(&a, &no_spacing).is_err());
        assert!(aligned(&no_spacing, &a).is_err());

        // A raster that no coordinates place is not an unresolved geometry: it
        // is `Unsupported`, which compares by its label, so a coordinate-less
        // slice still differences against itself.
        let unplaced = GridGeometry::Unsupported {
            label: "source".to_string(),
            declared: None,
        };
        assert!(
            aligned(
                &source(&unplaced, 8, 4, scan, "latlon"),
                &source(&unplaced.clone(), 8, 4, scan, "latlon")
            )
            .is_ok()
        );
    }

    fn latlon_params(g: &GridGeometry) -> LatLonParams {
        match g {
            GridGeometry::LatLon(p) => *p,
            other => panic!("not a lat/lon grid: {other:?}"),
        }
    }

    /// A swath whose coordinates carry a fill value still combines with itself.
    ///
    /// `SpatialIndex` stores a non-finite centre as `NaN` so the cell indices
    /// stay aligned, and `NaN != NaN`, so its derived `PartialEq` says an index
    /// is not equal to itself rebuilt from the same file. Comparing the
    /// geometries that way refused every difference map over such a granule —
    /// including a field against itself, which is the case a user reaches by
    /// clicking Compare twice on one variable. No committed curvilinear fixture
    /// has a fill-valued coordinate, so the corpus golden cannot see this; the
    /// index is built here by hand instead.
    #[test]
    fn a_swath_with_a_fill_valued_centre_still_matches_itself() {
        let lats = [10.0, 11.0, f64::NAN, 13.0];
        let lons = [20.0, 21.0, 22.0, 23.0];
        let index = |lats: &[f64]| {
            GridGeometry::Lookup(
                fieldglass_core::SpatialIndex::new(2, 2, lats, &lons).expect("an index"),
            )
        };
        let (a, b) = (index(&lats), index(&lats));
        assert_ne!(
            a, b,
            "the hazard, stated: derived equality must still disagree here, or \
             this test proves nothing"
        );
        assert!(
            same_grid(&a, &b),
            "the same mesh, rebuilt, is the same grid"
        );

        // Two *different* meshes are still refused, so the fix is not "say yes
        // to every lookup grid".
        let elsewhere = index(&[10.0, 11.0, f64::NAN, 13.5]);
        assert!(!same_grid(&a, &elsewhere));

        // And through the gate, in both directions.
        let scan = Scan::north_down();
        assert!(
            aligned(
                &source(&a, 2, 2, scan, "curvilinear"),
                &source(&b, 2, 2, scan, "curvilinear")
            )
            .is_ok()
        );
        let e = aligned(
            &source(&a, 2, 2, scan, "curvilinear"),
            &source(&elsewhere, 2, 2, scan, "curvilinear"),
        )
        .expect_err("a different mesh");
        assert!(
            e.message().contains("their grid differs"),
            "{}",
            e.message()
        );
    }

    /// The two shapes a refusal has to describe besides the ordinary one: a
    /// family with no uniform row spacing, and one with no raster at all.
    ///
    /// Both are reachable through a real message — a Gaussian grid's rows sit on
    /// Gauss–Legendre nodes, so it states `dx` and no `dy`, and a family this
    /// build does not model states neither dimensions nor a plane. Neither is
    /// exercised by the cases above, and a description that panicked or read
    /// `from (0, 0) deg by ()` for one of them would reach a VS Code error
    /// toast before anyone noticed.
    #[test]
    fn a_refusal_describes_a_grid_with_no_row_spacing_and_one_with_no_raster() {
        let gaussian = GridGeometry::Gaussian(fieldglass_core::GaussianParams {
            ni: 8,
            nj: 4,
            lat_first: 87.0,
            lon_first: 0.0,
            lat_last: -87.0,
            lon_last: 315.0,
            n_parallels: 2,
        });
        let described = describe(&gaussian);
        assert!(
            described.starts_with("gaussian 8x4 from (0, 87) deg by (45,"),
            "{described}"
        );
        assert!(
            described.ends_with(", -)"),
            "no row spacing to state: {described}"
        );

        let unmodelled = GridGeometry::Unsupported {
            label: "healpix".to_string(),
            declared: None,
        };
        assert_eq!(
            describe(&unmodelled),
            "healpix",
            "a family with no raster and no plane reports the name the decoder \
             gave it, and nothing more"
        );
    }

    /// A rotated grid's corners are degrees of something other than longitude
    /// and latitude, and this string reaches a VS Code error toast. Unlabelled
    /// it reads as a geographic position the field is nowhere near — a COSMO
    /// domain over Europe described as sitting at 18W, 20S.
    #[test]
    fn a_refusal_says_which_frame_a_rotated_grids_degrees_are_in() {
        let rotated = GridGeometry::RotatedLatLon(fieldglass_core::RotatedLatLonParams {
            ni: 40,
            nj: 42,
            lat_first: -20.0,
            lon_first: -18.0,
            lat_last: 21.0,
            lon_last: 21.0,
            south_pole_lat: -40.0,
            south_pole_lon: 10.0,
            angle_of_rotation: 0.0,
        });
        assert_eq!(
            describe(&rotated),
            "rotated_latlon 40x42 from (-18, -20) deg (rotated frame) by (1, 1)"
        );
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
            assert_eq!(out.parameter.as_deref(), Some("Temperature"), "{op:?}");
            assert_eq!(out.units.as_deref(), Some("K"), "{op:?}");
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
