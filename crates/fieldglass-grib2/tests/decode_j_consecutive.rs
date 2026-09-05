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
/// produces, which is what the reduced arm's precedence in the match delivers.
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
/// eccodes 2.34.1 is the oracle for that, and it was checked directly: setting
/// `0x20` on `second_order_boust_regular_latlon.grib2`'s §3 scanning-mode octet
/// and diffing `grib_get_data` decodes to the committed fixture's field with
/// its odd 16-runs un-reversed and its odd 31-runs reversed instead — the field
/// this test derives. The patch is applied here rather than committed because
/// the file would differ from the existing fixture in one bit, which is how
/// `decode_alternate_rows.rs` and the GRIB1 half both handle it.
#[test]
fn boustrophedonic_runs_are_meridians_under_j_consecutive() {
    let base = Grib2Reader::from_bytes(BOUST.to_vec())
        .expect("parses")
        .decode_message_values(0)
        .expect("row-major decode");
    assert_eq!(base.len(), BOUST_NI * BOUST_NJ);

    // Undo the Ni-run reversal to recover the packed stream's own order, then
    // re-apply it at the Nj run length the flag asks for.
    let flip = |values: &[Option<f64>], run: usize| {
        let mut v = values.to_vec();
        for row in (1..values.len() / run).step_by(2) {
            v[row * run..(row + 1) * run].reverse();
        }
        v
    };
    let expected = flip(&flip(&base, BOUST_NI), BOUST_NJ);
    assert_ne!(
        expected, base,
        "the two run lengths must disagree or this proves nothing"
    );

    let mut bytes = BOUST.to_vec();
    let off = scanning_mode_offset(&bytes, 72);
    assert_eq!(
        bytes[off], 0,
        "expected the fixture's scanning mode at {off}"
    );
    bytes[off] |= 0x20;

    let reader = Grib2Reader::from_bytes(bytes).expect("still parses");
    assert_eq!(reader.decode_message_values(0).expect("decode"), expected);
}
