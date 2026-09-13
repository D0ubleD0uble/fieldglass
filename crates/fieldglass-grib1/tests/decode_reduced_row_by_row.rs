//! `grid_second_order_row_by_row` on a *reduced* Gaussian grid, with and
//! without the zig-zag bit (#611).
//!
//! `row_by_row` has one group per row, so on a reduced grid group `j` holds
//! `pl[j]` points. The reader used to refuse the combination, sizing its groups
//! by a single column count the grid does not have.
//!
//! eccodes 2.34.1 cannot encode this packing, so both fixtures are hand-built
//! by `tools/build_grib1_reduced_row_by_row_fixtures.py` on the N32 grid of
//! `reduced_gg_n32_smooth.grib1` (64 rows running 20 to 128 points wide, 6114
//! total), and eccodes' *decode* is the oracle for each. The builder also
//! checked that decode against the field it packed, at every point.
//!
//! **The zig-zag bit changes nothing here.** eccodes' data definition for this
//! packing is its one second-order definition without a
//! `data_apply_boustrophedonic` wrapper, so the flagged fixture decodes to the
//! same values as the plain one — and this reader used to reverse its odd rows.
//! Provenance in `tests/fixtures/NOTICE.md`.

use fieldglass_grib1::{Grib1Reader, parse_bds_header};
use serde_json::Value;
use std::path::Path;

const PLAIN: &str = "reduced_gg_row_by_row.grib1";
const ZIGZAG: &str = "reduced_gg_row_by_row_boust.grib1";
const NUM_VALUES: usize = 6114;
const NUM_ROWS: usize = 64;

fn read_fixture(name: &str) -> Vec<u8> {
    std::fs::read(Path::new("tests/fixtures").join(name))
        .unwrap_or_else(|e| panic!("read fixture {name}: {e}"))
}

fn decode(name: &str) -> Vec<Option<f64>> {
    let reader = Grib1Reader::from_bytes(read_fixture(name))
        .unwrap_or_else(|e| panic!("{name} parses: {e:?}"));
    reader
        .decode_message_values(0)
        .unwrap_or_else(|e| panic!("{name} decodes: {e:?}"))
}

fn oracle(name: &str) -> Value {
    let path = Path::new("tests/fixtures").join(name.replace(".grib1", "_expected.json"));
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read oracle {}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse oracle: {e}"))
}

/// The BDS octets of the fixture's one message.
fn bds(bytes: &[u8]) -> &[u8] {
    let reader = Grib1Reader::from_bytes(bytes.to_vec()).expect("parses");
    let range = reader.messages[0].bds_range;
    &bytes[range.start as usize..(range.start + range.len) as usize]
}

/// Count, missing count, statistics, and four points from every one of the 64
/// rows. A group sized by the wrong width shifts every later row's residuals,
/// and reversing the odd rows moves their first, second and last points, so
/// sampling each row catches both.
fn assert_matches_oracle(name: &str) {
    let values = decode(name);
    let want = oracle(name);
    let tol = want["tolerance_absolute"].as_f64().expect("tolerance");

    assert_eq!(
        values.len(),
        want["count"].as_u64().expect("count") as usize
    );
    assert_eq!(
        values.iter().filter(|v| v.is_none()).count(),
        want["missing_count"].as_u64().expect("missing_count") as usize,
        "{name}: missing count"
    );

    let present: Vec<f64> = values.iter().flatten().copied().collect();
    let min = present.iter().copied().fold(f64::INFINITY, f64::min);
    let max = present.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let mean = present.iter().sum::<f64>() / present.len() as f64;
    for (label, got) in [("min", min), ("max", max), ("mean", mean)] {
        let expected = want[label]
            .as_f64()
            .unwrap_or_else(|| panic!("oracle {label}"));
        assert!(
            (got - expected).abs() < tol,
            "{name}: {label} was {got}, expected {expected}"
        );
    }

    let samples = want["samples"].as_object().expect("oracle samples");
    assert_eq!(
        samples.len(),
        4 * NUM_ROWS,
        "{name}: oracle samples per row"
    );
    for (index, expected) in samples {
        let i: usize = index.parse().expect("sample index");
        let expected = expected.as_f64().expect("sample value");
        let got = values[i].unwrap_or_else(|| panic!("{name}: values[{i}] is masked"));
        assert!(
            (got - expected).abs() < tol,
            "{name}: values[{i}] was {got}, expected {expected}"
        );
    }
}

#[test]
fn reduced_row_by_row_matches_eccodes() {
    assert_matches_oracle(PLAIN);
}

#[test]
fn reduced_row_by_row_with_the_zigzag_bit_matches_eccodes() {
    assert_matches_oracle(ZIGZAG);
}

#[test]
fn the_zigzag_bit_does_not_reorder_row_by_row() {
    let plain = decode(PLAIN);
    assert_eq!(plain.len(), NUM_VALUES);
    assert_eq!(plain, decode(ZIGZAG));
}

/// Both fixtures are the layout under test, and they differ only in the bit —
/// read from the section rather than assumed, since a builder that failed to
/// set it would leave every test above passing for the wrong reason.
#[test]
fn the_fixtures_differ_only_in_the_zigzag_bit() {
    let (plain, zigzag) = (read_fixture(PLAIN), read_fixture(ZIGZAG));
    let differing: Vec<usize> = (0..plain.len())
        .filter(|&i| plain[i] != zigzag[i])
        .collect();
    assert_eq!(differing.len(), 1, "octets that differ: {differing:?}");

    for (bytes, want) in [(&plain, false), (&zigzag, true)] {
        let header = parse_bds_header(bds(bytes)).expect("BDS header parses");
        let ext = header.complex_extended.expect("extended flags");
        assert_eq!(ext.packing_type_label(), "grid_second_order_row_by_row");
        assert_eq!(ext.boustrophedonic(), want);
    }
}

/// The groups exercise both expansion paths and more than one residual width:
/// zero-width rows are a run of the first-order value, the rest are read
/// point by point.
#[test]
fn the_fixture_has_one_group_per_row_with_varied_widths() {
    let bytes = read_fixture(PLAIN);
    let section = bds(&bytes);
    let groups = usize::from(u16::from_be_bytes([section[16], section[17]]));
    assert_eq!(groups, NUM_ROWS, "codedNumberOfFirstOrderPackedValues");
    let widths = &section[21..21 + NUM_ROWS];
    assert!(widths.contains(&0), "a zero-width group");
    let mut nonzero: Vec<u8> = widths.iter().copied().filter(|&w| w > 0).collect();
    nonzero.sort_unstable();
    nonzero.dedup();
    assert!(nonzero.len() > 1, "residual widths: {nonzero:?}");
}
