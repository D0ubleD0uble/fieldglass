# The LikeC4 model

A single model of the whole workspace, from "who uses Fieldglass" down to
"which §5 templates decode". Every element with children has a view of its
own, so a node in one diagram opens as the diagram below it, and the
breadcrumb walks back out to where it is one node again.

This is the *navigable* form of the same architecture the Mermaid diagrams in
the parent directory describe. The Mermaid files stay: they render inline on
GitHub and in an editor with no toolchain, and `01-` / `02-` / `03-` are drift
-guarded against the source. This model is what you open when you want to move
between altitudes rather than read one.

## View it

```sh
npx likec4@1.59.2 start docs/architecture/model      # dev server, hot reload
npx likec4@1.59.2 validate docs/architecture/model   # parse + semantic check
```

`start` opens a browser. Click a node to go in; the header breadcrumb and the
back arrow go out. `Ctrl + K` searches every element in the model.

Build a shareable single file (no server, no network):

```sh
npx likec4@1.59.2 build --output-single-file --use-hash-history \
  --theme dark -t 'Fieldglass architecture' -o build docs/architecture/model
```

The VS Code extension `likec4.likec4-vscode` gives syntax highlighting and a
live preview beside the source.

## Files

| File | Holds |
| --- | --- |
| `specification.c4` | Element kinds (`crate`, `module`, `seam`, `impl`, `part`, `decision`), relationship kinds, tags, colours. |
| `globals.c4` | Reusable view rules: the dashed-planned style, and the predicate that hides the ADR nodes. |
| `landscape.c4` | Consumers, external data, the workspace, the crates, and the dependency edges — today's and the planned ones. |
| `core.c4` | Inside `fieldglass-core`: the trait seams and their implementers. |
| `formats.c4` | Inside `grib1`, `grib2`, `netcdf`, `zarr`: message composition and the template enums. |
| `hosts.c4` | `napi`, the planned umbrella and `wasm`, `fetchplan`, and the proof targets. |
| `decisions.c4` | The seven ADRs, and what each one constrains. |
| `views.c4` | Every view, including three dynamic (sequence) views. |

## Conventions

- **A planned node is tagged `#planned`** and names its milestone or issue in a
  `link`, the same rule as [`../planned/`](../planned/README.md). The
  `planned-dashed` global style draws them dashed everywhere.
- **Descriptions carry the reasoning, not just the name.** A node whose
  description only repeats its title is not earning its place — the point of
  this model is that hovering a box tells you why it is a box.
- **Names are code names.** Prefer the identifier the source already uses.
  Where a planned name is provisional, the issue wins (see `../planned/`).
- **Not drift-guarded.** `tools/check_architecture_diagrams.py` reads only
  ` ```mermaid ` blocks in the top level of `docs/architecture/`. Nothing here
  is checked against the source, so when a seam or a crate changes, this model
  is updated by hand in the same PR as the Mermaid diagram.

## Why a second representation exists

See [`../tooling.md`](../tooling.md) for the comparison this came out of, and
for what to reach for if the answer ever needs to change.
