//! libFuzzer target for the GRIB2 decode path.
//!
//! `fieldglass-grib2` parses attacker-controllable bytes
//! (IS/IDS/LUS/GDS/PDS/DRS/BMS/DS), the highest-severity bug class for a binary
//! parser. This target drives the full scan-plus-decode pipeline —
//! `Grib2Reader::from_bytes` followed by every public decode entry point for
//! each message it finds — asserting the parser never panics, over-reads, or
//! hangs on arbitrary input. The §5 DRS templates each carry their own
//! length/offset-driven bit unpacking, which is the main thing this exercises.
//!
//! `decode_message_values` covers only the one-scalar-per-grid-point packings.
//! The forms whose output is not a scalar field have their own entry points and
//! their own bit unpacking, so driving only the scalar path would leave them
//! unfuzzed:
//!
//! * `decode_matrix_message` — template 5.1 with `matrixBitmapsPresent = 1`,
//!   the true `NR×NC` per-point matrix delimited by secondary bitmaps. Stock
//!   eccodes divides by zero and crashes on this variant, so there is no
//!   second implementation to compare against; it is exactly the code that
//!   most needs a fuzzer.
//! * `decode_spectral_message` — §5.50 / 5.51 spherical-harmonic coefficients,
//!   where 5.51 reads a sub-truncation of raw IEEE floats followed by a
//!   Laplacian-rescaled simple-packed remainder.
//! * `decode_bifourier_message` — §5.53, four coefficients per wavenumber pair
//!   over a rectangle / ellipse / diamond truncation.
//! * `synthesize_spectral_message`, `synthesize_spectral_message_full` and
//!   `evaluate_spectral_point` — the inverse spherical-harmonic transform on a
//!   grid (band-limited, then in full) and at one point, whose cost is driven
//!   by the truncation the *file* declares. They run only below
//!   `FUZZ_MAX_TRUNCATION`, as in the GRIB1 target.

#![no_main]

use libfuzzer_sys::fuzz_target;

use fieldglass_grib2::Grib2Reader;

/// Latitudes/longitudes for the synthesis probe. Deliberately tiny: the
/// transform's cost is `O(points × coefficients)` and the coefficient count
/// comes from the file, so a large grid here would turn a legitimate
/// high-truncation input into a fuzzer timeout rather than a finding.
const PROBE_LATS: [f64; 3] = [-60.0, 0.0, 60.0];
const PROBE_LONS: [f64; 3] = [0.0, 120.0, 240.0];

/// Whether message `i` declares a spherical-harmonic truncation past a bound
/// chosen for fuzzer throughput rather than for correctness — the GRIB1
/// target's rule, for the same reason.
///
/// The transforms' arithmetic is the same at `J = 512` as at `MAX_TRUNCATION`,
/// but the full sum at one point is `(J+1)(J+2)/2` terms: 131,000 here, 34
/// million at the cap. An input that reached the cap would be kept in the
/// corpus and pay a fifth of a second per point per exec forever.
fn declares_a_large_truncation(reader: &Grib2Reader, i: usize) -> bool {
    const FUZZ_MAX_TRUNCATION: u32 = 512;
    reader
        .messages
        .get(i)
        .and_then(|m| m.gds.spherical_harmonic())
        .is_some_and(|sh| sh.j > FUZZ_MAX_TRUNCATION)
}

fuzz_target!(|data: &[u8]| {
    // A malformed buffer must surface a structured error, never panic.
    if let Ok(reader) = Grib2Reader::from_bytes(data.to_vec()) {
        for i in 0..reader.message_count() {
            // Ignore the results: we only care that decoding cannot panic or
            // over-read. Errors on individual messages are expected and fine —
            // most inputs are the wrong packing for most of these entry points,
            // and a clean rejection is the correct outcome there.
            let _ = reader.decode_message_values(i);
            let _ = reader.decode_matrix_message(i);
            let _ = reader.decode_bifourier_message(i);
            // The resolve seam's cheap half (#580): reads §3 and decodes
            // nothing, so it is total by construction and the assertion is that
            // it stays total on a template whose fields are arbitrary.
            let _ = reader.synthesis_grid(i);
            let _ = reader.synthesis_truncation(i);
            // Its expensive half, on the one family whose cost is bounded by
            // the grid rather than by the file. A HEALPix resample is capped at
            // 720x361 by `healpix_render_dims` and its pixel count by
            // `MAX_GRID_POINTS`; the spherical-harmonic arm is excluded for the
            // reason `PROBE_LATS` exists. Since #637 its map is band-limited to
            // T359 whatever the file declares, so its cost is bounded too, but
            // that bound is 1.4e8 terms over the full 720x361 grid: a tenth of
            // a second per exec, which the fuzzer would pay on every input.
            if reader
                .messages
                .get(i)
                .is_some_and(|m| m.gds.spherical_harmonic().is_none())
            {
                let _ = reader.synthesize_message_global(i);
            }
            // Only attempt the synthesis when the coefficients themselves
            // decoded, so a failure here is a transform bug rather than a
            // re-run of the decode error above.
            let decoded = reader.decode_spectral_message(i).is_ok();
            if decoded && !declares_a_large_truncation(&reader, i) {
                // Band-limited to what the probe grid resolves (#637), then in
                // full, which is the range-safe kernel at every declared T.
                let _ = reader.synthesize_spectral_message(i, &PROBE_LATS, &PROBE_LONS);
                let _ = reader.synthesize_spectral_message_full(i, &PROBE_LATS, &PROBE_LONS);
                // The probe's full-detail evaluation: the full sum at one point.
                let _ = reader.evaluate_spectral_point(i, 60.0, 120.0);
            }
        }
    }
});
