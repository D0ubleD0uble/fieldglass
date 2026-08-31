# Architecture diagramming: what we use, and why

Written while adding [`model/`](model/README.md). It records the comparison so
the choice does not get re-made from scratch, and so the next person can tell
when it should be.

## The question

The Mermaid diagrams in this directory are good at what they do: they render
inline on GitHub, need no toolchain, and the ones at the top level are
drift-guarded against the source. What they cannot do is **move between
altitudes**. `01-crates.md`, `02-trait-seams.md` and `03-composition.md` are
three separate files that a reader stitches together by hand — nothing in
`01` says "click `fieldglass-core` to see its seams", and nothing in `03` says
"this template enum is one box in `01`".

That is the gap: one model, many zoom levels, navigable in both directions.

## The landscape, in three groups

**1. Diagram-as-code, general purpose.** Mermaid, PlantUML, D2, Graphviz.
You write a picture; the tool lays it out.

Mermaid is the popular default and has been for a while — it renders natively
in GitHub, GitLab, Notion and Obsidian with no server, which is why most teams
use it for repo-level docs. PlantUML is the more powerful UML tool and holds
up better on large, dense class and sequence diagrams; it is what tends to
live in Confluence. D2 is the newer entrant with the best automatic layout of
the three.

The limitation is structural, not cosmetic: **each file is a picture, not a
model.** Two diagrams that mention `fieldglass-core` have no idea they are
talking about the same thing. Consistency is a discipline you enforce from
outside — which is exactly what `tools/check_architecture_diagrams.py` is.

**2. Model-as-code, C4-shaped.** Structurizr, LikeC4.

You describe a *model* — elements, their nesting, their relationships — once,
and then declare **views** onto it. A crate is one object; the crate diagram
and the seam diagram are two views of it, so they cannot disagree. This is the
group that gives you zoom, because zoom is just "the view scoped to this
element".

[Structurizr](https://structurizr.com/) is the reference implementation: it is
by Simon Brown, who created the [C4 model](https://c4model.com/), and it is the
most principled option if strict C4 conformance matters. Its four levels
(context, container, component, code) are the point, and also the constraint.

[LikeC4](https://github.com/likec4/likec4) is the same idea with the level
count unfixed: you define your own element kinds and nest as deep as the
subject warrants. MIT, ~5.6k stars, actively developed, and it ships a CLI, a
VS Code extension, a Vite plugin, a React component library, an MCP server, and
exports to PNG, SVG, JSON, Mermaid, PlantUML, D2 and draw.io.

**3. Hosted, visual-first.** IcePanel, Lucidchart, Miro, draw.io.

IcePanel is the strongest of these for C4 specifically — it zooms through the
three C4 levels and is built for architecture reviews and stakeholder
sessions, with real-time multi-user editing that code-based tools cannot
match. It is a SaaS product (free tier, then per-seat), and the model lives in
their app rather than in the repo.

## What "best" means depends on who maintains it

There is no single winner. The honest split:

| If the constraint is | Reach for |
| --- | --- |
| Renders everywhere with zero setup | **Mermaid** |
| Dense UML, sequence-heavy, lives in a wiki | **PlantUML** |
| Strict C4 conformance, one model many views | **Structurizr** |
| One model, arbitrary nesting, interactive output, in-repo | **LikeC4** |
| Non-engineers editing alongside engineers | **IcePanel** or Lucidchart |

## What this repo uses, and why

**Both Mermaid and LikeC4, deliberately.**

Mermaid stays for `01-`, `02-`, `03-` and `planned/`. Those files render in a
pull request diff and in an editor with no toolchain at all, and the top-level
three are checked against the source on every commit. That is worth keeping
exactly as it is.

LikeC4 was added in [`model/`](model/README.md) for the navigable view. The
reasons it won over the alternatives, in the order they mattered here:

- **Nesting is not capped at four levels.** Fieldglass genuinely goes five
  deep — workspace, crate, module, seam, implementer — and a §5 packing is a
  sixth thing under `DataRepresentationTemplate`. Structurizr's four C4 levels
  would have meant either flattening the seams or abusing "component" for two
  different altitudes.
- **The vocabulary is ours.** `seam`, `impl`, `part`, `decision` are the words
  this codebase already uses. C4's container/component vocabulary describes
  deployable services, which is not what a Cargo workspace is.
- **Text in the repo, reviewed in a diff.** Same property that makes the
  Mermaid files work, and the same reason `docs/decisions/` are Markdown.
- **Output is a static single file.** `--output-single-file` inlines
  everything, layout included (Graphviz compiled to WASM runs in the browser),
  so a shareable interactive diagram needs no server and no network.
- **MIT, no account, no seat.** The model is ours whether or not the tool
  survives; `likec4 export` writes Mermaid, PlantUML, D2 and draw.io if it does
  not.

IcePanel would be the better answer if the audience were mostly non-engineers,
or if several people edited the architecture at once. Neither is true here.

## When to revisit this

- If a second maintainer starts editing the architecture regularly and the
  `.c4` merge conflicts get painful, the hosted-tool trade looks different.
- If the model and the Mermaid diagrams drift apart in practice, collapse them:
  `likec4 export` can generate the Mermaid, and the drift guard would then
  check generated output — which is a bigger change to
  `tools/check_architecture_diagrams.py` than it sounds, since the guard's
  value is that it reads the *source*, not the model.
- If LikeC4 goes unmaintained, the model is portable: the exports above, or a
  hand-port to Structurizr DSL, which is close enough to be mechanical.

## Sources

- [C4 model](https://c4model.com/) — the four-level abstraction this borrows from
- [Structurizr](https://structurizr.com/) and [why "as code"](https://docs.structurizr.com/as-code)
- [LikeC4](https://github.com/likec4/likec4)
- [IcePanel vs Structurizr](https://icepanel.io/blog/2025-11-14-icepanel-vs-structurizr) (by IcePanel; read accordingly)
- [Mermaid](https://mermaid.js.org/), [PlantUML](https://plantuml.com/), [D2](https://d2lang.com/)
