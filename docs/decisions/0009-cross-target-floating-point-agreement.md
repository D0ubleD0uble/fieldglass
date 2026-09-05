# 0009 — Cross-target agreement is a tolerance in grid cells, not identical bits

**Status:** Accepted (2026-09-05). Answers #617, and settles the question
ADR-0006's conformance suite (#573) would otherwise have to answer at
implementation time.

## Context

`crates/fieldglass-core/tests/planar_inverse_golden.rs` folds the exact `f64`
bits of every planar projector's `inverse` into a hash and compares it against a
constant recorded before the #486 extraction. On `x86_64-unknown-linux-gnu` it
matches. When #561 added a `wasm32-wasip1` test run to CI it did not, on
unmodified `master`:

```
assertion `left == right` failed: lambert inverts differently than the pre-#486 recording
  left: 4057552849463119528
 right: 9785898176119745066
```

The cause is not ours. A conformal inverse is built out of `ln`, `powf`, `tan`
and `atan2`, and those come from the platform's libm. `wasm32` has no host libm,
so Rust links its own port. IEEE 754 does not require correctly-rounded
transcendentals, and two conforming implementations may differ by an ULP.

The product consequence is the part worth stating plainly: **the browser build
computes slightly different projected coordinates than the native host, and
always will.** Fieldglass cannot make that go away by testing differently.

### What the disagreement actually is

Measured, not assumed. Every `inverse` output for all 22 cases in the golden —
14,710 probes each, 323,620 results — dumped from both targets and compared:

| | |
| --- | --- |
| results compared | 323,620 |
| points placed on a grid (`Some`) | 4,077 |
| **`Some`/`None` decisions that disagree** | **0** |
| scalar indices that differ at all | 186 |
| **widest disagreement** | **9.3 × 10⁻¹⁴ grid cells** |

Two facts carry the decision. The first is the magnitude: the worst case is a
ten-trillionth of a grid cell, eight orders of magnitude below what a raster, a
probe, a contour or a CSV column can resolve. The second matters more: the only
*discrete* output of `inverse` — whether a point is on the grid at all — is
identical on both targets across the whole probe set. The disagreement is
confined to fractional index digits nothing reads.

## Decision

**Cross-target agreement is asserted as an absolute tolerance in grid cells.
Bit-identical results are a property of one libm, recorded as such, and never a
requirement.**

Concretely, in the golden:

- The bit-exact column and its 22 constants are untouched. It now runs only
  where the target's libm is the one that recorded it, established by
  `libm_fingerprint()` — a fold over the fourteen transcendentals the inverses
  stand on, at fixed inputs, compared to a recorded constant. Eleven of the
  fourteen already differ between glibc 2.39 and the `wasm32` `libm`.
- A second column hashes the same probes in the same order at `QUANTUM` = 10⁻⁵
  grid cells. It runs everywhere, and it is what says the browser and the native
  host place a point in the same place.
- `the_quantised_golden_has_room_for_a_different_libm` asserts that no recorded
  index sits within `MARGIN_FLOOR` = 10⁻¹¹ cells of a bucket boundary. The
  closest any actually comes is 4.2 × 10⁻¹⁰ cells — 4,500 times the widest
  cross-target disagreement — so a value cannot silently round into a different
  bucket on a libm this repository has never seen. If one ever drifts that far,
  this fails first, with a message that says what happened.
- `the_reference_toolchain_is_the_recorded_libm` asserts the fingerprint
  outright on x86_64 glibc, so the gate cannot be edited into never matching and
  quietly turn the bit-exact column into a no-op everywhere.

The fingerprint replaces what would otherwise be a `target_arch = "wasm32"`
check. That check is wrong twice: it assumes every *other* target agrees with
the recording — musl, macOS and a future glibc need not — and it identifies a
target rather than the library actually underneath. Asking is cheap.

### What this asks of #573

ADR-0006 decision 3 has both hosts run one conformance suite. Its expectations
for anything geolocated are a tolerance, not equality, and this record supplies
the number: **10⁻⁵ of a grid cell is 100,000,000 times the observed
disagreement** and is far tighter than any output resolution. `Error::code()`,
buffer lengths, masks and the `Some`/`None`-shaped decisions stay exact
comparisons — the measurement above says they can.

## What was rejected

**A second golden per target family.** It doubles what has to be maintained,
says nothing about what the browser guarantees, and is unbounded: the
partitioning is not "native vs wasm" but every libm the crate may ever be built
against. Worse, it keeps calling a platform's libm revision a change in
Fieldglass — the confusion #617 was filed about. A recording that a glibc update
can turn red is not a characterisation of this code.

**Pin a software `libm` on every target.** This is the only option that would
make native and browser agree bit-for-bit, and it was tempting for exactly that
reason. It was rejected on cost:

- Every `.ln()`, `.sin()`, `.atan2()` in `projection/` becomes `libm::log(…)`,
  and by the project's own "one fix pattern covers every instance" rule the
  sweep would not stop at `projection/`. The result reads worse everywhere.
- Nothing would keep it that way. The next contributor writes `x.ln()`, it
  compiles, it passes, and bit-reproducibility is silently gone. Holding the
  invariant needs a lint that does not exist.
- `inverse` runs per output pixel in the warp. Substituting a software
  implementation for glibc's is a real cost paid on every render, by every user,
  to settle a 10⁻¹³-cell discrepancy.
- It does not even preserve the recording. A pinned `libm` produces the
  `wasm32` numbers, not glibc's, so all 22 constants would have to be
  re-recorded anyway.

It would also make bit-reproducibility a load-bearing part of the product's
numerical contract. That is a larger promise than this problem justifies, and
the door stays open: nothing here prevents adopting it later if a reason
appears that is about the product rather than about a test.

**Delete the bit-exact column.** It is the most sensitive characterisation the
repository has of the planar inverses, and the quantised column is deliberately
eight orders of magnitude blunter. Keeping both costs one constant per row.

## Consequences

- The `wasm32-wasip1` CI step no longer skips a projection test. Its remaining
  two skips are about what WASI cannot *do* — no `temp_dir`, no unwinding — not
  about what it computes.
- The portable constants were recorded from a tree whose bit-exact constants
  still matched the pre-#486 recording, verified in the same run, so the
  provenance chain the golden's header describes carries across to them.
- A downstream running `cargo test` on macOS or musl now gets a meaningful
  result instead of a spurious failure.
- This is a decision about *comparison*, not about arithmetic. Fieldglass still
  computes whatever the platform's libm computes, and different platforms still
  differ. The change is that the test suite now says so, with a number.
- **The two columns are not equally sensitive, and that is deliberate.** A
  regression of 10⁻⁹ cells fails the bit-exact column and passes the quantised
  one — verified by injecting exactly that. So the reference toolchain, where
  `cargo test --workspace` runs, is still where the sensitive characterisation
  lives; the `wasm32-wasip1` run is a portability check, not a second copy of
  it. Anyone reading a green wasm run as "the inverses are unchanged to the last
  bit" is reading it wrong.
- **A glibc that changes `log` turns one test red, and the right one.** When
  `ubuntu-latest` rolls to a newer image, `the_reference_toolchain_is_the_recorded_libm`
  may fail. That is the intended signal: the bit-exact column then stands aside
  everywhere until someone re-records it deliberately, and this assertion is
  what stops that from happening quietly. Before, the same glibc change failed
  the golden itself with a message blaming this repository. The response is to
  re-record both columns in one commit and note it in the golden's header, as
  #488 did for the two polar rows.
- It follows that the guard is coupled to CI keeping a job on x86_64 glibc. If
  the native gate ever moves off that platform, the `#[cfg]` on that test stops
  matching and the bit-exact column loses its keeper — so moving it means
  re-recording `REFERENCE_LIBM` for wherever it moved to, in the same change.

## When to revisit

- **A host needs bit-identical output as a product guarantee** — a
  reproducible-build claim, or cross-machine caching keyed on rendered bytes.
  Then the pinned-`libm` option comes back, with the lint that has to accompany
  it, and this record is superseded rather than amended.
- **The margin assertion fires.** That is a libm having moved a recorded index
  close to a bucket boundary. The fix is to widen `QUANTUM`, not to lower
  `MARGIN_FLOOR`, and the new margin is worth recording here.
- **A discrete output starts disagreeing across targets** — a `Some`/`None`, a
  pixel index, a contour segment count. The measurement above says that does not
  happen today, and it is the assumption most worth re-checking, because it is
  the one that would actually be visible.
