# 0005 — Byte access and the remote seam

**Status:** Accepted (2026-08-24). Resolves the [#417](https://github.com/D0ubleD0uble/fieldglass/issues/417) research issue.

Gates [#247](https://github.com/D0ubleD0uble/fieldglass/issues/247) (HTTP range /
OPeNDAP), [#252](https://github.com/D0ubleD0uble/fieldglass/issues/252) (S3),
[#246](https://github.com/D0ubleD0uble/fieldglass/issues/246) (Zarr) and
[#114](https://github.com/D0ubleD0uble/fieldglass/issues/114) (multi-GB files).
The analysis behind it is
[`docs/planning/byte-access-and-remote.md`](../planning/byte-access-and-remote.md).

## Context

Four issues wait on one unmade decision: how Fieldglass gets at bytes it does not
already hold in a `Vec<u8>`. Every decoder written before that decision is one
more call site to migrate afterwards, so the cost of not deciding grows linearly
whether or not anyone is working on remote data.

**"Remote" here means HTTP range and object stores.** Remote *filesystems* — SSH,
containers, virtual workspaces — already work, because the extension reads
through `vscode.workspace.fs.readFile`, which the host resolves. Nothing in this
record is needed for those.

An audit on 2026-08-23 established the thing that most shapes the decision:
**no rewrite is required.** The readers take `from_bytes(Vec<u8>)` and index
`&self.data[a..b]`, but decode is already range-addressed underneath:

- `Grib2Message` carries `lus_range: Option<(usize, usize)>`, `bms_range` and
  `ds_range`, and every decode path slices `self.data[range]`. The reader is the
  only thing that touches the buffer.
- HDF5 chunk reads already split plan from fetch: `collect_*_chunks` produces
  `ChunkRecord { address, size, filter_mask, offset }`, and `assemble_chunked`
  reads each one.

So the question was never the buffer type. Replacing `Vec<u8>` with a trait is a
wide mechanical diff, not a redesign.

## Decision

### 1. Prefetch, then decode synchronously

Resolve which byte ranges an operation needs, fetch them in one batch, and decode
synchronously from the result. **Decoders stay sync all the way down.**

What settles it is **wasm**: blocking inside a read call cannot be done on the
browser's main thread, and a wasm build is a named goal — PyO3 bindings and wasm
are both on the roadmap, and the format crates are already pure byte-in,
values-out engines. A design that only works where there is a thread to block on
forecloses that. Decision 3 takes the alternatives one at a time.

Prefetch keeps the async boundary at the *host* — where the fetching already
lives, in JavaScript — and leaves the Rust a pure function of bytes. The napi
crate has no async surface today: no `AsyncTask`, no tokio, plain `napi6`
bindings called straight from the editor's message handlers. Prefetch is the
option that keeps it that way.

### 2. `ByteSource` is a trait, and `Vec<u8>` implements it

The seam is a trait over "give me these ranges", with a blanket implementation
for the byte buffers the readers already hold. That makes migration incremental:
a reader can be moved one at a time, and every existing caller keeps working
because `Vec<u8>` is a `ByteSource`.

Its shape follows from decision 1 — a batch resolve, not a per-read call — but
the exact signature is deliberately **not** fixed here. #438 lands the trait with
the NetCDF classic reader as its first user, and one real implementation is worth
more than a signature argued in advance.

Two constraints on it that the audit did fix:

- **Identity must be stronger than length.** The HDF5 traversal memo keys
  everything by file offset, so it is only meaningful for the slice it was filled
  from; today it binds to that slice's length on first use and steps aside for any
  other, which means equal-length files still alias. A `ByteSource` has to carry a
  real identity.
- **A `ByteSource` may be borrowed.** The remaining buffer copy at the napi
  boundary — peak is ~2× file size, the extension's whole-file read plus the
  reader's copy — can only go away if the reader borrows the napi `Buffer`, which
  makes the handle self-referential. That is why removing it is a design item and
  not a cleanup.

### 3. What was rejected, and why

An ADR that names one option is advocacy. Three alternatives are real enough
that someone will propose them, so they are answered here rather than again.

**Async to the leaves, on a worker thread.** The conventional answer, and it
*works* for napi — a threadpool or napi's async work queue would let a decode
block on I/O without stalling the editor. It is rejected on wasm alone: there is
no thread to block on in the browser's main thread, and a design that needs one
forecloses a named goal. Worth being honest that this is the weakest-held part
of the record — if the wasm target were dropped, this decision should be
re-opened rather than inherited.

**Make remote look like a file** — FUSE, an object-store mount, `mmap`. Pushes
the problem to the OS, and for the desktop extension that is genuinely tempting
because `vscode.workspace.fs` already covers the filesystem case. Rejected for
the same reason: it does not exist in a browser, and it converts a latency
problem into an invisible one — a pointer chase over a mount is still a chain of
dependent round-trips, just without any way to see or batch them.

**Precompute a manifest for everything** — the kerchunk / VirtualiZarr move:
traverse once, offline, and publish a JSON index that makes any format
range-readable without a pointer chase. This is the *strong* form of the
constraint below, achieved by moving the work out of band, and it is not so much
rejected as scoped out: it needs a producer willing to publish the manifest, so
it cannot be the mechanism a reader falls back on. It is, however, exactly what
decision 5 does for GRIB — where the producers already publish one.

### 4. The constraint decoders must keep, starting now

> **A decoder should record the addresses it discovers, so a fetch plan can be
> replayed without re-traversing.**

This is deliberately the *weak* form. The strong form — "state every range up
front" — is not achievable, and writing it down as an aspiration would mislead
whoever tries:

| Format | Strong form? | Why |
|---|---|---|
| NetCDF classic | **yes** | The header carries every variable's `begin` offset. |
| GRIB | **no** | Message discovery is a serial chain: offset *N+1* comes from offset *N*'s total-length field, and garbage-skip advances a byte at a time. Remotely that is one dependent round-trip per message. |
| HDF5 | **no** | A pointer chase by construction: B-tree v1/v2 and fixed/extensible-array chunk indexes, group traversal, fractal heaps for dense attributes, and the global heap for variable-length `DIMENSION_LIST`. |

In practice this asks one thing of a decoder author today: **when you learn where
something is, put it somewhere a caller can keep** — a field on the message, a
record in the probe's memo — rather than recomputing it from the buffer on the
next call. `Grib2Message`'s ranges and `Hdf5Probe`'s chunk records are both
already that; the constraint is that new decode paths keep doing it.

What *is* achievable for the two formats that fail the strong form is caching
what a traversal found, so a file is walked once rather than once per call. That landed for HDF5 in #414,
on `Hdf5Probe` — the per-file handle the traversal functions already thread
everywhere. **The records that memo holds are the same ones a fetch plan needs**,
so the remote work inherits them rather than re-deriving them, and
`Hdf5Probe::traversals()` is already the metric: over a byte-range transport each
walk is a chain of dependent round-trips.

### 5. GRIB's answer is the `.idx` sidecar

The serial chain is not fixable in the format, so it is routed around. NOMADS and
the AWS Open Data mirrors publish a `<file>.idx` beside every GRIB2 file — the
wgrib2 convention, a few kilobytes of text with one line per message carrying its
byte offset. It is the same index kerchunk and VirtualiZarr consume.

**Read it when present; fall back to the serial scan when not.** The repo already
depends on this working: `tools/fetch_samples.sh` fetches single messages out of
multi-gigabyte GFS, HRRR, NAM and RAP files by parsing the `.idx` and issuing one
HTTP range request. That is the remote read path in miniature, in shell, today.

## Consequences

**What this makes cheap.** #247, #252 and #246 become one `ByteSource`
implementation each, over a decode stack that never learns they exist. #114 and a
wasm build come out of the same seam, because "fetch a plan, decode from it" is
also how you avoid holding a 4 GB file in memory.

**What this costs.** Prefetch means an operation must know its ranges before it
starts, and for GRIB and HDF5 that is only true after a traversal. The first
open of a remote file will therefore be round-trip-bound in a way the in-memory
reader is not — bounded, for GRIB, by the `.idx` sidecar, and for HDF5 by
whatever the memo already holds. There is no version of this that makes an
uncached remote HDF5 open fast.

**What is deliberately still open.** The `ByteSource` signature (#438), and
whether a fetch plan is a first-class type or just a `Vec<Range<u64>>`. Both want
a real consumer before they are fixed.

**Order of work.** ADR (this record) → `ByteSource` with NetCDF classic as the
first user (#438) → migrate GRIB2 as proof, since its messages already carry
ranges → only then HTTP range (#247), S3 (#252), Zarr (#246).

## When to revisit

A record is a snapshot. These are the observations that would make it wrong, so
that superseding it is a decision rather than a drift:

- **The wasm target is dropped.** Decision 1 rests on it more than on anything
  else. Without it, async-on-a-worker-thread is a live option again and this
  record should be superseded rather than worked around.
- **A prefetch plan cannot be resolved without decoding.** The weak-form
  constraint assumes traversal and decode are separable. A format where the
  ranges you need depend on *values* you have already decoded — not just on
  offsets you have read — breaks that, and would need a different shape.
- **`.idx` coverage turns out to be thin.** Decision 5 assumes the sidecar is
  the common case for public GRIB. If a target archive publishes none, GRIB
  remote reads fall back to a per-message round-trip chain, and the honest
  answer may be "GRIB is not usefully remote-readable there" rather than a
  workaround.
- **The first migrated reader needs a per-read call after all.** #438 is the
  test of decision 2. If NetCDF classic — the format that *does* satisfy the
  strong form — cannot be expressed as a batch resolve, the trait shape is
  wrong and the shape is what should change, not the reader.
