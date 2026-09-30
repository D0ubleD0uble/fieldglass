//! Two messages on one grid, one stored column-major, pair cell for cell
//! (#792), and a u/v pair is checked the way a combine is (#793).
//!
//! Both GRIB readers transpose a `j`-consecutive regular grid to row-major
//! while decoding, so `Georef::scan` describes the decoded raster and reports
//! `jConsecutive = false` for it. It used to report the stored bit, and
//! `combine::aligned` then refused a column-major message against a row-major
//! copy of the same grid for a scan-order difference their values do not have.
//!
//! The row-major copy is the committed column-major fixture with scanning-mode
//! bit `0x20` cleared, made in memory as `decode_j_consecutive.rs` in each
//! reader does: a one-bit edit to a small file. The copy's values are the same
//! stored sequence read the other way, so it is a different field on the same
//! grid, which is all a combine needs. The direction bits (`0x80`, `0x40`) are
//! set the same way to show a real scan difference is still refused.

use fieldglass::render::{VectorOptions, vector_polylines};
use fieldglass::{CombineOp, DecodeOptions, Field, RenderOptions, Session};

const G1_JCONS: &str = "../fieldglass-grib1/tests/fixtures/j_consecutive_latlon.grib1";
const G2_JCONS: &str = "../fieldglass-grib2/tests/fixtures/j_consecutive_latlon.grib2";

const I_NEGATIVE: u8 = 0x80;
const J_POSITIVE: u8 = 0x40;
const J_CONSECUTIVE: u8 = 0x20;

fn read(path: &str) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// Byte index of GDS octet 28, the scanning mode, in a single-message GRIB1
/// file: past the 8-byte IS and the PDS, whose 3-byte length leads it.
fn grib1_scan_octet(bytes: &[u8]) -> usize {
    let pds_len = usize::from(bytes[8]) << 16 | usize::from(bytes[9]) << 8 | usize::from(bytes[10]);
    8 + pds_len + 27
}

/// Byte index of §3 octet 72, the scanning mode of template 3.0, in a
/// single-message GRIB2 file. The sections are walked rather than hard-coded.
fn grib2_scan_octet(bytes: &[u8]) -> usize {
    let mut off = 16; // past §0
    loop {
        assert_ne!(&bytes[off..off + 4], b"7777", "message has no §3");
        let len = u32::from_be_bytes(bytes[off..off + 4].try_into().unwrap()) as usize;
        if bytes[off + 4] == 3 {
            return off + 72 - 1;
        }
        off += len;
    }
}

/// The fixture followed by three copies of it, in one file. Message 0 is the
/// original, 1 the row-major copy, 2 the original with `i` reversed and 3 the
/// original with `j` reversed.
fn file_of_variants(original: &[u8], at: usize) -> Vec<u8> {
    assert_ne!(
        original[at] & J_CONSECUTIVE,
        0,
        "the fixture is meant to be stored column-major"
    );
    assert_eq!(original[at] & (I_NEGATIVE | J_POSITIVE), 0);
    let with = |octet: u8| {
        let mut copy = original.to_vec();
        copy[at] = octet;
        copy
    };
    let mut file = original.to_vec();
    file.extend(with(original[at] & !J_CONSECUTIVE));
    file.extend(with(original[at] | I_NEGATIVE));
    file.extend(with(original[at] | J_POSITIVE));
    file
}

fn cells(field: &Field) -> Vec<Option<f64>> {
    let values = field.values.to_f64();
    field
        .mask
        .iter()
        .zip(values.iter())
        .map(|(&m, &v)| (m == 1).then_some(v))
        .collect()
}

fn check_edition(name: &str, file: Vec<u8>) {
    let session = Session::open(file).unwrap_or_else(|e| panic!("{name}: {e}"));
    assert_eq!(session.count(), 4, "{name}");
    let decode = |i: u32| {
        session
            .decode(i, &DecodeOptions::default())
            .unwrap_or_else(|e| panic!("{name} #{i}: {e}"))
    };
    let (column_major, row_major) = (decode(0), decode(1));

    // The scan is the decoded raster's, which is row-major for both — as the
    // field reports it and as the message list does.
    for (i, field) in [(0, &column_major), (1, &row_major)] {
        assert!(!field.georef.scan.j_consecutive, "{name} #{i}");
        let listed = session.message(i).expect("message").grid.expect("a grid");
        assert!(!listed.scan.j_consecutive, "{name} #{i} as listed");
    }
    assert_eq!(column_major.georef.scan, row_major.georef.scan, "{name}");

    // The two combine, and the result is the element-wise difference.
    let difference = session
        .combine(&column_major, &row_major, CombineOp::Difference)
        .unwrap_or_else(|e| panic!("{name}: the same grid in two orders: {e}"));
    let (a, b) = (cells(&column_major), cells(&row_major));
    let expected: Vec<Option<f64>> = a
        .iter()
        .zip(&b)
        .map(|(a, b)| Some(a.expect("no bitmap") - b.expect("no bitmap")))
        .collect();
    assert_eq!(cells(&difference), expected, "{name}");
    assert!(
        expected.iter().any(|d| d.is_some_and(|d| d != 0.0)),
        "{name}: the two orders are different fields, so the check discriminates"
    );

    // A real scan difference is still refused, for each direction flag.
    for (i, flag) in [(2, "iNegative"), (3, "jPositive")] {
        let flipped = decode(i);
        let err = session
            .combine(&column_major, &flipped, CombineOp::Difference)
            .expect_err("a reversed axis holds its cells elsewhere");
        assert_eq!(err.code(), "unsupported", "{name} #{i}");
        assert!(
            err.message().contains("their scan order differs")
                && err.message().contains(&format!("{flag}=true")),
            "{name} #{i}: {}",
            err.message()
        );
    }

    // Vector arrows pair their two fields the same way, and ask the same gate:
    // the two orders make a u/v pair, and a reversed `j` does not (#793).
    let options = RenderOptions::new("source", "nearest");
    let vectors = VectorOptions::new();
    vector_polylines(
        &column_major.source(),
        &a,
        &row_major.source(),
        &b,
        &options,
        &vectors,
    )
    .unwrap_or_else(|e| panic!("{name}: arrows over the same grid in two orders: {e}"));
    let south_up = decode(3);
    let err = vector_polylines(
        &column_major.source(),
        &a,
        &south_up.source(),
        &cells(&south_up),
        &options,
        &vectors,
    )
    .expect_err("a v scanned the other way up");
    assert!(
        err.message().contains("their scan order differs"),
        "{name}: {}",
        err.message()
    );
}

#[test]
fn a_column_major_grib1_message_combines_with_a_row_major_one() {
    let original = read(G1_JCONS);
    let at = grib1_scan_octet(&original);
    check_edition("grib1", file_of_variants(&original, at));
}

#[test]
fn a_column_major_grib2_message_combines_with_a_row_major_one() {
    let original = read(G2_JCONS);
    let at = grib2_scan_octet(&original);
    check_edition("grib2", file_of_variants(&original, at));
}
