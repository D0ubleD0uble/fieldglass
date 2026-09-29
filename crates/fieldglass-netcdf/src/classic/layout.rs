//! Length and offset arithmetic of the NetCDF classic data section (CDF-1,
//! CDF-2 and CDF-5): where each variable's values are in the file.
//!
//! Every number here comes from the header, which is untrusted, so every
//! product and sum is checked, and an overflow is `None` rather than a wrapped
//! offset that would read the wrong bytes.
//!
//! The rules are the Unidata "NetCDF Classic and 64-bit Offset Format"
//! specification's, which the CDF-5 specification repeats:
//!
//! - a variable holds the product of its dimension lengths in elements;
//! - a fixed-size variable is one contiguous slab at its `begin`;
//! - the data of the record variables is interleaved: record `r` of a record
//!   variable starts at `begin + r * recsize`, where `recsize` is the sum of
//!   every record variable's per-record slab, each padded to the next multiple
//!   of 4 bytes;
//! - except that when there is exactly one record variable, no padding is used
//!   between its record slabs. The header's `vsize` for it is still the padded
//!   size ("writers should store vsize as if padding were included"), so
//!   "readers should ignore vsize and assume no padding". The `vsize` field is
//!   redundant in general, and in CDF-1 and CDF-2 it holds `2^32 - 1` for a
//!   variable too large for it, so the sizes here are computed from the shape
//!   and the type, never read from `vsize`.
//!
//! The specification's sentence names the types `char`, `byte` and `short`,
//! the only ones narrower than 4 bytes when it was written. A slab of 4- or
//! 8-byte elements is already a whole number of 4-byte words, so leaving the
//! padding off changes nothing for those. CDF-5 added `ubyte` and `ushort`,
//! and libnetcdf packs a lone record variable of those the same way; the
//! fixtures in `tests/fixtures/record_single_ubyte_cdf5.nc` record that.
//!
//! "Exactly one record variable" is counted as the specification says, by
//! variables. libnetcdf instead tests whether `recsize` equals the first
//! record variable's padded slab, which also holds when every other record
//! variable has a zero-size slab. A valid file has none: a record slab is the
//! product of the dimensions after the unlimited one, a dimension of length 0
//! is the unlimited one, and a file has only one. So the two agree on every
//! valid file.
//!
//! # Verified kernel
//!
//! This file is compiled twice. This crate compiles it as ordinary Rust; the
//! verification crate `crates/fieldglass-verify` includes the same file with
//! `#[path]` and proves it with Verus. The proofs are the
//! `cfg_attr(verus_keep_ghost, ...)` attributes, the `verus_keep_ghost`
//! `proof!` statements, and the `verus_keep_ghost` items at the bottom, which a
//! normal build never sees. What is proved is listed in
//! `docs/verification.md`: no input overflows or panics, and every result is
//! exactly the specification's formula, or `None` exactly when that formula's
//! value does not fit a `u64`.
//!
//! The file names no other item of either crate, and its docs use plain
//! backticks rather than intra-doc links, which would resolve in only one of
//! the two. `tools/check_verified_kernels.py` fails if the verification crate
//! stops including it.

#[cfg(verus_keep_ghost)]
use vstd::prelude::*;

/// The number of elements of a variable of this shape: the product of its
/// dimension lengths, 1 for a scalar.
///
/// The product is exact: a zero-length dimension makes it 0 even when the
/// other lengths multiplied together would overflow. `None` means the product
/// does not fit a `u64`.
#[cfg_attr(verus_keep_ghost, verus_spec(r =>
    ensures
        r matches Some(n) ==> n == product(shape@),
        r is None ==> product(shape@) > u64::MAX,
))]
pub(crate) fn element_count(shape: &[u64]) -> Option<u64> {
    let mut i = 0;
    #[cfg_attr(verus_keep_ghost, verus_spec(
        invariant
            0 <= i <= shape@.len(),
            forall|j: int| 0 <= j < i ==> shape@[j] != 0,
        decreases shape@.len() - i,
    ))]
    while i < shape.len() {
        if shape[i] == 0 {
            #[cfg(verus_keep_ghost)]
            proof! { lemma_zero_factor(shape@, i as int, shape@.len() as int); }
            return Some(0);
        }
        i += 1;
    }
    let mut n: u64 = 1;
    let mut i = 0;
    #[cfg_attr(verus_keep_ghost, verus_spec(
        invariant
            0 <= i <= shape@.len(),
            forall|j: int| 0 <= j < shape@.len() ==> shape@[j] != 0,
            n == prefix_product(shape@, i as int),
        decreases shape@.len() - i,
    ))]
    while i < shape.len() {
        #[cfg(verus_keep_ghost)]
        proof! {
            lemma_prefix_product_mono(shape@, i as int + 1, shape@.len() as int);
        }
        n = n.checked_mul(shape[i])?;
        i += 1;
    }
    Some(n)
}

/// The bytes a slab of `count` elements of `element_size` bytes occupies, or
/// `None` when that does not fit a `u64`.
#[cfg_attr(verus_keep_ghost, verus_spec(r =>
    ensures
        r matches Some(b) ==> b == count * element_size,
        r is None ==> count * element_size > u64::MAX,
))]
pub(crate) fn slab_bytes(count: u64, element_size: u64) -> Option<u64> {
    count.checked_mul(element_size)
}

/// `n` rounded up to the next multiple of 4, or `None` when that does not fit
/// a `u64`.
#[cfg_attr(verus_keep_ghost, verus_spec(r =>
    ensures
        r matches Some(p) ==> p == padded(n as int),
        r is None ==> padded(n as int) > u64::MAX,
))]
fn padded_to_4(n: u64) -> Option<u64> {
    let rem = n % 4;
    if rem == 0 {
        Some(n)
    } else {
        n.checked_add(4 - rem)
    }
}

/// The distance in bytes from one record to the next: `recsize`.
///
/// `slabs` holds the bytes of one record's slab of every record variable, in
/// any order, `char` variables included, computed from their shapes rather
/// than read from `vsize` (see the module docs). With exactly one record
/// variable it is that variable's slab, unpadded; otherwise it is the sum of
/// the slabs, each padded to a multiple of 4. `None` means that sum does not
/// fit a `u64`.
#[cfg_attr(verus_keep_ghost, verus_spec(r =>
    ensures
        r matches Some(s) ==> s == record_stride(slabs@),
        r is None ==> record_stride(slabs@) > u64::MAX,
))]
pub(crate) fn record_size(slabs: &[u64]) -> Option<u64> {
    if slabs.len() == 1 {
        return Some(slabs[0]);
    }
    let mut total: u64 = 0;
    let mut i = 0;
    #[cfg_attr(verus_keep_ghost, verus_spec(
        invariant
            0 <= i <= slabs@.len(),
            slabs@.len() != 1,
            total == padded_sum(slabs@, i as int),
        decreases slabs@.len() - i,
    ))]
    while i < slabs.len() {
        #[cfg(verus_keep_ghost)]
        proof! {
            lemma_padded_sum_mono(slabs@, i as int + 1, slabs@.len() as int);
        }
        let slab = padded_to_4(slabs[i])?;
        total = total.checked_add(slab)?;
        i += 1;
    }
    Some(total)
}

/// Where each of `records` slabs of `len` bytes is: slab `r` starts at
/// `begin + r * stride`, and is returned as `(start, len)`.
///
/// A fixed-size variable is one slab (`records == 1`); a record variable is
/// one slab per record, `stride` being `recsize`. `None` means some slab's end,
/// `begin + r * stride + len`, does not fit a `u64`, so every range returned
/// has an end that does.
#[cfg_attr(verus_keep_ghost, verus_spec(r =>
    ensures
        r matches Some(v) ==> v@.len() == records,
        r matches Some(v) ==> forall|k: int|
            0 <= k < records ==> (#[trigger] v@[k]).0 == slab_start(begin, stride, k)
                && v@[k].1 == len && slab_start(begin, stride, k) + len <= u64::MAX,
        r is None ==> exists|k: int|
            0 <= k < records && #[trigger] slab_start(begin, stride, k) + len > u64::MAX,
))]
pub(crate) fn slab_ranges(
    begin: u64,
    stride: u64,
    records: usize,
    len: u64,
) -> Option<Vec<(u64, u64)>> {
    let mut out: Vec<(u64, u64)> = Vec::with_capacity(records);
    let mut rec = 0;
    #[cfg_attr(verus_keep_ghost, verus_spec(
        invariant
            0 <= rec <= records,
            out@.len() == rec,
            forall|k: int|
                0 <= k < rec ==> (#[trigger] out@[k]).0 == slab_start(begin, stride, k)
                    && out@[k].1 == len && slab_start(begin, stride, k) + len <= u64::MAX,
        decreases records - rec,
    ))]
    while rec < records {
        let start = slab_start_checked(begin, stride, rec)?;
        // Only the check is wanted: the range keeps its length, not its end.
        start.checked_add(len)?;
        out.push((start, len));
        rec += 1;
    }
    Some(out)
}

/// Whether every one of those slabs lies inside a file of `size` bytes:
/// `begin + r * stride + len <= size` for every `r < records`.
///
/// The slabs start in increasing order, so it is enough to check the last
/// one, which is what makes this cheap for a variable of many records.
#[cfg_attr(verus_keep_ghost, verus_spec(r =>
    ensures
        r == forall|k: int|
            0 <= k < records ==> #[trigger] slab_start(begin, stride, k) + len <= size,
))]
pub(crate) fn slabs_within(begin: u64, stride: u64, records: usize, len: u64, size: u64) -> bool {
    if records == 0 {
        return true;
    }
    #[cfg(verus_keep_ghost)]
    proof! {
        lemma_last_slab_ends_last(begin, stride, records as int, len);
    }
    let start = match slab_start_checked(begin, stride, records - 1) {
        Some(start) => start,
        None => return false,
    };
    match start.checked_add(len) {
        Some(end) => end <= size,
        None => false,
    }
}

/// Where slab `r` starts, `begin + r * stride`, or `None` when that does not
/// fit a `u64`.
#[cfg_attr(verus_keep_ghost, verus_spec(out =>
    ensures
        out matches Some(s) ==> s == slab_start(begin, stride, r as int),
        out is None ==> slab_start(begin, stride, r as int) > u64::MAX,
))]
fn slab_start_checked(begin: u64, stride: u64, r: usize) -> Option<u64> {
    #[cfg(verus_keep_ghost)]
    proof! { lemma_slab_start_nonneg(begin, stride, r as int); }
    let offset = (r as u64).checked_mul(stride)?;
    begin.checked_add(offset)
}

#[cfg(verus_keep_ghost)]
verus! {

/// The product of the first `i` entries of `s`.
pub open spec fn prefix_product(s: Seq<u64>, i: int) -> int
    decreases i,
{
    if i <= 0 {
        1
    } else {
        prefix_product(s, i - 1) * s[i - 1] as int
    }
}

/// The product of every entry of `s`: a variable's element count.
pub open spec fn product(s: Seq<u64>) -> int {
    prefix_product(s, s.len() as int)
}

/// `n` rounded up to the next multiple of 4.
pub open spec fn padded(n: int) -> int {
    if n % 4 == 0 {
        n
    } else {
        n + (4 - n % 4)
    }
}

/// The sum of the first `i` slabs, each padded to a multiple of 4.
pub open spec fn padded_sum(s: Seq<u64>, i: int) -> int
    decreases i,
{
    if i <= 0 {
        0
    } else {
        padded_sum(s, i - 1) + padded(s[i - 1] as int)
    }
}

/// `recsize`: a lone record variable's slab as it is, otherwise the padded
/// sum of every record variable's slab.
pub open spec fn record_stride(s: Seq<u64>) -> int {
    if s.len() == 1 {
        s[0] as int
    } else {
        padded_sum(s, s.len() as int)
    }
}

/// Where slab `k` starts: `begin + k * stride`.
pub open spec fn slab_start(begin: u64, stride: u64, k: int) -> int {
    begin as int + k * stride as int
}

/// A zero entry below `n` makes the product of the first `n` entries zero.
pub proof fn lemma_zero_factor(s: Seq<u64>, z: int, n: int)
    requires
        0 <= z < n <= s.len(),
        s[z] == 0,
    ensures
        prefix_product(s, n) == 0,
    decreases n,
{
    if n - 1 > z {
        lemma_zero_factor(s, z, n - 1);
        assert(prefix_product(s, n) == prefix_product(s, n - 1) * s[n - 1] as int);
    } else {
        assert(prefix_product(s, n) == prefix_product(s, n - 1) * 0);
    }
}

/// With no zero entry, the product of the first `h` entries is positive and
/// no more than the product of the first `g`, for `h <= g`.
pub proof fn lemma_prefix_product_mono(s: Seq<u64>, h: int, g: int)
    requires
        0 <= h <= g <= s.len(),
        forall|j: int| 0 <= j < s.len() ==> s[j] != 0,
    ensures
        1 <= prefix_product(s, h) <= prefix_product(s, g),
    decreases g,
{
    if g == 0 {
    } else if h == g {
        lemma_prefix_product_mono(s, h - 1, g - 1);
        let p = prefix_product(s, g - 1);
        let x = s[g - 1] as int;
        assert(p * x >= 1) by (nonlinear_arith)
            requires
                p >= 1,
                x >= 1,
        ;
    } else {
        lemma_prefix_product_mono(s, h, g - 1);
        let p = prefix_product(s, g - 1);
        let x = s[g - 1] as int;
        assert(p <= p * x) by (nonlinear_arith)
            requires
                p >= 1,
                x >= 1,
        ;
    }
}

/// A padded sum is not negative and does not decrease.
pub proof fn lemma_padded_sum_mono(s: Seq<u64>, h: int, g: int)
    requires
        0 <= h <= g <= s.len(),
    ensures
        0 <= padded_sum(s, h) <= padded_sum(s, g),
    decreases g,
{
    if g > 0 {
        if h == g {
            lemma_padded_sum_mono(s, h - 1, g - 1);
        } else {
            lemma_padded_sum_mono(s, h, g - 1);
        }
    }
}

/// Every slab starts at or after `begin`.
pub proof fn lemma_slab_start_nonneg(begin: u64, stride: u64, k: int)
    requires
        0 <= k,
    ensures
        slab_start(begin, stride, k) >= begin,
        forall|j: int| 0 <= j <= k ==> #[trigger] slab_start(begin, stride, j) <= slab_start(begin, stride, k),
{
    assert forall|j: int| 0 <= j <= k implies #[trigger] slab_start(begin, stride, j) <= slab_start(
        begin,
        stride,
        k,
    ) by {
        assert(j * stride as int <= k * stride as int) by (nonlinear_arith)
            requires
                0 <= j <= k,
                stride >= 0,
        ;
    }
    assert(k * stride as int >= 0) by (nonlinear_arith)
        requires
            k >= 0,
            stride >= 0,
    ;
}

/// The last of `records` slabs ends last.
pub proof fn lemma_last_slab_ends_last(begin: u64, stride: u64, records: int, len: u64)
    requires
        records >= 1,
    ensures
        forall|k: int|
            0 <= k < records ==> #[trigger] slab_start(begin, stride, k) <= slab_start(
                begin,
                stride,
                records - 1,
            ),
{
    lemma_slab_start_nonneg(begin, stride, records - 1);
}

/// A slab of whole 4- or 8-byte elements needs no padding, so a lone record
/// variable of `int`, `float`, `double` or a CDF-5 64-bit type has the same
/// stride whether or not the lone-variable rule applies: the rule changes
/// nothing for a type the specification's sentence does not name.
pub proof fn lemma_whole_words_need_no_padding(count: int, element_size: int)
    requires
        count >= 0,
        element_size == 4 || element_size == 8,
    ensures
        padded(count * element_size) == count * element_size,
{
    assert((count * element_size) % 4 == 0) by (nonlinear_arith)
        requires
            count >= 0,
            element_size == 4 || element_size == 8,
    ;
}

} // verus!
