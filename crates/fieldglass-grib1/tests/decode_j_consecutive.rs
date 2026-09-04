//! `jPointsAreConsecutive` (GDS octet 28, bit 3) — the flag that says the
//! message stores meridians instead of parallels.
//!
//! The bit was parsed and cross-checked against eccodes from the start, and
//! then consumed by nothing: a column-major message decoded column-major and
//! was painted row-major, which is a transposed picture with no error and no
//! way for a caller to tell (#542). These pin the two places the flag now
//! reaches — the raster order, and the run length the boustrophedonic undo
//! reverses.

use fieldglass_grib1::Grib1Reader;

/// 8x5 column-major lat/lon ramp, `value = 10*j + i`. See
/// `tests/fixtures/NOTICE.md` and `tools/build_grib1_j_consecutive_fixture.py`.
const JCONS: &[u8] = include_bytes!("fixtures/j_consecutive_latlon.grib1");
const NI: usize = 8;
const NJ: usize = 5;

/// 240x121 `grid_second_order_SPD3` with boustrophedonic ordering on. Row-major
/// as committed; the second-order test below patches its scanning-mode octet.
const BOUST: &[u8] = include_bytes!("fixtures/ecmwf_spd3_boust_msg0.grib1");
const BOUST_NI: usize = 240;
const BOUST_NJ: usize = 121;

/// Simple packing at 16 bits over a span of 44 resolves to under 1e-3.
const TOL: f64 = 1e-3;

fn decoded(bytes: &[u8]) -> (Vec<Option<f64>>, Vec<Option<f64>>) {
    let reader = Grib1Reader::from_bytes(bytes.to_vec()).expect("parses");
    (
        reader.decode_message_values(0).expect("stored decode"),
        reader.decode_message_raster(0).expect("raster decode"),
    )
}

/// Byte index of the GDS scanning-mode octet (octet 28) in a single-message
/// GRIB1 file: past the 8-byte IS and the PDS, whose 3-byte length leads it.
fn scanning_mode_offset(bytes: &[u8]) -> usize {
    let pds_len = usize::from(bytes[8]) << 16 | usize::from(bytes[9]) << 8 | usize::from(bytes[10]);
    8 + pds_len + 27
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

/// `decode_message_raster` promises `raster[j*ni + i]`. Before #542 it handed
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
    let bytes = include_bytes!("fixtures/cmc_wind_300_2010052400_p012.grib");
    let (stored, raster) = decoded(bytes);
    assert_eq!(stored, raster);
}

/// Under j-consecutive scanning the boustrophedonic undo reverses alternate
/// *columns*, because the stored run is a meridian of `Nj` points rather than a
/// parallel of `Ni`.
///
/// eccodes 2.34.1 is the oracle for that, and it was checked directly:
///
/// ```text
/// python3 -c "b=bytearray(open('ecmwf_spd3_boust_msg0.grib1','rb').read()); \
///             b[63] |= 0x20; open('patched.grib1','wb').write(bytes(b))"
/// grib_get_data patched.grib1
/// ```
///
/// decodes to the committed fixture's field with its odd 240-runs un-reversed
/// and its odd 121-runs reversed instead — the field this test derives. The
/// patch is applied here rather than committed because it differs from
/// `ecmwf_spd3_boust_msg0.grib1` in one bit and that fixture is 55 kB; the same
/// reason `fieldglass-grib2`'s alternate-row suite patches its own scanning
/// octet in the test.
#[test]
fn boustrophedonic_runs_are_meridians_under_j_consecutive() {
    let base = Grib1Reader::from_bytes(BOUST.to_vec())
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
    let off = scanning_mode_offset(&bytes);
    assert_eq!(
        bytes[off], 0,
        "expected the fixture's scanning mode at {off}"
    );
    bytes[off] |= 0x20;

    let reader = Grib1Reader::from_bytes(bytes).expect("still parses");
    assert!(
        reader.messages[0]
            .gds
            .as_ref()
            .and_then(|g| g.scanning_mode())
            .expect("a lat/lon grid has a scanning mode")
            .j_consecutive
    );
    assert_eq!(reader.decode_message_values(0).expect("decode"), expected);
}
