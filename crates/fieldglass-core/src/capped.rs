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
/// what it decodes until the caller's slice is full, as `ruzstd` does, would
/// otherwise hold a second copy of the output: handed one 3 MiB slice,
/// `ruzstd` grew its own buffer to 4 MiB beside it (#981).
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
pub const READ_STEP: usize = 128 << 10;

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
