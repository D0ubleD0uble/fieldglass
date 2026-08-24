//! Snapshot-based cross-check of `fieldglass-grib1` against eccodes.
//!
//! GRIB2 has had this since #109 and it has found real bugs; GRIB1 had nothing
//! at all (#475). Every GRIB1 metadata assertion in the suite was against a
//! value someone wrote down, never against a second implementation of the same
//! octets.
//!
//! For each fixture under `tests/fixtures/`, we ship a sibling
//! `.eccodes.ref.json` capturing `grib_dump -j` for a curated subset of keys.
//! This test loads each snapshot, parses the fixture, and asserts the two agree
//! on every key present. The fixture list comes from the directory, not from
//! here: an enumeration can fail open, a walk cannot. A fixture eccodes
//! genuinely cannot decode goes in [`NO_ECCODES_SNAPSHOT`] with its reason, and
//! that list is itself checked for staleness.
//!
//! The snapshots are checked into git, so this test needs no eccodes at
//! runtime; eccodes is required only to regenerate them, via
//! `tools/regenerate-eccodes-snapshots.py`.
//!
//! **What the key set deliberately leaves out.** Keys eccodes *derives* rather
//! than reads — `stepRange`, `isConstant`, the field statistics — say nothing
//! about whether two parsers read the same octets the same way. So do
//! `shortName` and friends, which come from eccodes' own parameter tables (the
//! GRIB1 tables are cross-checked against eccodes separately, in
//! `tables.rs`/`tables_ecmwf.rs`). The second-order sub-header widths (`N2`,
//! `NL`, `widthOfWidths`, `widthOfLengths`) are read inside the packing
//! decoder rather than published on a type, so there is nothing here to compare
//! them against; their effect is pinned by the value oracles in
//! `decode_second_order_classic.rs` and `decode_ecmwf_complex.rs`.

use fieldglass_grib1::{
    Grib1Message, Grib1Reader, GridDescription, parse_bds_header, reader::level_value,
};
use serde_json::Value;
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::path::Path;

thread_local! {
    /// Keys that a `check_*` actually compared, as opposed to keys an arm
    /// declared not applicable to this grid family. Every arm of the dispatch
    /// can return a bare `true`, which is indistinguishable from a passing
    /// comparison unless something counts — see
    /// [`the_cross_check_compares_every_key_it_ships`].
    static COMPARED: RefCell<BTreeSet<String>> = const { RefCell::new(BTreeSet::new()) };
}

fn record(key: &str) {
    COMPARED.with(|keys| keys.borrow_mut().insert(key.to_string()));
}

/// Float tolerance. GRIB1 stores coordinates as milli-degrees and eccodes
/// prints them back as decimals, so 1e-3 absorbs the rounding; a mismatch here
/// is a real scale-factor bug. `referenceValue` is an IBM float that eccodes
/// prints to six significant figures, which is coarser — see [`check_f64`].
const FLOAT_EPS: f64 = 1e-3;

fn snapshot_for(fixture: &str) -> Value {
    let path = Path::new("tests/fixtures").join(format!("{fixture}.eccodes.ref.json"));
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("read snapshot {}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse snapshot {}: {e}", path.display()))
}

/// eccodes' `gridType` for one of our [`GridDescription`] variants. GRIB1
/// selects the grid with a data-representation type code, and the two projects
/// spell the resulting families differently, so the mapping is written out
/// rather than string-compared.
fn eccodes_grid_type(gds: &GridDescription) -> &'static str {
    match gds {
        GridDescription::LatLon(_) => "regular_ll",
        GridDescription::RotatedLatLon(_) => "rotated_ll",
        GridDescription::ReducedLatLon(_) => "reduced_ll",
        GridDescription::Gaussian(_) => "regular_gg",
        GridDescription::ReducedGaussian(_) => "reduced_gg",
        GridDescription::PolarStereographic(_) => "polar_stereographic",
        GridDescription::LambertConformal(_) => "lambert",
        GridDescription::SphericalHarmonic(_) => "sh",
        GridDescription::Unsupported { .. } => "unsupported",
    }
}

/// Level types whose two PDS octets are independent layer bounds rather than
/// one 16-bit value. eccodes reports a layer's `level` from its own
/// `topLevel`/`bottomLevel` rules, so the raw combination is not the same
/// quantity and comparing them would assert a coincidence. No committed
/// fixture uses one today; if one arrives, this skips it rather than failing
/// on a difference that isn't a bug.
fn level_is_a_layer(level_type: u8) -> bool {
    matches!(
        level_type,
        101 | 104 | 106 | 108 | 110 | 112 | 114 | 116 | 120 | 121 | 128 | 141
    )
}

fn assert_message_matches(
    fixture: &str,
    msg: &Grib1Message,
    reader: &Grib1Reader,
    snap: &serde_json::Map<String, Value>,
) {
    let pds = &msg.pds;
    // The BDS header is 11 octets plus the complex-packing extension; parsing
    // it here (rather than decoding values) is what the §4 keys compare
    // against.
    let (bds_start, bds_end) = msg.bds_range;
    let bds = parse_bds_header(&reader.bytes()[bds_start..bds_end])
        .unwrap_or_else(|e| panic!("{fixture}: BDS header parse failed: {e}"));
    let ext = bds.complex_extended;

    for (key, expected) in snap {
        // `null` in the snapshot means eccodes itself omitted the field — a
        // reduced grid has no `Ni`, a constant field no increments.
        if expected.is_null() {
            continue;
        }
        // Every arm returns `true` for "not applicable to this grid family",
        // which is indistinguishable from "compared and equal" unless the
        // comparison says so. `check_*` records the key; a bare `true` does not.
        let pinned = match key.as_str() {
            // --- Product Definition Section ------------------------------
            "editionNumber" => check_u64(key, expected, msg.is.edition as u64),
            "table2Version" => check_u64(key, expected, pds.table_version as u64),
            "centre" => check_u64(key, expected, pds.originating_centre as u64),
            "subCentre" => check_u64(key, expected, pds.sub_centre as u64),
            "generatingProcessIdentifier" => {
                check_u64(key, expected, pds.generating_process as u64)
            }
            "indicatorOfParameter" => check_u64(key, expected, pds.parameter_id as u64),
            "indicatorOfTypeOfLevel" => check_u64(key, expected, pds.level_type as u64),
            "level" => {
                level_is_a_layer(pds.level_type) || check_f64(key, expected, level_value(pds))
            }
            "timeRangeIndicator" => check_u64(key, expected, pds.time_range as u64),
            "dataDate" => {
                let year = (pds.century as u64 - 1) * 100 + pds.reference_year as u64;
                check_u64(
                    key,
                    expected,
                    year * 10_000 + pds.reference_month as u64 * 100 + pds.reference_day as u64,
                )
            }
            "dataTime" => check_u64(
                key,
                expected,
                pds.reference_hour as u64 * 100 + pds.reference_minute as u64,
            ),
            "decimalScaleFactor" => check_i64(key, expected, pds.decimal_scale_factor as i64),
            "GDSPresent" => check_bool(key, expected, pds.has_gds),
            "bitmapPresent" => check_bool(key, expected, pds.has_bms),

            // --- Grid Description Section --------------------------------
            "gridType" => match &msg.gds {
                Some(gds) => check_str(key, expected, eccodes_grid_type(gds)),
                // No GDS: the grid comes from the predefined-grid catalogue,
                // which is cross-checked in `predefined_grid.rs`.
                None => true,
            },
            "Ni" | "Nx" => match msg.gds.as_ref().and_then(|g| g.dimensions()) {
                Some((ni, _)) => check_u64(key, expected, ni as u64),
                None => true,
            },
            "Nj" | "Ny" => match msg.gds.as_ref().and_then(|g| g.dimensions()) {
                Some((_, nj)) => check_u64(key, expected, nj as u64),
                None => true,
            },
            "numberOfDataPoints" => match msg.gds.as_ref().and_then(|g| g.num_data_points()) {
                Some(n) => check_u64(key, expected, n as u64),
                None => true,
            },
            "latitudeOfFirstGridPointInDegrees" => match grid_corner(msg) {
                Some((la1, _, _, _)) => check_f64(key, expected, la1),
                None => true,
            },
            "longitudeOfFirstGridPointInDegrees" => match grid_corner(msg) {
                Some((_, lo1, _, _)) => check_f64(key, expected, lo1),
                None => true,
            },
            "latitudeOfLastGridPointInDegrees" => match grid_corner(msg) {
                Some((_, _, Some(la2), _)) => check_f64(key, expected, la2),
                _ => true,
            },
            "longitudeOfLastGridPointInDegrees" => match grid_corner(msg) {
                Some((_, _, _, Some(lo2))) => check_f64(key, expected, lo2),
                _ => true,
            },
            "iDirectionIncrementInDegrees" => match &msg.gds {
                Some(GridDescription::LatLon(g)) => check_f64(key, expected, g.di),
                Some(GridDescription::RotatedLatLon(g)) => check_f64(key, expected, g.di),
                Some(GridDescription::Gaussian(g)) => check_f64(key, expected, g.di),
                _ => true,
            },
            "jDirectionIncrementInDegrees" => match &msg.gds {
                Some(GridDescription::LatLon(g)) => check_f64(key, expected, g.dj),
                Some(GridDescription::RotatedLatLon(g)) => check_f64(key, expected, g.dj),
                _ => true,
            },
            "DxInMetres" => match &msg.gds {
                Some(GridDescription::PolarStereographic(g)) => {
                    check_u64(key, expected, g.dx_m as u64)
                }
                Some(GridDescription::LambertConformal(g)) => {
                    check_u64(key, expected, g.dx_m as u64)
                }
                _ => true,
            },
            "DyInMetres" => match &msg.gds {
                Some(GridDescription::PolarStereographic(g)) => {
                    check_u64(key, expected, g.dy_m as u64)
                }
                Some(GridDescription::LambertConformal(g)) => {
                    check_u64(key, expected, g.dy_m as u64)
                }
                _ => true,
            },
            "orientationOfTheGridInDegrees" => match &msg.gds {
                Some(GridDescription::PolarStereographic(g)) => check_f64(key, expected, g.lov),
                Some(GridDescription::LambertConformal(g)) => check_f64(key, expected, g.lov),
                _ => true,
            },
            "southPoleOnProjectionPlane" => match &msg.gds {
                Some(GridDescription::PolarStereographic(g)) => {
                    check_bool(key, expected, g.south_pole)
                }
                Some(GridDescription::LambertConformal(g)) => {
                    check_bool(key, expected, g.south_pole)
                }
                _ => true,
            },
            "earthIsOblate" => match resolution_flags(msg) {
                Some(f) => check_bool(key, expected, f.earth_oblate),
                None => true,
            },
            "uvRelativeToGrid" => match resolution_flags(msg) {
                Some(f) => check_bool(key, expected, f.uv_relative_to_grid),
                None => true,
            },
            "iScansNegatively" => match scanning_mode(msg) {
                Some(s) => check_bool(key, expected, s.i_negative),
                None => true,
            },
            "jScansPositively" => match scanning_mode(msg) {
                Some(s) => check_bool(key, expected, s.j_positive),
                None => true,
            },
            "jPointsAreConsecutive" => match scanning_mode(msg) {
                Some(s) => check_bool(key, expected, s.j_consecutive),
                None => true,
            },
            // On a Gaussian grid `N` is the number of parallels between pole
            // and equator. On a spherical-harmonic message eccodes reuses the
            // name for the coefficient count, which is not a field we parse.
            "N" => match &msg.gds {
                Some(GridDescription::Gaussian(g)) => {
                    check_u64(key, expected, g.n_gaussians as u64)
                }
                Some(GridDescription::ReducedGaussian(g)) => {
                    check_u64(key, expected, g.n_gaussians as u64)
                }
                _ => true,
            },
            // The row-length list of a reduced grid, compared element for
            // element. `numberOfDataPoints` only pins its sum, which any
            // redistribution of points between rows would survive.
            "pl" => match &msg.gds {
                Some(GridDescription::ReducedLatLon(g)) => {
                    check_u32_array(key, expected, &g.points_per_row)
                }
                Some(GridDescription::ReducedGaussian(g)) => {
                    check_u32_array(key, expected, &g.points_per_row)
                }
                _ => true,
            },
            "J" => match &msg.gds {
                Some(GridDescription::SphericalHarmonic(g)) => check_u64(key, expected, g.j as u64),
                _ => true,
            },
            "K" => match &msg.gds {
                Some(GridDescription::SphericalHarmonic(g)) => check_u64(key, expected, g.k as u64),
                _ => true,
            },
            "M" => match &msg.gds {
                Some(GridDescription::SphericalHarmonic(g)) => check_u64(key, expected, g.m as u64),
                _ => true,
            },

            // --- Binary Data Section -------------------------------------
            // The label our decoders route on, against the one eccodes' own
            // concept dispatch produces from the same flag bits.
            "packingType" => match reader.packing_label(msg.message_index) {
                Some(label) => check_str(key, expected, label),
                None => true,
            },
            "sphericalHarmonics" => check_bool(key, expected, bds.is_spherical_harmonic),
            "complexPacking" => check_bool(key, expected, bds.is_complex_packing),
            "integerPointValues" => check_bool(key, expected, bds.is_integer_data),
            "additionalFlagPresent" => check_bool(key, expected, bds.has_extra_flags),
            // Octet 11 is the per-point width only while the packing is
            // simple. Under second-order packing eccodes stops reading an
            // octet for this key at all and computes it from the decoded field
            // (`data.grid_second_order.def`: `meta bitsPerValue
            // second_order_bits_per_value(codedValues, binaryScaleFactor,
            // decimalScaleFactor)`), which is a different quantity — 24 where
            // octet 11 holds 20. The octet itself is still compared, under the
            // name eccodes gives it there: `widthOfFirstOrderValues`.
            "bitsPerValue" => {
                (bds.is_complex_packing && !bds.is_spherical_harmonic)
                    || check_u64(key, expected, bds.bits_per_value as u64)
            }
            // Octet 11 again: complex packing repurposes the per-point width as
            // the width of the first-order values, and eccodes renames it to
            // match. Same octet, so the same field answers both keys.
            "widthOfFirstOrderValues" => check_u64(key, expected, bds.bits_per_value as u64),
            "binaryScaleFactor" => check_i64(key, expected, bds.binary_scale_factor as i64),
            "referenceValue" => check_f64(key, expected, bds.reference_value),
            "matrixOfValues" => match ext {
                Some(e) => check_bool(key, expected, e.matrix_of_values()),
                None => true,
            },
            "secondaryBitmapPresent" => match ext {
                Some(e) => check_bool(key, expected, e.secondary_bitmap_present()),
                None => true,
            },
            "secondOrderOfDifferentWidth" => match ext {
                Some(e) => check_bool(key, expected, e.second_order_of_different_width()),
                None => true,
            },
            "generalExtended2ordr" => match ext {
                Some(e) => check_bool(key, expected, e.general_extended_2ordr()),
                None => true,
            },
            "boustrophedonicOrdering" => match ext {
                Some(e) => check_bool(key, expected, e.boustrophedonic()),
                None => true,
            },
            "twoOrdersOfSPD" => match ext {
                Some(e) => check_bool(key, expected, e.two_orders_of_spd()),
                None => true,
            },
            "plusOneinOrdersOfSPD" => match ext {
                Some(e) => check_bool(key, expected, e.plus_one_in_orders_of_spd()),
                None => true,
            },

            unknown => panic!(
                "{fixture}: snapshot has key {unknown:?} with no parser-field mapping; \
                 update assert_message_matches in eccodes_reference.rs",
            ),
        };
        assert!(pinned, "{fixture}: key {key:?} mismatch");
    }
}

/// `(lat_first, lon_first, lat_last, lon_last)` for the grid families that
/// declare corners. The projected families (polar stereographic, Lambert)
/// declare only the first point, so their last corner is `None`.
type GridCorner = (f64, f64, Option<f64>, Option<f64>);

fn grid_corner(msg: &Grib1Message) -> Option<GridCorner> {
    match msg.gds.as_ref()? {
        GridDescription::LatLon(g) => {
            Some((g.lat_first, g.lon_first, Some(g.lat_last), Some(g.lon_last)))
        }
        GridDescription::RotatedLatLon(g) => {
            Some((g.lat_first, g.lon_first, Some(g.lat_last), Some(g.lon_last)))
        }
        GridDescription::ReducedLatLon(g) => {
            Some((g.lat_first, g.lon_first, Some(g.lat_last), Some(g.lon_last)))
        }
        GridDescription::Gaussian(g) => {
            Some((g.lat_first, g.lon_first, Some(g.lat_last), Some(g.lon_last)))
        }
        GridDescription::ReducedGaussian(g) => {
            Some((g.lat_first, g.lon_first, Some(g.lat_last), Some(g.lon_last)))
        }
        GridDescription::PolarStereographic(g) => Some((g.lat_first, g.lon_first, None, None)),
        GridDescription::LambertConformal(g) => Some((g.lat_first, g.lon_first, None, None)),
        GridDescription::SphericalHarmonic(_) | GridDescription::Unsupported { .. } => None,
    }
}

fn resolution_flags(msg: &Grib1Message) -> Option<&fieldglass_grib1::gds::ResolutionFlags> {
    match msg.gds.as_ref()? {
        GridDescription::LatLon(g) => Some(&g.resolution_flags),
        GridDescription::RotatedLatLon(g) => Some(&g.resolution_flags),
        GridDescription::ReducedLatLon(g) => Some(&g.resolution_flags),
        GridDescription::Gaussian(g) => Some(&g.resolution_flags),
        GridDescription::ReducedGaussian(g) => Some(&g.resolution_flags),
        GridDescription::PolarStereographic(g) => Some(&g.resolution_flags),
        GridDescription::LambertConformal(g) => Some(&g.resolution_flags),
        GridDescription::SphericalHarmonic(_) | GridDescription::Unsupported { .. } => None,
    }
}

fn scanning_mode(msg: &Grib1Message) -> Option<&fieldglass_grib1::gds::ScanningMode> {
    match msg.gds.as_ref()? {
        GridDescription::LatLon(g) => Some(&g.scanning_mode),
        GridDescription::RotatedLatLon(g) => Some(&g.scanning_mode),
        GridDescription::ReducedLatLon(g) => Some(&g.scanning_mode),
        GridDescription::Gaussian(g) => Some(&g.scanning_mode),
        GridDescription::ReducedGaussian(g) => Some(&g.scanning_mode),
        GridDescription::PolarStereographic(g) => Some(&g.scanning_mode),
        GridDescription::LambertConformal(g) => Some(&g.scanning_mode),
        GridDescription::SphericalHarmonic(_) | GridDescription::Unsupported { .. } => None,
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

/// eccodes prints its flag keys as 0/1 integers.
fn check_bool(key: &str, expected: &Value, actual: bool) -> bool {
    check_u64(key, expected, u64::from(actual))
}

fn check_u32_array(key: &str, expected: &Value, actual: &[u32]) -> bool {
    record(key);
    let exp = expected
        .as_array()
        .unwrap_or_else(|| panic!("snapshot {key:?} is not an array: {expected}"));
    if exp.len() != actual.len() {
        eprintln!(
            "key {key}: eccodes has {} entries, parser has {}",
            exp.len(),
            actual.len()
        );
        return false;
    }
    for (i, (want, got)) in exp.iter().zip(actual).enumerate() {
        let want = want
            .as_u64()
            .unwrap_or_else(|| panic!("snapshot {key:?}[{i}] is not an integer: {want}"));
        if want != *got as u64 {
            eprintln!("key {key}[{i}]: eccodes={want}, parser={got}");
            return false;
        }
    }
    true
}

fn check_str(key: &str, expected: &Value, actual: &str) -> bool {
    record(key);
    let exp = expected
        .as_str()
        .unwrap_or_else(|| panic!("snapshot {key:?} is not a string: {expected}"));
    if exp != actual {
        eprintln!("key {key}: eccodes={exp:?}, parser={actual:?}");
        return false;
    }
    true
}

/// Absolute tolerance for the coordinate keys, relative for the reference
/// value: `referenceValue` is an IBM-format float that eccodes prints to six
/// significant figures, so a field whose reference is in the hundreds of
/// thousands cannot be compared to a thousandth.
fn check_f64(key: &str, expected: &Value, actual: f64) -> bool {
    record(key);
    let exp = expected
        .as_f64()
        .unwrap_or_else(|| panic!("snapshot {key:?} is not a number: {expected}"));
    let tolerance = FLOAT_EPS.max(exp.abs() * 1e-5);
    if (exp - actual).abs() > tolerance {
        eprintln!(
            "key {key}: eccodes={exp}, parser={actual}, diff={}",
            (exp - actual).abs()
        );
        return false;
    }
    true
}

fn assert_fixture_matches_snapshot(fixture: &str, bytes: &[u8]) {
    let reader = Grib1Reader::from_bytes(bytes.to_vec())
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
        assert_message_matches(fixture, &reader.messages[i], &reader, snap_obj);
    }
}

fn read_fixture(fixture: &str) -> Vec<u8> {
    let path = Path::new("tests/fixtures").join(fixture);
    std::fs::read(&path).unwrap_or_else(|e| panic!("read fixture {}: {e}", path.display()))
}

/// Fixtures with no snapshot, and why. eccodes must be unable to decode them —
/// nothing else is a reason to skip the cross-check.
///
/// Keep in step with the `undecodable` set for GRIB1 in
/// `tools/regenerate-eccodes-snapshots.py`. The two lists cannot import each
/// other across languages, so [`the_exemption_list_has_no_stale_entries`]
/// checks this one against the filesystem instead.
const NO_ECCODES_SNAPSHOT: &[(&str, &str)] = &[(
    "hand_matrix_of_values.grib1",
    "the true `matrixOfValues` form, which eccodes 2.34.1 can neither encode \
     nor decode — `grib_dump` aborts inside its own secondary-bitmap accessor \
     (\"assertion failed: `m <= secondary_len'\"). Decode is cross-checked \
     against the GRIB2 matrix decoder on the same hand-computed field in \
     `decode_matrix.rs`.",
)];

/// Every committed GRIB1 fixture is cross-checked against eccodes.
///
/// The walk is the point (#475, following #471): a fixture added without a
/// snapshot is checked and fails, rather than being quietly skipped by an
/// enumeration nobody updated.
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
        // reported together: an eccodes bump moves many fixtures at once, and
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
        checked >= 15,
        "only {checked} fixtures were cross-checked — the directory walk found \
         too few, so this proves nothing (skipped: {skipped:?})"
    );
}

/// The cross-check compares every key it ships — no key is carried in the
/// snapshots but skipped by every fixture.
///
/// This is the failure this test is most exposed to. Every arm of the dispatch
/// can return a bare `true` meaning "not applicable to this grid family", which
/// is indistinguishable from a passing comparison; a key whose arm never
/// matches any committed fixture would sit in the snapshots looking like
/// coverage and assert nothing. So: collect the keys any fixture really
/// compared, collect the keys the snapshots actually carry a value for, and
/// require the two sets to be equal.
///
/// A key that is genuinely uncheckable everywhere belongs out of the curated
/// list in `tools/regenerate-eccodes-snapshots.py`, not silently in it.
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
        "{} snapshot keys are never compared against the parser by any fixture,          so they are coverage in name only: {never_compared:?}",
        never_compared.len()
    );
    assert!(
        compared.len() >= 30,
        "only {} distinct keys were compared ({compared:?}) — too few for this          to be a cross-check of the parser",
        compared.len()
    );
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

/// Every GRIB1 fixture under `tests/fixtures`, sorted so a failure is
/// reproducible. Both extensions in use are matched: the corpus carries `.grib`
/// (as the file was published) as well as `.grib1`.
fn fixture_names() -> Vec<String> {
    let dir = Path::new("tests/fixtures");
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
        .filter_map(|entry| {
            let name = entry.ok()?.file_name().to_string_lossy().into_owned();
            (name.ends_with(".grib1") || name.ends_with(".grib")).then_some(name)
        })
        .collect();
    names.sort();
    names
}
