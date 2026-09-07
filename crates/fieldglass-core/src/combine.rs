//! Element-wise combination of two decoded fields (difference, sum, ratio, …).
//!
//! Panoply's flagship workflow is the difference map (Array 1 − Array 2). Because
//! the render pipeline runs on a decoded `Vec<Option<f64>>` plus the grid
//! geometry, a *computed* field rides that pipeline untouched — projection,
//! overlays, palette, and manual bounds all apply to the combined field with no
//! special casing. This module is only the arithmetic; the requirement that both
//! inputs sit on identical grids is enforced by the caller. That caller is
//! `fieldglass::combine`, which compares the two fields' [`crate::GridGeometry`]
//! rather than a flat key of its own, so both hosts refuse the same pairs for
//! the same reason (#579).

/// How two aligned fields combine, element by element. `A` is the primary
/// (foreground) field, `B` the secondary.
///
/// The serde representation is the wire tag [`CombineOp::as_str`] reports and
/// [`CombineOp::from_wire`] parses, spelled out per variant rather than left to
/// a `rename_all`: `snake_case` would turn `Difference` into `"difference"` and
/// the two spellings would drift apart the first time a host read one and wrote
/// the other. `the_serde_tag_is_the_wire_tag` holds them in step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[non_exhaustive]
pub enum CombineOp {
    /// `A − B` — the difference / anomaly map.
    #[cfg_attr(feature = "serde", serde(rename = "a_minus_b"))]
    Difference,
    /// `B − A`.
    #[cfg_attr(feature = "serde", serde(rename = "b_minus_a"))]
    ReverseDifference,
    /// `A + B`.
    #[cfg_attr(feature = "serde", serde(rename = "a_plus_b"))]
    Sum,
    /// `(A + B) / 2`.
    #[cfg_attr(feature = "serde", serde(rename = "mean"))]
    Mean,
    /// `A / B`.
    #[cfg_attr(feature = "serde", serde(rename = "ratio"))]
    Ratio,
}

impl CombineOp {
    /// Stable lowercase wire tag for the napi / UI layer.
    pub fn as_str(&self) -> &'static str {
        match self {
            CombineOp::Difference => "a_minus_b",
            CombineOp::ReverseDifference => "b_minus_a",
            CombineOp::Sum => "a_plus_b",
            CombineOp::Mean => "mean",
            CombineOp::Ratio => "ratio",
        }
    }

    /// Parse a wire tag back into an op. Unknown tags return `None` so the
    /// caller can reject a typo rather than silently picking an operation.
    pub fn from_wire(tag: &str) -> Option<Self> {
        Some(match tag {
            "a_minus_b" => CombineOp::Difference,
            "b_minus_a" => CombineOp::ReverseDifference,
            "a_plus_b" => CombineOp::Sum,
            "mean" => CombineOp::Mean,
            "ratio" => CombineOp::Ratio,
            _ => return None,
        })
    }

    /// Every operation, in menu order. This is the single source both hosts
    /// surface (through `fieldglass::combine_ops`) so each UI picker, its
    /// validation set, and the tests all derive their op vocabulary from here
    /// rather than repeating it — a new op added to the enum flows out
    /// automatically.
    pub const ALL: [CombineOp; 5] = [
        CombineOp::Difference,
        CombineOp::ReverseDifference,
        CombineOp::Sum,
        CombineOp::Mean,
        CombineOp::Ratio,
    ];

    /// Human-readable label for the operation menu (e.g. `A − B`).
    pub fn label(&self) -> &'static str {
        match self {
            CombineOp::Difference => "A − B",
            CombineOp::ReverseDifference => "B − A",
            CombineOp::Sum => "A + B",
            CombineOp::Mean => "mean(A, B)",
            CombineOp::Ratio => "A / B",
        }
    }

    /// The scalar operation on two present values.
    fn apply(self, a: f64, b: f64) -> f64 {
        match self {
            CombineOp::Difference => a - b,
            CombineOp::ReverseDifference => b - a,
            CombineOp::Sum => a + b,
            CombineOp::Mean => (a + b) / 2.0,
            CombineOp::Ratio => a / b,
        }
    }
}

/// Combine two aligned fields element by element under `op`.
///
/// A cell is present in the output only when it is present in **both** inputs
/// *and* the result is finite; otherwise it is missing (`None`). Missing
/// propagates (a hole in either input is a hole in the output), and the ratio's
/// divide-by-zero (`A/0`, `0/0`) falls out as missing rather than painting
/// `±inf` / `NaN`.
///
/// The output has the primary field's length (`a.len()`), so it always matches
/// `A`'s grid geometry — the geometry the caller renders it against. The caller
/// guarantees identical grids (so `b.len() == a.len()`); if `b` is somehow
/// shorter, its absent tail reads as missing rather than panicking.
pub fn combine_fields(a: &[Option<f64>], b: &[Option<f64>], op: CombineOp) -> Vec<Option<f64>> {
    (0..a.len())
        .map(|i| combine_cell(a[i], b.get(i).copied().flatten(), op))
        .collect()
}

/// One cell of [`combine_fields`], for a caller that has the two fields in some
/// other shape.
///
/// `fieldglass::Session::combine` holds each field as a contiguous `Vec<f64>`
/// plus a `u8` mask (ADR-0006 decision 2) and walks them cell by cell rather
/// than building two `Vec<Option<f64>>` first: that shape is 16 bytes a cell, so
/// a 3.7-million-point NBM field would cost 120 MB of linear memory on top of
/// the two fields it already holds. Exported so that walk applies **this** rule
/// — present in both, and finite — rather than a second copy of it that could
/// drift.
#[must_use]
pub fn combine_cell(a: Option<f64>, b: Option<f64>, op: CombineOp) -> Option<f64> {
    match (a, b) {
        (Some(x), Some(y)) => {
            let v = op.apply(x, y);
            v.is_finite().then_some(v)
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_op_computes_the_expected_value() {
        let a = vec![Some(10.0), Some(3.0)];
        let b = vec![Some(4.0), Some(6.0)];
        assert_eq!(
            combine_fields(&a, &b, CombineOp::Difference),
            vec![Some(6.0), Some(-3.0)]
        );
        assert_eq!(
            combine_fields(&a, &b, CombineOp::ReverseDifference),
            vec![Some(-6.0), Some(3.0)]
        );
        assert_eq!(
            combine_fields(&a, &b, CombineOp::Sum),
            vec![Some(14.0), Some(9.0)]
        );
        assert_eq!(
            combine_fields(&a, &b, CombineOp::Mean),
            vec![Some(7.0), Some(4.5)]
        );
        assert_eq!(
            combine_fields(&a, &b, CombineOp::Ratio),
            vec![Some(2.5), Some(0.5)]
        );
    }

    #[test]
    fn missing_in_either_input_propagates() {
        let a = vec![Some(1.0), None, Some(3.0), None];
        let b = vec![Some(2.0), Some(9.0), None, None];
        // Only index 0 is present in both.
        for op in [
            CombineOp::Difference,
            CombineOp::ReverseDifference,
            CombineOp::Sum,
            CombineOp::Mean,
            CombineOp::Ratio,
        ] {
            let out = combine_fields(&a, &b, op);
            assert!(out[0].is_some(), "{op:?} index 0");
            assert_eq!(out[1], None, "{op:?} index 1 (B present, A missing)");
            assert_eq!(out[2], None, "{op:?} index 2 (A present, B missing)");
            assert_eq!(out[3], None, "{op:?} index 3 (both missing)");
        }
    }

    #[test]
    fn ratio_divide_by_zero_is_missing_not_infinite() {
        // A/0 → +inf and 0/0 → NaN both fall out as missing rather than
        // painting a non-finite value the renderer would have to special-case.
        let a = vec![Some(5.0), Some(0.0), Some(-5.0)];
        let b = vec![Some(0.0), Some(0.0), Some(0.0)];
        assert_eq!(
            combine_fields(&a, &b, CombineOp::Ratio),
            vec![None, None, None]
        );
    }

    #[test]
    fn a_non_finite_input_yields_missing() {
        // A field can carry a non-finite value (e.g. an unmasked sentinel that
        // decoded to inf); combining with it must not emit a non-finite cell.
        let a = vec![Some(f64::INFINITY), Some(f64::NAN)];
        let b = vec![Some(1.0), Some(1.0)];
        assert_eq!(
            combine_fields(&a, &b, CombineOp::Sum),
            vec![None, None],
            "inf/NaN + finite is non-finite → missing"
        );
    }

    #[test]
    fn output_matches_the_primary_length_even_if_b_is_short() {
        let a = vec![Some(1.0), Some(2.0), Some(3.0)];
        let b = vec![Some(10.0)];
        let out = combine_fields(&a, &b, CombineOp::Sum);
        assert_eq!(out.len(), 3, "output tracks A's geometry");
        assert_eq!(out[0], Some(11.0));
        assert_eq!(out[1], None, "B has no cell here");
        assert_eq!(out[2], None);
    }

    #[test]
    fn empty_inputs_combine_to_empty() {
        assert!(combine_fields(&[], &[], CombineOp::Difference).is_empty());
    }

    #[test]
    fn wire_tags_round_trip() {
        for op in CombineOp::ALL {
            assert_eq!(CombineOp::from_wire(op.as_str()), Some(op));
        }
        assert_eq!(CombineOp::from_wire("product"), None);
        assert_eq!(CombineOp::from_wire(""), None);
    }

    /// The serde tag and [`CombineOp::as_str`] are the same string by contract.
    ///
    /// Two spellings of one vocabulary is how a host that reads the JSON form
    /// and one that calls the method come to disagree — `fieldglass`'s
    /// conformance `Args` deserialises an op while the extension's picker sends
    /// `as_str`, so the two really are both in use.
    // The wire half of the vocabulary (#641); `as_str` and `from_wire` beside
    // it are the same with the derives off.
    #[cfg(feature = "serde")]
    #[test]
    fn the_serde_tag_is_the_wire_tag() {
        for op in CombineOp::ALL {
            let json = serde_json::to_string(&op).expect("serialises");
            assert_eq!(json, format!("\"{}\"", op.as_str()), "{op:?}");
            let back: CombineOp = serde_json::from_str(&json).expect("round trips");
            assert_eq!(back, op);
        }
        // And a tag `from_wire` rejects is one serde rejects too.
        assert!(serde_json::from_str::<CombineOp>("\"product\"").is_err());
    }

    #[test]
    fn all_ops_have_distinct_non_empty_tags_and_labels() {
        // `ALL` is the single source both hosts derive their op list from (see
        // `fieldglass::combine_ops`), so every entry must carry a usable,
        // unique wire tag and menu label.
        let mut tags = std::collections::HashSet::new();
        let mut labels = std::collections::HashSet::new();
        for op in CombineOp::ALL {
            assert!(!op.as_str().is_empty(), "{op:?} has an empty wire tag");
            assert!(!op.label().is_empty(), "{op:?} has an empty label");
            assert!(
                tags.insert(op.as_str()),
                "duplicate wire tag {}",
                op.as_str()
            );
            assert!(labels.insert(op.label()), "duplicate label {}", op.label());
        }
        assert_eq!(tags.len(), CombineOp::ALL.len());
    }
}
