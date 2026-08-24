# Planned architecture

The diagrams in the parent directory describe the workspace **as it is** and a
pre-commit drift guard keeps them honest against the source. The diagrams here
describe the workspace **as it will be** once the open milestones close. They
are the same three altitudes (crates, trait seams, composition) plus one new
one (hosts), so a reader can put each pair side by side.

Snapshot date: 2026-08-24. Milestones covered:

| Milestone | Title | What it adds to the picture |
| --- | --- | --- |
| [7](https://github.com/D0ubleD0uble/fieldglass/milestone/7) | Formal verification of the decode kernel (Verus) | `fieldglass-verify` proofs over the decode arithmetic, outside the workspace. |
| [10](https://github.com/D0ubleD0uble/fieldglass/milestone/10) | Grid geometry, local tables, and the byte-access seam | The spatial-index seam behind `SourceGrid`; HEALPix (§3.150) and 2-D coordinate curvilinear grids as its first consumers. |
| [11](https://github.com/D0ubleD0uble/fieldglass/milestone/11) | fieldglass-wasm: browser host surface | A second host (`fieldglass-wasm`), the `fieldglass` umbrella crate both hosts bind (ADR-0006) with its `Session`, `Error`, and conformance suite, the `GridGeometry` type in core, `Palette` as the one colour path (#485), `fieldglass-fetchplan`, and Zarr re-scoped to codecs. |

## Rules

- **Not drift-guarded.** `tools/check_architecture_diagrams.py` reads only the
  top level of `docs/architecture/`; nothing here is checked against the
  source, because most of the nodes do not exist yet. Do not move a file from
  here to the parent directory: fold its delta into the guarded diagram instead.
- **Every planned node names its milestone or issue.** In class diagrams the
  annotation is `<<planned #N>>`; in flowcharts the node is dashed and its
  label carries `#N`. A node with no tag exists today and is drawn for context.
- **When a milestone closes, delete its delta here** in the same PR that
  updates the guarded diagram. This directory should shrink to empty, not grow
  into a second architecture.
- **Names are provisional.** A planned type name is the one in the issue at
  the snapshot date. The issue wins if they disagree; update here, not there.

| File | Scope | Compare with |
| --- | --- | --- |
| [`01-crates.md`](01-crates.md) | Workspace after milestones 7, 10, 11 | [`../01-crates.md`](../01-crates.md) |
| [`02-trait-seams.md`](02-trait-seams.md) | New and widened dispatch points | [`../02-trait-seams.md`](../02-trait-seams.md) |
| [`03-composition.md`](03-composition.md) | New message parts and the host boundary after #464 | [`../03-composition.md`](../03-composition.md) |
| [`04-hosts.md`](04-hosts.md) | The two hosts and how a browser gets a field from a bucket | (new altitude) |
