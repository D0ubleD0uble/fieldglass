//! `Params::new` against libaec 1.1.7's own `aec_decode_init` verdicts.
//!
//! Every row of the manifest's `params_grid` is a parameter set the generator
//! handed to `aec_decode_init`, with the status it returned. The grid varies
//! one parameter at a time from a valid base, so each rejection it records is
//! for the one parameter that differs. `Params::new` must accept exactly the
//! rows libaec accepted.

mod common;

use common::{AEC_CASES, GRID_ROWS, int, manifest, narrow_u8, narrow_u16, rows, text, uint};
use fieldglass_aec::{Flags, Params};

const AEC_OK: i64 = 0;
const AEC_CONF_ERROR: i64 = -1;

fn params_for(row: &serde_json::Value) -> Result<Params, fieldglass_aec::AecError> {
    Params::new(
        narrow_u8(row, "bits_per_sample"),
        narrow_u16(row, "block_size"),
        narrow_u16(row, "rsi"),
        Flags::from_bits_truncate(narrow_u8(row, "flags")),
    )
}

#[test]
fn params_accepts_exactly_what_libaec_accepts() {
    let manifest = manifest();
    let grid = rows(&manifest, "params_grid", GRID_ROWS);
    let mut accepted = 0;
    let mut mismatches = Vec::new();
    for row in grid {
        let status = int(row, "status");
        assert!(
            status == AEC_OK || status == AEC_CONF_ERROR,
            "aec_decode_init can only accept or refuse a parameter set: {row}"
        );
        let libaec_accepts = status == AEC_OK;
        if libaec_accepts {
            accepted += 1;
        }
        if params_for(row).is_ok() != libaec_accepts {
            mismatches.push(row.to_string());
        }
    }
    assert!(
        mismatches.is_empty(),
        "{} of {GRID_ROWS} rows disagree with libaec, e.g.\n{}",
        mismatches.len(),
        mismatches
            .iter()
            .take(10)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
    // Both verdicts must be well represented, or the comparison is one-sided.
    assert!(
        accepted > 100 && GRID_ROWS - accepted > 100,
        "{accepted} of {GRID_ROWS} accepted"
    );
}

/// The boundaries the grid exists to pin, looked up by value so a regenerated
/// grid that lost one fails here rather than passing on what remains.
#[test]
fn the_grid_pins_every_boundary() {
    let manifest = manifest();
    let grid = rows(&manifest, "params_grid", GRID_ROWS);
    let status_of = |bps: u64, block: u64, rsi: u64, flags: u64| {
        grid.iter()
            .find(|r| {
                uint(r, "bits_per_sample") == bps
                    && uint(r, "block_size") == block
                    && uint(r, "rsi") == rsi
                    && uint(r, "flags") == flags
            })
            .map(|r| int(r, "status"))
            .unwrap_or_else(|| panic!("the grid has no row for {bps}/{block}/{rsi}/{flags}"))
    };
    let (ok, refused) = (AEC_OK, AEC_CONF_ERROR);
    let restricted = 16;
    let not_enforce = 64;
    for (bps, block, rsi, flags, want) in [
        (0, 16, 128, 0, refused),
        (1, 16, 128, 0, ok),
        (32, 16, 128, 0, ok),
        (33, 16, 128, 0, refused),
        (8, 0, 128, 0, refused),
        (8, 1, 128, 0, refused),
        (8, 2, 128, 0, ok),
        (8, 7, 128, 0, refused),
        (8, 10, 128, 0, ok),
        (8, 256, 128, 0, ok),
        (8, 258, 128, 0, refused),
        (8, 16, 0, 0, refused),
        (8, 16, 1, 0, ok),
        (8, 16, 4096, 0, ok),
        (8, 16, 4097, 0, refused),
        (4, 16, 128, restricted, ok),
        (5, 16, 128, restricted, refused),
        (8, 16, 128, restricted, refused),
        (9, 16, 128, restricted, ok),
        (32, 16, 128, not_enforce, ok),
    ] {
        assert_eq!(
            status_of(bps, block, rsi, flags),
            want,
            "{bps}/{block}/{rsi}/{flags}"
        );
    }
}

/// `bytes_per_sample` against libaec's output length, on every case libaec
/// decoded completely: it wrote `samples × width` bytes.
#[test]
fn bytes_per_sample_matches_libaecs_output_length() {
    let manifest = manifest();
    let cases = rows(&manifest, "aec_cases", AEC_CASES);
    let mut checked = 0;
    for case in cases {
        if matches!(text(case, "kind"), "truncated" | "libaec_rejects") {
            continue;
        }
        let params = params_for(case).unwrap_or_else(|e| panic!("{e}: {case}"));
        let samples = usize::try_from(uint(case, "samples")).unwrap();
        let total_out = usize::try_from(uint(case, "total_out")).unwrap();
        assert_eq!(
            samples * params.bytes_per_sample(),
            total_out,
            "{}",
            text(case, "name")
        );
        checked += 1;
    }
    assert!(checked + 10 > AEC_CASES, "only {checked} cases checked");
}
