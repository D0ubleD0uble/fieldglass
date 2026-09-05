//! Second-order packing on a *reduced* Gaussian grid, forwards and
//! boustrophedonic (#605).
//!
//! Second-order packing is ECMWF's and ECMWF's operational grids are reduced
//! Gaussian, so the two go together in the wild — but nothing paired them here
//! until now, and the reader skipped the boustrophedonic undo whenever the grid
//! had no single column count. Every second row of such a message came back
//! backwards, and each value was a plausible temperature, so nothing downstream
//! could tell.
//!
//! Both fixtures are repacked from the committed `reduced_gg_n32_smooth.grib1`
//! (N32: 64 rows running 20 to 128 points wide, 6114 total) by
//! `tools/build_grib1_reduced_second_order_fixtures.py`. They hold **one
//! field** in two storage orders, which is the property under test.
//!
//! **The oracle is not eccodes' decode of the boustrophedonic one.**
//! `DataApplyBoustrophedonic::unpack` takes a separate branch when the message
//! has a `pl` key, and that branch walks down from `start + pl[j]` where the
//! uniform branch uses `start + numberOfColumns - 1`. Every odd row lands one
//! slot right, and on a 64-row grid the last row is odd, so it writes one
//! element past the end of the value buffer: eccodes 2.34.1 segfaults on any
//! key that decodes this message. Its `pack_double` is correct (pre-decrement),
//! so the fixture is built by asking eccodes to *write* the boustrophedonic
//! form and taking the expected values from the forward-stored sibling, which
//! it decodes correctly. Provenance in `tests/fixtures/NOTICE.md`.

use fieldglass_grib1::{Grib1Reader, parse_bds_header};
use serde_json::Value;
use std::path::Path;

const FORWARDS: &str = "reduced_gg_second_order.grib1";
const BOUSTROPHEDONIC: &str = "reduced_gg_second_order_boust.grib1";
const NUM_VALUES: usize = 6114;

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

/// Both fixtures decode to the eccodes oracle: count, statistics, and the four
/// sampled points from every one of the 64 rows.
///
/// Sampling *every* row is what separates "the undo ran" from "the undo ran
/// with the right per-row widths". A reversal that used one width for a ragged
/// grid would leave the first rows right and drift from there; 125 of the 256
/// sampled points move if the undo is skipped entirely, which is what the
/// reader used to do.
fn assert_matches_oracle(name: &str) {
    let values = decode(name);
    let want = oracle(name);
    let tol = want["tolerance_absolute"].as_f64().expect("tolerance");

    assert_eq!(
        values.len(),
        want["count"].as_u64().expect("count") as usize,
        "{name}: value count"
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
    assert_eq!(samples.len(), 256, "{name}: oracle samples per row");
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
fn forward_stored_reduced_second_order_matches_eccodes() {
    assert_matches_oracle(FORWARDS);
}

#[test]
fn boustrophedonic_reduced_second_order_matches_eccodes() {
    assert_matches_oracle(BOUSTROPHEDONIC);
}

/// The pair holds one field in two storage orders, so the decoder must return
/// the same values for both — the property #605 broke.
#[test]
fn the_reduced_pair_decodes_to_one_field() {
    let forwards = decode(FORWARDS);
    assert_eq!(forwards.len(), NUM_VALUES);
    assert_eq!(
        forwards,
        decode(BOUSTROPHEDONIC),
        "boustrophedonic ordering is a storage detail; the field is the same"
    );
    // …and the two really do store it differently, or the assertion above would
    // hold with the undo missing.
    assert_ne!(
        read_fixture(FORWARDS),
        read_fixture(BOUSTROPHEDONIC),
        "the two fixtures are the same octets"
    );
}

/// The flag the decode branches on is the one eccodes reports, and only the
/// second fixture sets it. Read from the BDS rather than assumed, because a
/// build step that quietly failed to flip it would leave both tests above
/// passing for the wrong reason.
#[test]
fn only_the_second_fixture_sets_boustrophedonic_ordering() {
    for (name, want) in [(FORWARDS, false), (BOUSTROPHEDONIC, true)] {
        let bytes = read_fixture(name);
        let reader = Grib1Reader::from_bytes(bytes.clone()).expect("parses");
        let (start, end) = reader.messages[0].bds_range;
        let bds = parse_bds_header(&bytes[start..end]).expect("BDS header parses");
        let ext = bds
            .complex_extended
            .as_ref()
            .expect("second-order BDS carries the extended flags");
        assert_eq!(
            ext.boustrophedonic(),
            want,
            "{name}: boustrophedonicOrdering"
        );
        assert_eq!(ext.packing_type_label(), "grid_second_order", "{name}");
    }
}

/// eccodes writes this pair with GRIB1 octet-4 bit 4 (`additionalFlagPresent`)
/// **clear**, and reads the extended-flag octet anyway — its
/// `grib1/section.4.def` gates that block on the complex bit alone, and none of
/// its `grid_second_order*` concepts constrain the bit. Requiring it here
/// refused these messages outright ("complex packing without extra-flags
/// octet"), which is how the gap surfaced while building the #605 fixtures.
#[test]
fn a_second_order_section_decodes_without_the_additional_flag_bit() {
    let bytes = read_fixture(FORWARDS);
    let reader = Grib1Reader::from_bytes(bytes.clone()).expect("parses");
    let (start, end) = reader.messages[0].bds_range;
    let bds = parse_bds_header(&bytes[start..end]).expect("BDS header parses");
    assert!(bds.is_complex_packing, "complex packing flag");
    assert!(
        !bds.has_extra_flags,
        "eccodes leaves octet 4 bit 4 clear when it writes GRIB1 second-order"
    );
    assert!(
        bds.complex_extended.is_some(),
        "the extended-flag octet is still read, as eccodes reads it"
    );
}
