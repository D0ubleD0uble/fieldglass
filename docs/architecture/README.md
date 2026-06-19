# Architecture diagrams

Fieldglass is a five-crate Rust workspace, not an OO codebase, so there is no
single "class diagram." The structure is documented at three altitudes, each as
a [Mermaid](https://mermaid.js.org/) diagram that renders inline on GitHub:

| File | Altitude | What it shows |
| --- | --- | --- |
| [`01-crates.md`](01-crates.md) | Workspace | Crate dependency graph; the decode-decoupling invariant. |
| [`02-trait-seams.md`](02-trait-seams.md) | Abstractions | Traits and their implementors — the extension points (packings, projections, targets). |
| [`03-composition.md`](03-composition.md) | Data types | How a decoded message owns its sections / templates, per format. |

Read top-down: crates → trait seams → composition.

## Keeping these honest

The diagrams are curated, not exhaustive — they document the *seams and section
composition* where design decisions live, deliberately omitting field-level
getters. The fact set they rest on can be re-derived from the source:

```sh
# realizations behind 02-trait-seams.md
grep -rhoE '^impl( <[^>]+>)? [A-Za-z0-9_]+ for [A-Za-z0-9_]+' crates/*/src | sort -u

# the public type inventory behind 03-composition.md
grep -rhoE 'pub (struct|enum|trait) [A-Za-z0-9_]+' crates/*/src | sort -u
```

If a trait gains or loses an implementor, or a message type restructures its
sections, update the relevant diagram in the same PR — same discipline as the
README "GRIB2 packing modes" table and the eccodes reference snapshots. A
`tools/` extractor + CI drift check (fail when an `impl … for …` exists that no
diagram mentions) would automate this; not yet wired up.
