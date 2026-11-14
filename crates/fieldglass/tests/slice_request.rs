//! `check_slice_request` and `check_slice_axes`, the checks of a slice
//! request (#1804).
//!
//! `decode_slice`, the window calls and the napi NetCDF handle's plane read
//! refuse a bad request through `check_slice_request`; `place_slice` and the
//! napi handle's placement through `check_slice_axes`. So each says the same
//! thing in the same words, and a request wrong in more than one way is
//! refused for the same reason everywhere: the length, then the axis pair,
//! then the held indices.
//! The napi side is held to it in `fieldglass-napi`'s conformance host, the
//! window calls in `zarr_window.rs`, `place_slice` in `place_slice.rs`.
//!
//! Paths are relative for the `wasm32-wasip1` run's sandbox.

use fieldglass::{DecodeOptions, Error, Session, check_slice_axes, check_slice_request};
use fieldglass_core::array::Dimension;

/// `sst(time, lev, lat, lon)`; `lev` holds one level.
const ERSST: &str = "../fieldglass-netcdf/tests/fixtures/ersst_v5_187001_cdf1.nc";

fn dims() -> Vec<Dimension> {
    [("time", 2), ("lev", 1), ("lat", 3), ("lon", 4)]
        .into_iter()
        .map(|(name, length)| Dimension {
            name: name.to_string(),
            length,
        })
        .collect()
}

fn refused(y: usize, x: usize, indices: &[u32]) -> Error {
    check_slice_request("sst", &dims(), y, x, indices).expect_err("refused")
}

/// Each refusal, word for word, so a reword is a decision and not an accident.
/// The wording is an explanation, not API; the codes are API.
#[test]
fn each_refusal_is_worded_once() {
    let short = refused(2, 3, &[0, 0]);
    assert_eq!(short.code(), "invalid_option");
    assert_eq!(
        short.message(),
        "2 indices given for \"sst\", which has 4 dimensions \
         (one per dimension; the two drawn are ignored)"
    );
    assert_eq!(
        refused(2, 3, &[0]).message(),
        "1 index given for \"sst\", which has 4 dimensions \
         (one per dimension; the two drawn are ignored)"
    );

    // The same axis twice, both in range.
    let same = "Y and X are both dimension 3 of \"sst\"; a slice draws two different dimensions";
    // An axis past the rank names the first one past it, Y before X, even
    // when both are past it or the two are the same.
    let past =
        |d: usize| format!("dimension {d} is out of bounds for \"sst\", which has 4 dimensions");
    for (y, x, want) in [
        (3, 3, same.to_string()),
        (5, 3, past(5)),
        (2, 4, past(4)),
        (9, 7, past(9)),
        (7, 9, past(7)),
        (9, 9, past(9)),
    ] {
        let err = refused(y, x, &[0, 0, 0, 0]);
        assert_eq!(err.code(), "invalid_option", "({y}, {x})");
        assert_eq!(err.message(), want, "({y}, {x})");
        // Placing a slice has no held indices and is told the same thing.
        let placed = check_slice_axes("sst", &dims(), y, x).expect_err("refused");
        assert_eq!(
            (placed.code(), placed.message()),
            (err.code(), err.message())
        );
    }

    let past = refused(2, 3, &[0, 1, 0, 0]);
    assert_eq!(past.code(), "invalid_option");
    assert_eq!(
        past.message(),
        "index 1 is past the end of \"sst\"'s dimension 1 (\"lev\", length 1)"
    );
}

/// A request wrong two ways is refused for the first check it fails.
#[test]
fn the_length_then_the_axis_pair_then_the_held_indices() {
    let length = |err: &Error| err.message().contains("given for");
    let pair = |err: &Error| {
        let m = err.message();
        m.contains("a slice draws two different") || m.contains("is out of bounds")
    };

    // The length before the axis pair, either way the pair is wrong.
    assert!(length(&refused(3, 3, &[0, 0])));
    assert!(length(&refused(9, 3, &[0])));
    // The axis pair before the held indices.
    assert!(pair(&refused(3, 3, &[0, 1, 0, 0])));
    assert!(pair(&refused(2, 9, &[0, 1, 0, 0])));
    // And an axis of length 0 is the held-index check's, after the pair.
    let mut empty = dims();
    empty[0].length = 0;
    let err = check_slice_request("sst", &empty, 3, 3, &[0, 0, 0, 0]).expect_err("refused");
    assert!(pair(&err), "{err}");
    let err = check_slice_request("sst", &empty, 2, 3, &[0, 0, 0, 0]).expect_err("refused");
    assert_eq!(err.code(), "unsupported", "{err}");
}

/// `check_slice_axes` checks the axis pair alone: every bad pair is refused,
/// and a slice placed on an empty axis is placed, as `place_slice` places one.
#[test]
fn check_slice_axes_checks_the_axis_pair_alone() {
    for (y, x) in [(3, 3), (0, 0), (4, 3), (2, 4), (9, 9)] {
        let err = check_slice_axes("sst", &dims(), y, x).expect_err("not a pair");
        assert_eq!(err.code(), "invalid_option", "({y}, {x}): {err}");
    }
    let mut empty = dims();
    empty[0].length = 0;
    assert!(check_slice_axes("sst", &empty, 2, 3).is_ok());
    assert!(check_slice_axes("sst", &dims(), 2, 3).is_ok());
    assert!(check_slice_axes("sst", &dims(), 3, 2).is_ok());
    assert!(check_slice_request("sst", &dims(), 2, 3, &[0, 0, 0, 0]).is_ok());
}

/// Through a real file: `decode_slice` refuses with exactly what the check says.
#[test]
fn decode_slice_refuses_with_the_check() {
    let session = Session::open(std::fs::read(ERSST).expect("the fixture")).expect("opens");
    let var = session
        .variables()
        .into_iter()
        .find(|v| v.name == "sst")
        .expect("sst");
    let dims: Vec<Dimension> = var
        .dims
        .iter()
        .map(|d| Dimension {
            name: d.name.clone(),
            length: d.length,
        })
        .collect();
    for (y, x, indices) in [
        (2, 3, vec![0, 0]),
        (3, 3, vec![0, 0]),
        (3, 3, vec![0, 1, 0, 0]),
        (9, 3, vec![0, 0, 0, 0]),
        (2, 3, vec![0, 1, 0, 0]),
    ] {
        let err = session
            .decode_slice(var.index, y, x, &indices, &DecodeOptions::default())
            .expect_err("refused");
        let want = check_slice_request("sst", &dims, y as usize, x as usize, &indices)
            .expect_err("the check refuses it too");
        assert_eq!(
            (err.code(), err.message()),
            (want.code(), want.message()),
            "({y}, {x}, {indices:?})"
        );
    }
}

/// A line's wrong length is worded as a slice's is, ending in the count given.
#[test]
fn a_line_of_the_wrong_length_is_worded_as_a_slice_is() {
    let session = Session::open(std::fs::read(ERSST).expect("the fixture")).expect("opens");
    let var = session
        .variables()
        .into_iter()
        .find(|v| v.name == "sst")
        .expect("sst");
    let err = session
        .decode_line(var.index, 0, &[0, 0], &DecodeOptions::default())
        .expect_err("two indices for four dimensions");
    assert_eq!(err.code(), "invalid_option");
    assert_eq!(
        err.message(),
        "2 indices given for \"sst\", which has 4 dimensions \
         (one per dimension; the one being read along is ignored)"
    );
}
