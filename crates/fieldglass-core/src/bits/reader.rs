//! The MSB-first bit reader every packed-integer decoder reads through.
//!
//! # Verified kernel
//!
//! This file is compiled twice. `fieldglass-core` compiles it as ordinary Rust,
//! as the private module `bits::reader`, and re-exports `BitReader` from
//! `bits`. The verification crate `crates/fieldglass-verify` includes the same
//! file with `#[path]` and proves it with Verus. The proofs are the
//! `cfg_attr(verus_keep_ghost, ...)` attributes and the `verus_keep_ghost`
//! items at the bottom, which a normal build never sees. What is proved is
//! listed in `docs/verification.md`.
//!
//! The model the proofs state everything in is here too: `msb_bits`, the
//! `width` bits of a byte string starting at a bit offset, read MSB-first. Every
//! other kernel that reads packed bits states its result with it.
//!
//! The file names only `crate::FieldglassError`, which both crates provide, and
//! its docs use plain backticks rather than intra-doc links.
//! `tools/check_verified_kernels.py` fails if the verification crate stops
//! including it.

use crate::FieldglassError;
#[cfg(verus_keep_ghost)]
use vstd::arithmetic::div_mod::{lemma_div_denominator, lemma_mod_breakdown};
#[cfg(verus_keep_ghost)]
use vstd::arithmetic::power2::{
    lemma_pow2_adds, lemma_pow2_pos, lemma_pow2_strictly_increases, lemma_pow2_unfold, lemma2_to64,
    pow2,
};
#[cfg(verus_keep_ghost)]
use vstd::bits::{
    lemma_low_bits_mask_values, lemma_u8_low_bits_mask_is_mod, lemma_u8_shr_is_div,
    lemma_u16_shl_is_mul, lemma_u64_shl_is_mul, low_bits_mask,
};
#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

/// MSB-first bit reader for packed integer streams up to 32 bits per value.
/// Used by GRIB BDS / DRS decoders and any future format that packs values
/// at non-byte-aligned widths.
// `Clone` but deliberately not `Copy` (#556), though both fields are `Copy`
// and it would compile. A bit cursor that copies implicitly reads from the
// copy and leaves the original's position behind, at a call site that looks
// like it advanced it — the reason `std::slice::Iter` is `Clone` and not
// `Copy` either. Saving a position is then an explicit `.clone()`.
//
// `verus_verify` makes the type visible to the proofs, which give it a type
// invariant: the cursor rounded up to the next octet fits a `usize`.
// `external_derive` leaves the derived `Debug` and `Clone` to Rust: no kernel
// calls them, and Verus cannot yet specify a derived `Clone` of a type that is
// not `Copy`.
#[cfg_attr(verus_keep_ghost, verus_verify(external_derive))]
#[derive(Debug, Clone)]
pub struct BitReader<'a> {
    bytes: &'a [u8],
    bit_offset: usize,
}

impl<'a> BitReader<'a> {
    /// A reader positioned at the first bit of `bytes`.
    // The `allow` is the marker Verus's `verus_verify` puts on the methods of
    // an impl it annotates, telling `verus_spec` that `new` is an associated
    // function; without it the spec of a function with no `self` names `new`
    // as a free function and fails to compile. `verus_spec` removes it again.
    #[cfg_attr(verus_keep_ghost, allow(unused, verus_impl_method_marker))]
    #[cfg_attr(verus_keep_ghost, verus_spec(r =>
        ensures
            reader_bytes(r) == bytes@,
            reader_pos(r) == 0,
    ))]
    pub fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            bit_offset: 0,
        }
    }

    /// Advance the cursor to the next byte boundary, discarding any unused
    /// bits in the current byte. A no-op when already byte-aligned. GRIB2
    /// complex packing (§7) stores the group reference / width / length / data
    /// sub-blocks each starting on an octet boundary, so decoders pad to the
    /// next byte between them.
    ///
    /// Proved: the cursor becomes the next multiple of 8 at or after it, and
    /// the addition cannot overflow.
    #[cfg_attr(verus_keep_ghost, verus_spec(
        ensures
            reader_bytes(*final(self)) == reader_bytes(*old(self)),
            reader_pos(*final(self)) == align8(reader_pos(*old(self))),
    ))]
    pub fn align_to_byte(&mut self) {
        #[cfg(verus_keep_ghost)]
        proof! { use_type_invariant(&*self); }
        let rem = self.bit_offset % 8;
        if rem != 0 {
            self.bit_offset += 8 - rem;
        }
    }

    /// Read the next `n` bits (MSB-first) as an unsigned integer.
    ///
    /// `n` must be in `0..=32`: the value is returned as a `u32`, so a wider
    /// request can't be represented and is rejected. Without this guard a
    /// request for `n > 32` would accumulate into the internal `u64` and then
    /// silently truncate its top bits on the `as u32` return. Callers bound the
    /// stored field width to 32; enforcing it here makes the contract explicit
    /// and turns a would-be silent wrong result into a clean error on malformed
    /// input (e.g. a per-group residual width read from an untrusted GRIB
    /// stream).
    ///
    /// Proved: `Ok` exactly when `n ≤ 32` and either `n` is 0 or the `n` bits
    /// lie inside the buffer; the value is those `n` bits read MSB-first; the
    /// cursor moves by `n` on `Ok` and not at all on `Err`.
    #[cfg_attr(verus_keep_ghost, verus_spec(out =>
        ensures
            reader_bytes(*final(self)) == reader_bytes(*old(self)),
            out is Ok <==> read_fits(
                reader_bytes(*old(self)).len() as int,
                reader_pos(*old(self)),
                n as int,
            ),
            out matches Ok(v) ==> v as nat == msb_bits(
                reader_bytes(*old(self)),
                reader_pos(*old(self)),
                n as int,
            ),
            out is Ok ==> reader_pos(*final(self)) == reader_pos(*old(self)) + n,
            out is Err ==> reader_pos(*final(self)) == reader_pos(*old(self)),
    ))]
    pub fn read_bits(&mut self, n: u8) -> Result<u32, FieldglassError> {
        if n == 0 {
            return Ok(0);
        }
        if n > 32 {
            return Err(too_many_bits(n));
        }
        // checked: bit_offset near usize::MAX would wrap past the bounds check.
        let end_bit = match self.bit_offset.checked_add(n as usize) {
            Some(end_bit) => end_bit,
            None => return Err(offset_overflow()),
        };
        let total_bits = match self.bytes.len().checked_mul(8) {
            Some(total_bits) => total_bits,
            None => return Err(length_overflow()),
        };
        if end_bit > total_bits {
            return Err(exhausted(self.bit_offset, n));
        }

        let mut value: u64 = 0;
        let mut bits_collected = 0u8;
        let mut bit = self.bit_offset;
        #[cfg(verus_keep_ghost)]
        proof! { lemma2_to64(); }
        #[cfg_attr(verus_keep_ghost, verus_spec(
            invariant
                1 <= n <= 32,
                bits_collected <= n,
                bit == self.bit_offset + bits_collected,
                end_bit == self.bit_offset + n,
                end_bit <= self.bytes@.len() * 8,
                value as nat == msb_bits(self.bytes@, self.bit_offset as int, bits_collected as int),
                (value as nat) < pow2(bits_collected as nat),
            decreases n - bits_collected,
        ))]
        while bits_collected < n {
            let byte_idx = bit / 8;
            let bit_in_byte = bit % 8;
            let take = (8 - bit_in_byte).min((n - bits_collected) as usize) as u8;
            let shift = 8 - bit_in_byte - take as usize;
            #[cfg(verus_keep_ghost)]
            proof! {
                lemma_mask(take);
                lemma_chunk(self.bytes@, bit as int, take as int);
                lemma_msb_bits_concat(self.bytes@, self.bit_offset as int, bits_collected as int, take as int);
            }
            let mask = ((1u16 << take) - 1) as u8;
            let chunk = (self.bytes[byte_idx] >> shift) & mask;
            #[cfg(verus_keep_ghost)]
            proof! {
                lemma_shift_mask(self.bytes@[byte_idx as int], shift as u8, take as nat);
                lemma_accumulate(value, bits_collected as nat, take, chunk as u64);
            }
            value = (value << take) | chunk as u64;
            bits_collected += take;
            bit += take as usize;
        }
        #[cfg(verus_keep_ghost)]
        proof! {
            lemma_msb_bits_bound(self.bytes@, self.bit_offset as int, n as int);
            lemma2_to64();
            lemma_align8_within(end_bit as int, self.bytes@.len() as int);
        }
        self.bit_offset = end_bit;
        Ok(value as u32)
    }
}

// Error constructors. `format!` has no Verus specification, so each message
// is built outside the proof; a proof only needs to know an error is returned.

#[cfg_attr(verus_keep_ghost, verus_verify(external_body))]
fn too_many_bits(n: u8) -> FieldglassError {
    FieldglassError::Parse(format!(
        "bit reader asked for {n} bits, but read_bits returns a u32 (max 32)"
    ))
}

#[cfg_attr(verus_keep_ghost, verus_verify(external_body))]
fn offset_overflow() -> FieldglassError {
    FieldglassError::Parse("bit reader offset overflow".into())
}

#[cfg_attr(verus_keep_ghost, verus_verify(external_body))]
fn length_overflow() -> FieldglassError {
    FieldglassError::Parse("bit reader length overflow".into())
}

#[cfg_attr(verus_keep_ghost, verus_verify(external_body))]
fn exhausted(bit_offset: usize, n: u8) -> FieldglassError {
    FieldglassError::Parse(format!(
        "bit reader exhausted at offset {bit_offset} reading {n} bits"
    ))
}

#[cfg(verus_keep_ghost)]
verus! {

impl<'a> BitReader<'a> {
    /// The cursor, rounded up to the next octet, fits a `usize`, so
    /// `align_to_byte` cannot overflow. `new` starts at 0, and `read_bits` only
    /// ever moves the cursor to a bit inside the buffer, whose bit length is a
    /// multiple of 8 that fits a `usize`.
    #[verifier::type_invariant]
    spec fn cursor_can_align(self) -> bool {
        align8(self.bit_offset as int) <= usize::MAX
    }
}

/// The bytes a reader reads from.
pub closed spec fn reader_bytes<'a>(r: BitReader<'a>) -> Seq<u8> {
    r.bytes@
}

/// The reader's cursor, in bits from the first bit of `reader_bytes`.
pub closed spec fn reader_pos<'a>(r: BitReader<'a>) -> int {
    r.bit_offset as int
}

/// Bit `i` of `bytes`, counting from the most significant bit of byte 0.
pub open spec fn bit(bytes: Seq<u8>, i: int) -> nat {
    ((bytes[i / 8] as nat) / pow2((7 - i % 8) as nat)) % 2
}

/// The `width` bits of `bytes` starting at bit `start`, read MSB-first as an
/// unsigned integer.
pub open spec fn msb_bits(bytes: Seq<u8>, start: int, width: int) -> nat
    decreases width,
{
    if width <= 0 {
        0
    } else {
        2 * msb_bits(bytes, start, width - 1) + bit(bytes, start + width - 1)
    }
}

/// When `read_bits(n)` succeeds from bit `pos` of a `len`-byte buffer: `n` fits
/// a `u32`, and either nothing is read or the read ends inside the buffer
/// (whose bit length the reader computes in a `usize`).
pub open spec fn read_fits(len: int, pos: int, n: int) -> bool {
    n <= 32 && (n == 0 || (pos + n <= len * 8 && len * 8 <= usize::MAX))
}

/// `pos` rounded up to the next multiple of 8: the start of the next octet.
pub open spec fn align8(pos: int) -> int {
    if pos % 8 == 0 {
        pos
    } else {
        pos + (8 - pos % 8)
    }
}

/// A cursor inside a buffer whose bit length fits a `usize` rounds up to
/// an octet that still does.
proof fn lemma_align8_within(pos: int, len: int)
    requires
        0 <= pos <= len * 8,
        len * 8 <= usize::MAX,
    ensures
        align8(pos) <= len * 8,
{
    if pos % 8 != 0 {
        assert(pos / 8 < len);
        assert(align8(pos) == 8 * (pos / 8 + 1));
    }
}

/// `msb_bits` of `width` bits is below `2^width`.
proof fn lemma_msb_bits_bound(bytes: Seq<u8>, start: int, width: int)
    requires
        0 <= width,
    ensures
        msb_bits(bytes, start, width) < pow2(width as nat),
    decreases width,
{
    if width == 0 {
        lemma_pow2_pos(0);
        lemma2_to64();
    } else {
        lemma_msb_bits_bound(bytes, start, width - 1);
        lemma_pow2_unfold(width as nat);
    }
}

/// Reading `a` bits and then `b` more is reading `a + b` bits: the first
/// part shifted left by `b`, plus the second.
proof fn lemma_msb_bits_concat(bytes: Seq<u8>, start: int, a: int, b: int)
    requires
        0 <= a,
        0 <= b,
    ensures
        msb_bits(bytes, start, a + b) == msb_bits(bytes, start, a) * pow2(b as nat) + msb_bits(
            bytes,
            start + a,
            b,
        ),
    decreases b,
{
    if b == 0 {
        lemma2_to64();
        assert(msb_bits(bytes, start + a, 0) == 0);
        assert(msb_bits(bytes, start, a) * 1 == msb_bits(bytes, start, a));
    } else {
        lemma_msb_bits_concat(bytes, start, a, b - 1);
        lemma_pow2_unfold(b as nat);
        let hi = msb_bits(bytes, start, a);
        let lo = msb_bits(bytes, start + a, b - 1);
        let last = bit(bytes, start + a + b - 1);
        let p = pow2((b - 1) as nat);
        assert(msb_bits(bytes, start, a + b) == 2 * msb_bits(bytes, start, a + b - 1) + last);
        assert(msb_bits(bytes, start, a + b - 1) == hi * p + lo);
        assert(msb_bits(bytes, start + a, b) == 2 * lo + last);
        assert(2 * (hi * p + lo) + last == hi * (2 * p) + (2 * lo + last)) by (nonlinear_arith);
        assert(pow2(b as nat) == 2 * p);
    }
}

/// `take` bits that stay inside one byte, starting at bit `start`, are that
/// byte shifted right past the bits after them and cut to `take` bits.
proof fn lemma_chunk(bytes: Seq<u8>, start: int, take: int)
    requires
        0 <= start,
        0 <= take,
        start % 8 + take <= 8,
    ensures
        msb_bits(bytes, start, take) == (bytes[start / 8] as nat / pow2(
            (8 - start % 8 - take) as nat,
        )) % pow2(take as nat),
    decreases take,
{
    let x = bytes[start / 8] as nat;
    let s = (8 - start % 8 - take) as nat;
    lemma_pow2_pos(s);
    if take == 0 {
        lemma2_to64();
    } else {
        lemma_chunk(bytes, start, take - 1);
        let y = x / pow2(s);
        // The last of the `take` bits is in the same byte.
        let i = start + take - 1;
        assert(i / 8 == start / 8 && i % 8 == start % 8 + take - 1);
        assert(bit(bytes, i) == y % 2);
        // Dropping one more bit is halving.
        lemma_pow2_unfold(s + 1);
        lemma_div_denominator(x as int, pow2(s) as int, 2);
        assert(x / pow2(s + 1) == y / 2) by (nonlinear_arith)
            requires
                pow2(s + 1) == pow2(s) * 2,
                y == x / pow2(s),
                x / pow2(s) / 2 == x / (pow2(s) * 2),
        ;
        lemma_pow2_unfold(take as nat);
        lemma_pow2_pos((take - 1) as nat);
        lemma_mod_breakdown(y as int, 2, pow2((take - 1) as nat) as int);
    }
}

/// The byte-level step of the read loop: shifting a byte right and masking
/// it is the division and remainder `lemma_chunk` states.
proof fn lemma_shift_mask(x: u8, shift: u8, take: nat)
    requires
        1 <= take <= 8,
        shift as nat + take <= 8,
    ensures
        ((x >> shift) & (low_bits_mask(take) as u8)) as nat == (x as nat / pow2(shift as nat)) % pow2(
            take,
        ),
        (x as nat / pow2(shift as nat)) % pow2(take) < pow2(take),
{
    lemma2_to64();
    lemma_pow2_pos(take);
    lemma_u8_shr_is_div(x, shift);
    let y = x >> shift;
    if take == 8 {
        lemma_low_bits_mask_values();
        assert(shift == 0);
        assert(y & 0xff == y) by (bit_vector);
        assert((y as nat) % 256 == y as nat);
    } else {
        lemma_u8_low_bits_mask_is_mod(y, take);
        lemma_pow2_strictly_increases(take, 8);
        assert((y % (pow2(take) as u8)) as nat == (y as nat) % pow2(take));
    }
}

/// The mask the read loop builds, `(1 << take) - 1` cut to a byte, is the
/// low `take` bits.
proof fn lemma_mask(take: u8)
    requires
        1 <= take <= 8,
    ensures
        ((1u16 << take) - 1) as u8 == low_bits_mask(take as nat) as u8,
        (1u16 << take) >= 1,
{
    lemma2_to64();
    lemma_u16_shl_is_mul(1, take as u16);
    assert(low_bits_mask(take as nat) == pow2(take as nat) - 1) by {
        reveal(low_bits_mask);
    }
}

/// The accumulator step: shifting a value below `2^c` left by `t` and or-ing
/// in a chunk below `2^t` is `value · 2^t + chunk`, below `2^(c + t)`, with no
/// overflow while `c + t ≤ 32`.
proof fn lemma_accumulate(value: u64, c: nat, t: u8, chunk: u64)
    requires
        1 <= t <= 8,
        c + t <= 32,
        value < pow2(c),
        chunk < pow2(t as nat),
    ensures
        ((value << t) | chunk) as nat == value as nat * pow2(t as nat) + chunk as nat,
        (value as nat * pow2(t as nat) + chunk as nat) < pow2(c + t as nat),
{
    let tn = t as nat;
    lemma2_to64();
    lemma_pow2_adds(c, tn);
    lemma_pow2_strictly_increases(c + tn, 64);
    assert(value * pow2(tn) + pow2(tn) <= pow2(c + tn)) by (nonlinear_arith)
        requires
            value + 1 <= pow2(c),
            pow2(c) * pow2(tn) == pow2(c + tn),
    ;
    let tt = t as u64;
    lemma_u64_shl_is_mul(value, tt);
    lemma_u64_shl_is_mul(1, tt);
    let hi = value << tt;
    assert(hi == value * pow2(tn));
    assert(chunk < (1u64 << tt));
    assert((value << tt) & chunk == 0) by (bit_vector)
        requires
            tt < 64,
            chunk < (1u64 << tt),
    ;
    assert((value << tt) | chunk == add(value << tt, chunk)) by (bit_vector)
        requires
            (value << tt) & chunk == 0,
    ;
    assert((value << t) == (value << tt)) by (bit_vector)
        requires
            t <= 8,
            tt == t as u64,
    ;
}

} // verus!

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bit_reader_byte_aligned() {
        let mut r = BitReader::new(&[0xAB, 0xCD]);
        assert_eq!(r.read_bits(8).unwrap(), 0xAB);
        assert_eq!(r.read_bits(8).unwrap(), 0xCD);
    }

    #[test]
    fn bit_reader_unaligned() {
        // 0b1010_0101_1100_0011 — read as 3,5,8 bits MSB-first.
        let mut r = BitReader::new(&[0b1010_0101, 0b1100_0011]);
        assert_eq!(r.read_bits(3).unwrap(), 0b101);
        assert_eq!(r.read_bits(5).unwrap(), 0b00101);
        assert_eq!(r.read_bits(8).unwrap(), 0b1100_0011);
    }

    #[test]
    fn bit_reader_crosses_byte_boundary() {
        let mut r = BitReader::new(&[0xFF, 0x00]);
        assert_eq!(r.read_bits(12).unwrap(), 0xFF0);
    }

    #[test]
    fn bit_reader_align_to_byte() {
        let mut r = BitReader::new(&[0b1010_1111, 0b0011_0000]);
        assert_eq!(r.read_bits(3).unwrap(), 0b101);
        r.align_to_byte(); // skip the remaining 5 bits of byte 0
        assert_eq!(r.read_bits(4).unwrap(), 0b0011);
        // Already aligned here (4 bits into byte 1 is not, but align skips to byte 2).
        r.align_to_byte();
        assert!(r.read_bits(1).is_err(), "only 2 bytes of input");
    }

    #[test]
    fn bit_reader_align_is_noop_when_aligned() {
        let mut r = BitReader::new(&[0xAB, 0xCD]);
        assert_eq!(r.read_bits(8).unwrap(), 0xAB);
        r.align_to_byte(); // already on byte boundary — no-op
        assert_eq!(r.read_bits(8).unwrap(), 0xCD);
    }

    #[test]
    fn bit_reader_exhaustion() {
        let mut r = BitReader::new(&[0x00]);
        assert!(r.read_bits(9).is_err());
    }

    #[test]
    fn bit_reader_reads_full_32_bits() {
        // The boundary of the contract: 32 bits round-trips losslessly.
        let mut r = BitReader::new(&[0xFF, 0xFF, 0xFF, 0xFF]);
        assert_eq!(r.read_bits(32).unwrap(), 0xFFFF_FFFF);
        let mut r = BitReader::new(&[0x12, 0x34, 0x56, 0x78]);
        assert_eq!(r.read_bits(32).unwrap(), 0x1234_5678);
    }

    #[test]
    fn bit_reader_rejects_more_than_32_bits() {
        // n > 32 can't fit in the returned u32. It must error rather than read
        // the bits and silently truncate the top ones via `as u32`. The buffer
        // is deliberately long enough that the request would otherwise succeed.
        let bytes = [0xAAu8; 8];
        let mut r = BitReader::new(&bytes);
        assert!(r.read_bits(33).is_err());
        assert!(r.read_bits(40).is_err());
        // A rejected read must not advance the cursor.
        assert_eq!(r.bit_offset, 0);
        // A valid read right afterwards still works.
        assert_eq!(r.read_bits(8).unwrap(), 0xAA);
    }
}
