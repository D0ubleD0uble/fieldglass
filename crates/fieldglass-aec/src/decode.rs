//! The decoder: coded data sets (CDSes) in, samples out to a [`Sink`].
//!
//! One pass over the whole input slice, one block at a time into a stack
//! buffer. Nothing here allocates. The arithmetic that turns codes into
//! samples lives in small pure functions at the bottom of the file
//! ([`zero_run_blocks`], [`se_pair`], [`unmap_unsigned`], [`unmap_signed`]),
//! so each can be tested, and later proved, on its own.
//!
//! Reference: CCSDS 121.0-B-3, *Lossless Data Compression* (2020), sections 3
//! to 5. Checked against libaec 1.1.7 (`decode.c`), which is correct except
//! where [ADR-0012] lists a divergence.
//!
//! [ADR-0012]: https://github.com/D0ubleD0uble/fieldglass/blob/master/docs/decisions/0012-own-the-aec-decoder.md

use crate::bits::{BitReader, Eof};
use crate::sink::{ByteSink, Sink};
use crate::{AecError, Flags, Params};

/// The largest block size, and so the size of the stack block buffer.
const MAX_BLOCK: usize = 256;

/// "Remainder of segment": the zero-block count that means "to the end of
/// this 64-block segment or of the reference sample interval, whichever comes
/// first" (CCSDS 121.0-B-3 Table 3-2).
const ROS: u64 = 5;

/// Blocks per segment, for [`ROS`].
const SEGMENT_BLOCKS: u32 = 64;

/// The longest zero-block fundamental sequence Table 3-2 defines: 63 zeros
/// and a one, for 63 blocks.
const MAX_ZERO_RUN_FS: u64 = 63;

/// The largest second-extension codeword this decoder reads.
///
/// CCSDS 121.0-B-3 §3.4.2 puts no bound on a codeword `γ = (α+β)(α+β+1)/2 + β`
/// ("extending the mapping in table 3-1 in the obvious manner" when γ is 2^n
/// or more) beyond what the sample width allows, and libaec 1.1.7's own encoder writes
/// pair sums of 13 and 14 at small widths. libaec's decoder stops at a pair sum
/// of 12, which is codeword 90 (`SE_TABLE_SIZE`, `decode.h:53`), and refuses
/// the rest. This decoder reads them as the standard defines (maintainer
/// decision on #759, pinned by the corpus case
/// `se_pair_sum_over_12_b03_j256_r3_pp`). `Some(90)` would refuse them
/// as libaec does. Any value the codeword decodes to is still held to the
/// sample width, like every other code.
const SE_MAX_CODEWORD: Option<u64> = None;

/// Why a block could not be decoded; the caller adds where.
enum Stop {
    Eof,
    Invalid(&'static str),
}

impl From<Eof> for Stop {
    fn from(_: Eof) -> Self {
        Stop::Eof
    }
}

const TOO_WIDE: &str = "a value wider than the sample width";
const RUN_PAST_RSI: &str = "a zero-block run past the end of the reference sample interval";
const RUN_PAST_TABLE: &str = "a zero-block run longer than the 63 blocks of Table 3-2";
const SE_OUT_OF_RANGE: &str = "a second-extension codeword beyond the decoder's table";
const SE_REFERENCE_PAIR: &str =
    "a second-extension pair beside a reference sample that does not start with 0";

/// Decode `count` samples from `input` into `sink`.
///
/// Decoding stops as soon as `count` samples have gone to the sink: bytes
/// after them are never read, so trailing fill or garbage cannot cause an
/// error. `count = 0` reads nothing and succeeds.
///
/// # Errors
///
/// - [`AecError::Truncated`] when the input ends first. The sink has the
///   samples decoded before that, in whole blocks.
/// - [`AecError::InvalidCode`] for a code no valid encoder writes: a value of
///   2^n or more before postprocessing, or a zero-block run past the end of
///   its reference sample interval.
///
/// ```
/// use fieldglass_aec::{Flags, Params, Sink, decode};
///
/// struct Collect(Vec<u32>);
/// impl Sink for Collect {
///     fn samples(&mut self, block: &[u32]) {
///         self.0.extend_from_slice(block);
///     }
///     fn repeat(&mut self, value: u32, count: usize) {
///         self.0.extend(std::iter::repeat_n(value, count));
///     }
/// }
///
/// // 8-bit samples, block 8: one uncompressed block (id 0b111), then the
/// // eight samples 1 to 8 as they are.
/// let params = Params::new(8, 8, 1, Flags::empty())?;
/// let stream = [0xE0, 0x20, 0x40, 0x60, 0x80, 0xA0, 0xC0, 0xE1, 0x00];
/// let mut out = Collect(Vec::new());
/// decode(&stream[..], &params, 4, &mut out)?;
/// assert_eq!(out.0, [1, 2, 3, 4]);
/// # Ok::<(), fieldglass_aec::AecError>(())
/// ```
pub fn decode(
    input: &[u8],
    params: &Params,
    count: usize,
    sink: &mut dyn Sink,
) -> Result<(), AecError> {
    Kernel::new(input, params).run(count, sink).map(|_| ())
}

/// [`decode`], also returning how many bits of `input` it read. The szip
/// layer uses it to tell an output that stopped short of the stream from one
/// that reached its end.
pub(crate) fn decode_consumed(
    input: &[u8],
    params: &Params,
    count: usize,
    sink: &mut dyn Sink,
) -> Result<usize, AecError> {
    Kernel::new(input, params).run(count, sink)
}

/// Decode into `out` in libaec's output layout.
///
/// The sample count is `out.len()` divided by
/// [`Params::bytes_per_sample`]: 1, 2, 3 or 4 bytes per sample, most
/// significant byte first with [`Flags::MSB`], each sample as the [`Sink`]
/// docs describe it, truncated to its width.
///
/// # Errors
///
/// [`AecError::OutputLength`] when `out.len()` is not a whole number of
/// samples, and otherwise the errors of [`decode`]. On an error the contents
/// of `out` are unspecified.
///
/// ```
/// use fieldglass_aec::{Flags, Params, decode_to_bytes};
///
/// // 16-bit samples, block 2: one uncompressed block (id 0b1111), then 0x0102
/// // and 0x0304.
/// let params = Params::new(16, 2, 1, Flags::MSB)?;
/// let stream = [0b1111_0000, 0x10, 0x20, 0x30, 0x40];
/// let mut out = [0u8; 4];
/// decode_to_bytes(&stream[..], &params, &mut out)?;
/// assert_eq!(out, [1, 2, 3, 4]);
/// # Ok::<(), fieldglass_aec::AecError>(())
/// ```
pub fn decode_to_bytes(input: &[u8], params: &Params, out: &mut [u8]) -> Result<(), AecError> {
    let width = params.bytes_per_sample();
    if !out.len().is_multiple_of(width) {
        return Err(AecError::OutputLength {
            len: out.len(),
            bytes_per_sample: width,
        });
    }
    let count = out.len() / width;
    let msb = params.flags().contains(Flags::MSB);
    decode(input, params, count, &mut ByteSink::new(out, width, msb))
}

/// How mapped prediction errors turn back into samples.
#[derive(Debug, Clone, Copy)]
enum Mapping {
    /// No preprocessing: the codes are the samples.
    None,
    /// Unsigned samples, `0..=xmax`.
    Unsigned { xmax: u32 },
    /// Signed samples, `-xmax-1..=xmax`, carried sign-extended.
    Signed { xmax: u32, sign: u32 },
}

/// The decoder's fixed state for one call.
#[derive(Debug)]
struct Kernel<'a> {
    reader: BitReader<'a>,
    /// Bits per sample, 1 to 32.
    n: u32,
    /// The largest value a code may decode to, `2^n - 1`.
    max_code: u64,
    /// Width of the option identifier (CCSDS 121.0-B-3 Table 5-1).
    id_len: u32,
    /// The identifier of an uncompressed block: all ones.
    id_uncompressed: u32,
    /// Samples per block.
    block: usize,
    /// Blocks per reference sample interval.
    rsi: u32,
    pad_rsi: bool,
    mapping: Mapping,
}

impl<'a> Kernel<'a> {
    fn new(input: &'a [u8], params: &Params) -> Self {
        let n = u32::from(params.bits_per_sample());
        let flags = params.flags();
        let id_len = match n {
            1..=2 if flags.contains(Flags::RESTRICTED) => 1,
            3..=4 if flags.contains(Flags::RESTRICTED) => 2,
            1..=8 => 3,
            9..=16 => 4,
            _ => 5,
        };
        let max_code = u64::from(u32::MAX >> (32 - n));
        let mapping = if !flags.contains(Flags::PREPROCESS) {
            Mapping::None
        } else if flags.contains(Flags::SIGNED) {
            Mapping::Signed {
                // 2^(n-1) - 1, which is 0 at one bit.
                xmax: u32::MAX.checked_shr(33 - n).unwrap_or(0),
                sign: 1 << (n - 1),
            }
        } else {
            Mapping::Unsigned {
                xmax: u32::MAX >> (32 - n),
            }
        };
        Kernel {
            reader: BitReader::new(input),
            n,
            max_code,
            id_len,
            id_uncompressed: (1 << id_len) - 1,
            block: usize::from(params.block_size()),
            rsi: u32::from(params.rsi()),
            pad_rsi: flags.contains(Flags::PAD_RSI),
            mapping,
        }
    }

    /// Decode `count` samples into `sink`, and return how many bits of the
    /// input that read.
    fn run(mut self, count: usize, sink: &mut dyn Sink) -> Result<usize, AecError> {
        let mut buf = [0u32; MAX_BLOCK];
        let mut produced = 0usize;
        let mut blocks_in_rsi = 0u32;
        // The last reconstructed sample, which the next mapped error is
        // relative to. Reset by each reference sample.
        let mut last = 0u32;
        let preprocessed = !matches!(self.mapping, Mapping::None);

        while produced < count {
            if blocks_in_rsi == self.rsi {
                blocks_in_rsi = 0;
                if self.pad_rsi {
                    self.reader.align_to_byte();
                }
            }
            // With preprocessing, the first sample of each interval is a
            // reference sample, sent as it is.
            let has_ref = preprocessed && blocks_in_rsi == 0;
            let want = self.block.min(count - produced);
            let fail = |stop| match stop {
                Stop::Eof => AecError::Truncated {
                    decoded: produced,
                    requested: count,
                },
                Stop::Invalid(reason) => AecError::InvalidCode {
                    sample: produced,
                    reason,
                },
            };

            match self
                .cds(&mut buf, has_ref, want, blocks_in_rsi)
                .map_err(fail)?
            {
                Cds::Block => {
                    let block = buf.get_mut(..want).unwrap_or_default();
                    last = self.postprocess(block, has_ref, last);
                    sink.samples(block);
                    produced += want;
                    blocks_in_rsi += 1;
                }
                Cds::ZeroRun { blocks } => {
                    let mut run = blocks as usize * self.block;
                    if has_ref {
                        let reference = buf.get_mut(..1).unwrap_or_default();
                        last = self.postprocess(reference, true, last);
                        sink.samples(reference);
                        produced += 1;
                        run -= 1;
                    }
                    // A zero mapped error repeats the sample before it; with
                    // no preprocessing a zero code is the sample 0.
                    let value = if preprocessed { last } else { 0 };
                    let emit = run.min(count - produced);
                    if emit > 0 {
                        sink.repeat(value, emit);
                    }
                    produced += emit;
                    blocks_in_rsi += blocks;
                }
            }
        }
        Ok(self.reader.consumed_bits())
    }

    /// Read one CDS. A block's codes go into `buf[..want]`, with the reference
    /// sample (if any) at index 0. A zero-block run leaves only the reference
    /// sample in `buf[0]`.
    fn cds(
        &mut self,
        buf: &mut [u32; MAX_BLOCK],
        has_ref: bool,
        want: usize,
        blocks_in_rsi: u32,
    ) -> Result<Cds, Stop> {
        let id = self.reader.bits(self.id_len)?;
        if id == self.id_uncompressed {
            // Every sample as it is, the reference sample among them.
            for slot in buf.iter_mut().take(want) {
                *slot = self.reader.bits(self.n)?;
            }
            return Ok(Cds::Block);
        }
        if id == 0 {
            // Low entropy: one more bit picks zero block (0) or second
            // extension (1), and the reference sample follows it.
            let second_extension = self.reader.bits(1)? == 1;
            if has_ref {
                buf[0] = self.reader.bits(self.n)?;
            }
            if second_extension {
                self.second_extension(buf, has_ref, want)?;
                return Ok(Cds::Block);
            }
            let fs = self.reader.fs()?;
            let blocks = zero_run_blocks(fs, blocks_in_rsi, self.rsi).map_err(Stop::Invalid)?;
            return Ok(Cds::ZeroRun { blocks });
        }
        self.split(buf, id - 1, has_ref, want)?;
        Ok(Cds::Block)
    }

    /// A split-sample block with `k` low bits per sample: every sample's
    /// fundamental sequence first, then every sample's `k` bits
    /// (CCSDS 121.0-B-3 §5.2.3).
    fn split(
        &mut self,
        buf: &mut [u32; MAX_BLOCK],
        k: u32,
        has_ref: bool,
        want: usize,
    ) -> Result<(), Stop> {
        let start = usize::from(has_ref);
        if has_ref {
            buf[0] = self.reader.bits(self.n)?;
        }
        // The high part may use only the bits the sample width leaves.
        let high_max = self.max_code >> k;
        // Every high part is sent, even for samples past `want`, because the
        // low parts come after all of them.
        let block = buf.get_mut(start..self.block).unwrap_or_default();
        for slot in block.iter_mut() {
            let high = self.reader.fs()?;
            if high > high_max {
                return Err(Stop::Invalid(TOO_WIDE));
            }
            // `high << k <= max_code <= u32::MAX` by the check above, so the
            // cast keeps every bit.
            *slot = (high << k) as u32;
        }
        if k > 0 {
            let wanted = buf.get_mut(start..want.max(start)).unwrap_or_default();
            for slot in wanted.iter_mut() {
                *slot |= self.reader.bits(k)?;
            }
            // `k` can exceed `n` (libaec's identifiers run to k = 29 at 17
            // bits), and then the low part alone can be too wide.
            if k > self.n && wanted.iter().any(|&v| u64::from(v) > self.max_code) {
                return Err(Stop::Invalid(TOO_WIDE));
            }
        }
        Ok(())
    }

    /// A second-extension block: each codeword is a pair of samples
    /// (CCSDS 121.0-B-3 §3.4.1, §5.2.6).
    ///
    /// With a reference sample, the standard puts a 0 in front of the J - 1
    /// mapped errors, so the first codeword pairs that 0 with the first error.
    /// A first value other than 0 there is a code no valid encoder writes.
    /// libaec ignores it and keeps the second value; this refuses it.
    fn second_extension(
        &mut self,
        buf: &mut [u32; MAX_BLOCK],
        has_ref: bool,
        want: usize,
    ) -> Result<(), Stop> {
        let mut i = usize::from(has_ref);
        while i < want {
            let codeword = self.reader.fs()?;
            let (first, second) = se_pair(codeword).ok_or(Stop::Invalid(SE_OUT_OF_RANGE))?;
            if i % 2 == 0 {
                buf[i] = self.narrow(first)?;
                i += 1;
            } else if first != 0 {
                return Err(Stop::Invalid(SE_REFERENCE_PAIR));
            }
            if i < want {
                buf[i] = self.narrow(second)?;
            }
            i += 1;
        }
        Ok(())
    }

    /// `value` as a code, if it fits the sample width.
    fn narrow(&self, value: u64) -> Result<u32, Stop> {
        if value > self.max_code {
            return Err(Stop::Invalid(TOO_WIDE));
        }
        u32::try_from(value).map_err(|_| Stop::Invalid(TOO_WIDE))
    }

    /// Turn a block's codes into samples in place, and return the last one.
    fn postprocess(&self, block: &mut [u32], has_ref: bool, mut last: u32) -> u32 {
        let errors = match (self.mapping, block) {
            (Mapping::None, _) => return last,
            (_, [reference, errors @ ..]) if has_ref => {
                last = self.reference(*reference);
                *reference = last;
                errors
            }
            (_, errors) => errors,
        };
        match self.mapping {
            Mapping::None => {}
            Mapping::Unsigned { xmax } => {
                for d in errors {
                    last = unmap_unsigned(last, *d, xmax);
                    *d = last;
                }
            }
            Mapping::Signed { xmax, .. } => {
                for d in errors {
                    last = unmap_signed(last, *d, xmax);
                    *d = last;
                }
            }
        }
        last
    }

    /// A reference sample as a sample: sign-extended for signed data.
    fn reference(&self, raw: u32) -> u32 {
        match self.mapping {
            Mapping::Signed { sign, .. } => (raw ^ sign).wrapping_sub(sign),
            _ => raw,
        }
    }
}

/// What one CDS decoded to.
enum Cds {
    /// A block of codes in the buffer.
    Block,
    /// A run of `blocks` zero blocks.
    ZeroRun { blocks: u32 },
}

/// How many zero blocks a zero-block CDS covers, given its fundamental
/// sequence `fs`, the blocks already decoded in this reference sample interval
/// and the interval's length; or why the code is invalid.
///
/// CCSDS 121.0-B-3 Table 3-2: `fs + 1` blocks for 1 to 4, then the ROS code (5)
/// for "the rest of this 64-block segment", and `fs` blocks from 5 to 63. The
/// table ends at 63 zeros and a one, a whole segment, so a longer sequence is
/// refused; libaec accepts any length that fits the interval. Segments count
/// from the start of the interval, and the rest of a segment stops at the
/// interval's end.
///
/// A run other than ROS may cross a segment boundary. B-3 lists "specifies the
/// size of a segment as 64 blocks" among its changes affecting backward
/// compatibility, so an encoder written to the earlier issue can place runs
/// that way; they are accepted, as libaec accepts them. A run past the end of
/// the interval is refused.
pub(crate) fn zero_run_blocks(fs: u64, blocks_in_rsi: u32, rsi: u32) -> Result<u32, &'static str> {
    let left_in_rsi = rsi.checked_sub(blocks_in_rsi).ok_or(RUN_PAST_RSI)?;
    if fs > MAX_ZERO_RUN_FS {
        return Err(RUN_PAST_TABLE);
    }
    // `fs <= 63`, so each count fits a `u32`.
    let blocks = match fs + 1 {
        ROS => left_in_rsi.min(SEGMENT_BLOCKS - blocks_in_rsi % SEGMENT_BLOCKS),
        b if b < ROS => u32::try_from(b).map_err(|_| RUN_PAST_TABLE)?,
        b => u32::try_from(b - 1).map_err(|_| RUN_PAST_TABLE)?,
    };
    if blocks <= left_in_rsi {
        Ok(blocks)
    } else {
        Err(RUN_PAST_RSI)
    }
}

/// The pair of values `(α, β)` a second-extension codeword `γ` stands for,
/// where `γ = (α + β)(α + β + 1) / 2 + β` (CCSDS 121.0-B-3 §3.4.1); `None`
/// above [`SE_MAX_CODEWORD`], or too large for a `u64`.
///
/// The standard bounds neither value by anything but the sample width, which
/// the caller checks.
pub(crate) fn se_pair(codeword: u64) -> Option<(u64, u64)> {
    if SE_MAX_CODEWORD.is_some_and(|max| codeword > max) {
        return None;
    }
    // The pair sum `s` is the largest with `s(s+1)/2 <= γ`.
    let root = codeword.checked_mul(8)?.checked_add(1)?.isqrt();
    let sum = (root - 1) / 2;
    let beta = codeword - sum * (sum + 1) / 2;
    Some((sum - beta, beta))
}

/// Libaec's inverse of the prediction-error mapping for unsigned samples, in
/// wrapping `u32` arithmetic (`decode.c:80-99`): the sample after `last`, given
/// the mapped error `d` and the largest sample `xmax`.
///
/// CCSDS 121.0-B-3 §4.4: with `θ` the distance from `last` to the nearer end of
/// the range, `d <= 2θ` encodes the prediction error `d/2` (even) or
/// `-(d+1)/2` (odd), and a larger `d` steps `d - θ` away from that end.
pub(crate) fn unmap_unsigned(last: u32, d: u32, xmax: u32) -> u32 {
    let half = xmax / 2 + 1;
    // `last` is in the top half exactly when it has the top half's bit set,
    // and then the nearer end is `xmax`, where `xmax - last == xmax ^ last`.
    let near = if last & half != 0 { xmax } else { 0 };
    if (d >> 1) + (d & 1) <= near ^ last {
        last.wrapping_add(prediction_error(d))
    } else {
        near ^ d
    }
}

/// Libaec's inverse mapping for signed samples, carried sign-extended in a
/// `u32` (`decode.c:101-120`). `xmax` is `2^(n-1) - 1`.
pub(crate) fn unmap_signed(last: u32, d: u32, xmax: u32) -> u32 {
    let half_d = (d >> 1) + (d & 1);
    if last >> 31 != 0 {
        // Negative: the nearer end is `-xmax - 1`.
        if half_d <= xmax.wrapping_add(last).wrapping_add(1) {
            last.wrapping_add(prediction_error(d))
        } else {
            d.wrapping_sub(xmax).wrapping_sub(1)
        }
    } else if half_d <= xmax.wrapping_sub(last) {
        last.wrapping_add(prediction_error(d))
    } else {
        xmax.wrapping_sub(d)
    }
}

/// The prediction error an in-range mapped error `d` encodes, as a wrapping
/// `u32`: `d/2` for even `d`, `-(d+1)/2` for odd.
fn prediction_error(d: u32) -> u32 {
    (d >> 1) ^ !(d & 1).wrapping_sub(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_runs_follow_table_3_2() {
        // 1 to 4 blocks, then ROS, then fs blocks.
        for fs in 0..4 {
            assert_eq!(zero_run_blocks(fs, 0, 128), Ok(fs as u32 + 1));
        }
        assert_eq!(zero_run_blocks(5, 0, 128), Ok(5));
        assert_eq!(zero_run_blocks(63, 0, 128), Ok(63));
        // ROS: to the end of the 64-block segment...
        assert_eq!(zero_run_blocks(4, 0, 128), Ok(64));
        assert_eq!(zero_run_blocks(4, 70, 128), Ok(58));
        // ROS starting mid-segment counts to that segment's end, not 64 on.
        assert_eq!(zero_run_blocks(4, 10, 128), Ok(54));
        assert_eq!(zero_run_blocks(4, 100, 4096), Ok(28));
        // ...or of the interval, whichever is first.
        assert_eq!(zero_run_blocks(4, 10, 20), Ok(10));
        assert_eq!(zero_run_blocks(4, 0, 1), Ok(1));
        // A run other than ROS may cross a segment boundary (B-3's segment
        // definition is newer than some encoders).
        assert_eq!(zero_run_blocks(9, 60, 128), Ok(9));
    }

    #[test]
    fn a_zero_run_past_the_interval_or_the_table_is_refused() {
        assert_eq!(zero_run_blocks(3, 0, 4), Ok(4));
        assert_eq!(zero_run_blocks(3, 1, 4), Err(RUN_PAST_RSI));
        assert_eq!(zero_run_blocks(0, 5, 4), Err(RUN_PAST_RSI));
        // Table 3-2 stops at 63 zeros: longer is refused even inside the
        // interval, where libaec would accept it.
        assert_eq!(zero_run_blocks(63, 0, 4096), Ok(63));
        assert_eq!(zero_run_blocks(64, 0, 4096), Err(RUN_PAST_TABLE));
        assert_eq!(zero_run_blocks(u64::MAX, 0, 4096), Err(RUN_PAST_TABLE));
    }

    #[test]
    fn second_extension_inverts_the_pairing() {
        // Every pair with a sum up to 40, well past libaec's table (12).
        for sum in 0u64..=40 {
            for beta in 0..=sum {
                let alpha = sum - beta;
                let codeword = sum * (sum + 1) / 2 + beta;
                assert_eq!(se_pair(codeword), Some((alpha, beta)), "γ = {codeword}");
            }
        }
        // libaec's last codeword, and the first it refuses.
        assert_eq!(se_pair(90), Some((0, 12)));
        assert_eq!(se_pair(91), Some((13, 0)));
        // Huge codewords neither overflow nor panic.
        let (a, b) = se_pair(1 << 60).unwrap();
        assert!(a > 1 << 29 && b > 0);
        assert_eq!(se_pair(u64::MAX), None);
    }

    /// The standard's mapping (CCSDS 121.0-B-3 §4.4), written the long way.
    fn map_forward(x: i64, last: i64, xmin: i64, xmax: i64) -> u32 {
        let delta = x - last;
        let theta = (last - xmin).min(xmax - last);
        let d = if (0..=theta).contains(&delta) {
            2 * delta
        } else if (-theta..0).contains(&delta) {
            -2 * delta - 1
        } else {
            theta + delta.abs()
        };
        u32::try_from(d).unwrap()
    }

    #[test]
    fn unmapping_inverts_the_standards_mapping_exhaustively_at_small_widths() {
        for n in 1..=6u32 {
            let xmax_u = (1i64 << n) - 1;
            for last in 0..=xmax_u {
                for x in 0..=xmax_u {
                    let d = map_forward(x, last, 0, xmax_u);
                    let got = unmap_unsigned(last as u32, d, xmax_u as u32);
                    assert_eq!(i64::from(got), x, "unsigned n={n} last={last} x={x}");
                }
            }
            let xmax_s = (1i64 << (n - 1)) - 1;
            let xmin_s = -xmax_s - 1;
            for last in xmin_s..=xmax_s {
                for x in xmin_s..=xmax_s {
                    let d = map_forward(x, last, xmin_s, xmax_s);
                    let got = unmap_signed(last as i32 as u32, d, xmax_s as u32);
                    assert_eq!(i64::from(got as i32), x, "signed n={n} last={last} x={x}");
                }
            }
        }
    }

    #[test]
    fn unmapping_holds_at_the_ends_of_32_bits() {
        let xmax = u32::MAX;
        assert_eq!(
            unmap_unsigned(0, map_forward(0xFFFF_FFFF, 0, 0, 0xFFFF_FFFF), xmax),
            xmax
        );
        assert_eq!(
            unmap_unsigned(xmax, map_forward(0, 0xFFFF_FFFF, 0, 0xFFFF_FFFF), xmax),
            0
        );
        let smax = i64::from(i32::MAX);
        let smin = i64::from(i32::MIN);
        let d = map_forward(smin, smax, smin, smax);
        assert_eq!(
            unmap_signed(i32::MAX as u32, d, i32::MAX as u32) as i32,
            i32::MIN
        );
        let d = map_forward(smax, smin, smin, smax);
        assert_eq!(
            unmap_signed(i32::MIN as u32, d, i32::MAX as u32) as i32,
            i32::MAX
        );
    }
}
