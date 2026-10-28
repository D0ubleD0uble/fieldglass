# Planned architecture

The diagrams in the parent directory describe the workspace as it is, and a
pre-commit check keeps them true to the source. The diagrams here describe
the workspace as it will be once the open milestones close. They're drawn at
the same three levels (crates, trait seams and composition), plus a new one
for hosts, so you can put each pair side by side.

They were drawn on 2026-09-07, with milestone 13 added on 2026-09-29, and they
cover these milestones:

| Milestone | Title | What it adds to the picture |
| --- | --- | --- |
| [7](https://github.com/D0ubleD0uble/fieldglass/milestone/7) | Formal verification of the decode kernel (Verus) | `fieldglass-verify` proofs over the decode arithmetic, outside the workspace. |
| [10](https://github.com/D0ubleD0uble/fieldglass/milestone/10) | Grid geometry, local tables, and the byte-access seam | The spatial-index seam behind `SourceGrid`, with HEALPix (§3.150) and 2-D coordinate curvilinear grids as its first users. |
| [11](https://github.com/D0ubleD0uble/fieldglass/milestone/11) | fieldglass-wasm: browser host surface | A second host (`fieldglass-wasm`); the `fieldglass` umbrella crate both hosts bind (ADR-0006), with its `Session`, `Error` and conformance suite; the `GridGeometry` type in core; `Palette` as the one color path (#485); `fieldglass-fetchplan`; and the Zarr store walker (#658) and its host command (#659). |
| [12](https://github.com/D0ubleD0uble/fieldglass/milestone/12) | Cleanliness and refactor | The layered workspace of [ADR-0010](../../decisions/0010-a-common-array-model-and-containers-as-drivers.md): a common array model in `core` (#677, #678), `ObjectSource` beside `ByteSource` (#680, #681), the NetCDF view and HDF5 reader on those seams (#684, #682), one Zarr metadata parser (#686), `fetchplan` reshaped to take manifests in and hand a chunk plan out (#685, #687), and the Variables addressing mode under conformance (#679). |
| [13](https://github.com/D0ubleD0uble/fieldglass/milestone/13) | Own the AEC decoder (CCSDS 121.0 / szip) | A codec crate, `fieldglass-aec` ([ADR-0012](../../decisions/0012-own-the-aec-decoder.md)), below the format crates and beside `core`. `grib2` takes it for 5.42 (#762), and `netcdf` for the szip filter (#421). |

## Rules

- These diagrams aren't checked. `tools/check_architecture_diagrams.py` reads
  only the top level of `docs/architecture/`, and nothing here is checked
  against the source, because most of the nodes don't exist yet. Don't move a
  file from here to the parent directory. Fold its changes into the checked
  diagram instead.
- Every planned Fieldglass node names its milestone or issue. In class
  diagrams the label is `<<planned #N>>`. In flowcharts the node is dashed and
  its label carries `#N`. A node with no tag exists today and is drawn for
  context. Nodes that belong to a consumer, such as the extension or the
  browser app, are tagged by the box they sit in, not by an issue here.
- When a milestone closes, delete its changes here in the same pull request
  that updates the checked diagram. This directory should shrink to empty, not
  grow into a second architecture.
- Names are provisional. A planned type name is the one in the issue on the
  date the diagrams were drawn. If they disagree, the issue wins, so update
  this directory, not the issue.

| File | Scope | Compare with |
| --- | --- | --- |
| [`01-crates.md`](01-crates.md) | The workspace after milestones 7, 11, 12 and 13, described as layers | [`../01-crates.md`](../01-crates.md) |
| [`02-trait-seams.md`](02-trait-seams.md) | New and widened dispatch points | [`../02-trait-seams.md`](../02-trait-seams.md) |
| [`03-composition.md`](03-composition.md) | New message parts, and the host boundary after #464 | [`../03-composition.md`](../03-composition.md) |
| [`04-hosts.md`](04-hosts.md) | The two hosts, and how a browser gets a field from a bucket | (a new level) |
