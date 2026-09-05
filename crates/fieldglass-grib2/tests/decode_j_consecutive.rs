//! `jPointsAreConsecutive` (§3 Flag Table 3.4, bit 3) — the flag that says the
//! message stores meridians instead of parallels.
//!
//! `fieldglass-grib2` refused the bit only when bit 4 (alternate rows) was set
//! with it. Plain j-consecutive fell through `decode_message_raster` untouched,
//! so a column-major message decoded column-major and was painted row-major —
//! a transposed picture with no error and no way for a caller to tell (#602).
//! `fieldglass::Session` surfaced the flag for these messages the whole time,
//! so the API reported a layout the decoder did not act on.
//!
//! These pin the two places the flag now reaches — the raster order, and the
//! run length the second-order boustrophedonic undo reverses. It is the GRIB2
//! twin of `fieldglass-grib1/tests/decode_j_consecutive.rs` (#542), on the same
//! 8x5 grid, so the two editions read alike.

use fieldglass_grib2::Grib2Reader;

/// 8x5 column-major lat/lon ramp, `value = 10*j + i`. See
/// `tests/fixtures/NOTICE.md` and `tools/build_grib2_j_consecutive_fixture.py`.
const JCONS: &[u8] = include_bytes!("fixtures/j_consecutive_latlon.grib2");
const NI: usize = 8;
const NJ: usize = 5;

/// 16x31 second-order packed (template 5.50002) with `boustrophedonicOrdering`
/// on. Row-major as committed; the second-order test below patches its
/// scanning-mode octet.
const BOUST: &[u8] = include_bytes!("fixtures/second_order_boust_regular_latlon.grib2");
const BOUST_NI: usize = 16;
const BOUST_NJ: usize = 31;

/// Simple packing at 16 bits over a span of 47 resolves to well under 1e-3.
const TOL: f64 = 1e-3;

fn decoded(bytes: &[u8]) -> (Vec<Option<f64>>, Vec<Option<f64>>) {
    let reader = Grib2Reader::from_bytes(bytes.to_vec()).expect("parses");
    (
        reader.decode_message_values(0).expect("stored decode"),
        reader.decode_message_raster(0).expect("raster decode"),
    )
}

/// Byte offset of §3's scanning-mode octet within a single-message GRIB2 file.
///
/// `octet` is the WMO octet number within the GDS, which differs by template:
/// 72 for §3.0 (regular lat/lon). The section is walked rather than hard-coded,
/// so this survives a fixture rebuild that shifts the sections — the same shape
/// as `decode_alternate_rows.rs`'s helper.
fn scanning_mode_offset(bytes: &[u8], octet: usize) -> usize {
    let mut off = 16; // past §0
    loop {
        assert!(off + 5 <= bytes.len(), "ran off the end looking for §3");
        assert_ne!(&bytes[off..off + 4], b"7777", "message has no §3");
        let len = u32::from_be_bytes(bytes[off..off + 4].try_into().unwrap()) as usize;
        if bytes[off + 4] == 3 {
            return off + octet - 1;
        }
        off += len;
    }
}

/// `decode_message_values` keeps the message's own order, which for this
/// fixture is column-major — the contract its docs state, and the order the
/// `.eccodes.ref.json` sample block was taken in.
#[test]
fn stored_order_stays_column_major() {
    let (stored, _) = decoded(JCONS);
    assert_eq!(stored.len(), NI * NJ);
    for i in 0..NI {
        for j in 0..NJ {
            let got = stored[i * NJ + j].expect("no bitmap, so every point is present");
            let want = (10 * j + i) as f64;
            assert!(
                (got - want).abs() < TOL,
                "stored[{i}*{NJ}+{j}] = {got}, expected {want}"
            );
        }
    }
}

/// `decode_message_raster` promises `raster[j*ni + i]`. Before #602 it handed
/// back the stored order untouched, so this is the assertion that failed.
#[test]
fn raster_is_transposed_into_row_major() {
    let (stored, raster) = decoded(JCONS);
    assert_eq!(raster.len(), NI * NJ);
    for j in 0..NJ {
        for i in 0..NI {
            let got = raster[j * NI + i].expect("no bitmap, so every point is present");
            let want = (10 * j + i) as f64;
            assert!(
                (got - want).abs() < TOL,
                "raster[{j}*{NI}+{i}] = {got}, expected {want}"
            );
        }
    }
    // Ni != Nj, so the two orders are different *lists* and not merely a
    // different reading of one. A decoder that skipped the transpose would
    // still satisfy the loop above on a square grid.
    assert_ne!(
        stored, raster,
        "the fixture is column-major; a raster equal to the stored order is the bug"
    );
}

/// The transpose is exactly a permutation: nothing gained, lost or rescaled.
/// Statistics alone cannot see a scan-order bug — which is why the test above
/// checks positions — but they can see one that damages the field on the way.
#[test]
fn the_transpose_moves_points_without_changing_them() {
    let (stored, raster) = decoded(JCONS);
    let sorted = |v: &[Option<f64>]| {
        let mut f: Vec<f64> = v.iter().flatten().copied().collect();
        f.sort_by(f64::total_cmp);
        f
    };
    assert_eq!(sorted(&stored), sorted(&raster));
}

/// A grid whose rows are stored west-to-east is untouched by the new branch.
/// Every other fixture in the corpus is one, but pinning it here says the
/// regression this change could cause is being watched for, not assumed away.
#[test]
fn a_row_major_grid_is_left_alone() {
    let bytes = include_bytes!("fixtures/regular_latlon_surface.grib2");
    let (stored, raster) = decoded(bytes);
    assert_eq!(stored, raster);
}

/// A reduced grid keeps its row expansion and gains no transpose, even with the
/// bit set: it has no columns to store, and the pinned eccodes ignores the flag
/// on one.
///
/// Checked the same way as the GRIB1 half — `grib_get_data` output for
/// `reduced_gaussian_pressure_level.grib2` and `octahedral_gaussian_o32.grib2`
/// is byte-identical with and without `0x20` set on their scanning-mode octet.
/// So the raster this produces must equal the one the unpatched fixture
/// produces. What it guards is a transpose arm written to fire on any grid with
/// the bit set — the two arms match on different values of `points_per_row()`,
/// so this cannot see their order, only a pattern loose enough to catch a
/// reduced grid.
#[test]
fn a_reduced_grid_is_expanded_and_not_transposed() {
    let clean = include_bytes!("fixtures/reduced_gaussian_pressure_level.grib2").to_vec();
    let mut patched = clean.clone();
    let off = scanning_mode_offset(&patched, 72);
    assert_eq!(patched[off] & 0x20, 0, "fixture already sets bit 3");
    patched[off] |= 0x20;

    let reader = Grib2Reader::from_bytes(patched).expect("still parses");
    assert!(
        reader.messages[0]
            .gds
            .scanning_mode()
            .is_some_and(|sm| sm & fieldglass_grib2::SCAN_J_CONSECUTIVE != 0),
        "the patch did not land on the scanning-mode octet"
    );
    let (_, want) = decoded(&clean);
    assert_eq!(reader.decode_message_raster(0).expect("raster"), want);
}

/// Under j-consecutive scanning the second-order boustrophedonic undo reverses
/// alternate *columns*, because the stored run is a meridian of `Nj` points
/// rather than a parallel of `Ni`.
///
/// **The expectation is eccodes', not ours.** `j_consecutive_boust_expected.json`
/// is what the pinned `grib_get_data` prints for
/// `second_order_boust_regular_latlon.grib2` with `0x20` set on its §3
/// scanning-mode octet — bit 4 is clear on that message, so the geoiterator
/// applies no row flip and what it prints is the stored order
/// `decode_message_values` must return. Deriving the expectation by
/// re-implementing the undo at the other run length would only have proved the
/// reader picked `Nj`, not that `Nj` is right; the second assertion below still
/// makes that comparison, but now as a check that eccodes and the model agree.
///
/// The patched message itself is not committed: it differs from the existing
/// fixture in one bit, which is how `decode_alternate_rows.rs` and the GRIB1
/// half both handle the same situation.
#[test]
fn boustrophedonic_runs_are_meridians_under_j_consecutive() {
    let oracle: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/j_consecutive_boust_expected.json"))
            .expect("oracle parses");
    let tol = oracle["tolerance_absolute"].as_f64().expect("tolerance");
    let expected: Vec<f64> = oracle["values"]
        .as_array()
        .expect("values array")
        .iter()
        .map(|v| v.as_f64().expect("oracle entry is a number"))
        .collect();
    assert_eq!(expected.len(), BOUST_NI * BOUST_NJ);

    let mut bytes = BOUST.to_vec();
    let off = scanning_mode_offset(&bytes, 72);
    assert_eq!(
        bytes[off], 0,
        "expected the fixture's scanning mode at {off}"
    );
    bytes[off] |= 0x20;

    let reader = Grib2Reader::from_bytes(bytes).expect("still parses");
    let got = reader.decode_message_values(0).expect("decode");
    assert_eq!(got.len(), expected.len());
    for (k, (g, w)) in got.iter().zip(&expected).enumerate() {
        let g = g.expect("no bitmap, so every point is present");
        assert!((g - w).abs() <= tol, "point {k}: decoded {g}, eccodes {w}");
    }

    // eccodes' answer and the run-length model agree: undoing the committed
    // fixture's Ni-run reversal recovers the packed stream's own order, and
    // re-applying it at Nj reproduces the oracle. If the pin ever stopped
    // keying the run off `numberOfColumns`, the two would part here.
    let base = Grib2Reader::from_bytes(BOUST.to_vec())
        .expect("parses")
        .decode_message_values(0)
        .expect("row-major decode");
    let flip = |values: &[Option<f64>], run: usize| {
        let mut v = values.to_vec();
        for row in (1..values.len() / run).step_by(2) {
            v[row * run..(row + 1) * run].reverse();
        }
        v
    };
    let modelled = flip(&flip(&base, BOUST_NI), BOUST_NJ);
    assert_ne!(
        modelled, base,
        "the two run lengths must disagree or this proves nothing"
    );
    assert_eq!(modelled, got, "eccodes disagrees with the run-length model");
}
