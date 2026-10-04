//! libFuzzer target for the GRIB1 decode path.
//!
//! `fieldglass-grib1` parses attacker-controllable bytes (IS/PDS/GDS/BMS/BDS),
//! the highest-severity bug class for a binary parser. This target drives the
//! full scan-plus-decode pipeline — `Grib1Reader::from_bytes` followed by
//! `decode_message_values` for every message it finds — asserting the parser
//! never panics, over-reads, or hangs on arbitrary input. The length- and
//! offset-driven bit reading in the second-order packing decoders is the main
//! thing this exercises.
//!
//! `decode_spectral_message` joins it because it is a second decode path with
//! its own bit unpacking, reached only by a spherical-harmonic BDS and so never
//! touched by `decode_message_values`. Leaving it out is what let an unbounded
//! coefficient allocation live there: `J` is a bare `u16` from the GDS, and at
//! a zero bit width no §7 budget constrains it, so a 112-byte message sized a
//! `Vec` at up to 34 GB (#631). The GRIB2 target has always driven its
//! equivalent, and had a cap; this one did not, and did not.
//!
//! `decode_matrix_message` joins it for the same reason: the true
//! `matrixOfValues = 1` form is refused by `decode_message_values`, and its
//! secondary-bitmap reshape is sized by `NR·NC`, two bare `u16`s. A primary
//! bitmap marking every point absent empties the secondary bitmaps and the
//! coded stream, so no section length constrains them, and a 1.3 KB message
//! asked for about 2 TB and aborted the process (#802). The GRIB2 target drove
//! its matrix path and had the cap; this one did not. The seed
//! `hand_matrix_of_values_all_absent.grib1` is that message, which now stops at
//! the cap.
//!
//! `synthesis_grid` joins it because it is a public entry point that reads §2
//! and answers a grid size derived from attacker-controlled fields (#580). Its
//! partner `synthesize_message_global` is deliberately **not** called: the only
//! GRIB1 synthesis family is spherical-harmonic, so every call would run the
//! full inverse transform on the 720×361 grid, and the transform's cost is
//! `O(points × coefficients)` with the coefficient count coming from the file —
//! a legitimate high-truncation input would surface as a fuzzer timeout rather
//! than a finding. The GRIB2 target states the same rule and can still reach
//! its HEALPix arm, whose cost `Nside` bounds.

#![no_main]

use libfuzzer_sys::fuzz_target;

use fieldglass_grib1::{Grib1Reader, GridDescription, MAX_FIELD_POINTS};

/// Latitudes/longitudes for the synthesis probe, tiny for the reason the GRIB2
/// target gives: the coefficient count comes from the file.
const PROBE_LATS: [f64; 3] = [-60.0, 0.0, 60.0];
const PROBE_LONS: [f64; 3] = [0.0, 120.0, 240.0];

/// The declared spherical-harmonic truncation of message `i`, if it has one,
/// against a bound chosen for fuzzer throughput rather than for correctness.
///
/// `(J+1)(J+2)` values at eight bytes each is 2.1 MB at `J = 512` and 537 MB at
/// `MAX_TRUNCATION`; the decoder's arithmetic is the same either way.
fn declares_a_large_truncation(reader: &Grib1Reader, i: usize) -> bool {
    const FUZZ_MAX_TRUNCATION: u16 = 512;
    matches!(
        reader.messages.get(i).and_then(|m| m.gds.as_ref()),
        Some(GridDescription::SphericalHarmonic(sh)) if sh.j > FUZZ_MAX_TRUNCATION
    )
}

/// The grid points above which a message's scalar and matrix decodes are not
/// run, unless the reader refuses them outright.
///
/// Both are bounded by `MAX_FIELD_POINTS` (64 Mi points), sized for a real
/// grid rather than for a fuzz run. A constant field (zero bits per value)
/// needs no BDS data, so an 84-byte message declaring an 8192 × 8192 grid
/// decodes to a gigabyte of `Option<f64>`, about a second of writes; a dozen
/// such messages in one input exceed the run's ten-second timeout. That is the
/// reader working as designed. The bit unpacking and the bitmap are reached at
/// small sizes, so nothing is lost below this, and a declared size past the cap
/// is still decoded so the refusal stays fuzzed.
///
/// A matrix field multiplies its grid by `NR·NC` (see [`matrix_cells`]).
const FUZZ_MAX_FIELD_POINTS: u64 = 1 << 22;

/// The grid points message `i` declares: its GDS's own count (the `PL` sum for
/// a reduced grid) or the raster it states, whichever is larger. A message with
/// no GDS uses a predefined grid, which is small.
fn declared_points(reader: &Grib1Reader, i: usize) -> u64 {
    let Some(gds) = reader.messages.get(i).and_then(|m| m.gds.as_ref()) else {
        return 0;
    };
    let raster = gds
        .dimensions()
        .map_or(0, |(ni, nj)| u64::from(ni) * u64::from(nj));
    gds.num_data_points()
        .map_or(u64::MAX, |n| n as u64)
        .max(raster)
}

/// `NR·NC` as BDS octets 15-18 state it, which is where the matrix decoder
/// reads it; one when the section is too short to hold them, or states zero.
///
/// Read whatever the BDS flags say, so this is an upper bound: for a message
/// that is not a matrix those octets mean something else, and the only cost is
/// skipping a `decode_matrix_message` the reader would refuse anyway. Reading
/// the flags here instead would mean restating which octet-4 bits select the
/// matrix packing and which `extendedFlag` bit marks it in that context.
fn matrix_cells(reader: &Grib1Reader, data: &[u8], i: usize) -> u64 {
    let Some(range) = reader.messages.get(i).map(|m| m.bds_range) else {
        return 1;
    };
    let Some(bds) = usize::try_from(range.start)
        .ok()
        .and_then(|start| data.get(start..start.checked_add(18)?))
    else {
        return 1;
    };
    let nr = u16::from_be_bytes([bds[14], bds[15]]);
    let nc = u16::from_be_bytes([bds[16], bds[17]]);
    (u64::from(nr) * u64::from(nc)).max(1)
}

fuzz_target!(|data: &[u8]| {
    // A malformed buffer must surface a structured error, never panic.
    if let Ok(reader) = Grib1Reader::from_bytes(data.to_vec()) {
        for i in 0..reader.message_count() {
            // Ignore the result: we only care that decoding cannot panic or
            // over-read. Errors on individual messages are expected and fine.
            let points = declared_points(&reader, i);
            let affordable = |n: u64| n <= FUZZ_MAX_FIELD_POINTS || n > MAX_FIELD_POINTS as u64;
            if affordable(points) {
                let _ = reader.decode_message_values(i);
            }
            // The spherical-harmonic decode path, which `decode_message_values`
            // refuses outright. `MAX_TRUNCATION` bounds it, so a declared
            // truncation can no longer turn a short input into an allocation
            // the fuzzer reports as an OOM instead of as a finding — but the
            // bound is 537 MB, which is inside libFuzzer's default RSS limit
            // and still half a second of writes per exec. An input that reaches
            // it would be kept in the corpus and pay that cost forever, so a
            // large declared truncation is skipped here. Nothing in the bit
            // unpacking needs one: the traversal, the IBM float decode and the
            // sub-truncation weave are all exercised at small `J`.
            if !declares_a_large_truncation(&reader, i) {
                let _ = reader.decode_spectral_message(i);
                // The transforms behind the same bound (#637): a caller's grid
                // band-limited to what it resolves, then in full, and the full
                // sum at one point, `(J+1)(J+2)/2` terms.
                let _ = reader.synthesize_spectral_message(i, &PROBE_LATS, &PROBE_LONS);
                let _ = reader.synthesize_spectral_message_full(i, &PROBE_LATS, &PROBE_LONS);
                let _ = reader.evaluate_spectral_point(i, 60.0, 120.0);
            }
            // The true matrix-of-values path, which `decode_message_values`
            // refuses. `MAX_FIELD_POINTS` bounds its output at 64 Mi cells.
            if affordable(points.saturating_mul(matrix_cells(&reader, data, i))) {
                let _ = reader.decode_matrix_message(i);
            }
            // Total by construction, so the assertion is that it stays total.
            let _ = reader.synthesis_grid(i);
            let _ = reader.synthesis_truncation(i);
        }
    }
});
