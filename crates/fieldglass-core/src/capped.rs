//! Output buffers that grow toward a limit and never reserve past it (#981).
//!
//! A bounded decompressor stops once its output reaches a limit, but the limit
//! says how many bytes it may *write*, not how many it may *reserve*. A `Vec`
//! grown by `resize` or `read_to_end` takes amortised growth, which sets the
//! capacity to at least twice the old one. So the step that reaches the limit
//! can reserve nearly twice it: a deflate bomb stopped at a 4,722,688-byte
//! limit reserved 8,351,488 bytes (#975), and near the 256 MiB chunk ceiling
//! that is about 512 MiB committed. On wasm linear memory never shrinks, so
//! that stays committed for the life of the page.
//!
//! Both helpers here grow a buffer by doubling, as before, but reserve exactly
//! the length they grow to and clamp that to the limit, so a buffer's capacity
//! is never more than the limit. The codecs themselves stay in the format
//! crates that need them (`miniz_oxide` and `ruzstd` are dependencies of the
//! NetCDF and Zarr readers, not of this crate); what they share is this
//! growth rule, which depends on nothing.
//!
//! A buffer whose final length is known starts at that length, so a valid
//! stream fills one allocation and nothing is copied. But the length comes
//! from the container, not the stream, so the start is also bounded by the
//! most the stream's own bytes can decode to, [`inflate_bound`] and
//! [`unzstd_bound`]: a few bytes stating 256 MiB are not handed 256 MiB to
//! fill (#986). Those two are arithmetic on the formats' limits, so they too
//! depend on nothing.
//!
//! A zstd decoder also holds the frame's window beside the output, which no
//! output limit sees, so [`zstd_window_limit`] sets the largest window a frame
//! may declare from the same limit (#989).
//!
//! GRIB's grouped packings state a group count and store each group's width
//! and length, and a table sized by the count can be far larger than the
//! section: with every descriptor zero bits wide, a 25-byte GRIB1 section can
//! state 16 Mi groups. [`group_needs`] sums what the groups ask of their section
//! without storing any of them, so a decoder can refuse one that cannot be
//! filled before it allocates the tables or the output (#1001).

use std::io::{self, Read};

/// Zero-extend `buf` toward `limit`: to twice its length, or to `hint` while
/// that is larger, never past `limit`. Only the new length is reserved, so the
/// capacity stays within `limit` when it started there.
///
/// Returns `false`, leaving `buf` as it is, when it already holds `limit`
/// bytes: a decoder that still has output then has more than the limit allows.
/// A `hint` of zero still grows an empty buffer by one byte.
pub fn grow_toward(buf: &mut Vec<u8>, hint: usize, limit: usize) -> bool {
    let len = buf.len();
    if len >= limit {
        return false;
    }
    let new_len = len.saturating_mul(2).max(hint).max(1).min(limit);
    if buf.capacity() == 0 {
        // `vec!` asks the allocator for zeroed memory, which a large fresh
        // block already is, so the first step writes nothing. A chunk whose
        // first guess covers it never pays for zeroing at all.
        *buf = vec![0; new_len];
        return true;
    }
    buf.reserve_exact(new_len - len);
    buf.resize(new_len, 0);
    true
}

/// Read `reader` to its end into a buffer grown by [`grow_toward`], or
/// `Ok(None)` when it holds more than `limit` bytes.
///
/// The buffer is never more than `limit` bytes. Telling a stream of exactly
/// `limit` bytes from a longer one takes one more byte, which is read into a
/// one-byte probe on the stack rather than into the buffer.
///
/// Each `read` asks for at most [`READ_STEP`] bytes. A decoder that buffers
/// what it decodes until the caller's slice is full would otherwise hold a
/// second copy of the output: `ruzstd` 0.9.0, handed one 3 MiB slice, grew its
/// own buffer to 4 MiB beside it (#981). `ruzstd` 0.9.1 decodes one block at a
/// time and returns as soon as it holds anything past the window, so the step
/// is now only a defensive cap, kept so the bound does not depend on the
/// decoder's version.
pub fn read_to_end_within(
    mut reader: impl Read,
    hint: usize,
    limit: usize,
) -> io::Result<Option<Vec<u8>>> {
    let mut buf = Vec::new();
    let mut filled = 0;
    loop {
        if filled == buf.len() && !grow_toward(&mut buf, hint, limit) {
            return Ok(if read_one(&mut reader)? {
                None
            } else {
                Some(buf)
            });
        }
        let end = buf.len().min(filled.saturating_add(READ_STEP));
        match reader.read(&mut buf[filled..end]) {
            Ok(0) => {
                buf.truncate(filled);
                return Ok(Some(buf));
            }
            Ok(n) => filled += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
}

/// The most [`read_to_end_within`] asks a reader for at once: zstd's largest
/// block, so a streaming decoder need not hold more than a block past its
/// window to answer.
pub const READ_STEP: usize = ZSTD_MAX_BLOCK;

/// The most a DEFLATE stream (RFC 1951) of `input_len` bytes can inflate to:
/// 1,032 bytes per input byte.
///
/// The cheapest output DEFLATE codes is a match of 258 bytes, the longest
/// there is (length code 285, no extra bits; RFC 1951 section 3.2.5), behind
/// a one-bit literal/length code and a one-bit distance code with no extra
/// bits. No code is shorter than one bit: a dynamic block's code lengths
/// start at one, and even a lone distance code "is encoded using one bit,
/// not zero bits" (section 3.2.7). So every two bits of input code at most
/// 258 bytes, 1,032 per byte; a literal codes one byte in at least one bit,
/// and block headers and a zlib wrapper code nothing at all.
///
/// A start for an output buffer whose final length is stated by something
/// other than the stream, as `min(stated, inflate_bound(input))`: a stream
/// that can really fill the stated length gets it in one allocation, and one
/// that cannot is never handed more than it could fill (#986). It is a start,
/// not a limit, so a decoder that somehow went past it would only grow.
pub fn inflate_bound(input_len: usize) -> usize {
    input_len.saturating_mul(DEFLATE_MAX_EXPANSION)
}

/// Bytes of output one byte of DEFLATE can code at most; see
/// [`inflate_bound`].
const DEFLATE_MAX_EXPANSION: usize = 1032;

/// The most a zstd frame (RFC 8878) of `input_len` bytes can decompress to:
/// 128 KiB for every three bytes.
///
/// A frame's data is a run of blocks, each behind a three-byte header
/// (RFC 8878 section 3.1.1.2), and no block decompresses past
/// `Block_Maximum_Size`, at most 128 KiB: "This maximum is applicable to both
/// the decompressed size and the compressed size of any block in the frame"
/// (section 3.1.1.2.4). The bound counts every three input bytes as a whole
/// block, which is loose by the frame header and the block contents, and is
/// used the same way as [`inflate_bound`], as a start and never as a limit.
pub fn unzstd_bound(input_len: usize) -> usize {
    input_len
        .div_ceil(ZSTD_BLOCK_HEADER)
        .saturating_mul(ZSTD_MAX_BLOCK)
}

/// The size of a zstd block header (RFC 8878 section 3.1.1.2).
const ZSTD_BLOCK_HEADER: usize = 3;

/// The most one zstd block decompresses to (RFC 8878 section 3.1.1.2.4).
const ZSTD_MAX_BLOCK: usize = 128 << 10;

/// The most a DEFLATE encoder writes for `len` bytes of input, with no
/// wrapper: zlib's `deflateBound` for any parameters (#1012).
///
/// The larger of its fixed-block bound (9-bit literals, RFC 1951 §3.2.6),
/// `len + len/8 + len/256 + len/512 + 4`, and its bound for stored blocks of
/// 127 bytes. An encoder that falls back to stored blocks on data that does
/// not compress, as zlib does at its defaults, adds 5 bytes per block of up
/// to 65,535 (RFC 1951 §3.2.4), well inside it. A zlib wrapper (RFC 1950
/// §2.2) adds 6 bytes and a gzip member's (RFC 1952 §2.3) 18 without its
/// optional header fields; the caller adds whichever its container has.
///
/// The inverse of [`inflate_bound`]: that one bounds what a stream decodes
/// to, this one what a codec ahead of the stream in a pipeline may be handed
/// back once it is inflated.
pub fn deflate_compress_bound(len: u64) -> u64 {
    // zlib 1.3.1 `deflate.c`: `fixedlen` and `storelen`.
    let fixed = len
        .saturating_add(len >> 3)
        .saturating_add(len >> 8)
        .saturating_add(len >> 9)
        .saturating_add(4);
    let stored = len
        .saturating_add(len >> 5)
        .saturating_add(len >> 7)
        .saturating_add(len >> 11)
        .saturating_add(7);
    fixed.max(stored)
}

/// The most a zstd encoder writes for `len` bytes of input, frame header
/// included: `ZSTD_COMPRESSBOUND` in `zstd.h`, `len + len/256` plus a margin
/// of at most 64 bytes under 128 KiB (#1012).
///
/// The inverse of [`unzstd_bound`], as [`deflate_compress_bound`] is of
/// [`inflate_bound`].
pub fn zstd_compress_bound(len: u64) -> u64 {
    const SMALL: u64 = 128 << 10;
    len.saturating_add(len / 256)
        .saturating_add(SMALL.saturating_sub(len) >> 11)
}

/// The largest window a zstd decoder whose output is held to `limit` bytes
/// lets a frame declare: `limit`, but never under 8 MiB and never past
/// `ceiling` (#989).
///
/// A decoder keeps the frame's whole window, `Window_Size` (RFC 8878 section
/// 3.1.1.1.2), before it can hand back a byte: `ruzstd` returns nothing until
/// it holds more than the window, in a ring that `ruzstd` 0.9.1 sizes by
/// rounding the window less 256 KiB up to a power of two, then adding 256 KiB
/// back. A power-of-two window costs about the window plus 256 KiB; any other
/// window costs up to about twice it. So the window, not the output limit,
/// sets what a decode holds, and a frame stating 64 MiB in a 4 KiB chunk held
/// about 64.25 MiB before the limit saw any output (0.9.0 rounded the whole
/// window up to a power of two: about 192 MiB).
///
/// The RFC lets a decoder refuse a frame whose window is past its own limit,
/// and recommends accepting at least 8 MiB, so a window up to that decodes
/// whatever its output. Past it, the output limit is the bound: with no
/// dictionary, which neither HDF5 nor Zarr uses, a decode stopped at `limit`
/// bytes never reaches back further than that. A single-segment frame's
/// window is its content size, so one refused here would have decoded past
/// the limit anyway; only a frame stating a window wider than its content,
/// which a streaming writer with no pledged size can produce, is refused
/// while valid.
pub fn zstd_window_limit(limit: usize, ceiling: u64) -> u64 {
    u64::try_from(limit)
        .unwrap_or(u64::MAX)
        .max(ZSTD_RECOMMENDED_WINDOW)
        .min(ceiling)
}

/// The window RFC 8878 recommends every decoder accept: "at least 8 MB"
/// (section 3.1.1.1.2).
const ZSTD_RECOMMENDED_WINDOW: u64 = 8 << 20;

/// Whether `frame` begins with a single-segment zstd frame header, whose
/// window is its content size (RFC 8878 section 3.1.1.1.1:
/// `Single_Segment_Flag`, bit 5 of the byte after the magic number).
///
/// `false` for bytes too short to say; it reads that one bit and checks
/// nothing else, so it is only meaningful once a decoder has accepted the
/// header.
pub fn zstd_single_segment(frame: &[u8]) -> bool {
    frame
        .get(4)
        .is_some_and(|descriptor| descriptor & 0x20 != 0)
}

/// What a run of GRIB groups asks of its section, from [`group_needs`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GroupNeeds {
    /// The sum of the group lengths: the points the groups hold.
    pub points: u64,
    /// The sum of each group's length times its width: the bits of per-point
    /// offsets the groups read after their descriptors.
    pub offset_bits: u64,
}

/// Sum [`GroupNeeds`] over `count` groups, where `group(g)` reads group `g`'s
/// width and length, without storing either (#1001).
///
/// A grouped decoder calls this before it allocates anything sized by the
/// group count or the point count, then refuses a section whose groups do not
/// hold the points it must produce, or whose offsets run past its end. What
/// is left to allocate is then proved by the section: the group tables hold
/// no more entries than the output has points, and every point that is not a
/// zero-width group's run of its reference has its bits in the section.
///
/// The sums saturate, so a hostile width or length can only make them larger
/// than any section, never wrap to a small one. An error from `group` stops
/// the sum and is returned.
pub fn group_needs<E>(
    count: usize,
    mut group: impl FnMut(usize) -> Result<(u64, u64), E>,
) -> Result<GroupNeeds, E> {
    let mut needs = GroupNeeds::default();
    for g in 0..count {
        let (width, length) = group(g)?;
        needs.points = needs.points.saturating_add(length);
        needs.offset_bits = needs
            .offset_bits
            .saturating_add(length.saturating_mul(width));
    }
    Ok(needs)
}

/// Whether `ahead` bytes, allocated from a section's stated counts before
/// its data is read, are no more than its `section_len` bytes could fill:
/// [`inflate_bound`], the most a stream of that length can produce, the bound
/// a codec's first buffer starts at (#986).
///
/// A grouped decoder skips the [`group_needs`] pass when this holds, since
/// what it would protect is already held to the section's bytes. That pass
/// reads every group descriptor a second time, which on a typical field is
/// about 7% of the decode, and a field big enough to need it is one whose
/// section is small beside it, so has few descriptors to read (#1001).
pub fn section_can_fill(ahead: usize, section_len: usize) -> bool {
    ahead <= inflate_bound(section_len)
}

/// Whether `reader` has at least one more byte.
fn read_one(reader: &mut impl Read) -> io::Result<bool> {
    let mut probe = [0u8; 1];
    loop {
        match reader.read(&mut probe) {
            Ok(n) => return Ok(n > 0),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every step doubles, the last one stops at the limit, and the capacity
    /// is the length at every step: no step reserves past what it grew to.
    #[test]
    fn growth_doubles_and_stops_at_the_limit_without_reserving_past_it() {
        let limit = 3_000;
        let mut buf = Vec::new();
        let mut lens = Vec::new();
        while grow_toward(&mut buf, 100, limit) {
            assert_eq!(buf.capacity(), buf.len(), "exact reservation");
            lens.push(buf.len());
        }
        assert_eq!(lens, [100, 200, 400, 800, 1_600, 3_000]);
        assert!(buf.iter().all(|&b| b == 0));
    }

    /// `Vec::resize` alone, the growth this replaces, reserves past the limit
    /// on the last step. Pinned so the test above is known to be measuring
    /// something `Vec` would otherwise do.
    #[test]
    fn plain_resize_overshoots_where_grow_toward_does_not() {
        let mut buf = vec![0u8; 1_600];
        buf.resize(3_000, 0);
        assert!(buf.capacity() >= 3_200, "capacity {}", buf.capacity());
    }

    /// The bounds per input byte, and that a hostile input length saturates
    /// rather than wrapping to a small start.
    #[test]
    fn codec_bounds_scale_with_the_input_and_saturate() {
        assert_eq!(inflate_bound(0), 0);
        assert_eq!(inflate_bound(12), 12_384);
        assert_eq!(unzstd_bound(0), 0);
        assert_eq!(unzstd_bound(17), 6 * (128 << 10));
        assert_eq!(unzstd_bound(18), 6 * (128 << 10));
        assert_eq!(inflate_bound(usize::MAX), usize::MAX);
        assert_eq!(unzstd_bound(usize::MAX), usize::MAX);
    }

    /// The encoders' worst cases, against zlib's `deflateBound` (less its
    /// 6-byte wrapper) and `ZSTD_COMPRESSBOUND` at hand-worked lengths, and
    /// saturating at the top.
    #[test]
    fn compress_bounds_match_zlib_and_zstd_and_saturate() {
        // 0: the stored bound's 7 beats the fixed bound's 4.
        assert_eq!(deflate_compress_bound(0), 7);
        // 1,000: fixed is 1000 + 125 + 3 + 1 + 4.
        assert_eq!(deflate_compress_bound(1_000), 1_133);
        assert_eq!(zstd_compress_bound(0), 64);
        assert_eq!(zstd_compress_bound(24), 24 + 63);
        assert_eq!(zstd_compress_bound(1 << 20), (1 << 20) + (1 << 12));
        assert_eq!(deflate_compress_bound(u64::MAX), u64::MAX);
        assert_eq!(zstd_compress_bound(u64::MAX), u64::MAX);
    }

    /// The window is the output limit, raised to the RFC's 8 MiB and capped
    /// by the caller's ceiling, edges included (#989).
    #[test]
    fn the_zstd_window_follows_the_limit_between_8_mib_and_the_ceiling() {
        const MIB: u64 = 1 << 20;
        let ceiling = 64 * MIB;
        assert_eq!(zstd_window_limit(0, ceiling), 8 * MIB);
        assert_eq!(zstd_window_limit(4096, ceiling), 8 * MIB);
        assert_eq!(zstd_window_limit(8 << 20, ceiling), 8 * MIB);
        assert_eq!(zstd_window_limit((8 << 20) + 1, ceiling), 8 * MIB + 1);
        assert_eq!(zstd_window_limit(64 << 20, ceiling), ceiling);
        assert_eq!(zstd_window_limit(usize::MAX, ceiling), ceiling);
        // A ceiling under 8 MiB wins: the caller's bound is never widened.
        assert_eq!(zstd_window_limit(4096, MIB), MIB);
    }

    /// Points and offset bits are summed over every group, a zero-width group
    /// adding points but no bits, and the sums saturate rather than wrap.
    #[test]
    fn group_needs_sums_points_and_offset_bits_and_saturates() {
        let groups = [(0u64, 5u64), (3, 4), (32, 2)];
        let needs = group_needs(groups.len(), |g| Ok::<_, ()>(groups[g])).unwrap();
        assert_eq!(
            needs,
            GroupNeeds {
                points: 11,
                offset_bits: 12 + 64,
            }
        );
        let huge = group_needs(3, |_| Ok::<_, ()>((u64::MAX, u64::MAX))).unwrap();
        assert_eq!(huge.points, u64::MAX);
        assert_eq!(huge.offset_bits, u64::MAX);
        assert_eq!(
            group_needs(2, |g| if g == 1 { Err(g) } else { Ok((1, 1)) }),
            Err(1)
        );
        assert_eq!(
            group_needs(0, |_| Ok::<_, ()>((1, 1))).unwrap(),
            GroupNeeds::default()
        );
    }

    /// A section can fill up to its inflate bound, edge included.
    #[test]
    fn a_section_can_fill_up_to_its_inflate_bound() {
        assert!(section_can_fill(0, 0));
        assert!(!section_can_fill(1, 0));
        assert!(section_can_fill(1032 * 25, 25));
        assert!(!section_can_fill(1032 * 25 + 1, 25));
        assert!(section_can_fill(usize::MAX, usize::MAX));
    }

    #[test]
    fn the_single_segment_flag_is_bit_5_of_the_descriptor() {
        let magic = [0x28, 0xB5, 0x2F, 0xFD];
        assert!(zstd_single_segment(&[magic.as_slice(), &[0x20]].concat()));
        assert!(zstd_single_segment(&[magic.as_slice(), &[0xE4]].concat()));
        assert!(!zstd_single_segment(
            &[magic.as_slice(), &[0x00, 0x80]].concat()
        ));
        assert!(!zstd_single_segment(&[magic.as_slice(), &[0xDF]].concat()));
        assert!(!zstd_single_segment(&magic));
    }

    #[test]
    fn a_zero_hint_still_grows_and_a_zero_limit_never_does() {
        let mut buf = Vec::new();
        assert!(grow_toward(&mut buf, 0, 10));
        assert_eq!(buf.len(), 1);
        let mut empty = Vec::new();
        assert!(!grow_toward(&mut empty, 64, 0));
        assert!(empty.is_empty());
    }

    /// A reader that hands out at most `step` bytes a call, so the buffer has
    /// to grow several times and partial reads are exercised.
    struct Trickle<'a> {
        data: &'a [u8],
        step: usize,
    }

    impl Read for Trickle<'_> {
        fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
            let n = self.step.min(out.len()).min(self.data.len());
            out[..n].copy_from_slice(&self.data[..n]);
            self.data = &self.data[n..];
            Ok(n)
        }
    }

    #[test]
    fn reads_a_stream_up_to_and_exactly_at_the_limit() {
        let data: Vec<u8> = (0..=255).cycle().take(5_000).collect();
        for limit in [5_000, 5_001, 1 << 20] {
            let got = read_to_end_within(
                Trickle {
                    data: &data,
                    step: 777,
                },
                64,
                limit,
            )
            .unwrap()
            .expect("within the limit");
            assert_eq!(got, data);
            assert!(got.capacity() <= limit, "capacity {}", got.capacity());
        }
    }

    #[test]
    fn a_stream_one_byte_past_the_limit_is_refused() {
        let data = vec![7u8; 5_001];
        let got = read_to_end_within(
            Trickle {
                data: &data,
                step: 777,
            },
            64,
            5_000,
        )
        .unwrap();
        assert!(got.is_none());
        assert!(read_to_end_within(&[1u8][..], 0, 0).unwrap().is_none());
        assert_eq!(read_to_end_within(&[][..], 0, 0).unwrap(), Some(Vec::new()));
    }

    #[test]
    fn interrupted_reads_are_retried_and_other_errors_returned() {
        struct Flaky(u8);
        impl Read for Flaky {
            fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
                self.0 += 1;
                match self.0 {
                    1 | 3 => Err(io::ErrorKind::Interrupted.into()),
                    2 => {
                        out[0] = 9;
                        Ok(1)
                    }
                    4 => Ok(0),
                    _ => Err(io::ErrorKind::InvalidData.into()),
                }
            }
        }
        assert_eq!(read_to_end_within(Flaky(0), 4, 16).unwrap(), Some(vec![9]));
        let err = read_to_end_within(Flaky(4), 4, 16).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }
}
