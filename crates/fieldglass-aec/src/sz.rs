//! szip decompression with libsz's semantics: `SZ_BufftoBuffDecompress` from
//! libaec 1.1.7's `sz_compat.c`, the layer the HDF5 szip filter (and HDF4
//! SZIP) calls on top of the coder.
//!
//! libsz adds three things to a plain CCSDS 121.0 decode:
//!
//! - **Scanlines.** The reference sample interval is one scanline:
//!   `rsi = ceil(pps / ppb)` blocks. When `pps` is not a multiple of `ppb`,
//!   each scanline is padded to `rsi × ppb` samples before encoding, and the
//!   pad samples are dropped after decoding. Preprocessing runs through the
//!   pads, so they are decoded and then skipped, never removed from the
//!   stream.
//! - **Byte planes.** 32- and 64-bit pixels are coded as an 8-bit stream of
//!   byte planes: every pixel's first byte, then every pixel's second byte,
//!   and so on, in the pixels' own byte order. Byte `j` of the decoded stream
//!   (pads dropped) belongs at `out[(j mod P)·w + j div P]`, where `w` is the
//!   pixel width in bytes and `P = out.len() / w`.
//! - **Options.** Of the option mask, only [`MSB_OPTION_MASK`] (byte order of
//!   the output) and [`NN_OPTION_MASK`] (preprocessing) change how a stream
//!   decodes. `EC`, `LSB`, `K13`, `CHIP` and `RAW` do not: `EC` and `LSB` are
//!   the absence of `NN` and `MSB`, a decoder that reads every split option
//!   reads a stream written with or without `K13`, and `RAW` asks for no szip
//!   header, which libaec never writes.
//!
//! Output is 1, 2 or 4 bytes per pixel up to 32 bits (never 3: libsz does
//! not set `AEC_DATA_3BYTE`), and 8 at 64 bits.
//!
//! # Where this differs from libsz
//!
//! libsz decodes into a padded copy of up to `ppb` times the output (32× at
//! one pixel per scanline and 32 pixels per block), then copies it again to
//! deinterleave byte planes. [`decompress`] builds neither: pad samples are
//! skipped as they arrive, and byte planes are scattered straight to their
//! place in `out`. It allocates nothing.
//!
//! It differs from libsz in three more ways (ADR-0012 decision 4):
//!
//! - **A stream that runs out is an error**, [`AecError::Truncated`]. libsz
//!   returns `SZ_OK` either way. With unpadded scanlines it lowers `destLen`
//!   to what it decoded (`sz_compat.c:302-303`). With padded scanlines it
//!   reports the full length, `scanlines × pps` pixels (`sz_compat.c:295`),
//!   and the bytes past what it decoded come from an uninitialised buffer.
//!   HDF5 checks the length only in a debug-build `assert` (`H5Zszip.c:300`),
//!   so a release build passes either result on as the chunk.
//! - **A 32- or 64-bit output that is not a whole number of pixels is an
//!   error**, [`AecError::OutputLength`], as is a partial pixel at any
//!   width. At 32 and 64 bits libsz returns `SZ_OK` with the bytes out of
//!   place: it deinterleaves with `P = destLen / w` rounded down
//!   (`sz_compat.c:84-93, 305-306`), so a length one byte short of whole
//!   pixels moves bytes and leaves the last `destLen mod w` unwritten.
//! - **Decoding stops at the last output pixel.** libsz decodes every
//!   scanline whole when scanlines are padded, including the blocks after
//!   the last pixel of a partial last scanline, and fails if one of them
//!   holds a bad code. Those blocks describe samples nobody asked for, so
//!   here they are never read and the result is `Ok`, with the bytes libsz
//!   would have written.
//!
//! Parameter validation is libsz's (`sz_compat.c:229-235`) plus the one check
//! libaec's decoder adds behind it: at most 256 pixels per block. See
//! [`SzParams::new`].

use crate::decode::decode;
use crate::sink::{ByteSink, Sink};
use crate::{AecError, Flags, Params};

/// `SZ_ALLOW_K13_OPTION_MASK`: the encoder may use the k = 13 split option.
/// No effect on decoding.
pub const ALLOW_K13_OPTION_MASK: u32 = 1;
/// `SZ_CHIP_OPTION_MASK`. No effect on decoding.
pub const CHIP_OPTION_MASK: u32 = 2;
/// `SZ_EC_OPTION_MASK`: entropy coding without preprocessing. Decoding reads
/// only whether [`NN_OPTION_MASK`] is set.
pub const EC_OPTION_MASK: u32 = 4;
/// `SZ_LSB_OPTION_MASK`: least significant byte first. Decoding reads only
/// whether [`MSB_OPTION_MASK`] is set.
pub const LSB_OPTION_MASK: u32 = 8;
/// `SZ_MSB_OPTION_MASK`: output pixels most significant byte first.
pub const MSB_OPTION_MASK: u32 = 16;
/// `SZ_NN_OPTION_MASK`: nearest-neighbour preprocessing.
pub const NN_OPTION_MASK: u32 = 32;
/// `SZ_RAW_OPTION_MASK`: no szip header. HDF5 always sets it, and libaec
/// never writes a header, so it has no effect on decoding.
pub const RAW_OPTION_MASK: u32 = 128;

/// The largest scanline libsz accepts, `SZ_MAX_PIXELS_PER_SCANLINE`.
const MAX_PIXELS_PER_SCANLINE: u32 = 4096;

/// The largest block libaec's decoder accepts (`aec_decode_init`).
const MAX_PIXELS_PER_BLOCK: u32 = 256;

/// The parameters of an szip stream, validated as libsz validates them.
///
/// The fields are libsz's `SZ_com_t`, in its order: options mask, bits per
/// pixel, pixels per block, pixels per scanline. **That is not HDF5's order.**
/// The HDF5 szip filter stores its `cd_values` as mask, pixels per block,
/// bits per pixel, pixels per scanline (`H5Zpublic.h`), so a reader that
/// passes them through positionally swaps the middle two. Most such swaps are
/// still valid parameters, so the mistake decodes to garbage rather than
/// failing.
///
/// ```
/// use fieldglass_aec::sz::{MSB_OPTION_MASK, NN_OPTION_MASK, SzParams};
///
/// // HDF5 cd_values [mask, ppb, bpp, pps] = [48, 16, 32, 64]:
/// let cd = [NN_OPTION_MASK | MSB_OPTION_MASK, 16, 32, 64];
/// let params = SzParams::new(cd[0], cd[2], cd[1], cd[3])?;
/// assert_eq!(params.bits_per_pixel(), 32);
/// assert_eq!(params.pixels_per_block(), 16);
/// # Ok::<(), fieldglass_aec::AecError>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SzParams {
    options_mask: u32,
    bits_per_pixel: u32,
    pixels_per_block: u32,
    pixels_per_scanline: u32,
    /// The coder's parameters: 8-bit samples for 32- and 64-bit pixels,
    /// `rsi = ceil(pps / ppb)`, and the flags the mask maps to.
    aec: Params,
}

impl SzParams {
    /// Validate a parameter set, in `SZ_com_t` order.
    ///
    /// * `options_mask`: any value. Only [`MSB_OPTION_MASK`] and
    ///   [`NN_OPTION_MASK`] are read; every other bit is ignored, as libsz
    ///   ignores it.
    /// * `bits_per_pixel`: 1 to 32, or 64.
    /// * `pixels_per_block`: an even number from 2 to 256. libsz itself checks
    ///   only for even and non-zero; the upper bound is libaec's decoder,
    ///   which refuses larger blocks behind it. HDF5 writes 2 to 32.
    /// * `pixels_per_scanline`: 1 to 4096.
    ///
    /// # Errors
    ///
    /// [`AecError::PixelsPerScanline`], [`AecError::PixelsPerBlock`] or
    /// [`AecError::BitsPerPixel`], checked in that order, which is libsz's.
    ///
    /// ```
    /// use fieldglass_aec::AecError;
    /// use fieldglass_aec::sz::{NN_OPTION_MASK, SzParams};
    ///
    /// assert!(SzParams::new(NN_OPTION_MASK, 16, 10, 45).is_ok());
    /// assert_eq!(SzParams::new(0, 48, 16, 64), Err(AecError::BitsPerPixel(48)));
    /// assert_eq!(SzParams::new(0, 16, 7, 64), Err(AecError::PixelsPerBlock(7)));
    /// ```
    pub fn new(
        options_mask: u32,
        bits_per_pixel: u32,
        pixels_per_block: u32,
        pixels_per_scanline: u32,
    ) -> Result<Self, AecError> {
        if !(1..=MAX_PIXELS_PER_SCANLINE).contains(&pixels_per_scanline) {
            return Err(AecError::PixelsPerScanline(pixels_per_scanline));
        }
        if pixels_per_block == 0
            || !pixels_per_block.is_multiple_of(2)
            || pixels_per_block > MAX_PIXELS_PER_BLOCK
        {
            return Err(AecError::PixelsPerBlock(pixels_per_block));
        }
        let bits_per_sample = match bits_per_pixel {
            32 | 64 => 8,
            1..=31 => bits_per_pixel,
            _ => return Err(AecError::BitsPerPixel(bits_per_pixel)),
        };
        let mut flags = Flags::empty();
        if options_mask & MSB_OPTION_MASK != 0 {
            flags.insert(Flags::MSB);
        }
        if options_mask & NN_OPTION_MASK != 0 {
            flags.insert(Flags::PREPROCESS);
        }
        let rsi = pixels_per_scanline.div_ceil(pixels_per_block);
        // Each conversion is in range by the checks above: 1 to 31 bits,
        // blocks up to 256, and at most 4096 blocks per scanline.
        let narrow = |_| AecError::PixelsPerBlock(pixels_per_block);
        let aec = Params::new(
            u8::try_from(bits_per_sample).map_err(|_| AecError::BitsPerPixel(bits_per_pixel))?,
            u16::try_from(pixels_per_block).map_err(narrow)?,
            u16::try_from(rsi).map_err(narrow)?,
            flags,
        )?;
        Ok(SzParams {
            options_mask,
            bits_per_pixel,
            pixels_per_block,
            pixels_per_scanline,
            aec,
        })
    }

    /// The options mask, as given.
    pub const fn options_mask(&self) -> u32 {
        self.options_mask
    }

    /// Bits per pixel: 1 to 32, or 64.
    pub const fn bits_per_pixel(&self) -> u32 {
        self.bits_per_pixel
    }

    /// Pixels per block, an even number from 2 to 256.
    pub const fn pixels_per_block(&self) -> u32 {
        self.pixels_per_block
    }

    /// Pixels per scanline, 1 to 4096.
    pub const fn pixels_per_scanline(&self) -> u32 {
        self.pixels_per_scanline
    }

    /// Bytes per decoded pixel: 1 up to 8 bits, 2 up to 16, 4 up to 32, and
    /// 8 at 64. The output of [`decompress`] must be a whole number of them.
    pub const fn bytes_per_pixel(&self) -> usize {
        match self.bits_per_pixel {
            0..=8 => 1,
            9..=16 => 2,
            17..=32 => 4,
            _ => 8,
        }
    }

    /// Whether pixels are coded as byte planes (32 and 64 bits).
    const fn byte_planes(&self) -> bool {
        matches!(self.bits_per_pixel, 32 | 64)
    }
}

/// Decompress an szip stream into `out`, which it must fill exactly.
///
/// `out.len()` is the uncompressed size, in bytes. It must be a whole number
/// of [`SzParams::bytes_per_pixel`]; the pixels are written in libsz's
/// layout, most significant byte first with [`MSB_OPTION_MASK`], and as the
/// encoder's input bytes for 32- and 64-bit pixels. Bytes after the last
/// pixel's code are never read.
///
/// # Errors
///
/// - [`AecError::OutputLength`] when `out.len()` is not a whole number of
///   pixels. `bytes_per_sample` in the error is the pixel width.
/// - [`AecError::Truncated`] when the stream ends before `out` is full, where
///   libsz returns `SZ_OK` (with a smaller `destLen`, or with unwritten
///   bytes when scanlines are padded). Its counts are in
///   decoded samples with pads dropped: pixels, or bytes for 32- and 64-bit
///   pixels.
/// - [`AecError::InvalidCode`] for a code no valid encoder writes. Its
///   `sample` is counted the same way, and points at the first sample the
///   bad block would have written.
/// - [`AecError::OutputTooLarge`] when the padded stream behind `out` has more
///   samples than `usize` can count. Only a 32-bit target can reach it.
///
/// On an error the contents of `out` are unspecified.
///
/// ```
/// use fieldglass_aec::sz::{self, SzParams};
///
/// // 8-bit pixels, blocks of 2, 1 pixel per scanline: each scanline is a
/// // block of 2 samples with one pad. The stream is three uncompressed
/// // blocks (id 0b111), each a pixel and its pad: 7, 8 and 9.
/// let params = SzParams::new(0, 8, 2, 1)?;
/// let stream = [0xE0, 0xE0, 0x1C, 0x20, 0x03, 0x84, 0x80, 0x00];
/// let mut out = [0u8; 3];
/// sz::decompress(&stream, &params, &mut out)?;
/// assert_eq!(out, [7, 8, 9]);
///
/// // A stream that ends early is an error, never a shorter success.
/// let mut out = [0u8; 4];
/// assert!(sz::decompress(&stream, &params, &mut out).is_err());
/// # Ok::<(), fieldglass_aec::AecError>(())
/// ```
pub fn decompress(input: &[u8], params: &SzParams, out: &mut [u8]) -> Result<(), AecError> {
    let pixel = params.bytes_per_pixel();
    if !out.len().is_multiple_of(pixel) {
        return Err(AecError::OutputLength {
            len: out.len(),
            bytes_per_sample: pixel,
        });
    }
    let aec = &params.aec;
    // Samples in the output, pads dropped. With byte planes each is a byte.
    let samples = out.len() / aec.bytes_per_sample();
    let pps = params.pixels_per_scanline as usize;
    let line = usize::from(aec.rsi()) * usize::from(aec.block_size());
    let count =
        padded_count(samples, pps, line).ok_or(AecError::OutputTooLarge { len: out.len() })?;

    let len = out.len();
    let layout = if params.byte_planes() {
        Layout::Planes(Planes::new(out, pixel))
    } else {
        let msb = aec.flags().contains(Flags::MSB);
        Layout::Bytes(ByteSink::new(out, aec.bytes_per_sample(), msb))
    };
    let mut sink = SzSink {
        layout,
        pps,
        line,
        pos: 0,
        written: 0,
    };
    let result = decode(input, aec, count, &mut sink);
    let written = sink.written;
    match result {
        Ok(()) => {
            debug_assert_eq!(written, samples, "an output of {len} bytes");
            Ok(())
        }
        Err(AecError::Truncated { .. }) => Err(AecError::Truncated {
            decoded: written,
            requested: samples,
        }),
        Err(AecError::InvalidCode { sample, reason }) => Err(AecError::InvalidCode {
            sample: unpadded_index(sample, pps, line),
            reason,
        }),
        Err(other) => Err(other),
    }
}

/// How many stream samples, pads included, cover the first `samples` output
/// samples, when each `line`-sample scanline holds `pps` of them; `None` if
/// that does not fit a `usize`.
///
/// The stream stops at the last output sample, so the pads after it in the
/// last scanline are never decoded.
fn padded_count(samples: usize, pps: usize, line: usize) -> Option<usize> {
    let Some(last) = samples.checked_sub(1) else {
        return Some(0);
    };
    (last / pps)
        .checked_mul(line)?
        .checked_add(last % pps)?
        .checked_add(1)
}

/// The output index of stream sample `sample`, or of the next output sample
/// if it is a pad.
fn unpadded_index(sample: usize, pps: usize, line: usize) -> usize {
    (sample / line)
        .saturating_mul(pps)
        .saturating_add((sample % line).min(pps))
}

/// Drops pad samples and forwards the rest, in order, to the output layout.
#[derive(Debug)]
struct SzSink<'a> {
    layout: Layout<'a>,
    /// Output samples per scanline.
    pps: usize,
    /// Stream samples per scanline, pads included.
    line: usize,
    /// Position in the current scanline, `0..line`.
    pos: usize,
    /// Output samples written.
    written: usize,
}

impl SzSink<'_> {
    /// Walk `count` stream samples from the current position, calling
    /// `real(from, len)` for each run of output samples, where `from` is the
    /// run's offset in the walk. At most two runs per scanline crossed.
    fn walk(&mut self, count: usize, mut real: impl FnMut(&mut Layout<'_>, usize, usize)) {
        let mut done = 0;
        while done < count {
            let left = count - done;
            let step = if self.pos < self.pps {
                let run = (self.pps - self.pos).min(left);
                real(&mut self.layout, done, run);
                self.written += run;
                run
            } else {
                (self.line - self.pos).min(left)
            };
            done += step;
            self.pos += step;
            if self.pos == self.line {
                self.pos = 0;
            }
        }
    }
}

impl Sink for SzSink<'_> {
    fn samples(&mut self, block: &[u32]) {
        self.walk(block.len(), |layout, from, len| {
            if let Some(run) = block.get(from..from + len) {
                layout.samples(run);
            }
        });
    }

    fn repeat(&mut self, value: u32, count: usize) {
        self.walk(count, |layout, _, len| layout.repeat(value, len));
    }
}

/// Where output samples go.
#[derive(Debug)]
enum Layout<'a> {
    /// One pixel per sample, 1, 2 or 4 bytes, in order.
    Bytes(ByteSink<'a>),
    /// One byte per sample, scattered into byte planes.
    Planes(Planes<'a>),
}

impl Layout<'_> {
    fn samples(&mut self, run: &[u32]) {
        match self {
            Layout::Bytes(bytes) => bytes.samples(run),
            Layout::Planes(planes) => {
                for &v in run {
                    planes.put(v);
                }
            }
        }
    }

    fn repeat(&mut self, value: u32, count: usize) {
        match self {
            Layout::Bytes(bytes) => bytes.repeat(value, count),
            Layout::Planes(planes) => {
                for _ in 0..count {
                    planes.put(value);
                }
            }
        }
    }
}

/// Scatters byte `j` of the deinterleaved stream to `out[(j mod P)·w + j div P]`
/// (`deinterleave_buffer`, `sz_compat.c:84-93`), tracking the index
/// incrementally rather than dividing per byte.
#[derive(Debug)]
struct Planes<'a> {
    out: &'a mut [u8],
    /// Pixel width in bytes: 4 or 8.
    width: usize,
    /// Pixels in the output, `P`: the length of each plane.
    pixels: usize,
    /// `j mod P`: the pixel the next byte belongs to.
    pixel: usize,
    /// The next byte's index in `out`.
    at: usize,
}

impl<'a> Planes<'a> {
    fn new(out: &'a mut [u8], width: usize) -> Self {
        let pixels = out.len() / width;
        Planes {
            out,
            width,
            pixels,
            pixel: 0,
            at: 0,
        }
    }

    #[inline]
    fn put(&mut self, value: u32) {
        // Once every plane is full `at` is parked at `out.len()`, so a byte
        // past the end is dropped rather than written over the first plane.
        // The kernel never delivers one; the test below holds the guard.
        let Some(dst) = self.out.get_mut(self.at) else {
            return;
        };
        *dst = value.to_le_bytes()[0];
        self.pixel += 1;
        if self.pixel == self.pixels {
            // The next plane starts at the next byte of the first pixel,
            // unless that was the last plane.
            self.pixel = 0;
            let next = self.at + self.width - self.out.len() + 1;
            self.at = if next == self.width {
                self.out.len()
            } else {
                next
            };
        } else {
            self.at += self.width;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn padded_count_skips_the_last_scanlines_trailing_pads() {
        // 11 pixels per scanline in lines of 12.
        assert_eq!(padded_count(0, 11, 12), Some(0));
        assert_eq!(padded_count(1, 11, 12), Some(1));
        assert_eq!(padded_count(11, 11, 12), Some(11));
        assert_eq!(padded_count(12, 11, 12), Some(13));
        assert_eq!(padded_count(36, 11, 12), Some(3 * 12 + 3));
        // No padding: the count is the output.
        assert_eq!(padded_count(1000, 64, 64), Some(1000));
        // One pixel per scanline in blocks of 32.
        assert_eq!(padded_count(50, 1, 32), Some(49 * 32 + 1));
    }

    #[test]
    fn padded_count_refuses_what_usize_cannot_count() {
        assert_eq!(padded_count(usize::MAX, 1, 32), None);
        assert_eq!(padded_count(usize::MAX / 16, 1, 32), None);
        assert_eq!(padded_count(usize::MAX, 64, 64), Some(usize::MAX));
        assert_eq!(padded_count(usize::MAX, 2, 2), Some(usize::MAX));
        // The first scanline past the limit.
        assert_eq!(padded_count(usize::MAX / 2 + 1, 1, 2), Some(usize::MAX));
        assert_eq!(padded_count(usize::MAX / 2 + 2, 1, 2), None);
    }

    #[test]
    fn unpadded_index_maps_pads_to_the_next_pixel() {
        assert_eq!(unpadded_index(0, 11, 12), 0);
        assert_eq!(unpadded_index(10, 11, 12), 10);
        assert_eq!(unpadded_index(11, 11, 12), 11); // a pad
        assert_eq!(unpadded_index(12, 11, 12), 11);
        assert_eq!(unpadded_index(25, 11, 12), 23);
        assert_eq!(unpadded_index(usize::MAX, 1, 2), usize::MAX / 2 + 1);
    }

    #[test]
    fn planes_scatter_as_libsz_deinterleaves() {
        // deinterleave_buffer: dest[i*w + j] = src[j*(n/w) + i].
        for (n, w) in [(4, 4), (8, 4), (24, 4), (16, 8), (56, 8)] {
            let mut out = vec![0xEEu8; n];
            let mut planes = Planes::new(&mut out, w);
            for j in 0..n {
                planes.put(u32::try_from(j).unwrap() | 0xFF00);
            }
            let pixels = n / w;
            let mut want = vec![0u8; n];
            for i in 0..pixels {
                for j in 0..w {
                    want[i * w + j] = u8::try_from(j * pixels + i).unwrap();
                }
            }
            assert_eq!(out, want, "n {n} w {w}");
        }
    }

    #[test]
    fn a_byte_past_the_planes_is_dropped() {
        // One pixel, and several: after the last plane the next index would
        // be `w`, which is inside `out` whenever there is more than one pixel.
        for (n, w) in [(4, 4), (8, 4), (24, 8)] {
            let mut out = vec![0u8; n];
            let mut planes = Planes::new(&mut out, w);
            for v in 0..n + 2 * w {
                planes.put(u32::try_from(v).unwrap());
            }
            let pixels = n / w;
            let want: Vec<u8> = (0..n)
                .map(|k| u8::try_from((k % w) * pixels + k / w).unwrap())
                .collect();
            assert_eq!(out, want, "n {n} w {w}");
        }
    }
}
