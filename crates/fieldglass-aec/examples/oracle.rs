//! Compare this crate's decoder with libaec's recorded output, case by case.
//!
//! ```text
//! AEC_ORACLE_MODE=corpus AEC_ORACLE_INPUT=DIR [AEC_ORACLE_EXPECT_AEC=N] [AEC_ORACLE_EXPECT_SZ=N] \
//!     cargo run --release -p fieldglass-aec --example oracle
//! AEC_ORACLE_MODE=sampledata AEC_ORACLE_INPUT=LIST AEC_ORACLE_EXPECT=N \
//!     cargo run --release -p fieldglass-aec --example oracle
//! ```
//!
//! The CI oracle job (`tools/aec_oracle.py`, #763) runs this. It is not a
//! test: the data it reads is generated in CI from a pinned libaec build and is
//! too large to commit, or is not ours to commit.
//!
//! Configured through the environment rather than arguments: reading
//! `std::env::args()` trips semgrep's `rust.lang.security.args.args`, and this
//! repo keeps zero suppressions (the same reason `bench_decode` and the perf
//! harness's `report` take none).
//!
//! The `corpus` mode reads `DIR/manifest.json`, as `tools/build_aec_fixtures.py`
//! writes it (the committed corpus, or the `--full` matrix), decodes every
//! `aec_cases` and `sz_cases` row, and checks the result against what libaec
//! recorded. Where the manifest's `kind` names a recorded divergence from
//! libaec (ADR-0012 decision 4), the libaec verdict must have exactly the
//! shape that divergence describes, and the decode must give the standard's
//! answer instead; see [`expectation`]. Every other outcome fails.
//!
//! The `sampledata` mode decodes the CCSDS 121.0-B-2 sample streams from libaec's
//! tarball. Each line of the list is `stream<TAB>expected<TAB>graec options`, as
//! libaec's `tests/sampledata.sh` passes them to its `graec` tool, and the
//! decode must equal the expected file byte for byte. There is no divergence
//! allowance here: the sample data is the standard's own.
//!
//! Exits non-zero, listing every failure, if anything differs.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use fieldglass_aec::sz::{self, SzParams};
use fieldglass_aec::{AecError, Flags, Params, decode_to_bytes};
use serde_json::Value;

/// `AEC_DATA_ERROR` in `libaec.h`.
const AEC_DATA_ERROR: i64 = -3;

fn main() -> ExitCode {
    let result = match env("AEC_ORACLE_MODE").as_deref() {
        Some("corpus") => corpus(),
        Some("sampledata") => sampledata(),
        _ => Err(vec![
            "set AEC_ORACLE_MODE to `corpus` or `sampledata`, and AEC_ORACLE_INPUT; \
             see the example's docs"
                .to_owned(),
        ]),
    };
    match result {
        Ok(summary) => {
            println!("{summary}");
            ExitCode::SUCCESS
        }
        Err(failures) => {
            for failure in &failures {
                eprintln!("FAIL {failure}");
            }
            eprintln!("{} failure(s)", failures.len());
            ExitCode::FAILURE
        }
    }
}

type Outcome = Result<String, Vec<String>>;

/// The environment variable `name`, if set and not empty.
fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// `AEC_ORACLE_INPUT`, which every mode needs.
fn input() -> Result<String, Vec<String>> {
    env("AEC_ORACLE_INPUT").ok_or_else(|| vec!["AEC_ORACLE_INPUT is not set".to_owned()])
}

/// The number in environment variable `name`, if set.
fn count(name: &str) -> Result<Option<usize>, Vec<String>> {
    env(name)
        .map(|v| {
            v.parse()
                .map_err(|_| vec![format!("{name}={v} is not a number")])
        })
        .transpose()
}

fn one<T>(message: String) -> Result<T, Vec<String>> {
    Err(vec![message])
}

// --------------------------------------------------------------------------
// corpus
// --------------------------------------------------------------------------

/// What a manifest row says this crate must return.
#[derive(Debug, PartialEq, Eq)]
enum Expect {
    /// `Ok`, with bytes whose SHA-256 is `output_sha256`: libaec's output.
    Libaec,
    /// `Ok`, with bytes whose SHA-256 is `source_sha256`: the field the
    /// encoder was given, where libaec's decoder refuses a valid stream.
    Source,
    /// `AecError::Truncated`, where libaec returns success with short output.
    Truncated,
}

/// The one place recorded divergences are allowed.
///
/// Each arm is a row of ADR-0012 decision 4's table, and admits only libaec
/// verdicts of the shape that row describes. The generator assigns these
/// kinds, and assigns `libaec_rejects` only after a libaec build with a wider
/// second-extension table decodes the stream to the encoder's input. A kind
/// not listed, or a verdict of the wrong shape for its kind, is a failure: it
/// is either a new divergence nobody has recorded or a corrupt manifest.
fn expectation(kind: &str, status: i64, total_out: u64, full: u64) -> Result<Expect, String> {
    match kind {
        // No divergence: libaec decoded the whole field.
        "option" | "field" | "trailing" if status == 0 && total_out == full => Ok(Expect::Libaec),
        // "Trailing fill after the last sample": libaec produces every sample
        // and then returns AEC_DATA_ERROR on fill it reads as a zero block.
        // This crate stops at the last sample: the same bytes, and `Ok`.
        "trailing_fill" if status == AEC_DATA_ERROR && total_out == full => Ok(Expect::Libaec),
        // "A second-extension pair sum above 12": libaec's decoder stops
        // part-way with AEC_DATA_ERROR. The standard reads the stream, and the
        // answer is the encoder's input.
        "libaec_rejects" if status == AEC_DATA_ERROR && total_out < full => Ok(Expect::Source),
        // "Truncated input": libaec returns AEC_OK with short output. This
        // crate returns an error.
        "truncated" if status == 0 && total_out < full => Ok(Expect::Truncated),
        _ => Err(format!(
            "kind `{kind}` with libaec status {status} and {total_out} of {full} bytes \
             is not a recorded divergence"
        )),
    }
}

struct Manifest {
    dir: PathBuf,
    value: Value,
}

impl Manifest {
    fn load(dir: &Path) -> Result<Self, Vec<String>> {
        let path = dir.join("manifest.json");
        let text =
            std::fs::read_to_string(&path).map_err(|e| vec![format!("{}: {e}", path.display())])?;
        let value =
            serde_json::from_str(&text).map_err(|e| vec![format!("{}: {e}", path.display())])?;
        Ok(Self {
            dir: dir.to_owned(),
            value,
        })
    }

    /// The rows under `key`, which must number what the header says.
    fn rows(&self, key: &str) -> Result<&[Value], Vec<String>> {
        let rows = self.value[key]
            .as_array()
            .ok_or_else(|| vec![format!("manifest has no `{key}` array")])?;
        let declared = self.value["header"]["counts"][key].as_u64();
        if declared != Some(rows.len() as u64) {
            return one(format!(
                "`{key}` has {} rows, the header declares {declared:?}",
                rows.len()
            ));
        }
        Ok(rows)
    }

    /// The stream a row names, which must be the bytes the manifest records.
    fn stream(&self, row: &Value) -> Result<Vec<u8>, String> {
        let path = self.dir.join(text(row, "stream")?);
        let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        if sha256_hex(&bytes) != text(row, "stream_sha256")? {
            return Err(format!("{} is not the stream libaec wrote", path.display()));
        }
        Ok(bytes)
    }
}

fn text<'a>(row: &'a Value, key: &str) -> Result<&'a str, String> {
    row[key]
        .as_str()
        .ok_or_else(|| format!("`{key}` is not a string in {row}"))
}

fn uint(row: &Value, key: &str) -> Result<u64, String> {
    row[key]
        .as_u64()
        .ok_or_else(|| format!("`{key}` is not an unsigned integer in {row}"))
}

fn int(row: &Value, key: &str) -> Result<i64, String> {
    row[key]
        .as_i64()
        .ok_or_else(|| format!("`{key}` is not an integer in {row}"))
}

fn narrow<T: TryFrom<u64>>(row: &Value, key: &str) -> Result<T, String> {
    T::try_from(uint(row, key)?).map_err(|_| format!("`{key}` is out of range in {row}"))
}

fn corpus() -> Outcome {
    let dir = input()?;
    let manifest = Manifest::load(Path::new(&dir))?;
    let aec = manifest.rows("aec_cases")?;
    let sz = manifest.rows("sz_cases")?;
    for (key, rows, want) in [
        ("aec_cases", aec.len(), count("AEC_ORACLE_EXPECT_AEC")?),
        ("sz_cases", sz.len(), count("AEC_ORACLE_EXPECT_SZ")?),
    ] {
        if let Some(want) = want
            && rows != want
        {
            return one(format!("`{key}` has {rows} rows, expected {want}"));
        }
    }
    if aec.is_empty() {
        return one("the manifest has no aec cases".to_owned());
    }

    let mut failures = Vec::new();
    let mut tally = std::collections::BTreeMap::<String, usize>::new();
    for row in aec {
        let name = text(row, "name").unwrap_or("?");
        match aec_case(&manifest, row) {
            Ok(kind) => *tally.entry(kind).or_default() += 1,
            Err(e) => failures.push(format!("{name}: {e}")),
        }
    }
    for row in sz {
        let name = text(row, "name").unwrap_or("?");
        match sz_case(&manifest, row) {
            Ok(()) => *tally.entry("sz".to_owned()).or_default() += 1,
            Err(e) => failures.push(format!("{name}: {e}")),
        }
    }
    if !failures.is_empty() {
        return Err(failures);
    }
    let mut summary = format!(
        "{dir}: {} aec and {} sz cases agree with the oracle (",
        aec.len(),
        sz.len()
    );
    for (i, (kind, n)) in tally.iter().enumerate() {
        let sep = if i == 0 { "" } else { ", " };
        let _ = write!(summary, "{sep}{kind} {n}");
    }
    summary.push(')');
    Ok(summary)
}

/// Decode one `aec_cases` row; return its kind on agreement.
fn aec_case(manifest: &Manifest, row: &Value) -> Result<String, String> {
    let kind = text(row, "kind")?;
    let params = Params::new(
        narrow(row, "bits_per_sample")?,
        narrow(row, "block_size")?,
        narrow(row, "rsi")?,
        Flags::from_bits_truncate(narrow(row, "flags")?),
    )
    .map_err(|e| format!("libaec decoded it, but {e}"))?;
    let samples: usize = narrow(row, "samples")?;
    let full = samples * params.bytes_per_sample();
    let expect = expectation(
        kind,
        int(row, "libaec_status")?,
        uint(row, "total_out")?,
        full as u64,
    )?;
    let stream = manifest.stream(row)?;
    let mut out = vec![0u8; full];
    let result = decode_to_bytes(&stream, &params, &mut out);
    let digest = |key| -> Result<(), String> {
        let want = text(row, key)?;
        if sha256_hex(&out) == want {
            Ok(())
        } else {
            Err(format!("{kind}: decoded bytes differ from `{key}`"))
        }
    };
    match (expect, result) {
        (Expect::Libaec, Ok(())) => digest("output_sha256")?,
        (Expect::Source, Ok(())) => digest("source_sha256")?,
        (Expect::Truncated, Err(AecError::Truncated { decoded, requested }))
            if decoded < requested && requested == samples => {}
        (expect, result) => {
            return Err(format!("{kind}: expected {expect:?}, got {result:?}"));
        }
    }
    Ok(kind.to_owned())
}

/// Decode one `sz_cases` row. The corpus has no szip divergence cases:
/// libsz decoded each one completely, so its bytes are the oracle.
fn sz_case(manifest: &Manifest, row: &Value) -> Result<(), String> {
    let params = SzParams::new(
        narrow(row, "options_mask")?,
        narrow(row, "bits_per_pixel")?,
        narrow(row, "pixels_per_block")?,
        narrow(row, "pixels_per_scanline")?,
    )
    .map_err(|e| format!("libsz decoded it, but {e}"))?;
    let dest_len = uint(row, "dest_len")?;
    let (status, total_out) = (int(row, "libsz_status")?, uint(row, "total_out")?);
    if status != 0 || total_out != dest_len {
        return Err(format!(
            "libsz status {status} with {total_out} of {dest_len} bytes is not a recorded divergence"
        ));
    }
    let stream = manifest.stream(row)?;
    let mut out = vec![0u8; usize::try_from(dest_len).map_err(|e| e.to_string())?];
    sz::decompress(&stream, &params, &mut out).map_err(|e| e.to_string())?;
    if sha256_hex(&out) != text(row, "output_sha256")? {
        return Err("decoded bytes differ from libsz's".to_owned());
    }
    Ok(())
}

// --------------------------------------------------------------------------
// sampledata
// --------------------------------------------------------------------------

/// `graec`'s options, as `src/graec.c` in libaec 1.1.7 parses them: 8 bits,
/// block 8, RSI 2 and PREPROCESS by default; `-N` clears PREPROCESS, `-p` sets
/// PAD_RSI, `-t` RESTRICTED, `-m` MSB, `-s` SIGNED, `-3` 3BYTE, and `-n`,
/// `-j`, `-r` take a number. `-d` (decode) changes nothing here.
fn graec_params(options: &str) -> Result<Params, String> {
    let (mut bits, mut block, mut rsi) = (8u8, 8u16, 2u16);
    let mut flags = Flags::PREPROCESS;
    for opt in options.split_whitespace() {
        let number = |s: &str| {
            s.parse::<u16>()
                .map_err(|_| format!("graec option {opt}: not a number"))
        };
        match opt.strip_prefix('-') {
            Some("d") => {}
            Some("N") => flags.remove(Flags::PREPROCESS),
            Some("p") => flags.insert(Flags::PAD_RSI),
            Some("t") => flags.insert(Flags::RESTRICTED),
            Some("m") => flags.insert(Flags::MSB),
            Some("s") => flags.insert(Flags::SIGNED),
            Some("3") => flags.insert(Flags::THREE_BYTE),
            Some(o) if o.starts_with('n') => {
                bits = u8::try_from(number(&o[1..])?).map_err(|e| e.to_string())?;
            }
            Some(o) if o.starts_with('j') => block = number(&o[1..])?,
            Some(o) if o.starts_with('r') => rsi = number(&o[1..])?,
            _ => return Err(format!("graec option {opt} is not handled")),
        }
    }
    Params::new(bits, block, rsi, flags).map_err(|e| format!("{options}: {e}"))
}

fn sampledata() -> Outcome {
    let list = input()?;
    let Some(expect) = count("AEC_ORACLE_EXPECT")? else {
        return one("sampledata needs AEC_ORACLE_EXPECT".to_owned());
    };
    let text = std::fs::read_to_string(&list).map_err(|e| vec![format!("{list}: {e}")])?;
    let mut failures = Vec::new();
    let mut compared = 0;
    for line in text.lines().filter(|l| !l.trim().is_empty()) {
        match sample(line) {
            Ok(()) => compared += 1,
            Err(e) => failures.push(e),
        }
    }
    if compared + failures.len() != expect {
        failures.push(format!(
            "{list} names {} streams, expected {expect}",
            compared + failures.len()
        ));
    }
    if !failures.is_empty() {
        return Err(failures);
    }
    Ok(format!(
        "{compared} CCSDS 121.0-B-2 sample streams decode to their expected files"
    ))
}

fn sample(line: &str) -> Result<(), String> {
    let mut fields = line.split('\t');
    let (Some(stream), Some(expected), Some(options), None) =
        (fields.next(), fields.next(), fields.next(), fields.next())
    else {
        return Err(format!("not `stream<TAB>expected<TAB>options`: {line:?}"));
    };
    let params = graec_params(options).map_err(|e| format!("{stream}: {e}"))?;
    let input = std::fs::read(stream).map_err(|e| format!("{stream}: {e}"))?;
    let want = std::fs::read(expected).map_err(|e| format!("{expected}: {e}"))?;
    let width = params.bytes_per_sample();
    if want.is_empty() || want.len() % width != 0 {
        return Err(format!(
            "{expected}: {} bytes is not a whole number of {width}-byte samples",
            want.len()
        ));
    }
    let mut out = vec![0u8; want.len()];
    decode_to_bytes(&input, &params, &mut out).map_err(|e| format!("{stream} ({options}): {e}"))?;
    if out != want {
        let at = out.iter().zip(&want).position(|(a, b)| a != b).unwrap_or(0);
        return Err(format!(
            "{stream} ({options}): differs from {expected} first at byte {at}"
        ));
    }
    Ok(())
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .fold(String::new(), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}
