# Verification

Verification here has two halves, and each holds the code to something outside
itself rather than to its own history.

- **Correctness: the numbers are right.** Every decoder is checked against an
  outside oracle (eccodes, netCDF4, zarr-python) through committed fixtures and
  snapshots; the conformance suite holds every host to the same recorded
  answers; fuzz targets feed the readers hostile bytes; refactors are proved
  byte-identical before and after; and the decode kernel carries Verus proofs,
  described below.
- **Cost: producing them is close to the best anyone could.** Bytes read,
  allocations, peak heap, instructions and wasm memory per operation, each held
  to a stated bound: [performance.md](performance.md).

Both halves share one rule. A check that cannot fail proves nothing, and a gate
that quietly skips when a prerequisite is missing has measured nothing, so every
one of them fails loudly instead.

## Formal verification with Verus

*Bootstrapped 2026-08-23 ([#197](https://github.com/D0ubleD0uble/fieldglass/issues/197)).*

Fieldglass's third commitment is "be right, provably where it matters". The
decode kernel — the few hundred lines that turn untrusted bytes into numbers —
is where a malformed file causes wrong values, an overflow, or a panic, and
every GRIB value in the project passes through it. [Verus](https://github.com/verus-lang/verus)
verifies real Rust in place, as ghost code that carries no runtime cost, so
there is no port and no second source of truth.

### Running it

```sh
scripts/verify.sh              # verify
scripts/verify.sh --install    # fetch the pinned Verus first, then verify
```

That is the whole contract. The script pins the Verus release *and* the rustup
toolchain it needs, prefers that install over anything on `PATH`, and refuses to
report success unless Verus actually produced results.

To verify a single function while working on it, run Verus directly — the crate
is small enough that whole-crate verification takes well under a second once
`vstd` is cached:

```sh
cd crates/fieldglass-verify && cargo verus verify
```

### Why the verification crate stands apart

`crates/fieldglass-verify` is **not** a member of the root workspace. Like each
`fuzz/` crate it declares an empty `[workspace]` table and carries its own
`Cargo.lock`, so `cargo build`, `cargo test --workspace`, `cargo deny`, and the
six-target cross-compile of [ADR-0001](decisions/0001-grib2-compressed-packing-codecs.md)
never see Verus at all.

That isolation is a choice, not a necessity, and it is worth being precise about
why — because the obvious objection is wrong. Verus's own model is to depend on
`vstd` from crates.io and write `verus! { }` in ordinary source, and that model
*works*: measured on this repo's toolchain, a crate with `vstd` and a `verus!`
block builds under stock `rustc` in about 9 seconds and cross-compiles cleanly
to `x86_64-pc-windows-msvc` and `wasm32-unknown-unknown` with no C toolchain.
The macro really is transparent to a normal build.

The reason to isolate anyway is different: `fieldglass-core` is published to
crates.io, and `vstd` is a date-stamped pre-1.0 crate that tracks Verus's
release cadence. Giving a published crate that dependency puts every downstream
consumer on Verus's schedule, for a benefit none of them asked for. The proofs
of shipped code keep that property: they are in the shipped source, but `vstd`
is not in its dependency graph (see below).

The CI job asserts the isolation rather than trusting it: no `vstd`,
`verus_builtin`, or `verus_builtin_macros` may appear in the workspace
dependency graph.

### How a proof binds to shipped code

*Decided in [#199](https://github.com/D0ubleD0uble/fieldglass/issues/199).*

A proved function lives in a **kernel file**: an ordinary source file of the
crate that ships it, whose specs are written as
`#[cfg_attr(verus_keep_ghost, verus_spec(...))]` attributes. The file is
compiled twice:

- the shipped crate compiles it as plain Rust. `verus_keep_ghost` is never set
  for that build, so the attributes vanish, and the crate gains no dependency,
  no lockfile entry and no runtime cost;
- `crates/fieldglass-verify` includes the same file with
  `#[path = "..."] mod ...;` and Verus proves it there.

There is one copy of the code, so nothing can drift between a proof and the
function it is about. `vstd` appears in no published manifest and in no lockfile
but the verification crate's own. The workspace declares the cfg in
`[workspace.lints.rust]` (`unexpected_cfgs`) so rustc does not warn about it.
The cfg is only supported through `scripts/verify.sh` (cargo-verus): a build
that sets `--cfg verus_keep_ghost` globally, say through `RUSTFLAGS`, fails to
compile `fieldglass-core`, because under that cfg `scaling.rs` names
`crate::bits_model` and `vstd`, which only the verification crate provides.

The other layouts were measured before this one was chosen. A restated copy in
the verification crate needs a checker to keep the copy honest. `vstd` as a
dependency of the shipped crate, even gated on `cfg(verus_keep_ghost)`, still
lands in `Cargo.lock` and in the packaged manifest, so every downstream lockfile
and `cargo deny` run sees it.

A kernel file follows three rules, because two crates compile it:

- it names only items both crates provide. Today those are
  `crate::FieldglassError`, `crate::bits::BitReader`, and, under Verus only,
  `crate::bits_model`, the specifications and trusted statements below. The
  verification crate re-exports `FieldglassError` from `fieldglass-core`.
  `BitReader` is itself a kernel: core ships `bits/reader.rs` as the private
  module `bits::reader` and re-exports the type from `bits`, and the
  verification crate includes the same file and re-exports it from a `bits`
  module of its own, so every kernel's `crate::bits::BitReader` is the proved
  reader;
- its docs use plain backticks, not intra-doc links, which would resolve in only
  one of the two crates;
- ghost code in a function body, where a proof needs a hint, is a
  `#[cfg(verus_keep_ghost)] proof! { ... }` statement, which a plain build
  drops. A loop invariant is a `verus_spec(it => invariant ...)` attribute on
  the `for` statement; the verification crate enables `proc_macro_hygiene`
  for it, since Verus expands that attribute onto an expression.

A few things the pinned Verus does not accept in a kernel, each found by
writing one:

- `verus_spec` behind `cfg_attr` on an associated function without a
  receiver; write a free function instead (`red_scale`, not `Scaling::new`).
  Where the function has to stay associated, as `BitReader::new` does, add
  `#[cfg_attr(verus_keep_ghost, allow(unused, verus_impl_method_marker))]`
  to it. That is the marker Verus's own `verus_verify` puts on an impl's
  methods, which tells `verus_spec` the function is not a free one;
  `verus_verify` on the impl block would do the same but cannot see a
  `verus_spec` behind `cfg_attr` and adds a second one;
- a destructuring assignment such as `(a, b) = (b, a)`; assign one at a time;
- a `const` declared outside `verus!`; write the literal;
- `format!`, so a function that builds an error message is marked
  `#[cfg_attr(verus_keep_ghost, verus_verify(external_body))]`, and the proof
  takes its body on trust;
- a `let ghost` outside `verus!`. A ghost snapshot of a value mid-loop is
  `#[cfg(verus_keep_ghost)] proof_decl! { let ghost before = v@; }`.

A `while` loop takes its invariant and `decreases` the same way a `for` loop
does, as a `verus_spec(...)` attribute on the statement.

A type can carry an invariant the proofs keep. `BitReader`'s is that its
cursor, rounded up to the next octet, fits a `usize`. The struct is marked
`#[cfg_attr(verus_keep_ghost, verus_verify(external_derive))]`, the invariant
is a `#[verifier::type_invariant]` spec function in a second `impl` block
inside the file's `verus!` section, and a method that needs it starts with
`#[cfg(verus_keep_ghost)] proof! { use_type_invariant(&*self); }`. The
`external_derive` leaves derived impls to Rust: Verus checks a derived `Clone`
against the invariant and cannot specify it for a type that is not `Copy`.

Two habits the shuffle kernel needed. A loop invariant starts from nothing but
itself, so a fact the body relies on and that was known before the loop, such
as a slice's length fitting a `usize` or a bound a lemma proved, has to be
restated in the invariant of every loop that uses it, inner loops included.
And the pinned `vstd` specifies `Vec::with_capacity` and `extend_from_slice`
but not `<[T]>::to_vec`, so a kernel copies a slice with the first two.
Likewise it specifies `is_multiple_of` but not `div_ceil`, so the bitmap
kernel rounds up with the first (clippy rejects the hand-written forms).

Two things keep the arrangement from failing silently, and
`tools/check_verified_kernels.py` (pre-commit) checks both: every file under
`crates/` that carries `verus_spec` must be `#[path]`-included by the
verification crate, and must be listed in the `push` and `pull_request` path
filters of `.github/workflows/verify.yml`. Without the first, a deleted include
leaves specs that look like proofs and are never checked; without the second, an
edit to a kernel never runs Verus and the job stays green. Only a live include
counts: one that is commented out, or that sits on a `mod` item carrying a
`cfg` or nested in a block, is reported. The path filters must also cover the
core files a kernel's build in the verification crate reads (`bits.rs`,
`error.rs`, and core's `lib.rs` and `Cargo.toml`).

### What is proved, and what is trusted

**`crates/fieldglass-core/src/bits/reader.rs`**, the MSB-first bit reader that
every packed integer in GRIB1 and GRIB2 is read through. The kernels below
state their results with its model, `msb_bits(bytes, start, width)`: the
`width` bits of `bytes` from bit `start`, most significant first.

- `BitReader::new(bytes)` reads `bytes` from bit 0.
- `read_bits(n)` returns `Ok` exactly when `n ≤ 32` and either `n` is 0 or
  the `n` bits end inside the buffer (whose bit length the reader computes in
  a `usize`, so a slice longer than `usize::MAX / 8` bytes fails every
  non-empty read). On `Ok` the value is `msb_bits(bytes, pos, n)` and the
  cursor moves by `n`; on `Err` the cursor does not move. It has no
  precondition: no request or cursor position overflows, indexes out of
  bounds or panics.
- `align_to_byte()` moves the cursor to the next multiple of 8, or leaves it
  where it is if it is on one, and cannot overflow. That rests on the type
  invariant above, which `new` establishes and `read_bits` keeps: a
  successful read leaves the cursor inside a buffer whose bit length is a
  multiple of 8 that fits a `usize`.

These are exactly the statements the proofs below took on trust until #771,
and no proof that uses them changed.

Each claim was checked by breaking it: taking a byte's bits from the low end,
putting each new chunk above the bits already read (both LSB-first orders),
dropping the bounds check, loosening it by a byte, rejecting a read that ends
on the last bit, accepting up to 64 bits, moving the cursor on the exhaustion
error or before the `n > 32` rejection, not moving it on success, moving it
on a zero-bit read, starting `new` at bit 1, a mask one bit too wide, taking
one bit too many from a byte, aligning one bit short, aligning down, moving
an aligned cursor a byte on, dropping the type invariant from
`align_to_byte`, and weakening the invariant to `true` each make Verus reject
the proof. Taking one bit fewer from a byte than it could still reads the
right value, and Verus accepts it.

**`crates/fieldglass-core/src/scaling.rs`**, the GRIB `(R + X·2^E)·10^-D`
transform every GRIB1 and GRIB2 integer packing unpacks values with:

- `binary_factor(E)` is `powi(2, E)` and `decimal_factor(D)` is `powi(10, -D)`,
  and `red_scale` builds its `Scaling` from exactly those. This is the headline:
  it rules out `2^-E`, `10^D` and a swapped base, the bugs a test with `D = 0`
  never sees.
- `Scaling::apply(x)` is `(R + x·2^E)·10^-D`, evaluated in that order, and
  `Scaling::constant()` is `R·10^-D`.
- `unpack_simple(packed, width, scaling, count)` returns `Ok` exactly when the
  fields fit (none are read, or `width` is 0, or `width ≤ 32` and `packed` holds
  `count · width` bits); an `Ok` result has `count` values; and value `i` is
  `scaling.apply` of the `i`-th `width`-bit field of `packed`, read MSB-first.
  It has no precondition, so there is none for a caller to break.
  `unpack_simple_into`, which appends to a vector the caller already holds, is
  proved the same way and also keeps what the vector held.

Each claim was checked by breaking it: flipping either exponent's sign, swapping
the bases, swapping the factors in `apply` or in `red_scale`, scaling the
reference, reading one field too few or one bit too narrow, swallowing a read
error, overwriting what the appended-to vector held, dropping any clause of the
`Ok` condition, and removing the `u32 as f64` axiom each make Verus reject the
proof.

**`crates/fieldglass-core/src/spatial_diff.rs`**, the inverse spatial
differencing of GRIB1 second-order packing, the GRIB2 second-order templates
5.50001 and 5.50002, and GRIB2 complex packing with spatial differencing
(5.3). The recurrence is stated once, in `next_value_spec`, with the wrapping
operations themselves: order 1 is `d + g[i-1] + bias`, order 2
`d + 2·g[i-1] − g[i-2] + bias`, order 3 `d + 3·g[i-1] − 3·g[i-2] + g[i-3] + bias`,
each `+`, `−` and `·` wrapping. Over unbounded integers the claim would be
false whenever an intermediate overflows, and eccodes' answer is the wrapped
one.

- `apply_spd_inverse(x, order, bias)` returns `Ok` exactly when `order ≤ 3`,
  and leaves `x` untouched otherwise. On `Ok`, the first `order` slots (the
  seeds) are unchanged, and every later slot is the recurrence over its stored
  difference and the rebuilt values before it. Order 0 changes nothing.
- `apply_spd_inverse_skipping_missing(vals, seeds, bias)` returns `Ok`
  exactly when there are at most 3 seeds. On `Ok`, a point is present
  afterwards exactly when it was before, and the present values, read in
  order, are the seeds followed by the recurrence over each stored difference
  and the present values rebuilt before it. Missing points take no part, as
  in eccodes' `DataG22OrderPacking`.
- Neither has a precondition. No run length, order, seed count or pattern of
  missing points makes either index out of bounds, overflow or panic. That
  covers a run shorter than its order, including an empty one, which the code
  before the kernel did not: its order-1 branch read `x[0]` of an empty run.

eccodes writes the GRIB1 reconstruction with running accumulators and the
GRIB2 one in the value form above, and they are the same function in wrapping
arithmetic. The kernel uses the value form for both. That the switch changed
no GRIB1 output is checked at runtime rather than proved: a unit test compares
the old accumulator code with the kernel over thousands of runs, overflow
included, and every GRIB1 and GRIB2 fixture decodes as before.

Each claim was checked by breaking it: starting one slot before or after the
seeds, a plain `+` or `*` in place of a wrapping one, swapping the order-1 and
order-2 formulas, dropping the bias, dispatching on the wrong order, reading
`g[i-2]` for order 1, removing either order-0 guard, taking the wrong seed or
one seed too many, not shifting the history of present values, treating a
missing point as a zero difference, and rejecting order 3 each make Verus
reject the proof.

**`crates/fieldglass-core/src/shuffle.rs`**, the byte shuffle. It is the
transform of HDF5's shuffle filter, of Zarr's standalone `shuffle` filter and
of blosc's byte shuffle, so the NetCDF-4 and Zarr readers both call it. Read as
matrices, the stored bytes are an `element_size × count` matrix and the
elements its transpose.

- `unshuffle(data, element_size)` returns a buffer of `data`'s length in
  which byte `b` of element `e` is `data[b·count + e]`, for every whole
  element (`count = len / element_size`) and every `b < element_size`. The
  trailing `len % element_size` bytes are unchanged, and for an element size
  of 0 or 1 nothing moves. That tail rule is libhdf5's (`H5Z__filter_shuffle`)
  and c-blosc's (`unshuffle_generic_inline`).
- `shuffle(data, element_size)` is the transpose the other way, with the same
  tail rule.
- Neither has a precondition. No length or element size, including a length
  that is not a multiple of the element size, makes an index go out of bounds,
  an index computation overflow, or either function panic.
- Each undoes the other, and each specification determines its output
  completely, so the `ensures` is the function's whole behaviour. These are
  lemmas over the two specifications (`lemma_shuffle_undoes_unshuffle`,
  `lemma_unshuffle_undoes_shuffle`, `lemma_unshuffle_is_determined`,
  `lemma_shuffle_is_determined`).

Before the kernel, the NetCDF-4 reader returned a chunk that was not a whole
number of elements still shuffled, which is not what libhdf5 does. No
libhdf5-written chunk has that shape, since a chunk holds whole elements of
the dataset's type, so no fixture changed.

Each claim was checked by breaking it: transposing the wrong way, counting one
element too many, skipping the last byte plane, treating an element size of 2
as nothing to do, reading one byte further on, leaving a ragged buffer
untouched, swapping row and column in the write, and in `shuffle` writing to
the element position or striding elements by the count each make Verus reject
the proof. So do dropping the tail clause from the specification and stating
the same transpose for both directions. The proof trusts only `vstd`'s
specifications of `Vec` and slice indexing, and allocation succeeding.

**`crates/fieldglass-core/src/groups.rs`**, the group expansion of every grouped
GRIB packing: a group is a reference plus one offset per point at the group's
own width, and a zero-width group is a run of its reference. GRIB2 complex
packing (5.2, 5.3), GRIB2 second-order (5.50001, 5.50002), and GRIB1
second-order (the extended SPD layout and the classic `row_by_row` and
`general_grib1`) all call it.

- `expand_group_into(reader, width, len, reference, out)` returns `Ok` exactly
  when `width ≤ 32`, the reader holds the group's `len · width` bits and `out`
  has room for `len` more; then `out` keeps what it held and gains `len`
  values, the `k`-th being `reference` plus the `k`-th `width`-bit field (0 at
  width 0), and the reader has moved exactly `len · width` bits. The sum
  cannot overflow: `reference` and the field are both `u32`. The group is
  written in place after a `resize` rather than pushed, which the
  second-order decode's instruction count needs; `fill_group` and
  `fill_offsets`, the two private steps that do it, carry the same proof.
- `read_group_widths(reader, count, bits)`, the second-order width block:
  on `Ok`, width `g` is the `g`-th `bits`-bit field and at most 32. A width is
  checked whole before it is kept as a byte.
- `expand_groups_into(reader, widths, lengths, references, out)`, the
  second-order loop, appends exactly `lengths[0] + … + lengths[NG − 1]` values
  on `Ok`, every width is at most 32, and point `k` of group `g` sits at
  `lengths[0] + … + lengths[g − 1] + k` past what `out` held, with its offset
  read where the previous group's end.
- `expand_complex_groups(reader, layout, present_count)`, the whole of complex
  packing's §7 group structure. On `Ok`: `1 ≤ NG ≤ present_count`; the group
  lengths, with the last one overridden by `length_last`, sum to exactly
  `present_count`, and the result has that many values; every group width
  (`width_reference` plus the stored width) is at most 32, so no read is
  truncated; and point `k` of group `g` is at `len[0] + … + len[g − 1] + k`
  and equals the value the layout defines: the four blocks each start on the
  next octet, the point is `None` when its offset (or, in a zero-width group,
  the reference) is all ones at its width, or all ones minus one under
  management 2, and `group_ref[g] + X` otherwise. Length scaling
  (`stored · increment + reference`) is checked, and on `Ok` it is the exact
  integer. None of the four has a precondition, so there is no hostile `NG`,
  width or length that panics, overflows or indexes out of bounds.

The three expansion loops place a group the same way, so that step is one lemma,
`lemma_place_group`, proved once over any element type and used by both the
dense second-order output and complex packing's `Option` output.

Each claim was checked by breaking it (22 planted bugs, each rejected): dropping
the last-length override, the `sum == present_count` check, the `width > 32`
check on a complex group, on a second-order group or on a stored width as it is
read (the byte cut GRIB1 used to make), the `NG == 0` or `NG > present_count`
check; reading a zero-width group one point short; starting a group's offsets
one slot late; skipping any of the three octet realignments, before the width,
length or data block; a sentinel of `2^w` instead of `2^w − 1`; accepting the
secondary sentinel under management 1; losing the group reference from a
complex point, the width reference from a width, or the reference from a
second-order point; filling a zero-width second-order group with 0; using group
0's reference for every group; ignoring the length increment; unchecked length
arithmetic; and a model of `align_to_byte` that rounds down.

**`crates/fieldglass-core/src/bitmap.rs`**, the presence bitmaps: one bit per
point, MSB-first, a set bit meaning present. The GRIB1 Bit Map Section, the
GRIB2 Bit-Map Section, the secondary bitmaps of matrix-of-values packing in
both editions, and the secondary bitmap of GRIB1 second-order packing
(`constant_width`, `general_grib1`) are all unpacked by it.

- `unpack_bitmap(bytes, count)` returns `Some` exactly when `bytes` holds at
  least `count` bits; then the result has `count` entries and entry `i` is
  bit `i` of `bytes`, read MSB-first. "Bit `i`" is the proved bit reader's
  own `bit`, and `lemma_one_bit_read` shows it is what `read_bits(1)` returns
  there, so the matrix and second-order bitmaps, which were read that way,
  read the same bits.
- `bitmap_bit_len(body_len, unused)`, GRIB1's `8 · body_len − unused`, is
  `Some` exactly when the padding fits the body and the bit length fits a
  `usize`, and is then exact.
- `count_present(bits)` is the number of `true` entries and cannot overflow.
  For an unpacked bitmap that is the number of set bits among the first
  `count` (`lemma_present_is_popcount`). Every decoder that counts present
  points calls it.
- None of the three has a precondition.

Before the kernel, a GRIB1 bitmap with fewer bits than the grid has points
decoded to a field that many points short, with no error. A bitmap has one
bit per grid point (FM 92 GRIB edition 1, Section 3, octet 7 onwards), so it is
now an error, as it already was in GRIB2. eccodes 2.34.1 also returns the
short field, and its geoiterator then refuses the message.

Each claim was checked by breaking it (17 planted bugs, each rejected):
reading LSB-first, inverting the bit, reading the next byte, masking the
wrong bit, not rounding the byte count up or always rounding it up, an
off-by-one length check, skipping point 0, returning `None` for an empty
bitmap, a saturating or missing padding subtraction, seven bits per octet,
counting the absent entries, starting the count at 1, skipping the last
entry, stating the popcount one bit long, and reading the one-bit field one
bit on.

**Trusted, not proved.** The proofs rest on two statements in
`crates/fieldglass-verify/src/bits_model.rs` and its `axioms.rs`, each short
enough to check by reading, on allocation succeeding, and on the error-message
constructors returning:

| Assumption | Why it is assumed | What would remove it |
|---|---|---|
| `f64::powi(b, n)` is some fixed function `powi_spec(b, n)`, and nothing more | Verus has no specification for `powi`; the proofs only need to know *which* base and exponent each factor uses | not needed: the claim is about direction, not about `powi`'s accuracy |
| Allocation succeeds: `Vec::with_capacity` and `push` in the kernels do not panic or abort on a huge count | Verus models `Vec` without an allocator, so a capacity overflow or running out of memory is outside the proof, as it is for any Rust function that allocates | not planned: callers bound every count by the grid or message size before calling, and `expand_complex_groups` checks `NG ≤ present_count` and the length sum before it allocates |
| The error constructors in `groups.rs` and `bits/reader.rs` return a `FieldglassError` | they build their message with `format!`, which Verus has no specification for, so they are `external_body`; each only formats its arguments | a `vstd` that specifies `format!` |
| `BitReader`'s derived `Debug` and `Clone` behave as Rust derives them | the type is marked `external_derive`, so Verus does not check the derives: the pinned Verus cannot specify a derived `Clone` of a type that is not `Copy`, and no kernel calls either | a Verus that specifies derived `Clone` for non-`Copy` types |
| `f64` `+` and `*` never panic and are deterministic, and `u32 as f64` is exact | the pinned `vstd` requires an `add_req` / `mul_req` of `f64` arithmetic and defines neither, so no `f64` expression verifies without this | a `vstd` that specifies `f64` arithmetic |

Beyond those, the spatial-differencing proof trusts only `vstd`'s own
specifications of `i64::wrapping_add`, `wrapping_sub` and `wrapping_mul`, and
the one-line function that formats the error for an order above 3.

The `f64` axiom is a `broadcast` lemma, and under the pinned Verus it fires for
a parameter but not for a value read from a struct field. So the arithmetic
sits in private functions that take the factors as parameters, and `Scaling`'s
methods call them.

The specification of `read_bits` is exact about its failures, including the
one no real buffer reaches: the reader computes the buffer's bit length in a
`usize`, so a slice longer than `usize::MAX / 8` bytes fails. The `Ok`
condition of `unpack_simple` carries that clause rather than assuming it away.

### Pinning

Verus, `vstd`, and the Rust toolchain are pinned **together** and bumped
together:

| What | Pin | Where |
|---|---|---|
| Verus release | `0.2026.08.15.7d4628a` | `scripts/verify.sh` |
| Rust toolchain | `1.97.1` | `scripts/verify.sh` (CI reads it from there) |
| `vstd` | `=0.0.0-2026-08-09-0044` | `crates/fieldglass-verify/Cargo.toml` |

`vstd` on crates.io is versioned by release date and is only guaranteed to work
with a matching Verus, so bumping one alone will fail in confusing ways. Take
the toolchain requirement from the release's own `version.json` — the Verus
docs and most search results still name an older one.

On a bump, re-check the `verus_impl_method_marker` workaround on
`BitReader::new` in `bits/reader.rs`: it is what lets `verus_spec` see an
associated function with no receiver, and a release that fixes that makes it
removable (or one that changes the marker breaks it).

Verus is a bus-factor concern in the same sense as `rust-aec` and `rust-j2k`
under ADR-0001, but with an important difference: nothing that ships depends on
it, so an upstream that stalls costs us proofs, never releases.

### Policy: unverified is fine, verified must stay verified

Incremental by design. Most of the codebase is not verified and does not need to
be. What is not allowed is regression: once a function carries a proof, a change
that breaks it must fix the proof rather than delete it.

The CI job is **non-blocking** for now (`continue-on-error: true`), because a
suite this small cannot yet distinguish a real regression from a toolchain
hiccup. Remove that once the proofs are broad enough that red always means
something. Until then, treat a red Verus run as you would a failing test that
someone has not gotten to yet — not as noise.

Two things that make a proof worth having, both learned bootstrapping this:

- **A proof that cannot fail proves nothing.** The smoke test was checked by
  breaking it: a wrong implementation reports "postcondition not satisfied", and
  weakening the precondition reports "possible arithmetic underflow/overflow".
  Do the same for every new proof — a `requires` that is too strong makes the
  theorem vacuous, and nothing will tell you.
- **Cargo caches a successful verification.** A second `cargo verus verify`
  prints nothing and exits 0, which is a gate that passes without checking
  anything. `scripts/verify.sh` discards just this crate's artifacts first
  (`vstd` stays cached, so it costs 0.6 s rather than 30 s) and then fails if
  Verus produced no results at all.

### Formatting

`verusfmt` formats `verus! { }` blocks; `rustfmt` does not understand them, and
the two are designed to coexist.

Nothing to install: the pre-commit hook declares `verusfmt` **0.7.2** as a Rust
`additional_dependencies`, so pre-commit builds it into its own cached
environment on first use — locally and in CI alike, pinned the same way the
fetched hooks are. It is scoped to `crates/fieldglass-verify/src/`, so a
contributor who never touches this crate never pays for that build.

That detail matters more than it looks: an earlier draft ran `verusfmt` from
`PATH` and failed with an install hint when it was missing. CI runs
`pre-commit run --all-files`, so that version would have turned the main lint
job red on every pull request.

### What gets verified next

Ordered by blast radius, from the milestone:

| Tier | Target | Issue |
|---|---|---|
| 0 | `BitReader::read_bits`, `new` and `align_to_byte` (done) | [#771](https://github.com/D0ubleD0uble/fieldglass/issues/771) |
| 1 | GRIB simple-packing scaling arithmetic, both editions (done) | [#199](https://github.com/D0ubleD0uble/fieldglass/issues/199) |
| 1 | Inverse spatial differencing, both editions (done) | [#200](https://github.com/D0ubleD0uble/fieldglass/issues/200) |
| 1 | GRIB group expansion: complex packing and both editions' second-order (done) | [#201](https://github.com/D0ubleD0uble/fieldglass/issues/201) |
| 2 | Presence bitmaps, both editions (done) | [#202](https://github.com/D0ubleD0uble/fieldglass/issues/202) |
| 2 | Byte shuffle, HDF5 and Zarr (done) | [#203](https://github.com/D0ubleD0uble/fieldglass/issues/203) |
| 3 | NetCDF classic length/offset arithmetic | [#204](https://github.com/D0ubleD0uble/fieldglass/issues/204) |

The groundwork has already paid once: it surfaced and fixed a `read_bits`
truncation defect (#198, shipped in #233) before any proof was written.
