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

### Drift guard (automated for the trait seams)

`tools/check_architecture_diagrams.py` enforces `02-trait-seams.md`: every
`impl <FirstPartyTrait> for <Type>` in `crates/*/src` (excluding `#[cfg(test)]`
mocks) must appear as a `Trait <|.. Type` edge, and every such edge must have a
matching impl. It fails on either kind of drift. Run it directly:

```sh
python3 tools/check_architecture_diagrams.py
```

It runs automatically on commit via pre-commit (the `architecture-diagrams`
hook), so it also runs in CI through `pre-commit run --all-files`. A
deliberately-undocumented first-party trait goes in the script's
`UNDIAGRAMMED_TRAITS` set with a reason.

The composition diagram (`03-composition.md`) is not auto-checked — section
ownership changes rarely and is harder to verify mechanically. If a message
type restructures its sections, update it in the same PR, same discipline as
the README "GRIB2 packing modes" table and the eccodes reference snapshots.
