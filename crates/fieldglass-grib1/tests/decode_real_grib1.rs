//! End-to-end decode of a real-world GRIB1 message.
//!
//! Fixture: a 14.5 KB single-message GRIB1 file from the Canadian
//! Meteorological Centre regional model (wind speed at 300 hPa,
//! polar-stereographic 60 km grid, 2010-05-24 00Z + 12 h). Originally
//! distributed via the pygrib sample data set (MIT-licensed, J. Whitaker).
//!
//! Reference values were independently computed with a spec-only Python
//! decoder (no eccodes/pygrib dependency) — see /tmp/reference_decode.py
//! in the development environment.

use fieldglass_grib1::Grib1Reader;

const FIXTURE: &[u8] = include_bytes!("fixtures/cmc_wind_300_2010052400_p012.grib");

fn open() -> Grib1Reader {
    Grib1Reader::from_bytes(FIXTURE.to_vec()).expect("fixture parses")
}

#[test]
fn parses_one_message_with_polar_stereo_grid() {
    let reader = open();
    assert_eq!(reader.message_count(), 1);
    let msg = &reader.messages[0];
    assert_eq!(msg.byte_offset, 0);
    let gds = msg.gds.as_ref().expect("GDS present");
    assert_eq!(gds.grid_type_name(), "polar_stereo");
    assert_eq!(gds.dimensions(), Some((135, 95)));
}

#[test]
fn decode_grid_matches_independent_reference() {
    let reader = open();
    let values = reader.decode_message_values(0).expect("decode succeeds");

    // 135 × 95 polar-stereographic grid.
    assert_eq!(values.len(), 12_825);

    // No bitmap in this message: every point is present.
    let present: Vec<f64> = values
        .into_iter()
        .map(|v| v.expect("no missing values"))
        .collect();

    // Spot-check first / mid / last values against the reference decoder.
    let first = &present[..5];
    let expected_first = [
        5.459_607_660_770_416,
        5.709_607_660_770_416,
        5.959_607_660_770_416,
        6.459_607_660_770_416,
        7.209_607_660_770_416,
    ];
    for (got, want) in first.iter().zip(expected_first.iter()) {
        assert!((got - want).abs() < 1e-9, "got {got}, want {want}");
    }

    let last = &present[present.len() - 5..];
    let expected_last = [
        16.459_607_660_770_416,
        14.709_607_660_770_416,
        12.959_607_660_770_416,
        13.209_607_660_770_416,
        11.709_607_660_770_416,
    ];
    for (got, want) in last.iter().zip(expected_last.iter()) {
        assert!((got - want).abs() < 1e-9, "got {got}, want {want}");
    }

    let mid = present[present.len() / 2];
    assert!((mid - 64.959_607_660_770_42).abs() < 1e-9);

    // Aggregate checks: min/max/sum from the reference decoder.
    let min = present.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = present.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let sum: f64 = present.iter().sum();
    assert!(
        (min - 0.209_607_660_770_416_26).abs() < 1e-9,
        "min was {min}"
    );
    assert!((max - 75.209_607_660_770_42).abs() < 1e-9, "max was {max}");
    assert!((sum - 284_436.968_249_380_6).abs() < 1e-3, "sum was {sum}");
}
