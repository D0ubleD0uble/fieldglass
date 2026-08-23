# Byte access and the remote seam

*Assessment 2026-08-23. Input to a future ADR-0005.*

"Remote" here means **HTTP range / object-store access**. Remote
*filesystems* (SSH, virtual workspaces) already work, because the extension
reads through `vscode.workspace.fs`.

## No rewrite is coming, and the full refactor should not be done yet

The readers take `from_bytes(Vec<u8>)` and index `&self.data[a..b]`, but
decode is already range-addressed: `Grib2Message` stores `lus_range`,
`bms_range`, and `ds_range`, and decode slices `self.data[range]`. HDF5 chunk
reads already go through a plan/fetch split (`collect_*_chunks` produces
`ChunkRecord { address, size }`, then `assemble_chunked` reads each).
Replacing `Vec<u8>` with a `ByteSource` trait is a wide mechanical diff, not a
redesign; deferring it costs linearly in new call sites.

The `#[allow(dead_code)]` on `Grib2Reader.data` is stale; the field is used in
every decode path.

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
  once per call. Today `decode_variable_values` re-walks the dataset tree on
  every call, `read_dataset_values` walks the object header again for
  sentinels, and `dimensions::resolve` walks every dataset.
- **NetCDF classic** satisfies the strong form: the header carries every
  offset.

## Actions, in order

1. **Stop the redundant copies.** The file is read whole by the extension,
   copied at the napi boundary by `bytes.to_vec()`, and `Grib1Handle::from_bytes`
   clones again: roughly 3× peak memory. Seam-independent, the immediate
   partial fix for #114, no design work.
2. **Cache traversal results on the NetCDF reader.** Root address, per-dataset
   object-header addresses, chunk-record lists. Pure win in memory,
   prerequisite remotely.
3. **ADR-0005.** Record the prefetch-sync decision, the `ByteSource` shape,
   the record-discovered-addresses constraint, and the `.idx` sidecar plan.
4. **Migrate one reader as proof, when there is a consumer.** GRIB2 is the
   natural candidate: its messages already carry ranges, so the reader is the
   only thing that touches the buffer.
5. Only then: HTTP range (#247), S3 (#252), Zarr (#246).

The same seam serves huge-file (#114) and wasm-memory cases.
