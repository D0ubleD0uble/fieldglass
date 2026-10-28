# Architecture diagrams

Fieldglass turns a weather-data file (GRIB1, GRIB2 or NetCDF) or a Zarr store
into a grid of values, plus the geometry that places them on a map. Decoding,
projection and rendering stay separate, so adding a format or a packing
doesn't touch the rest.

Three [Mermaid](https://mermaid.js.org/) diagrams trace that pipeline, from
the whole workspace down to what one file holds. Read them in order:

| File | Scope | How it works |
| --- | --- | --- |
| [`01-crates.md`](01-crates.md) | Workspace | Each format crate decodes a container, and `core` projects and renders the result. |
| [`02-trait-seams.md`](02-trait-seams.md) | Dispatch | The traits a reader calls through to pick a packing, projection or target at run time. |
| [`03-composition.md`](03-composition.md) | Per format | How a file parses into a message, and the sections the message holds. |

The same three views, drawn for the open milestones, are under
[`planned/`](planned/README.md). Those aren't checked against the code, since
the checker reads only this directory's top level, and each planned node names
its issue. When a milestone closes, its changes are folded into the diagrams
here and deleted there.

## Keeping these honest

The diagrams are chosen, not exhaustive. They trace the seams and how each
message is put together, which is where the design decisions live, and leave
out most individual fields. You can re-derive the facts they rest on from the
source:

```sh
# realizations behind 02-trait-seams.md
grep -rhoE '^impl( <[^>]+>)? [A-Za-z0-9_]+ for [A-Za-z0-9_]+' crates/*/src | sort -u

# the public type inventory behind 03-composition.md
grep -rhoE 'pub (struct|enum|trait) [A-Za-z0-9_]+' crates/*/src | sort -u
```

### The drift check

`tools/check_architecture_diagrams.py` checks the parts a script can check. It
runs on every commit through pre-commit (the `architecture-diagrams` hook), and
so in CI too, through `pre-commit run --all-files`. To run it yourself:

```sh
python3 tools/check_architecture_diagrams.py        # the checks
python3 tools/test_check_architecture_diagrams.py   # the checker's own tests
```

Here's what it checks in each diagram:

- In `01-crates.md`, the flowchart's edges match the path dependencies under
  `[dependencies]` between workspace crates. Dev and build dependencies are
  ignored.
- In `02-trait-seams.md`, every `impl <FirstPartyTrait> for <Type>` in
  `crates/*/src` (leaving out `#[cfg(test)]` mocks) is drawn as a
  `Trait <|.. Type` edge, and every edge has a matching impl. A first-party
  trait that's deliberately left out goes in the script's
  `UNDIAGRAMMED_TRAITS` set, with a reason.
- In `03-composition.md`, the check is partial: every type node must still
  exist as a `pub struct`, `pub enum` or `pub trait`. That catches renames and
  deletions, but not a missing piece. Adding a new section to a message
  without drawing it won't fail. That part stays a matter of care, like the
  README's "GRIB2 packing modes" table and the ecCodes snapshots.

The checker reads only ` ```mermaid ` blocks, so prose can mention edge syntax
freely. It gives two warnings that don't fail the check: a NOTE when a
`macro_rules!` could hide a trait impl from its pattern, and a WARNING when two
public types share a base name (such as `IndicatorSection` in grib1 and grib2),
because it matches by base name.
