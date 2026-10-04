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
//!   by the truncation the *file* declares. They, and the coefficient decode
//!   in front of them, run only below `FUZZ_MAX_TRUNCATION`, as in the GRIB1
//!   target.

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

/// The value count above which a message's scalar, matrix and bi-Fourier
/// decodes (and the HEALPix resample, which decodes first) are not run, unless
/// the reader refuses them outright.
///
/// Both are bounded by `MAX_FIELD_POINTS` (64 Mi points), sized for a real
/// grid rather than for a fuzz run. A constant field (zero bits per value)
/// needs no §7 data, so a 196-byte message declaring an 8192 × 8192 grid
/// decodes to a gigabyte of `Option<f64>`, about a second of writes; a dozen
/// such messages in one input exceed the run's ten-second timeout. That is the
/// reader working as designed. The bit unpacking, the bitmap and the matrix
/// reshape are all reached at small sizes, so nothing is lost below this, and a
/// declared size past the cap is still decoded so the refusal stays fuzzed.
const FUZZ_MAX_FIELD_POINTS: u64 = 1 << 22;

/// What message `i` declares about its size: the grid (§3's own count or the
/// raster its template states, whichever is larger) and §5's value count.
///
/// They are kept apart because the reader treats them differently. It refuses
/// a grid past `MAX_FIELD_POINTS`, or a §3 count the geometry disagrees with,
/// before it allocates anything; §5's count it checks only against what it
/// has already sized from the grid. So a decode is skipped when any count is
/// past the budget, and run anyway only when the *grid* is past the cap.
fn declared(reader: &Grib2Reader, i: usize) -> (u64, u64) {
    let Some(m) = reader.messages.get(i) else {
        return (0, 0);
    };
    let raster = m
        .gds
        .dimensions()
        .map_or(0, |(ni, nj)| u64::from(ni) * u64::from(nj));
    (
        u64::from(m.gds.num_data_points).max(raster),
        u64::from(m.drs.num_data_points),
    )
}

/// Whether a decode should run: everything it is sized by is within the
/// budget, or the grid is past the reader's cap and so refused first.
fn affordable(grid: u64, values: u64, cells: u64) -> bool {
    grid.max(values).saturating_mul(cells) <= FUZZ_MAX_FIELD_POINTS
        || grid.saturating_mul(cells) > fieldglass_core::MAX_FIELD_POINTS as u64
}

/// `NR·NC`, the cells per point a template 5.1 matrix holds; one otherwise.
fn matrix_cells(reader: &Grib2Reader, i: usize) -> u64 {
    reader
        .messages
        .get(i)
        .and_then(|m| m.drs.matrix_simple())
        .map_or(1, |t| u64::from(t.nr) * u64::from(t.nc))
}

/// Whether message `i`'s bi-Fourier decode should run: its truncation layout
/// is within the budget or past the reader's cap, or it has no bi-Fourier grid
/// at all, which the reader refuses at once.
fn bifourier_affordable(reader: &Grib2Reader, i: usize) -> bool {
    let Some(t) = reader.messages.get(i).and_then(|m| m.gds.bifourier()) else {
        return true;
    };
    let layout = (u64::from(t.bif_i) + 1)
        .saturating_mul(u64::from(t.bif_j) + 1)
        .saturating_mul(4);
    layout <= FUZZ_MAX_FIELD_POINTS || layout > fieldglass_core::sht::MAX_COEFFICIENTS as u64
}

/// The samples above which a JPEG 2000 codestream (§5.40) is not decoded.
///
/// The codestream states its own image in its SIZ marker, and the reader
/// decodes all of it before comparing the sample count with the field's, so
/// the grid and §5 counts bound nothing here: a 300-byte message on a 1 × 1
/// grid can carry an 8192 × 8192 codestream, about 14 s of wavelet transform.
/// A single-tile 1024 × 1024 image is about 190 ms, so eight messages stay
/// near 1.5 s; 2048 × 2048 is 0.8 s each, which is why this is below
/// `FUZZ_MAX_FIELD_POINTS`.
const FUZZ_MAX_J2K_SAMPLES: u64 = 1 << 20;

/// `rust_j2k`'s own guard on the image area, which its SIZ validation refuses
/// before it allocates (`MAX_IMAGE_SAMPLES` there, not exported).
const J2K_MAX_IMAGE_SAMPLES: u64 = 1 << 26;

/// Whether message `i`'s §7, if it is a JPEG 2000 codestream, can be decoded:
/// its image area times its component count is within the budget, or the area
/// alone is past the decoder's guard. A §7 that does not open with SOC and SIZ
/// is not decoded as an image, so it is not charged.
///
/// The component count multiplies the image area rather than summing the
/// subsampled component areas the decoder budgets, so this can only
/// over-count.
fn j2k_affordable(reader: &Grib2Reader, data: &[u8], i: usize) -> bool {
    let Some(range) = reader.messages.get(i).map(|m| m.ds_range) else {
        return true;
    };
    // §7 is a 5-byte header, then the codestream: SOC (FF4F), SIZ (FF51), Lsiz,
    // Rsiz, Xsiz, Ysiz, XOsiz, YOsiz, XTsiz, YTsiz, XTOsiz, YTOsiz, Csiz.
    let Some(cs) = usize::try_from(range.start)
        .ok()
        .and_then(|start| data.get(start.checked_add(5)?..start.checked_add(5 + 42)?))
    else {
        return true;
    };
    if cs[0..4] != [0xFF, 0x4F, 0xFF, 0x51] {
        return true;
    }
    let word = |at: usize| {
        u64::from(u32::from_be_bytes([
            cs[at],
            cs[at + 1],
            cs[at + 2],
            cs[at + 3],
        ]))
    };
    let width = word(8).saturating_sub(word(16));
    let height = word(12).saturating_sub(word(20));
    let components = u64::from(u16::from_be_bytes([cs[40], cs[41]]));
    let area = width.saturating_mul(height);
    area.saturating_mul(components.max(1)) <= FUZZ_MAX_J2K_SAMPLES || area > J2K_MAX_IMAGE_SAMPLES
}

/// Messages decoded per input. The gates above bound one message; libFuzzer's
/// `-timeout` bounds one input, and a 10 KB input holds dozens of messages at
/// the budget. Measured on the costliest path the gates admit, a constant
/// bi-Fourier field at 1023 × 1023 (4,194,304 coefficients), one message takes
/// about 280 ms and eight about 2.2 s; HEALPix at Nside 591 (4,191,372 points,
/// decoded and then resampled) is about 150 ms. The committed seeds are all
/// single messages.
const MAX_DECODED_MESSAGES: usize = 8;

fuzz_target!(|data: &[u8]| {
    // A malformed buffer must surface a structured error, never panic.
    if let Ok(reader) = Grib2Reader::from_bytes(data.to_vec()) {
        for i in 0..reader.message_count().min(MAX_DECODED_MESSAGES) {
            // Ignore the results: we only care that decoding cannot panic or
            // over-read. Errors on individual messages are expected and fine —
            // most inputs are the wrong packing for most of these entry points,
            // and a clean rejection is the correct outcome there.
            let (grid, values) = declared(&reader, i);
            let field = affordable(grid, values, 1) && j2k_affordable(&reader, data, i);
            if field {
                let _ = reader.decode_message_values(i);
            }
            if affordable(grid, values, matrix_cells(&reader, i)) {
                let _ = reader.decode_matrix_message(i);
            }
            // Sized by the §3 truncation: the reader builds and walks its
            // `4·(N+1)·(M+1)`-coefficient layout before it compares that with
            // §5's count, so §5 bounds nothing here. A layout past
            // `MAX_COEFFICIENTS` is refused before anything is built.
            if bifourier_affordable(&reader, i) {
                let _ = reader.decode_bifourier_message(i);
            }
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
            //
            // Its HEALPix arm decodes the message's values first, through
            // `decode_message_values`, so it is gated as that call is.
            if field
                && reader
                    .messages
                    .get(i)
                    .is_some_and(|m| m.gds.spherical_harmonic().is_none())
            {
                let _ = reader.synthesize_message_global(i);
            }
            // The coefficient decode is sized by the declared truncation, not
            // by §5, and a constant field needs no §7 data, so it is skipped
            // past `FUZZ_MAX_TRUNCATION` as the GRIB1 target skips it: at the
            // cap it is 537 MB. Below it, the synthesis runs only when the
            // coefficients decoded, so a failure there is a transform bug
            // rather than a re-run of the decode error.
            if !declares_a_large_truncation(&reader, i) && reader.decode_spectral_message(i).is_ok()
            {
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
