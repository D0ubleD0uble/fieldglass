# Architecture diagrams

Fieldglass turns a weather-data file (GRIB1, GRIB2, or NetCDF) into a grid of
values plus the geometry to place them on a map. Decode, projection, and
rendering stay separate, so adding a format or packing does not touch the rest.

Three [Mermaid](https://mermaid.js.org/) diagrams trace that pipeline from the
workspace down to one file's contents. Read them in order:

| File | Scope | How it works |
| --- | --- | --- |
| [`01-crates.md`](01-crates.md) | Workspace | Each format crate decodes a container; `core` projects and renders the result. |
| [`02-trait-seams.md`](02-trait-seams.md) | Dispatch | The traits a reader calls through to pick a packing, projection, or target at runtime. |
| [`03-composition.md`](03-composition.md) | Per format | How a file parses into one message that holds its sections. |

## Keeping these honest

The diagrams are curated, not exhaustive. They trace the *seams and section
composition* where the design decisions live, not every field. The fact set
they rest on can be re-derived from the source:

```sh
# realizations behind 02-trait-seams.md
grep -rhoE '^impl( <[^>]+>)? [A-Za-z0-9_]+ for [A-Za-z0-9_]+' crates/*/src | sort -u

# the public type inventory behind 03-composition.md
grep -rhoE 'pub (struct|enum|trait) [A-Za-z0-9_]+' crates/*/src | sort -u
```

### Drift guard

`tools/check_architecture_diagrams.py` enforces the mechanically-checkable
parts and runs on commit via pre-commit (the `architecture-diagrams` hook), so
it also runs in CI through `pre-commit run --all-files`. Run it directly:

```sh
python3 tools/check_architecture_diagrams.py        # the checks
python3 tools/test_check_architecture_diagrams.py   # the checker's own tests
```

What it verifies, per diagram:

- **`01-crates.md`** — flowchart edges match the actual `[dependencies]`
  path-deps between workspace crates (dev/build-deps ignored).
- **`02-trait-seams.md`** — every `impl <FirstPartyTrait> for <Type>` in
  `crates/*/src` (excluding `#[cfg(test)]` mocks) is drawn as a
  `Trait <|.. Type` edge, and every edge has a matching impl. A deliberately
  undocumented first-party trait goes in the script's `UNDIAGRAMMED_TRAITS`
  set with a reason.
- **`03-composition.md`** — *partial*: every type node still exists as a
  `pub struct`/`pub enum`/`pub trait`. This catches renames and deletions, not
  incompleteness of the section ownership: adding a new section to a message
  without drawing it will **not** fail. That part stays manual, same discipline
  as the README "GRIB2 packing modes" table and the eccodes snapshots.

The checker only reads ` ```mermaid ` blocks, so prose may mention edge syntax
freely. Two advisories are non-fatal: a NOTE if a `macro_rules!` could hide a
trait impl from the regex, and a WARNING when two pub types share a base name
(e.g. `IndicatorSection` in grib1 and grib2) since matching is by base name.
