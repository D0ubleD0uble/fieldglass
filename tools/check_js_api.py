#!/usr/bin/env python3
"""Hold each JavaScript host's generated `.d.ts` to the one API shape (#1073).

    python3 tools/check_js_api.py --napi extension/bin/index.d.ts
    python3 tools/check_js_api.py --wasm crates/fieldglass-wasm/pkg/nodejs/fieldglass_wasm.d.ts
    python3 tools/check_js_api.py --table          # the two hosts side by side, as Markdown

The Node addon and the browser module are two bindings of one `Session`
(ADR-0006). They drifted apart when nothing compared them: four classes in one
and one in the other, `decodeGrid` against `decode`, snake_case parameters,
Rust doc links in the declarations. This is the comparison, written the way the
conformance suite is: the contract is data here, and each host is checked
against it on its own, because no CI job builds both.

What a host must satisfy:

1. **A shared method has the shared signature.** Every name in `SHARED` is on
   the host's `Handle` with exactly the parameters stated (names and types),
   and every function in `SHARED_FUNCTIONS` is exported with its parameters.
   Return types may differ: the bulk buffer each host hands back is its own
   (ADR-0006 decision 1).
2. **Nothing else is shared by accident.** Every other method on the host's
   `Handle`, and every other exported function, is listed for that host with
   the reason only it has it. A name both hosts use for different operations
   is the failure this exists to stop; `SAME_NAME_OTHER_INPUT` holds the one
   function that is the same operation reached from a different source.
3. **Parameters are camelCase.** A `_` in a parameter name is a Rust name
   leaking through.
4. **No Rust doc link.** `[`Name`]` or `](Self::...)` in a doc comment is
   markup a JavaScript reader cannot follow.
5. **The async names are free.** `load` and every `<name>Async` are reserved
   for the asynchronous layer (#1085), which lands beside the synchronous one
   without renaming it; neither host may declare one before then.

A host's run fails loudly when its file is missing, rather than passing on
nothing.
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

# ---------------------------------------------------------------------------
# The contract
# ---------------------------------------------------------------------------

# `Handle` methods both hosts bind, and the parameter list each takes.
SHARED: dict[str, str] = {
    "close": "",
    "format": "",
    "addressing": "",
    "count": "",
    "message": "index: number",
    "variables": "",
    "leftOut": "",
    "undrawable": "",
    "decode": "index: number, options?: DecodeOptions | null",
    "decodeSlice": "slice: SliceRequest, options?: DecodeOptions | null",
    "decodeLine": "line: LineRequest, options?: DecodeOptions | null",
    "decodeDisplay": "index: number, options?: DisplayDecodeOptions | null",
}

# Exported functions both hosts have, with their parameters.
SHARED_FUNCTIONS: dict[str, str] = {
    "open": "bytes: Uint8Array",
    "combineOps": "",
}

# One operation under one name, reached from the source each host has.
SAME_NAME_OTHER_INPUT: dict[str, str] = {
    "openStore": (
        "opens a Zarr store: from a directory path in Node, from the objects a "
        "page fetched in the browser"
    ),
}

# What only one host has, and why.
NAPI_ONLY: dict[str, str] = {
    "metadata": "the NetCDF metadata view the extension's editor shows",
    "renderGrid": "the extension's map pipeline: decode, project and paint in one call over a memo",
    "renderDisplayGrid": "the map pipeline for a field decoded to draw",
    "projectOverlay": "coastlines and graticules in the map pipeline's pixel space",
    "projectContours": "isolines in the map pipeline's pixel space",
    "projectVectors": "arrows in the map pipeline's pixel space",
    "probePixel": "a pixel of the map pipeline's raster; the browser's `probe` reads a point",
    "zonalMean": "the extension's zonal-mean plot",
    "exportCsv": "the extension's CSV export",
    "sliceGrid": "the slice picker's placement answers, without the geometry",
    "axisValues": "a cross-section's axis labels",
    "setP1": "the GRIB1 P1 edit",
}
NAPI_ONLY_FUNCTIONS: dict[str, str] = {
    "detectBytes": "the editor picks its view before opening",
    "isStore": "the folder picker refuses a non-store in its own words",
    "colormaps": "the picker's colormap list",
    "parseColorTable": "the colour-table import",
    "packingLabel": "the message table's packing column",
}
WASM_ONLY: dict[str, str] = {
    "free": "wasm-bindgen's own release, which `close` also is",
    "dimensions": "the shared dimensions a page builds one slider from",
    "vectorPairing": "whether two fields are a u/v pair, asked before drawing",
    "warp": "values resampled for a GPU texture",
    "warpDisplay": "values resampled for a GPU texture, for a field decoded to draw",
    "palette": "colour as data for a shader",
    "paletteDisplay": "colour as data for a shader, for a field decoded to draw",
    "render": "the CPU fallback over a field the page holds",
    "renderDisplay": "the CPU fallback for a field decoded to draw",
    "probe": "a field the page holds, at a geographic point",
    "probeMessage": "a message, at a geographic point",
    "probeFullDetail": "a band-limited spectral message's full sum at a point",
    "combine": "two fields the page holds, combined into a third",
    "contours": "isolines in grid space, for the page to draw",
    "shaderValues": "the values texture a shader reads",
    "shaderMask": "the mask texture a shader reads",
    "coordinateChunks": "the Zarr window plan's first round (#1083)",
    "planWindow": "the Zarr window plan (#1083)",
    "decodeWindow": "a Zarr window decoded from the fetched chunks (#1083)",
}
WASM_ONLY_FUNCTIONS: dict[str, str] = {
    "glslSnippet": "the shader snippet a GPU page pastes in",
    "parseWgrib2Idx": "the fetch planner (#1081)",
    "parseEcmwfIndex": "the fetch planner (#1081)",
    "httpRangeHeader": "the fetch planner (#1081)",
    "verifyEnvelope": "the fetch planner (#1081)",
    "verifyMessage": "the fetch planner (#1093)",
    "validateSource": "the fetch planner's catalogue",
    "candidates": "the fetch planner's catalogue",
}

# Reserved for the asynchronous layer (#1085).
RESERVED_FUNCTIONS = {"load"}
RESERVED_SUFFIX = "Async"

HOSTS = {
    "napi": (NAPI_ONLY, NAPI_ONLY_FUNCTIONS),
    "wasm": (WASM_ONLY, WASM_ONLY_FUNCTIONS),
}

# ---------------------------------------------------------------------------
# Reading a declaration file
# ---------------------------------------------------------------------------


def strip_comments(text: str) -> str:
    """Remove `/** … */` and `// …`, so prose is not read as a member."""
    text = re.sub(r"/\*.*?\*/", "", text, flags=re.DOTALL)
    return re.sub(r"//[^\n]*", "", text)


def class_body(text: str, name: str) -> str | None:
    """The body of `export (declare )?class <name>`, brace-counted."""
    m = re.search(r"export (?:declare )?class " + re.escape(name) + r"\b[^{]*\{", text)
    if not m:
        return None
    depth = 0
    for j in range(m.end() - 1, len(text)):
        if text[j] == "{":
            depth += 1
        elif text[j] == "}":
            depth -= 1
            if depth == 0:
                return text[m.end() : j]
    return None


def split_params(params: str) -> list[str]:
    """Top-level comma split, so `Map<string, Uint8Array>` stays one."""
    out, depth, cur = [], 0, ""
    for c in params:
        if c in "<([{":
            depth += 1
        elif c in ">)]}":
            depth -= 1
        if c == "," and depth == 0:
            out.append(cur)
            cur = ""
        else:
            cur += c
    if cur.strip():
        out.append(cur)
    return [p.strip() for p in out]


def normalise(params: str) -> str:
    """One spelling per parameter list: single spaces, an optional parameter's
    `| undefined` dropped (`x?: T` already admits it)."""
    out = []
    for p in split_params(params):
        p = re.sub(r"\s+", " ", p)
        m = re.match(r"(\w+)\?: (.*)$", p)
        if m:
            types = [t.strip() for t in m.group(2).split("|") if t.strip() != "undefined"]
            p = f"{m.group(1)}?: " + " | ".join(types)
        out.append(p)
    return ", ".join(out)


MEMBER_RE = re.compile(r"^\s*(?P<name>\[[^\]]+\]|[A-Za-z_]\w*)\s*\((?P<params>[^)]*)\)", re.M)
FUNCTION_RE = re.compile(
    r"^export (?:declare )?function (?P<name>\w+)\s*\((?P<params>[^)]*)\)", re.M
)


def members(body: str) -> dict[str, str]:
    """Method name -> its parameter list, constructors and symbols left out."""
    out: dict[str, str] = {}
    for m in MEMBER_RE.finditer(body):
        name = m.group("name")
        if name.startswith("[") or name == "constructor":
            continue
        out[name] = normalise(m.group("params"))
    return out


def functions(text: str) -> dict[str, str]:
    return {m.group("name"): normalise(m.group("params")) for m in FUNCTION_RE.finditer(text)}


# ---------------------------------------------------------------------------
# The checks
# ---------------------------------------------------------------------------

DOC_LINK_RE = re.compile(r"\[`[^`\]]+`\](?!\()|\]\((?:Self|crate|super|fieldglass)\b")


def check(host: str, text: str) -> list[str]:
    """Every way `text`, the host's generated declarations, breaks the contract."""
    only, only_functions = HOSTS[host]
    other_only, other_functions = next(v for k, v in HOSTS.items() if k != host)
    problems: list[str] = []

    for m in DOC_LINK_RE.finditer(text):
        line = text.count("\n", 0, m.start()) + 1
        problems.append(f"line {line}: a Rust doc link reached the declarations: {m.group(0)!r}")

    code = strip_comments(text)
    body = class_body(code, "Handle")
    if body is None:
        return problems + ["no `Handle` class is declared"]
    methods = members(body)
    exported = functions(code)

    for name, params in SHARED.items():
        if name not in methods:
            problems.append(f"Handle.{name} is shared and missing")
        elif methods[name] != normalise(params):
            problems.append(
                f"Handle.{name}({methods[name]}) differs from the shared ({normalise(params)})"
            )
    for name, params in SHARED_FUNCTIONS.items():
        if name not in exported:
            problems.append(f"function {name} is shared and missing")
        elif exported[name] != normalise(params):
            problems.append(
                f"function {name}({exported[name]}) differs from the shared ({normalise(params)})"
            )

    for name in methods:
        if name in SHARED:
            continue
        if name in other_only:
            problems.append(
                f"Handle.{name} is the other host's operation ({other_only[name]}); "
                "a name both hosts use must mean one operation — add it to SHARED or rename it"
            )
        elif name not in only:
            problems.append(f"Handle.{name} is in neither SHARED nor the {host} list; classify it")
    for name in exported:
        if name in SHARED_FUNCTIONS or name in SAME_NAME_OTHER_INPUT:
            continue
        if name in other_functions:
            problems.append(f"function {name} is the other host's ({other_functions[name]})")
        elif name not in only_functions:
            problems.append(f"function {name} is in neither shared list nor the {host} list")

    for scope, table in (("Handle", methods), ("function", exported)):
        for name, params in table.items():
            for p in split_params(params):
                pname = re.match(r"\w+", p)
                if pname and "_" in pname.group(0):
                    problems.append(f"{scope} {name}: parameter `{pname.group(0)}` is not camelCase")
            if name in RESERVED_FUNCTIONS or name.endswith(RESERVED_SUFFIX):
                problems.append(f"{scope} {name} uses a name reserved for the async layer (#1085)")
    return problems


def table() -> str:
    """The two hosts side by side: what the PR shows as the diff of the surfaces."""
    rows = ["| | Node addon (`fieldglass-napi`) | Browser module (`@fieldglass/wasm`) |", "|---|---|---|"]
    for name, params in SHARED.items():
        sig = _cell(f"`{name}({params})`")
        rows.append(f"| `Handle.{name}` | {sig} | {sig} |")
    for name, params in SHARED_FUNCTIONS.items():
        sig = _cell(f"`{name}({params})`")
        rows.append(f"| `{name}` | {sig} | {sig} |")
    for name, why in SAME_NAME_OTHER_INPUT.items():
        rows.append(f"| `{name}` | `{name}(path)` | `{name}(objects)` — {why} |")
    for name, why in NAPI_ONLY.items():
        rows.append(f"| `Handle.{name}` | {why} | — |")
    for name, why in WASM_ONLY.items():
        rows.append(f"| `Handle.{name}` | — | {why} |")
    return "\n".join(rows)


def _cell(text: str) -> str:
    """A Markdown table cell: a `|` inside it would end the cell."""
    return text.replace("|", "\\|")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--napi", type=Path, help="the addon's generated index.d.ts")
    parser.add_argument("--wasm", type=Path, action="append", default=[], help="a browser build's .d.ts")
    parser.add_argument("--table", action="store_true", help="print the side-by-side table")
    args = parser.parse_args(argv)
    if args.table:
        print(table())
        return 0
    runs = ([("napi", args.napi)] if args.napi else []) + [("wasm", p) for p in args.wasm]
    if not runs:
        parser.error("name a host's declarations with --napi or --wasm")
    failed = False
    for host, path in runs:
        if not path.is_file():
            print(f"{path}: missing — build the {host} host first", file=sys.stderr)
            failed = True
            continue
        problems = check(host, path.read_text(encoding="utf-8"))
        for p in problems:
            print(f"{path}: {p}", file=sys.stderr)
        if problems:
            failed = True
        else:
            print(f"{path}: the {host} host matches the shared JavaScript API")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
