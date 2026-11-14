//! The numcodecs `fixedscaleoffset` filter in a Zarr v2 array (#1823).
//!
//! numcodecs encodes `round((x - offset) * scale)` as `astype` and decodes
//! `enc / scale + offset`, cast to `dtype`. NASA POWER's map stores carry it
//! on nearly every field.
//!
//! Over `fixedscaleoffset/`, a consolidated v2 group written by zarr-python
//! and numcodecs, whose oracle (`fixedscaleoffset_oracle.json`) states the
//! values zarr-python reads back and the physical values xarray decodes. The
//! builder checks every chunk against numcodecs alone too, and checks that
//! each case tells the right reading from the wrong one it is there for: the
//! arithmetic type, the rounding the codes were written with, the order of
//! the filters, and the cast to an integer `dtype`.
//!
//! Paths are relative, so the suite also runs under `wasmtime --dir=.`.
#![cfg(feature = "codecs")]

use std::ops::Range;
use std::path::Path;

use fieldglass_core::array::ArraySource;
use fieldglass_core::bytes::MemoryObjects;
use fieldglass_zarr::{ChunkDecoder, FieldglassError, ZarrStore};
use serde_json::{Value, json};

const STORE: &str = "tests/fixtures/fixedscaleoffset";

fn files() -> Vec<(String, Vec<u8>)> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        for entry in std::fs::read_dir(dir).expect("fixture directory") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                let key = path
                    .strip_prefix(root)
                    .expect("under the root")
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/");
                out.push((key, std::fs::read(&path).expect("read")));
            }
        }
    }
    let mut out = Vec::new();
    walk(Path::new(STORE), Path::new(STORE), &mut out);
    out
}

fn oracle() -> Value {
    let text = std::fs::read_to_string("tests/fixtures/fixedscaleoffset_oracle.json")
        .expect("the oracle is committed");
    serde_json::from_str(&text).expect("JSON")
}

/// A cell as the oracle spells it: `null` for masked, `"NaN"` for NaN.
fn cells(list: &Value) -> Vec<Option<f64>> {
    list.as_array()
        .expect("cells")
        .iter()
        .map(|v| match v {
            Value::Null => None,
            Value::String(s) if s == "NaN" => Some(f64::NAN),
            other => Some(other.as_f64().expect("a number")),
        })
        .collect()
}

fn whole(case: &Value) -> Vec<Range<u64>> {
    case["shape"]
        .as_array()
        .expect("a shape")
        .iter()
        .map(|n| 0..n.as_u64().expect("an extent"))
        .collect()
}

/// Bit for bit, NaN included: a value one float32 step off is the arithmetic
/// type read wrong, which is what several cases are there to catch.
fn assert_cells(got: &[Option<f64>], want: &[Option<f64>], what: &str) {
    assert_eq!(got.len(), want.len(), "{what}: length");
    for (i, (g, w)) in got.iter().zip(want).enumerate() {
        let same = match (g, w) {
            (Some(g), Some(w)) => g.to_bits() == w.to_bits() || (g.is_nan() && w.is_nan()),
            (None, None) => true,
            _ => false,
        };
        assert!(same, "{what}: cell {i} is {g:?}, expected {w:?}");
    }
}

fn cases(oracle: &Value) -> Vec<(&String, &Value)> {
    let cases: Vec<_> = oracle.as_object().expect("arrays").iter().collect();
    // Every case the builder writes, so a case dropped there is noticed here.
    assert_eq!(
        cases
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        [
            "big_endian",
            "cf_int",
            "fortran",
            "half_even",
            "int16",
            "same_type",
            "shuffle_after",
            "shuffle_before",
            "uint8",
        ]
    );
    cases
}

/// Every array reads, through the store, to the values zarr-python reads:
/// `astype` the same as `dtype` and wider or narrower than it, signed and
/// unsigned codes, both byte orders, Fortran order, and the filter before and
/// after a `shuffle`.
#[test]
fn every_array_reads_what_zarr_python_reads() {
    let oracle = oracle();
    let objects = MemoryObjects::from_iter(files());
    let store = ZarrStore::open(&objects).expect("the store opens");
    let source: &dyn ArraySource = &store;
    for (name, case) in cases(&oracle) {
        let got = source
            .read_region(name, &whole(case))
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_cells(&got, &cells(&case["values"]), name);
    }
}

/// CF's `scale_factor` and `add_offset`, and the fill value, apply to what the
/// filter decodes, as xarray applies them (#1823). A cell whose code decodes
/// to the fill value is masked.
#[test]
fn cf_applies_after_the_filter() {
    let oracle = oracle();
    let objects = MemoryObjects::from_iter(files());
    let store = ZarrStore::open(&objects).expect("the store opens");
    let source: &dyn ArraySource = &store;
    for (name, case) in cases(&oracle) {
        let got = source
            .read_region_physical(name, &whole(case))
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        // xarray gives a stored NaN as missing, where the physical read keeps
        // it as NaN; both are "no value", and that is not this filter's rule.
        let got: Vec<_> = got
            .into_iter()
            .map(|cell| cell.filter(|v| !v.is_nan()))
            .collect();
        assert_cells(&got, &cells(&case["physical"]), &format!("{name} physical"));
    }
    // The case that has both: packed, and masked where the code decodes to
    // the fill value.
    let physical = cells(&oracle["cf_int"]["physical"]);
    assert_eq!(physical.iter().filter(|c| c.is_none()).count(), 4);
    assert_eq!(physical[2], Some(1233.0 * 0.5 - 100.0));
}

/// A region within one chunk reads the same cells as the whole array does.
#[test]
fn a_region_reads_its_cells() {
    let oracle = oracle();
    let objects = MemoryObjects::from_iter(files());
    let store = ZarrStore::open(&objects).expect("the store opens");
    let source: &dyn ArraySource = &store;
    let want = cells(&oracle["same_type"]["values"]);
    let got = source
        .read_region("same_type", &[12..14, 20..23])
        .expect("reads");
    let expected: Vec<_> = (12..14)
        .flat_map(|row| (20..23).map(move |col| row * 30 + col))
        .map(|at| want[at])
        .collect();
    assert_cells(&got, &expected, "same_type region");
}

/// A `.zarray` of 2x3 `dtype` elements, no compressor, and these filters.
fn zarray(dtype: &str, filters: Value) -> String {
    json!({
        "zarr_format": 2, "shape": [2, 3], "chunks": [2, 3],
        "dtype": dtype, "order": "C", "fill_value": 0,
        "compressor": null, "filters": filters,
    })
    .to_string()
}

fn filter(scale: Value, offset: Value, dtype: &str, astype: &str) -> Value {
    json!([{"id": "fixedscaleoffset", "scale": scale, "offset": offset,
            "dtype": dtype, "astype": astype}])
}

fn refusal(json: &str) -> FieldglassError {
    match ChunkDecoder::from_v2_metadata(json) {
        Ok(_) => panic!("built a decoder from {json}"),
        Err(e) => e,
    }
}

/// A filter whose decode would divide by zero, or by or into a value that is
/// not finite, is refused when the metadata is read, rather than decoding
/// every cell to NaN or infinity.
#[test]
fn a_scale_or_offset_that_cannot_decode_is_refused() {
    for (scale, offset, astype) in [
        (json!(0), json!(0), "<f4"),
        (json!(0.0), json!(1), "<i2"),
        (json!(-0.0), json!(1), "<f8"),
        // Not zero as a double, zero as the float32 numpy divides in.
        (json!(1e-50), json!(0), "<f4"),
        // Finite as a double, infinite as a float32.
        (json!(1e300), json!(0), "<f4"),
        (json!(1), json!(1e300), "<f4"),
        // JSON holds no NaN; a writer that spells one as text is no number.
        (json!("NaN"), json!(0), "<f4"),
        (json!(1), json!("Infinity"), "<f4"),
        (json!(null), json!(0), "<f4"),
        (json!(true), json!(0), "<f4"),
    ] {
        let doc = zarray("<f4", filter(scale.clone(), offset.clone(), "<f4", astype));
        let err = refusal(&doc);
        assert!(
            matches!(err, FieldglassError::Parse(_)),
            "scale {scale}, offset {offset}, astype {astype}: {err:?}"
        );
    }
    // Missing altogether.
    let doc = zarray(
        "<f4",
        json!([{"id": "fixedscaleoffset", "offset": 0, "dtype": "<f4"}]),
    );
    assert!(matches!(refusal(&doc), FieldglassError::Parse(_)));
}

/// An `astype` or `dtype` that is not a number, or not one this crate reads,
/// is refused by name; one that disagrees with the array's own type is
/// refused as a chain that cannot be what the array holds.
#[test]
fn a_type_that_is_not_numeric_or_does_not_match_is_refused() {
    for (array, dtype, astype) in [
        ("<f4", "<f4", "|S4"),
        ("<f4", "<f4", "|O"),
        ("<f4", "<f4", "<f2"),
        ("<f4", "<f4", "<c8"),
        ("<f4", "<f4", "|b1"),
        ("|b1", "|b1", "|u1"),
        ("<f4", "<f4", "=i2"),
        ("<f4", "<f4", "float32"),
        ("<f4", "<f4", ""),
    ] {
        let err = refusal(&zarray(array, filter(json!(1), json!(0), dtype, astype)));
        assert!(
            matches!(
                err,
                FieldglassError::Parse(_) | FieldglassError::UnsupportedSection(_)
            ),
            "{array} via {dtype} as {astype}: {err:?}"
        );
    }
    // A filter `dtype` that is not the array's: wider, another kind, and the
    // other byte order.
    for (array, dtype) in [("<f4", "<f8"), ("<f4", "<i4"), ("<f4", ">f4")] {
        let err = refusal(&zarray(array, filter(json!(1), json!(0), dtype, "<i2")));
        assert!(
            matches!(&err, FieldglassError::Parse(m) if m.contains("fixedscaleoffset")),
            "{array} via {dtype}: {err:?}"
        );
    }
    // Two filters whose types do not chain.
    let chained = json!([
        {"id": "fixedscaleoffset", "scale": 1, "offset": 0, "dtype": "<f4", "astype": "<i4"},
        {"id": "fixedscaleoffset", "scale": 1, "offset": 0, "dtype": "<i2", "astype": "<i2"},
    ]);
    assert!(matches!(
        refusal(&zarray("<f4", chained)),
        FieldglassError::Parse(_)
    ));
    // `dtype` missing, or not a string.
    for missing in [
        json!([{"id": "fixedscaleoffset", "scale": 1, "offset": 0}]),
        json!([{"id": "fixedscaleoffset", "scale": 1, "offset": 0, "dtype": 4}]),
    ] {
        assert!(matches!(
            refusal(&zarray("<f4", missing)),
            FieldglassError::Parse(_)
        ));
    }
}

/// Two filters chain when the second is handed the first one's codes, and
/// they are undone second first.
#[test]
fn two_filters_chain_through_their_codes() {
    let chained = json!([
        {"id": "fixedscaleoffset", "scale": 2, "offset": 0, "dtype": "<f4", "astype": "<i4"},
        {"id": "fixedscaleoffset", "scale": 1, "offset": 100, "dtype": "<i4", "astype": "<i2"},
    ]);
    let decoder =
        ChunkDecoder::from_v2_metadata(&zarray("<f4", chained)).expect("the metadata reads");
    let codes: Vec<u8> = [0i16, 2, -100, 10, 4, 7]
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    assert_eq!(
        decoder.decode_raw_values(&codes).unwrap(),
        [50.0, 51.0, 0.0, 55.0, 52.0, 53.5]
    );
}

/// With no `astype`, the codes are `dtype`, as numcodecs reads it.
#[test]
fn a_missing_astype_is_the_dtype() {
    for astype in [None, Some(Value::Null)] {
        let mut config =
            json!({"id": "fixedscaleoffset", "scale": 4, "offset": 10, "dtype": "<f8"});
        if let Some(value) = astype {
            config["astype"] = value;
        }
        let decoder = ChunkDecoder::from_v2_metadata(&zarray("<f8", json!([config])))
            .expect("the metadata reads");
        let chunk: Vec<u8> = [0.0f64, 4.0, -8.0, 1.0, 2.0, 3.0]
            .iter()
            .flat_map(|v| v.to_le_bytes())
            .collect();
        assert_eq!(
            decoder.decode_raw_values(&chunk).unwrap(),
            [10.0, 11.0, 8.0, 10.25, 10.5, 10.75]
        );
    }
}

/// A chunk whose bytes are not a whole number of `astype` elements, or not
/// as many as the chunk holds, is refused, not read short or padded.
#[test]
fn a_chunk_that_does_not_fit_astype_is_refused() {
    let decoder =
        ChunkDecoder::from_v2_metadata(&zarray("<f4", filter(json!(1), json!(0), "<f4", "<i2")))
            .expect("the metadata reads");
    // Six int16 codes are twelve bytes; the float32 elements would be 24.
    assert!(decoder.decode(&[0u8; 12]).is_ok());
    for len in [0, 11, 13, 24] {
        let err = decoder.decode(&vec![0u8; len]).unwrap_err();
        assert!(matches!(err, FieldglassError::Parse(_)), "{len}: {err:?}");
    }
}

/// A code that decodes past an integer `dtype`, or to NaN, has no value
/// there: refused, where numpy's cast gives whatever the platform gives.
#[test]
fn a_value_past_an_integer_dtype_is_refused() {
    let decoder =
        ChunkDecoder::from_v2_metadata(&zarray("<i2", filter(json!(1), json!(0), "<i2", "<i4")))
            .expect("the metadata reads");
    let codes =
        |values: [i32; 6]| -> Vec<u8> { values.iter().flat_map(|v| v.to_le_bytes()).collect() };
    assert_eq!(
        decoder
            .decode_raw_values(&codes([-32768, 32767, 0, 1, -1, 7]))
            .unwrap(),
        [-32768.0, 32767.0, 0.0, 1.0, -1.0, 7.0]
    );
    for past in [32768, -32769, i32::MAX] {
        let err = decoder
            .decode_raw_values(&codes([0, 0, past, 0, 0, 0]))
            .unwrap_err();
        assert!(matches!(err, FieldglassError::Parse(_)), "{past}: {err:?}");
    }
    let nan =
        ChunkDecoder::from_v2_metadata(&zarray("<i2", filter(json!(1), json!(0), "<i2", "<f4")))
            .expect("the metadata reads");
    let chunk: Vec<u8> = [0.0f32, 1.0, f32::NAN, 2.0, 3.0, 4.0]
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    assert!(matches!(
        nan.decode_raw_values(&chunk).unwrap_err(),
        FieldglassError::Parse(_)
    ));
}

/// A chunk shape whose codes would need more bytes than an address holds is
/// refused before anything is allocated, also when its elements alone fit.
#[test]
fn codes_wider_than_an_address_are_refused() {
    let doc = json!({
        "zarr_format": 2, "shape": [1u64 << 31, 1u64 << 30], "chunks": [1u64 << 31, 1u64 << 30],
        "dtype": "|u1", "order": "C", "fill_value": 0, "compressor": null,
        "filters": [{"id": "fixedscaleoffset", "scale": 1, "offset": 0,
                     "dtype": "|u1", "astype": "<f8"}],
    })
    .to_string();
    let built = ChunkDecoder::from_v2_metadata(&doc);
    if cfg!(target_pointer_width = "64") {
        // The elements fit an address; the codes do not.
        let err = built
            .expect("the metadata reads")
            .decode(&[0u8; 16])
            .unwrap_err();
        assert!(
            matches!(&err, FieldglassError::Parse(m) if m.contains("overflows")),
            "{err:?}"
        );
    } else {
        // On a 32-bit target the chunk is refused when the metadata is read.
        assert!(built.is_err());
    }
}
