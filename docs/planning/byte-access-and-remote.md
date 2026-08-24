# Byte access and the remote seam

*Assessment 2026-08-23. The decision it fed is
[ADR-0005](../decisions/0005-byte-access-and-the-remote-seam.md) (#417); this page
stays as the assessment, not as the record.*

"Remote" here means **HTTP range / object-store access**. Remote
*filesystems* (SSH, virtual workspaces) already work, because the extension
reads through `vscode.workspace.fs`.

## No rewrite is coming, and the full refactor should not be done yet

The readers take `from_bytes(Vec<u8>)` and index `&self.data[a..b]`, but
decode is already range-addressed: `Grib2Message` stores `lus_range`,
`bms_range`, and `ds_range`, and decode slices `self.data[range]`. HDF5 chunk
reads already go through a plan/fetch split (`collect_*_chunks` produces
`ChunkRecord { address, size, filter_mask, offset }`, then `assemble_chunked`
reads each).
Replacing `Vec<u8>` with a `ByteSource` trait is a wide mechanical diff, not a
redesign; deferring it costs linearly in new call sites.

The `#[allow(dead_code)]` on `Grib2Reader.data` was stale — the field is used in
every decode path — and was removed alongside ADR-0005.

## The real question is the sync/async boundary

The answer the codebase already implies is **prefetch**: resolve which byte
ranges an operation needs, fetch them in one batch, then decode synchronously
from the result. Going async to the leaves would fight both napi and a future
wasm build; blocking inside a read call is impossible in wasm.

## The property to protect, starting now

> A decoder should record the addresses it discovers, so a fetch plan can be
> replayed without re-traversing.

This is deliberately the weak form. "State every range up front" is not
achievable:

- **GRIB** message discovery is a serial chain: offset *N+1* comes from offset
  *N*'s total-length field, and garbage-skip advances a byte at a time.
  Remotely that is one dependent round-trip per message. The practical answer
  is the **`.idx` sidecar** convention (NOMADS / wgrib2; what kerchunk and
  VirtualiZarr consume): a small text index of message offsets fetched once.
  Read it when present, fall back to the serial scan.
- **HDF5** is a pointer chase by construction: chunk indexes (B-tree v1/v2,
  fixed and extensible arrays), group traversal, fractal heaps for dense
  attributes, the global heap for variable-length `DIMENSION_LIST`. What *is*
  achievable is caching what a traversal found so it is walked once instead of
  once per call — done for the in-memory reader in #414 (see action 2 below).
  The records that memo holds are the same ones a fetch plan needs, so the
  remote work inherits them rather than re-deriving them.
- **NetCDF classic** satisfies the strong form: the header carries every
  offset.

## Actions, in order

1. ~~**Stop the redundant copies.**~~ Done in #411, as far as it goes without
   the seam. Measured through the built addon, opening a large file:

   - Every handle now holds the file **exactly once**, at 1.00× file size for
     GRIB1, GRIB2 and NetCDF-4 alike. GRIB1 was the outlier at 1.96×, keeping a
     second copy alongside the reader's; GRIB2 and NetCDF already held one.
   - Peak is therefore **~2× file size**, not 3×: the extension's whole-file
     read plus one copy in the reader. Removing that last copy is what needs
     this seam — the reader would have to borrow the napi `Buffer`, making the
     handle self-referential, which is why it is a design item and not a
     cleanup.
   - The editor also opened NetCDF files twice, once for the metadata table and
     once for a render handle. It now builds one reader (110 MB NetCDF-4: 103 ms
     to open, 40 ms after). Peak RSS never showed this — the first reader is
     freed before the second allocates, so the allocator reuses the pages, and
     only wall-clock or an allocation counter sees it. Worth remembering when
     judging the seam by peak memory alone.
2. ~~**Cache traversal results on the NetCDF reader.**~~ Done in #414. Worth
   knowing before building on it:

   - The memo lives on `Hdf5Probe`, not on `NetcdfReader`. The probe is the
     per-file handle the traversal functions already thread everywhere, so
     nothing below it changed signature; `probe.header(bytes, addr)` replaced
     the direct `object_header::walk` at every call site, which is the single
     seam a `ByteSource` would later be threaded through.
   - It holds the root address, the depth-first child list (the order that
     *defines* each dataset's decode index), every parsed object header, and
     chunk records per dataset.
   - Every key is a **file offset**, so the memo is only meaningful for the
     slice it was filled from. It binds to that slice's length on first use and
     steps aside for any other; equal-length files still alias. A `ByteSource`
     would need a stronger identity than length.
   - Chunk records are keyed by `(index address, rank)`. A record's `offset` is
     rank-length, so address alone is not a safe key for a malformed file.
   - Object-header bodies are retained for the reader's life under a 64 MiB
     budget; past it the memo stops inserting and stays correct. Keep that in
     view alongside action 1 — this trades a little steady-state memory for the
     walks, and action 1 is about reducing peak.
   - Failures are not memoised, so a malformed header is re-parsed per call.
   - `Hdf5Probe::traversals()` counts walks actually performed. It is how the
     win was measured, and it is the metric to hold when the seam moves: over a
     byte-range transport each walk is a chain of dependent round-trips.
3. ~~**ADR-0005.**~~ Done in #417 —
   [`docs/decisions/0005-byte-access-and-the-remote-seam.md`](../decisions/0005-byte-access-and-the-remote-seam.md).
   That record is now the decision; this page stays as the assessment it was
   drawn from. Two details checking it against the source firmed up:
   `ChunkRecord` carries `filter_mask` and a rank-length `offset` as well as
   `{address, size}`, and the `#[allow(dead_code)]` was indeed stale — removed
   with the ADR, since a record describing how the reader owns its bytes should
   not sit beside an attribute claiming the field is unused.
4. **Migrate one reader as proof, when there is a consumer.** GRIB2 is the
   natural candidate: its messages already carry ranges, so the reader is the
   only thing that touches the buffer.
5. Only then: HTTP range (#247), S3 (#252), Zarr (#246).

The same seam serves huge-file (#114) and wasm-memory cases.
