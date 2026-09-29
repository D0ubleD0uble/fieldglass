//! Where decoded samples go: the [`Sink`] trait, and the byte sink behind
//! [`decode_to_bytes`](crate::decode_to_bytes).

/// Receives decoded samples, in order.
///
/// [`decode`](crate::decode) calls it once per decoded block, and once per run
/// of zero blocks, so a consumer can convert samples straight into its own
/// output without an intermediate byte buffer. Between them the calls deliver
/// exactly the requested number of samples on success. On an error the sink
/// has received the samples decoded before it, and no more.
///
/// Each sample is a `u32` as libaec's postprocessor leaves it:
///
/// - without [`Flags::PREPROCESS`](crate::Flags::PREPROCESS), the coded n-bit
///   value, zero-extended, whether or not the data is signed;
/// - with it, the reconstructed sample: an n-bit value for unsigned data, and
///   a two's-complement value sign-extended to 32 bits for
///   [`Flags::SIGNED`](crate::Flags::SIGNED) data.
///
/// ```
/// use fieldglass_aec::Sink;
///
/// /// Sums the samples without storing them.
/// struct Total(u64);
///
/// impl Sink for Total {
///     fn samples(&mut self, block: &[u32]) {
///         self.0 += block.iter().map(|&s| u64::from(s)).sum::<u64>();
///     }
///     fn repeat(&mut self, value: u32, count: usize) {
///         self.0 += u64::from(value) * count as u64;
///     }
/// }
/// ```
pub trait Sink {
    /// The next samples, one block or part of one (at most 256).
    fn samples(&mut self, block: &[u32]);

    /// The next `count` samples, all equal to `value`: a run of zero blocks.
    ///
    /// Without preprocessing `value` is 0; with it, the run repeats the sample
    /// before it. A run can span many blocks, up to the end of a reference
    /// sample interval.
    fn repeat(&mut self, value: u32, count: usize);
}

/// Writes samples in libaec's output layout: `width` bytes each (1, 2, 3 or
/// 4), most significant byte first when `msb` is set. Wider values keep their
/// low bytes, which is how libaec's `put_*` functions truncate.
///
/// The kernel never hands it more samples than `out` holds; if it did, the
/// excess would be dropped rather than written out of bounds.
#[derive(Debug)]
pub(crate) struct ByteSink<'a> {
    out: &'a mut [u8],
    pos: usize,
    width: usize,
    msb: bool,
}

impl<'a> ByteSink<'a> {
    pub(crate) fn new(out: &'a mut [u8], width: usize, msb: bool) -> Self {
        ByteSink {
            out,
            pos: 0,
            width,
            msb,
        }
    }

    /// `value` in the output layout, in the first `width` bytes.
    #[inline]
    fn encode(&self, value: u32) -> [u8; 4] {
        let be = value.to_be_bytes();
        let le = value.to_le_bytes();
        match (self.width, self.msb) {
            (1, _) => [le[0], 0, 0, 0],
            (2, true) => [be[2], be[3], 0, 0],
            (2, false) => [le[0], le[1], 0, 0],
            (3, true) => [be[1], be[2], be[3], 0],
            (3, false) => [le[0], le[1], le[2], 0],
            (_, true) => be,
            (_, false) => le,
        }
    }

    fn rest(&mut self) -> &mut [u8] {
        self.out.get_mut(self.pos..).unwrap_or_default()
    }
}

impl Sink for ByteSink<'_> {
    fn samples(&mut self, block: &[u32]) {
        let width = self.width;
        let msb = self.msb;
        let rest = self.rest();
        let written = (rest.len() / width).min(block.len()) * width;
        // One loop per layout, so the per-sample work is a store.
        match (width, msb) {
            (1, _) => store(rest, block, |v| [v.to_le_bytes()[0]]),
            (2, true) => store(rest, block, |v| {
                let [_, _, b2, b3] = v.to_be_bytes();
                [b2, b3]
            }),
            (2, false) => store(rest, block, |v| {
                let [b0, b1, _, _] = v.to_le_bytes();
                [b0, b1]
            }),
            (3, true) => store(rest, block, |v| {
                let [_, b1, b2, b3] = v.to_be_bytes();
                [b1, b2, b3]
            }),
            (3, false) => store(rest, block, |v| {
                let [b0, b1, b2, _] = v.to_le_bytes();
                [b0, b1, b2]
            }),
            (_, true) => store(rest, block, u32::to_be_bytes),
            (_, false) => store(rest, block, u32::to_le_bytes),
        }
        self.pos += written;
    }

    fn repeat(&mut self, value: u32, count: usize) {
        let width = self.width;
        let bytes = self.encode(value);
        let pattern = bytes.get(..width).unwrap_or_default();
        let rest = self.rest();
        let written = (rest.len() / width).min(count) * width;
        let run = rest.get_mut(..written).unwrap_or_default();
        if let [byte] = pattern {
            run.fill(*byte);
        } else {
            for dst in run.chunks_exact_mut(width) {
                dst.copy_from_slice(pattern);
            }
        }
        self.pos += written;
    }
}

/// Write each sample of `block` into the next `W` bytes of `out`, as far as
/// both go.
#[inline]
fn store<const W: usize>(out: &mut [u8], block: &[u32], layout: impl Fn(u32) -> [u8; W]) {
    for (dst, &v) in out.as_chunks_mut::<W>().0.iter_mut().zip(block) {
        *dst = layout(v);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn written(width: usize, msb: bool, f: impl FnOnce(&mut ByteSink<'_>)) -> Vec<u8> {
        let mut out = vec![0xEE; 12];
        let mut sink = ByteSink::new(&mut out, width, msb);
        f(&mut sink);
        out
    }

    #[test]
    fn every_layout_matches_libaecs_put_functions() {
        let v = 0x1234_5678;
        let cases: [(usize, bool, &[u8]); 7] = [
            (1, false, &[0x78]),
            (2, true, &[0x56, 0x78]),
            (2, false, &[0x78, 0x56]),
            (3, true, &[0x34, 0x56, 0x78]),
            (3, false, &[0x78, 0x56, 0x34]),
            (4, true, &[0x12, 0x34, 0x56, 0x78]),
            (4, false, &[0x78, 0x56, 0x34, 0x12]),
        ];
        for (width, msb, want) in cases {
            let out = written(width, msb, |s| s.samples(&[v, v]));
            assert_eq!(&out[..width], want, "samples, width {width} msb {msb}");
            assert_eq!(&out[width..2 * width], want);
            assert_eq!(out[2 * width], 0xEE, "wrote past two samples");

            let out = written(width, msb, |s| s.repeat(v, 2));
            assert_eq!(&out[..width], want, "repeat, width {width} msb {msb}");
            assert_eq!(&out[width..2 * width], want);
            assert_eq!(out[2 * width], 0xEE);
        }
    }

    #[test]
    fn samples_and_runs_follow_each_other() {
        let out = written(2, true, |s| {
            s.samples(&[1]);
            s.repeat(2, 2);
            s.samples(&[3]);
        });
        assert_eq!(&out[..8], &[0, 1, 0, 2, 0, 2, 0, 3]);
    }

    #[test]
    fn more_than_the_buffer_holds_is_dropped_not_a_panic() {
        let out = written(4, false, |s| {
            s.samples(&[1, 2, 3, 4]);
            s.repeat(9, usize::MAX);
            s.samples(&[5]);
        });
        assert_eq!(out.len(), 12);
        assert_eq!(&out[8..], &[3, 0, 0, 0]);
        let out = written(1, false, |s| s.repeat(7, 100));
        assert_eq!(out, vec![7; 12]);
    }
}
