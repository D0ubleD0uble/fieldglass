# Formal verification with Verus

*Bootstrapped 2026-08-23 ([#197](https://github.com/D0ubleD0uble/fieldglass/issues/197)).*

Fieldglass's third commitment is "be right, provably where it matters". The
decode kernel — the few hundred lines that turn untrusted bytes into numbers —
is where a malformed file causes wrong values, an overflow, or a panic, and
every GRIB value in the project passes through it. [Verus](https://github.com/verus-lang/verus)
verifies real Rust in place, as ghost code that carries no runtime cost, so
there is no port and no second source of truth.

## Running it

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

## Why the proofs live in their own crate

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
consumer on Verus's schedule, for a benefit none of them asked for.

The CI job asserts the isolation rather than trusting it: no `vstd`,
`verus_builtin`, or `verus_builtin_macros` may appear in the workspace
dependency graph.

## The open question this bootstrap does not settle

The function proved today is *written* in the verification crate. Proving the
shipped `fieldglass-core` functions instead needs one of:

- **`vstd` in `fieldglass-core`**, gated or not. Transparent to the build and
  the cross-compile, as measured above; the cost is the published-crate
  dependency.
- **Moving the kernel into a crate like this one** and having `fieldglass-core`
  depend on *it*. Keeps the dependency one level down, at the cost of moving
  shipped code.

[#199](https://github.com/D0ubleD0uble/fieldglass/issues/199) makes that call
with a real proof in hand. Deciding it here, before anything non-trivial has
been proved, would be guessing.

## Pinning

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

Verus is a bus-factor concern in the same sense as `rust-aec` and `rust-j2k`
under ADR-0001, but with an important difference: nothing that ships depends on
it, so an upstream that stalls costs us proofs, never releases.

## Policy: unverified is fine, verified must stay verified

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

## Formatting

`verusfmt` formats `verus! { }` blocks; `rustfmt` does not understand them, and
the two are designed to coexist. A pre-commit hook runs it, scoped to
`crates/fieldglass-verify/src/`, so a contributor who never touches this crate
never needs the tool. If you do, install it from
[verus-lang/verusfmt](https://github.com/verus-lang/verusfmt/releases) — the
repo pins **0.7.2**.

## What gets verified next

Ordered by blast radius, from the milestone:

| Tier | Target | Issue |
|---|---|---|
| 1 | GRIB1 simple-packing scaling arithmetic | [#199](https://github.com/D0ubleD0uble/fieldglass/issues/199) |
| 1 | Inverse spatial differencing (GRIB1 + GRIB2) | [#200](https://github.com/D0ubleD0uble/fieldglass/issues/200) |
| 1 | GRIB2 complex-packing group expansion | [#201](https://github.com/D0ubleD0uble/fieldglass/issues/201) |
| 2 | Bitmap decoders | [#202](https://github.com/D0ubleD0uble/fieldglass/issues/202) |
| 2 | HDF5 unshuffle filter | [#203](https://github.com/D0ubleD0uble/fieldglass/issues/203) |
| 3 | NetCDF classic length/offset arithmetic | [#204](https://github.com/D0ubleD0uble/fieldglass/issues/204) |

The groundwork has already paid once: it surfaced and fixed a `read_bits`
truncation defect (#198, shipped in #233) before any proof was written.
