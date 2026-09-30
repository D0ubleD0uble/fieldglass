//! The libaec conformance manifest, shared by the integration tests.
//!
//! `tests/fixtures/manifest.json` is written by `tools/build_aec_fixtures.py`
//! from a pinned libaec 1.1.7 build, never by hand. Every loader here panics
//! rather than returning an empty list, and the counts are pinned below rather
//! than read from the manifest's own header, so a missing, emptied or
//! truncated manifest fails the suite instead of passing it vacuously.
//!
//! Paths are relative to the package root, not built from
//! `CARGO_MANIFEST_DIR`: the suite also runs under `wasmtime --dir=.`, where
//! only the working directory cargo sets is visible.

// Each test binary uses a different part of this module.
#![allow(dead_code)]

use serde_json::Value;

/// Rows in `params_grid`. A regeneration that changes the grid changes this
/// number in the same commit.
pub(crate) const GRID_ROWS: usize = 981;
/// Rows in `aec_cases`.
pub(crate) const AEC_CASES: usize = 540;
/// Rows in `sz_cases`.
pub(crate) const SZ_CASES: usize = 84;

/// The libaec commit the manifest must name.
pub(crate) const LIBAEC_COMMIT: &str = "0c4c01463d2c64a112a61271d317b74efb660608";

pub(crate) const FIXTURES: &str = "tests/fixtures";

pub(crate) fn manifest() -> Value {
    let path = format!("{FIXTURES}/manifest.json");
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{path}: {e}"))
}

/// The array under `key`, asserted to hold exactly `expected` rows.
pub(crate) fn rows<'a>(manifest: &'a Value, key: &str, expected: usize) -> &'a [Value] {
    let rows = manifest[key]
        .as_array()
        .unwrap_or_else(|| panic!("manifest has no `{key}` array"));
    assert_eq!(
        rows.len(),
        expected,
        "`{key}` has {} rows, the suite pins {expected}: regenerate with \
         tools/build_aec_fixtures.py and update the pinned count in the same commit",
        rows.len()
    );
    rows
}

pub(crate) fn uint(row: &Value, key: &str) -> u64 {
    row[key]
        .as_u64()
        .unwrap_or_else(|| panic!("`{key}` is not an unsigned integer in {row}"))
}

pub(crate) fn int(row: &Value, key: &str) -> i64 {
    row[key]
        .as_i64()
        .unwrap_or_else(|| panic!("`{key}` is not an integer in {row}"))
}

pub(crate) fn text<'a>(row: &'a Value, key: &str) -> &'a str {
    row[key]
        .as_str()
        .unwrap_or_else(|| panic!("`{key}` is not a string in {row}"))
}

/// `u8` from a manifest integer that the generator keeps in range.
pub(crate) fn narrow_u8(row: &Value, key: &str) -> u8 {
    u8::try_from(uint(row, key)).unwrap_or_else(|_| panic!("`{key}` does not fit u8 in {row}"))
}

/// `u16` from a manifest integer that the generator keeps in range.
pub(crate) fn narrow_u16(row: &Value, key: &str) -> u16 {
    u16::try_from(uint(row, key)).unwrap_or_else(|_| panic!("`{key}` does not fit u16 in {row}"))
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Writes a stream most significant bit first.
#[derive(Default)]
pub(crate) struct Bits {
    bytes: Vec<u8>,
    used: u64,
}

impl Bits {
    pub(crate) fn put(&mut self, value: u64, n: u32) -> &mut Self {
        for i in (0..n).rev() {
            if self.used.is_multiple_of(8) {
                self.bytes.push(0);
            }
            let bit = u8::from((value >> i) & 1 == 1);
            *self.bytes.last_mut().unwrap() |= bit << (7 - self.used % 8);
            self.used += 1;
        }
        self
    }

    /// A fundamental sequence: `count` zeros, then a one.
    pub(crate) fn fs(&mut self, count: u64) -> &mut Self {
        for _ in 0..count {
            self.put(0, 1);
        }
        self.put(1, 1)
    }

    pub(crate) fn done(&mut self) -> Vec<u8> {
        self.used = 0;
        std::mem::take(&mut self.bytes)
    }
}
