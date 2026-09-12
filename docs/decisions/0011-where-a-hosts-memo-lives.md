# 0011 — Where a host's memo lives

**Status:** Accepted (2026-09-12). Closes the design question left open on #662;
governs the remaining `fieldglass-napi` → `Session` migration and every host
named in ADR-0006.

## Context

Placing a slice of an array dataset on the Earth is expensive, and hosts repaint.

`Session::place_slice` and `Session::decode_slice` both derive a slice's
placement — where its cells are and what order they are stored in — from the
container's coordinate arrays. For the 2-D-coordinate case (a swath, a tripolar
ocean mesh) that derivation builds a spatial index over **every cell**: an
`O(n log n)` build and roughly 28 bytes of index per cell. On RTOFS's global
mesh, 4500 × 3298 = 14.8 million cells, that is about two seconds and 400 MB.

A repaint asks the same question again, and gets the same answer, because a
placement is a pure function of metadata and coordinate values that are both
fixed for as long as the file is open.

**Only one host had noticed.** `fieldglass-napi`'s `NetcdfHandle` has carried a
curvilinear cache since #445, keyed on the coordinate pair, with a comment
saying the cache belongs to the host because "holding it is a host's decision
about memory, not a decoder's." That reasoning was about the *decoder*, and it
was right about the decoder. But the conclusion did not survive contact with the
second array format and the second host:

| Host | NetCDF | Zarr |
| --- | --- | --- |
| `fieldglass-napi` | cached since #445 | **not cached** — `ZarrHandle` re-derives per repaint (#659) |
| `fieldglass-wasm` | **not cached** | n/a (no `zarr` feature) |

`ZarrHandle` was written after that comment, holds a `Session`, and calls
`place_slice` and `decode_slice` fresh on all eight of its display methods. The
browser host opens NetCDF (#675) and caches nothing at all. So "each host
decides" produced, in practice, "each host forgot" — and the two hosts ADR-0006
exists to keep identical diverged on performance instead of on output, which is
harder to see and nothing measures.

Measured on two committed fixtures, release build, repeated calls:

| | `place_slice`, repeat call | | `decode_slice` | |
| --- | --- | --- | --- | --- |
| | derived each time | memoised | derived each time | memoised |
| MiRS swath, 96 × 100 | 2.01 ms | 7.7 µs | 2.78 ms | 0.96 ms |
| RTOFS tripolar, 260 × 200 | 8.45 ms | 6.8 µs | 13.60 ms | 5.69 ms |

Re-deriving the placement was **70 %** of a whole `decode_slice` on the swath and
**62 %** on the mesh. These are small grids: 52,000 cells against RTOFS's 14.8
million, where the same build is the two seconds recorded above.

The question #662 could not answer from its own scope: does `Session` memoise
this, or does each host?

## Decision

**`Session` memoises a slice's *placement*. Retention of decoded *values* stays
with the host.** The two halves of a decode get opposite answers, and the reason
is not symmetry-breaking for its own sake.

### 1. The placement memo lives in `Session`

Three properties make it safe to do implicitly, and all three are properties of
the placement specifically:

- **It is a memo of a pure function.** The container's metadata and its
  coordinate arrays are fixed for the session's lifetime — the readers already
  resolve the group structure once, on open — so a cached placement cannot
  differ from a freshly derived one. Nothing observable changes except time.
- **It is bounded by the file's geometry, not by how much of the file you
  read.** The expensive value is keyed on the *coordinate pair* that builds it,
  and a file has about one: every RTOFS field sits on the same tripolar mesh, so
  one entry serves all of them however many are drawn. A memo that cannot grow
  with use is not a leak a caller has to manage.
- **Every host needs it, and it is the same need.** Repainting is what hosts do.
  Putting it in the one place all of them route through is the ADR-0006 rule
  applied to performance rather than to output.

### 2. Keyed on what determines the placement

Two shapes, because the expensive placement is not per-slice:

- **`Coordinates { lat, lon }`** — a slice placed by a 2-D coordinate pair, keyed
  on the group-qualified names of that pair. This is the collapse that matters:
  keyed on the *field* instead, a mesh's index would be rebuilt once per
  variable.
- **`Axes { array, y, x }`** — everything else: 1-D lat/lon, a WRF or CF
  projected domain, or nothing placed it. Cheap to derive, memoised anyway
  because deriving it re-reads the coordinate arrays, which on a chunked backing
  is a fetch and a decompress per repaint.

**The guard that makes coordinate-keying correct**: the pair may only answer for
the axes it spans. A cross-section through a curvilinear array — the time axis
against X — is not placed by the 2-D coordinates at all, and must not be handed
their answer, which would put the raster on entirely the wrong cells. The key
therefore applies the *same* test `slice_placement` applies before it reaches for
the pair — the pair's axis names, in order, against the two axes asked for — so
the key cannot disagree with the placement about whether the pair places this
slice. Restating that test positionally instead was the first version and was
wrong for an array that declares one dimension name twice, where both positions
resolve to the first: the key would have said "not the pair" for a slice the
placement does place by it, costing a second entry rather than a wrong answer,
but disagreeing all the same.

### 3. `PlacedSlice` shares the placement rather than copying it

`PlacedSlice` now holds an `Arc<SlicePlacement>` and every accessor borrows out
of it. Returning the geometry *by value* would pay an `O(n)` copy of the index on
each call and hand back most of what the memo saves — the same reason
`NetcdfHandle` caches the whole geometry rather than the bare index. The type is
`#[non_exhaustive]` with private fields, so this is a representation change and
not an API one.

### 4. Value retention stays with the host

Decoded values get the opposite answer because none of the three properties
above hold for them:

- **Unbounded in count.** A CMIP6 variable with `time = 1000` and `plev = 19` is
  19,000 distinct slices. A memo keyed per slice grows with use, without limit,
  and a library that retains everything a caller ever read is a leak wearing a
  cache's clothes.
- **Unbounded in size.** A whole-variable memo — which is what `NetcdfHandle`
  keeps, so that scrubbing a time axis costs one read — is every timestep
  resident.
- **Genuinely a policy question.** An interactive viewer wants the last slice
  and maybe its neighbours. A batch export wants none, and would be harmed by
  retention. A browser has a memory ceiling a desktop does not. There is no
  answer here that is right for all three, which is exactly the case for leaving
  it to the caller.

`Field` is already the unit a host retains, and `Arc` is the host's tool. A host
that wants a value memo writes the policy it wants; the placement it no longer
has to think about.

### 5. What was rejected

- **Memoise values in `Session` too.** Rejected on the three counts above. It
  would also make the cheap, always-correct half of this decision inseparable
  from the half that needs a policy, so a host wanting one would take both.
- **Leave both to the host.** The status quo, and it is the option the evidence
  refutes: two of the three host/format pairs that need the cache do not have
  it. A rule that relies on each new host remembering has already failed twice,
  the second time in code written months after the comment asserting the rule.
- **A cache the host constructs and passes back in.** Explicit, no hidden
  retention, and a real option. Rejected because it puts a parameter on every
  display method for a value that has exactly one correct content, and because
  the memo's boundedness removes the problem the explicitness was buying.

## Consequences

- The browser host gets the cache it never had, with no change to
  `fieldglass-wasm`; `ZarrHandle` gets it with no change to `fieldglass-napi`.
- **A session is a snapshot, more visibly than before.** The readers already
  resolve the group structure on open, so a directory store edited underneath an
  open session was already serving stale structure; it now serves a stale
  placement too. Reopen to see a changed file. This is a clarification of
  existing behaviour, not a new constraint.
- A poisoned memo lock is treated as a miss and the placement rebuilt. A memo
  can always recompute, so nothing here panics: the failure mode is slower
  answers, not wrong ones.
- **The remaining napi → `Session` migration now has its answer.** Moving
  `NetcdfHandle` behind `Session` means dropping its curvilinear cache in favour
  of this one, keeping its decoded-value memo as the host policy it is, and
  keying that memo on what `Session` exposes rather than on reader internals.
  That was the blocker recorded on #662, and it is no longer open.
- `Session` still exposes no cache controls. If a consumer ever needs to release
  a placement without dropping the session — a long-lived process holding many
  sessions over many meshes — that is a new decision with a measurement behind
  it, not a knob to add speculatively.

## When to revisit

If a container arrives whose coordinates can change while it is open — a live
store, a growing append-only dataset — the purity argument in decision 1 fails
and the memo needs invalidation rather than a longer life. If a host appears
that opens many files at once and draws each of them once, the boundedness
argument still holds per session but the sum across sessions may not, and
decision 5's rejected third option becomes worth re-costing.
