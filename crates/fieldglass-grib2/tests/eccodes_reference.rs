//! Snapshot-based cross-check of `fieldglass-grib2` against eccodes.
//!
//! For each fixture under `tests/fixtures/`, we ship a sibling
//! `.eccodes.ref.json` that captures the output of `grib_dump -j` for a
//! curated subset of WMO keys. This test loads each snapshot, decodes the
//! fixture with our parser, and asserts that the two agree on every key
//! present in the snapshot.
//!
//! **The fixture list comes from the directory, not from here** (#471). It used
//! to be 34 hand-written one-line tests, and a fixture added without one was
//! simply not cross-checked — eight had accumulated that way. An enumeration
//! can fail open; a walk cannot. A fixture eccodes genuinely cannot decode goes
//! in [`NO_ECCODES_SNAPSHOT`] with its reason, and that list is itself checked
//! for staleness.
//!
//! The snapshots are checked into git so this test has zero runtime
//! dependencies — eccodes is only required when regenerating snapshots
//! via `tools/regenerate-eccodes-snapshots.py` (typically after upgrading
//! eccodes or adding a new fixture).
//!
//! The key-to-parser-field mapping is intentionally explicit in
//! [`assert_message_matches`]: when eccodes adds a new key we want surfaced,
//! add it to both `GRIB2_KEYS` (in the regen script) and the dispatch match
//! here. [`the_cross_check_compares_every_key_it_ships`] then holds the two
//! together — a key that reaches no fixture, or reaches one and is skipped by
//! every arm, is coverage in name only, which is how §5 came to be listed here
//! for years while being compared nowhere (#475).

use fieldglass_grib2::{Grib2Reader, GridTemplate, parse_bit_map};
use serde_json::Value;
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::path::Path;

thread_local! {
    /// Keys that a `check_*` actually compared, as opposed to keys an arm
    /// declared not applicable to this template. Every arm of the dispatch can
    /// return a bare `true`, which is indistinguishable from a passing
    /// comparison unless something counts — see
    /// [`the_cross_check_compares_every_key_it_ships`].
    static COMPARED: RefCell<BTreeSet<String>> = const { RefCell::new(BTreeSet::new()) };
}

fn record(key: &str) {
    COMPARED.with(|keys| keys.borrow_mut().insert(key.to_string()));
}

/// Float tolerance for fields encoded as scaled integers by GRIB2 but
/// emitted as decimals by eccodes (e.g. 2.5° increments stored as
/// 2_500_000 μ°). 1e-3 absorbs any rounding eccodes applies; mismatches
/// here mean a real scale-factor bug.
const FLOAT_EPS: f64 = 1e-3;

fn snapshot_for(fixture: &str) -> Value {
    let path = Path::new("tests/fixtures").join(format!("{fixture}.eccodes.ref.json"));
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read snapshot {}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse snapshot {}: {e}", path.display()))
}

fn assert_message_matches(
    fixture: &str,
    msg: &fieldglass_grib2::Grib2Message,
    snap: &serde_json::Map<String, Value>,
    raw_file_bytes: &[u8],
) {
    for (key, expected) in snap {
        // `null` in the snapshot means eccodes itself omitted the field —
        // skip the comparison rather than asserting on a missing value.
        if expected.is_null() {
            continue;
        }
        let pinned = match key.as_str() {
            // §0 Indicator
            "discipline" => check_u64(key, expected, msg.is.discipline as u64),
            "editionNumber" => check_u64(key, expected, msg.is.edition as u64),
            "totalLength" => check_u64(key, expected, msg.is.total_length),

            // §1 Identification
            "centre" => check_u64(key, expected, msg.ids.centre as u64),
            "subCentre" => check_u64(key, expected, msg.ids.sub_centre as u64),
            "significanceOfReferenceTime" => {
                check_u64(key, expected, msg.ids.reference_time_significance as u64)
            }
            "dataDate" => check_u64(
                key,
                expected,
                msg.ids.year as u64 * 10_000 + msg.ids.month as u64 * 100 + msg.ids.day as u64,
            ),
            "dataTime" => check_u64(
                key,
                expected,
                msg.ids.hour as u64 * 100 + msg.ids.minute as u64,
            ),
            "productionStatusOfProcessedData" => {
                check_u64(key, expected, msg.ids.production_status as u64)
            }
            "typeOfProcessedData" => check_u64(key, expected, msg.ids.data_type as u64),

            // §3 Grid Definition
            "gridDefinitionTemplateNumber" => {
                check_u64(key, expected, msg.gds.template_number as u64)
            }
            "shapeOfTheEarth" => match &msg.gds.template {
                GridTemplate::LatLon(t) => check_u64(key, expected, t.shape_of_earth as u64),
                GridTemplate::RotatedLatLon(t) => check_u64(key, expected, t.shape_of_earth as u64),
                GridTemplate::Mercator(t) => check_u64(key, expected, t.shape_of_earth as u64),
                GridTemplate::TransverseMercator(t) => {
                    check_u64(key, expected, t.shape_of_earth as u64)
                }
                GridTemplate::PolarStereographic(t) => {
                    check_u64(key, expected, t.shape_of_earth as u64)
                }
                GridTemplate::Lambert(t) => check_u64(key, expected, t.shape_of_earth as u64),
                GridTemplate::LambertAzimuthal(t) => {
                    check_u64(key, expected, t.shape_of_earth as u64)
                }
                GridTemplate::Gaussian(t) => check_u64(key, expected, t.shape_of_earth as u64),
                GridTemplate::SpaceView(t) => check_u64(key, expected, t.shape_of_earth as u64),
                GridTemplate::Healpix(t) => check_u64(key, expected, t.shape_of_earth as u64),
                // Spherical harmonics carry no earth shape (not a projected grid).
                GridTemplate::SphericalHarmonic(_) => true,
                // Bi-Fourier coefficients likewise carry no earth shape.
                GridTemplate::BiFourier(_) => true,
                GridTemplate::Unsupported(_) => true, // can't check
            },
            "numberOfDataPoints" => check_u64(key, expected, msg.gds.num_data_points as u64),
            // A reduced grid's `dimensions()` reports the raster its rows
            // expand into (#503), which is not the message's `Ni` — but eccodes
            // emits `Ni: null` for one, and a null snapshot value is skipped
            // before this dispatch, so the two never meet. Anything that does
            // reach here declares a real `Ni`.
            "Ni" => match msg.gds.dimensions() {
                Some((ni, _)) => check_u64(key, expected, ni as u64),
                None => true, // no raster at all: spectral, bi-Fourier, HEALPix
            },
            "Nj" => match msg.gds.dimensions() {
                Some((_, nj)) => check_u64(key, expected, nj as u64),
                None => match &msg.gds.template {
                    GridTemplate::Gaussian(t) => check_u64(key, expected, t.nj as u64),
                    _ => true,
                },
            },
            "latitudeOfFirstGridPointInDegrees" => match msg.gds.bounds() {
                Some((la1, _, _, _)) => check_f64(key, expected, la1),
                None => true,
            },
            "longitudeOfFirstGridPointInDegrees" => match msg.gds.bounds() {
                Some((_, lo1, _, _)) => check_f64(key, expected, lo1),
                None => true,
            },
            "latitudeOfLastGridPointInDegrees" => match &msg.gds.template {
                GridTemplate::LatLon(t) => check_f64(key, expected, t.la2),
                GridTemplate::RotatedLatLon(t) => check_f64(key, expected, t.la2),
                GridTemplate::Gaussian(t) => check_f64(key, expected, t.la2),
                _ => true,
            },
            "longitudeOfLastGridPointInDegrees" => match &msg.gds.template {
                GridTemplate::LatLon(t) => check_f64(key, expected, t.lo2),
                GridTemplate::RotatedLatLon(t) => check_f64(key, expected, t.lo2),
                GridTemplate::Gaussian(t) => check_f64(key, expected, t.lo2),
                _ => true,
            },
            "iDirectionIncrementInDegrees" => match &msg.gds.template {
                GridTemplate::LatLon(t) => match t.di {
                    Some(di) => check_f64(key, expected, di),
                    None => true,
                },
                GridTemplate::RotatedLatLon(t) => match t.di {
                    Some(di) => check_f64(key, expected, di),
                    None => true,
                },
                GridTemplate::Gaussian(t) => match t.di {
                    Some(di) => check_f64(key, expected, di),
                    None => true,
                },
                _ => true,
            },
            "jDirectionIncrementInDegrees" => match &msg.gds.template {
                GridTemplate::LatLon(t) => match t.dj {
                    Some(dj) => check_f64(key, expected, dj),
                    None => true,
                },
                GridTemplate::RotatedLatLon(t) => match t.dj {
                    Some(dj) => check_f64(key, expected, dj),
                    None => true,
                },
                _ => true,
            },

            // §4 Product Definition
            "productDefinitionTemplateNumber" => {
                check_u64(key, expected, msg.pds.template_number as u64)
            }
            "parameterCategory" => match msg.pds.common() {
                Some(c) => check_u64(key, expected, c.parameter_category as u64),
                None => true,
            },
            "parameterNumber" => match msg.pds.common() {
                Some(c) => check_u64(key, expected, c.parameter_number as u64),
                None => true,
            },
            "typeOfGeneratingProcess" => match msg.pds.common() {
                Some(c) => check_u64(key, expected, c.generating_process_type as u64),
                None => true,
            },
            "indicatorOfUnitOfTimeRange" => match msg.pds.common() {
                Some(c) => check_u64(key, expected, c.forecast_time_unit as u64),
                None => true,
            },
            "forecastTime" => match msg.pds.common() {
                Some(c) => check_i64(key, expected, c.forecast_time),
                None => true,
            },
            "typeOfFirstFixedSurface" => match msg.pds.common() {
                Some(c) => check_u64(key, expected, c.first_surface.surface_type as u64),
                None => true,
            },
            "scaleFactorOfFirstFixedSurface" => match msg.pds.common() {
                Some(c) => check_i64(
                    key,
                    expected,
                    c.first_surface.scale_factor.unwrap_or(0) as i64,
                ),
                None => true,
            },
            "scaledValueOfFirstFixedSurface" => match msg.pds.common() {
                Some(c) => check_i64(key, expected, c.first_surface.scaled_value.unwrap_or(0)),
                None => true,
            },

            // §5 Data Representation. These four route through
            // [`packing_params`] rather than `drs.simple()`: R / E / D / bits
            // are the first fields of nearly every §5 template, so reading them
            // only for 5.0 left the packing parameters of complex, CCSDS, PNG,
            // JPEG 2000, spectral and second-order messages unchecked — most of
            // the corpus.
            "dataRepresentationTemplateNumber" => {
                check_u64(key, expected, msg.drs.template_number as u64)
            }
            "referenceValue" => match packing_params(&msg.drs) {
                Some(p) => check_f64(key, expected, p.reference_value as f64),
                None => true,
            },
            "binaryScaleFactor" => match packing_params(&msg.drs) {
                Some(p) => check_i64(key, expected, p.binary_scale_factor as i64),
                None => true,
            },
            "decimalScaleFactor" => match &msg.drs.template {
                // Template 5.200 spends one octet on `D` and declares it
                // `unsigned[1]`, so eccodes' *key* reports the raw byte (129)
                // while eccodes' own run-length decoder folds the same byte
                // sign-magnitude to -1 (`DataRunLengthPacking.cc`: `if (dsf >
                // 127) dsf = -(dsf - 128)`). We fold it at parse time, so
                // re-encode rather than skip — the comparison is still real,
                // and a parser that stopped folding would still fail it.
                fieldglass_grib2::DataRepresentationTemplate::RunLength(t) => {
                    check_u64(key, expected, sign_magnitude_octet(t.decimal_scale_factor))
                }
                _ => match packing_params(&msg.drs) {
                    Some(p) => check_i64(key, expected, p.decimal_scale_factor as i64),
                    None => true,
                },
            },
            "bitsPerValue" => match packing_params(&msg.drs) {
                Some(p) => check_u64(key, expected, p.bits_per_value as u64),
                None => true,
            },

            // §6 Bit-Map
            "bitMapIndicator" => {
                let (start, end) = msg.bms_range;
                // grid_points argument is only used by the inline-bitmap
                // branch, and that path needs an accurate count; use the
                // GDS-declared num_data_points so this works for every
                // template.
                let grid_points = msg.gds.num_data_points as usize;
                let bms =
                    parse_bit_map(&raw_file_bytes[start..end], grid_points).expect("BMS parse");
                check_u64(key, expected, bms.indicator as u64)
            }

            unknown => panic!(
                "{fixture}: snapshot has key {unknown:?} with no parser-field mapping; \
                 update assert_message_matches in eccodes_reference.rs",
            ),
        };
        assert!(pinned, "{fixture}: key {key:?} mismatch");
    }
}

/// A signed scale factor back in the single sign-magnitude octet it was read
/// from: the high bit is the sign, the rest the magnitude.
fn sign_magnitude_octet(value: i16) -> u64 {
    if value < 0 {
        128 + value.unsigned_abs() as u64
    } else {
        value as u64
    }
}

/// The reference value, scale factors and value width — the four parameters
/// almost every §5 template opens with, whichever template it is.
///
/// IEEE (5.4) genuinely has none of them: it stores raw floats. Run-length
/// (5.200) has a width and a decimal scale but no reference value or binary
/// scale, and eccodes reports the pair it does have through the same key names,
/// so it is served here with the two it lacks left at their neutral values —
/// which is what eccodes prints for them too.
struct PackingParams {
    reference_value: f32,
    binary_scale_factor: i16,
    decimal_scale_factor: i16,
    bits_per_value: u8,
}

fn packing_params(drs: &fieldglass_grib2::DataRepresentationSection) -> Option<PackingParams> {
    use fieldglass_grib2::DataRepresentationTemplate as T;
    let params = |reference_value, binary_scale_factor, decimal_scale_factor, bits_per_value| {
        Some(PackingParams {
            reference_value,
            binary_scale_factor,
            decimal_scale_factor,
            bits_per_value,
        })
    };
    match &drs.template {
        T::Simple(t) => params(
            t.reference_value,
            t.binary_scale_factor,
            t.decimal_scale_factor,
            t.bits_per_value,
        ),
        T::MatrixSimple(t) => params(
            t.reference_value,
            t.binary_scale_factor,
            t.decimal_scale_factor,
            t.bits_per_value,
        ),
        T::Complex(t) => params(
            t.reference_value,
            t.binary_scale_factor,
            t.decimal_scale_factor,
            t.bits_per_value,
        ),
        // 5.3 is 5.2 plus the differencing descriptors, and carries the
        // complex parameters verbatim.
        T::ComplexSpatialDiff(t) => params(
            t.complex.reference_value,
            t.complex.binary_scale_factor,
            t.complex.decimal_scale_factor,
            t.complex.bits_per_value,
        ),
        T::Png(t) => params(
            t.reference_value,
            t.binary_scale_factor,
            t.decimal_scale_factor,
            t.bits_per_value,
        ),
        T::Ccsds(t) => params(
            t.reference_value,
            t.binary_scale_factor,
            t.decimal_scale_factor,
            t.bits_per_value,
        ),
        T::Jpeg2000(t) => params(
            t.reference_value,
            t.binary_scale_factor,
            t.decimal_scale_factor,
            t.bits_per_value,
        ),
        T::LogPreprocessing(t) => params(
            t.reference_value,
            t.binary_scale_factor,
            t.decimal_scale_factor,
            t.bits_per_value,
        ),
        T::SpectralSimple(t) => params(
            t.reference_value,
            t.binary_scale_factor,
            t.decimal_scale_factor,
            t.bits_per_value,
        ),
        T::SpectralComplex(t) => params(
            t.reference_value,
            t.binary_scale_factor,
            t.decimal_scale_factor,
            t.bits_per_value,
        ),
        T::BiFourier(t) => params(
            t.reference_value,
            t.binary_scale_factor,
            t.decimal_scale_factor,
            t.bits_per_value,
        ),
        T::SecondOrder(t) => params(
            t.reference_value,
            t.binary_scale_factor,
            t.decimal_scale_factor,
            t.bits_per_value,
        ),
        T::RunLength(t) => params(0.0, 0, t.decimal_scale_factor, t.bits_per_value),
        T::Ieee(_) | T::Unsupported(_) => None,
    }
}

fn check_u64(key: &str, expected: &Value, actual: u64) -> bool {
    record(key);
    let exp = expected
        .as_u64()
        .or_else(|| expected.as_i64().map(|i| i as u64))
        .unwrap_or_else(|| panic!("snapshot {key:?} is not an integer: {expected}"));
    if exp != actual {
        eprintln!("key {key}: eccodes={exp}, parser={actual}");
        return false;
    }
    true
}

fn check_i64(key: &str, expected: &Value, actual: i64) -> bool {
    record(key);
    let exp = expected
        .as_i64()
        .unwrap_or_else(|| panic!("snapshot {key:?} is not an integer: {expected}"));
    if exp != actual {
        eprintln!("key {key}: eccodes={exp}, parser={actual}");
        return false;
    }
    true
}

fn check_f64(key: &str, expected: &Value, actual: f64) -> bool {
    record(key);
    let exp = expected
        .as_f64()
        .unwrap_or_else(|| panic!("snapshot {key:?} is not a number: {expected}"));
    if (exp - actual).abs() > FLOAT_EPS {
        eprintln!(
            "key {key}: eccodes={exp}, parser={actual}, diff={}",
            (exp - actual).abs()
        );
        return false;
    }
    true
}

fn assert_fixture_matches_snapshot(fixture: &str, bytes: &[u8]) {
    let reader = Grib2Reader::from_bytes(bytes.to_vec())
        .unwrap_or_else(|e| panic!("{fixture}: parse failed: {e}"));
    let snap = snapshot_for(fixture);
    let msgs = snap["messages"]
        .as_array()
        .unwrap_or_else(|| panic!("{fixture}: snapshot has no `messages` array"));
    assert_eq!(
        msgs.len(),
        reader.messages.len(),
        "{fixture}: message count mismatch (snapshot={}, parser={})",
        msgs.len(),
        reader.messages.len(),
    );
    for (i, msg_snap) in msgs.iter().enumerate() {
        let snap_obj = msg_snap
            .as_object()
            .unwrap_or_else(|| panic!("{fixture}: snapshot message {i} is not an object"));
        assert_message_matches(fixture, &reader.messages[i], snap_obj, bytes);
    }
}

// §5 packing fixtures: these cross-check that §0–§4 metadata and the §5
// template number parse for templates 5.2 / 5.3 / 5.40 / 5.41 / 5.42. The
// value decode for each is pinned to the sibling `*_expected.json` oracle in
// the matching `decode_*.rs` test (complex → `decode_complex.rs` /
// `decode_complex_spd.rs`; JPEG 2000 5.40 → `decode_jpeg2000.rs`; PNG 5.41 →
// `decode_png.rs`; CCSDS 5.42 → `decode_ccsds.rs`).
// Missing-value management / row-by-row splitting fixtures (#217); value
// decode pinned in `decode_complex_missing.rs`.
// NG == 0 constant-field fixtures (#222, eccodes ECC-2095); value decode
// pinned in `decode_complex_constant.rs`.
// Run-length packing fixtures (#301, template 5.200); value decode pinned in
// `decode_runlength.rs`. bitsPerValue / decimalScaleFactor route through
// `msg.drs.simple()` in the snapshot dispatch, which is `None` for run-length,
// so those keys are skipped here (as for every non-simple packing) and the
// §5 parameters are cross-checked in `decode_runlength.rs` instead.
// Second-order fixtures (#307, templates 5.50001 / 5.50002); value decode
// pinned in `decode_second_order.rs`.
// Log-preprocessing fixtures (#305, template 5.61); value decode pinned in
// `decode_log_preprocessing.rs`. The §5 R/E/D/bits keys route through
// `msg.drs.simple()` in the snapshot dispatch (None for this template), so
// they are skipped here; the §5 parameters are cross-checked in the value test.
// Pre-standard local JPEG 2000 (5.40000, part of #307); value decode pinned in
// `decode_local_templates.rs`. eccodes decodes it as `grid_jpeg`. The sibling
// local PNG (5.40010) has no snapshot here: eccodes cannot decode it at all, so
// it is cross-checked against the original 5.41 fixture's decode in that test.
// Spherical-harmonic spectral message (§3.50 + §5.50, #302); coefficient decode
// pinned in `spectral.rs`. This exercises §0–§4 metadata parsing for a message
// with no grid (no Ni/Nj, no earth shape).
/// The real-model fixtures below are ~1 MB each — two orders of magnitude
/// larger than the re-encoded ones — so read them at runtime rather than
/// embedding them in the test binary with `include_bytes!`.
fn read_fixture(fixture: &str) -> Vec<u8> {
    let path = Path::new("tests/fixtures").join(fixture);
    std::fs::read(&path).unwrap_or_else(|e| panic!("read fixture {}: {e}", path.display()))
}

/// Fixtures with no snapshot, and why. eccodes must be unable to decode them —
/// nothing else is a reason to skip the cross-check.
///
/// Keep in step with `ECCODES_UNDECODABLE` in
/// `tools/regenerate-eccodes-snapshots.py`. The two lists cannot import each
/// other across languages, so [`the_exemption_list_has_no_stale_entries`]
/// checks this one against the filesystem instead: an entry that has acquired
/// a snapshot is an exemption that has outlived its reason.
const NO_ECCODES_SNAPSHOT: &[(&str, &str)] = &[(
    "png_local_40010.grib2",
    "local template 5.40010, which eccodes has no definition for — it errors \
     with \"No final 7777\". Decode is cross-checked against a different oracle \
     in `decode_png.rs`.",
)];

/// Every committed GRIB2 fixture is cross-checked against eccodes.
///
/// This walks the fixture directory rather than naming files, which is the
/// whole point (#471). The 34 tests this replaced were identical one-line calls
/// enumerated by hand, so a fixture added without one was simply not checked
/// and nothing anywhere noticed — eight had accumulated that way, including
/// both `matrix_*` fixtures and all four bi-Fourier ones. Enumeration cannot
/// fail open; a directory walk can only fail closed.
#[test]
fn every_fixture_matches_eccodes() {
    let mut checked = 0usize;
    let mut skipped = Vec::new();
    let mut failures = Vec::new();
    for fixture in fixture_names() {
        if let Some((_, why)) = NO_ECCODES_SNAPSHOT
            .iter()
            .find(|(name, _)| *name == fixture)
        {
            skipped.push((fixture, *why));
            continue;
        }
        // Every fixture is checked even after one fails, and the failures are
        // reported together. Stopping at the first is the diagnostic the 34
        // named tests gave for free, and it matters most exactly when this test
        // matters most: an eccodes bump moves many fixtures at once, and
        // finding that out one `cargo test` at a time is the slow way.
        let bytes = read_fixture(&fixture);
        let name = fixture.clone();
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            assert_fixture_matches_snapshot(&name, &bytes)
        })) {
            Ok(()) => checked += 1,
            Err(payload) => {
                let why = payload
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_string()))
                    .unwrap_or_else(|| "(non-string panic)".to_string());
                failures.push(format!("  {why}"));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} fixtures disagree with their eccodes snapshot:\n{}",
        failures.len(),
        failures.len() + checked,
        failures.join("\n")
    );
    assert!(
        checked > 41,
        "only {checked} fixtures were cross-checked — the directory walk found \
         too few, so this proves nothing (skipped: {skipped:?})"
    );
}

/// The cross-check compares every key it ships — no key is carried in the
/// snapshots but skipped by every fixture.
///
/// Every arm of [`assert_message_matches`] can return a bare `true` meaning
/// "not applicable to this template", which is indistinguishable from a passing
/// comparison. A key whose arm never matches any committed fixture would sit in
/// the snapshots looking like coverage and assert nothing — the same shape of
/// gap as the enumeration #471 replaced, one level down. Added with the GRIB1
/// counterpart (#475).
#[test]
fn the_cross_check_compares_every_key_it_ships() {
    COMPARED.with(|keys| keys.borrow_mut().clear());
    let mut shipped: BTreeSet<String> = BTreeSet::new();
    for fixture in fixture_names() {
        if NO_ECCODES_SNAPSHOT.iter().any(|(name, _)| *name == fixture) {
            continue;
        }
        let bytes = read_fixture(&fixture);
        assert_fixture_matches_snapshot(&fixture, &bytes);
        for message in snapshot_for(&fixture)["messages"]
            .as_array()
            .expect("messages array")
        {
            for (key, value) in message.as_object().expect("message object") {
                if !value.is_null() {
                    shipped.insert(key.clone());
                }
            }
        }
    }
    let compared = COMPARED.with(|keys| keys.borrow().clone());
    let never_compared: Vec<&String> = shipped.difference(&compared).collect();
    assert!(
        never_compared.is_empty(),
        "{} snapshot keys are never compared against the parser by any fixture, \
         so they are coverage in name only: {never_compared:?}",
        never_compared.len()
    );
    assert!(
        compared.len() >= 30,
        "only {} distinct keys were compared ({compared:?}) — too few for this \
         to be a cross-check of the parser",
        compared.len()
    );
}

/// Fixtures whose *values* have no comparable eccodes oracle, and why. The
/// metadata cross-check above still covers them.
///
/// Kept honest by [`the_value_exemptions_have_not_outlived_their_reason`],
/// which re-derives each claim rather than trusting the comment.
const NO_VALUE_CHECK: &[(&str, &str)] = &[
    (
        "matrix_reshape_16x31.grib2",
        "eccodes does not decode the true matrix form (`matrixBitmapsPresent = 1`) \
         — it reports 496 zeros for a field that is nothing of the kind, so there \
         is no oracle here to compare against. The decode is pinned against the \
         independently-checked GRIB1 matrix decoder in `decode_matrix.rs`.",
    ),
    (
        "complex_spd2_ng0_regular_latlon.grib2",
        "the pinned eccodes 2.34.1 predates ECC-2095 (fixed in 2.42.0) and \
         mis-decodes `numberOfGroupsOfDataValues = 0` for template 5.3: it \
         reads past the truncated §7 and returns garbage without erroring, \
         reporting a minimum of 284.271 for a field that is 270.466796875 \
         everywhere. The value oracle is `<fixture>_expected.json`, decoded \
         with eccodes 2.47.3 — see NOTICE.md — and checked in \
         `decode_complex_constant.rs`.",
    ),
];

/// Every committed GRIB2 fixture decodes to the values eccodes decodes.
///
/// The metadata pass (#475) compares two readings of the same octets; this
/// compares two decodes of the same packed field, which is where a packing bug
/// actually shows. It is the same walk, for the same reason: a fixture added
/// without a value oracle would otherwise be checked by nothing but whatever
/// per-fixture test its author happened to write (#481).
///
/// Three shapes of message route to their own entry point, because a field of
/// coefficients is not values on a grid: the §3.50 spectral fixtures go through
/// `decode_spectral_message` and the four §3.6x bi-Fourier ones through
/// `decode_bifourier_message`, each producing the flat coefficient list eccodes
/// reports for those messages.
///
/// **What this cannot see.** `grib_dump -j` prints values to six significant
/// figures, so the comparison is to a relative 1e-5 — enough for the errors a
/// packing bug actually makes (a wrong scale factor moves values by a power of
/// two or ten; a wrong bit width by orders of magnitude) and not a bit-exact
/// oracle. The per-fixture `<fixture>_expected.json` files remain the exact
/// check where one exists; this is the floor under every fixture, including the
/// ones nobody wrote an oracle for.
#[test]
fn every_fixture_decodes_to_the_values_eccodes_decodes() {
    let mut checked = 0usize;
    let (mut compared_stats, mut compared_points) = (0usize, 0usize);
    let mut failures = Vec::new();
    for fixture in fixture_names() {
        if NO_ECCODES_SNAPSHOT.iter().any(|(n, _)| *n == fixture)
            || NO_VALUE_CHECK.iter().any(|(n, _)| *n == fixture)
        {
            continue;
        }
        let bytes = read_fixture(&fixture);
        let name = fixture.clone();
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            assert_values_match_snapshot(&name, &bytes)
        })) {
            Ok((stats, points)) => {
                checked += 1;
                compared_stats += stats;
                compared_points += points;
            }
            Err(payload) => failures.push(format!("  {}", panic_message(payload))),
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} fixtures decode to different values than eccodes:\n{}",
        failures.len(),
        failures.len() + checked,
        failures.join("\n")
    );
    assert!(
        checked >= 47,
        "only {checked} fixtures were value-checked — too few to prove anything"
    );
    // The same guard the metadata pass carries: every branch of the comparison
    // is skippable when the snapshot omits a key, so count what was really
    // compared rather than trusting that the walk implies it.
    assert!(
        compared_stats >= 120 && compared_points >= 770,
        "the value check made {compared_stats} statistic and {compared_points} \
         point comparisons — too few to be checking the corpus"
    );
}

/// Decode one fixture and compare it with the snapshot's `values` block.
fn assert_values_match_snapshot(fixture: &str, bytes: &[u8]) -> (usize, usize) {
    let (mut stats, mut points) = (0usize, 0usize);
    let reader = Grib2Reader::from_bytes(bytes.to_vec())
        .unwrap_or_else(|e| panic!("{fixture}: parse failed: {e}"));
    let snap = snapshot_for(fixture);
    let blocks = snap["values"]
        .as_array()
        .unwrap_or_else(|| panic!("{fixture}: snapshot has no `values` array"));
    for (i, block) in blocks.iter().enumerate() {
        let Some(block) = block.as_object() else {
            // eccodes decoded nothing for this message; the metadata pass still
            // covers it and its own test pins whatever it does carry.
            continue;
        };
        let (decoded, kind) = decode_for_comparison(&reader, i)
            .unwrap_or_else(|e| panic!("{fixture}: message {i} decode failed: {e}"));
        let (s, p) = compare_values(fixture, &decoded, block, kind);
        stats += s;
        points += p;
    }
    (stats, points)
}

/// What a decoded message holds, which decides whether eccodes' statistics are
/// about the same quantity as ours.
#[derive(Clone, Copy, PartialEq)]
enum Decoded {
    /// Values on a grid: eccodes' `minimum`/`maximum`/`average` are the
    /// statistics of exactly this list.
    Field,
    /// Spherical-harmonic coefficients. eccodes still fills `average`, but with
    /// the mean of the *field* those coefficients describe (289.097 K for the
    /// T63 fixtures) rather than the mean of the coefficients (0.0668) — a
    /// different quantity, so only the coefficients themselves are compared.
    Coefficients,
}

/// The decoded field as a flat list, whichever entry point the message needs.
/// `None` marks a bitmap-masked point, which is what eccodes writes its missing
/// sentinel into.
fn decode_for_comparison(
    reader: &Grib2Reader,
    index: usize,
) -> Result<(Vec<Option<f64>>, Decoded), fieldglass_core::FieldglassError> {
    let coefficients = |values: Vec<f64>| {
        Ok((
            values.into_iter().map(Some).collect::<Vec<_>>(),
            Decoded::Coefficients,
        ))
    };
    match &reader.messages[index].gds.template {
        // eccodes reports a spectral message's `values` as the coefficient
        // list, real and imaginary interleaved — the same order and length
        // `decode_spectral_message` produces.
        GridTemplate::SphericalHarmonic(_) => {
            coefficients(reader.decode_spectral_message(index)?.coefficients)
        }
        GridTemplate::BiFourier(_) => {
            coefficients(reader.decode_bifourier_message(index)?.coefficients)
        }
        _ => {
            let mut values = reader.decode_message_values(index)?;
            put_back_in_storage_order(&reader.messages[index].gds, &mut values);
            Ok((values, Decoded::Field))
        }
    }
}

/// Re-apply alternate-row scanning, because eccodes' snapshot is in storage
/// order and ours is not.
///
/// `decode_message_values` regularises a boustrophedon field's rows (#541).
/// eccodes' `values` key does not: the flip lives in its *geoiterator*
/// (`transform_iterator_data`, which is what `grib_get_data` prints), and
/// rewriting the `values` array is the separate, opt-in
/// `swapScanningAlternativeRows` key that `grib_dump -j` never packs. So the
/// two decodes genuinely disagree about layout while agreeing about every
/// value, and comparing them means picking one convention. This picks
/// eccodes', since that is what the snapshots hold.
///
/// The cost is real and worth naming: for a fixture with this flag set, the
/// sampled points stop being a check on row order — the helper is an
/// involution, so applying it here undoes whatever the decoder did, right or
/// wrong. What this pass still checks for such a fixture is the packing, which
/// is its subject. Row order is checked, against eccodes' geoiterator rather
/// than its `values` key, in `decode_alternate_rows.rs`.
fn put_back_in_storage_order(
    gds: &fieldglass_grib2::GridDefinitionSection,
    values: &mut [Option<f64>],
) {
    let Some(sm) = gds.scanning_mode() else {
        return;
    };
    if sm & fieldglass_grib2::SCAN_ALTERNATE_ROWS == 0 {
        return;
    }
    match gds.points_per_row() {
        Some(pl) => fieldglass_grib2::undo_alternate_reduced_rows(values, pl),
        None => {
            if let Some((ni, _)) = gds.dimensions() {
                fieldglass_grib2::undo_alternate_rows(values, ni as usize);
            }
        }
    }
}

/// Compare a decoded field against the snapshot's value block: the point
/// count, how many the bitmap masks, the statistics over the present points,
/// and the sampled points themselves.
///
/// Statistics alone would not see a permutation — a scan-order bug leaves the
/// min, max and mean exactly where they were — so the sample is the half of
/// this that catches one.
fn compare_values(
    fixture: &str,
    decoded: &[Option<f64>],
    block: &serde_json::Map<String, Value>,
    kind: Decoded,
) -> (usize, usize) {
    let (mut stats, mut points) = (0usize, 0usize);
    let number = |key: &str| block.get(key).and_then(Value::as_f64);

    if let Some(count) = number("count") {
        assert_eq!(
            decoded.len(),
            count as usize,
            "{fixture}: decoded {} points, eccodes decoded {count}",
            decoded.len()
        );
    }
    if let Some(missing) = number("numberOfMissing") {
        let ours = decoded.iter().filter(|v| v.is_none()).count();
        assert_eq!(
            ours, missing as usize,
            "{fixture}: {ours} masked points, eccodes says {missing}"
        );
    }

    let present: Vec<f64> = decoded.iter().flatten().copied().collect();
    if !present.is_empty() && kind == Decoded::Field {
        // eccodes prints its statistics to six significant figures, so they are
        // compared relatively; an absolute floor keeps a field of near-zero
        // values from demanding impossible precision.
        let close = |got: f64, want: f64| (got - want).abs() <= 1e-5 * want.abs().max(1e-3);
        if let Some(want) = number("minimum") {
            let got = present.iter().copied().fold(f64::INFINITY, f64::min);
            assert!(close(got, want), "{fixture}: minimum {got}, eccodes {want}");
            stats += 1;
        }
        if let Some(want) = number("maximum") {
            let got = present.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            assert!(close(got, want), "{fixture}: maximum {got}, eccodes {want}");
            stats += 1;
        }
        if let Some(want) = number("average") {
            let got = present.iter().sum::<f64>() / present.len() as f64;
            assert!(close(got, want), "{fixture}: average {got}, eccodes {want}");
            stats += 1;
        }
    }

    let Some(sample) = block.get("sample").and_then(Value::as_array) else {
        return (stats, points);
    };
    for entry in sample {
        let pair = entry.as_array().expect("sample entry is [index, value]");
        let index = pair[0].as_u64().expect("sample index") as usize;
        let want = &pair[1];
        let got = decoded[index];
        if want.is_null() {
            assert!(
                got.is_none(),
                "{fixture}: point {index} is masked for eccodes, we decoded {got:?}"
            );
            points += 1;
            continue;
        }
        let want = want.as_f64().expect("sample value is a number");
        let got =
            got.unwrap_or_else(|| panic!("{fixture}: point {index} masked, eccodes has {want}"));
        assert!(
            (got - want).abs() <= 1e-5 * want.abs().max(1e-3),
            "{fixture}: point {index} decoded {got}, eccodes decoded {want}"
        );
        points += 1;
    }
    (stats, points)
}

/// The message from a caught panic, for the aggregated failure report.
fn panic_message(payload: Box<dyn std::any::Any + Send>) -> String {
    payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_string()))
        .unwrap_or_else(|| "(non-string panic)".to_string())
}

/// A value exemption has to keep earning itself: an exempt fixture must either
/// still be undecodable, or still have the alternative oracle its reason names.
/// Anything else is an exemption that has outlived its reason — the failure
/// mode #471 and #475 were both about, one level further down.
#[test]
fn the_value_exemptions_have_not_outlived_their_reason() {
    let fixtures = fixture_names();
    for (name, why) in NO_VALUE_CHECK {
        assert!(
            fixtures.iter().any(|f| f == name),
            "{name} is exempted from the value check but is not a fixture"
        );
        let bytes = read_fixture(name);
        let decodes = Grib2Reader::from_bytes(bytes.to_vec())
            .ok()
            .and_then(|r| decode_for_comparison(&r, 0).ok())
            .is_some();
        if !decodes {
            continue; // "we cannot decode it" — still true.
        }
        let oracle = Path::new("tests/fixtures").join(name.replace(".grib2", "_expected.json"));
        assert!(
            oracle.exists(),
            "{name} decodes and has no {} to check it against, so its exemption \
             ({why}) covers nothing",
            oracle.display()
        );
    }
}

/// An exemption that has acquired a snapshot is stale, and a name that is not a
/// fixture is a typo. Both would silently shrink the set above.
#[test]
fn the_exemption_list_has_no_stale_entries() {
    let fixtures = fixture_names();
    for (name, why) in NO_ECCODES_SNAPSHOT {
        assert!(
            fixtures.iter().any(|f| f == name),
            "{name} is exempted but is not a fixture"
        );
        assert!(
            !Path::new("tests/fixtures")
                .join(format!("{name}.eccodes.ref.json"))
                .exists(),
            "{name} is exempted ({why}) but now has a snapshot — drop the exemption"
        );
    }
}

/// Every `*.grib2` under `tests/fixtures`, sorted so a failure is reproducible.
fn fixture_names() -> Vec<String> {
    let dir = Path::new("tests/fixtures");
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
        .filter_map(|entry| {
            let name = entry.ok()?.file_name().to_string_lossy().into_owned();
            name.ends_with(".grib2").then_some(name)
        })
        .collect();
    names.sort();
    names
}
