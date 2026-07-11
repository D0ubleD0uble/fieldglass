#!/usr/bin/env python3
"""Build the render panel's vector overlay assets from Natural Earth.

Writes `extension/media/<layer>-110m.json` for the boundary, lake, and river
layers. Run it to refresh them; do not hand-edit the output.

    python3 tools/build_overlay_layers.py

Source: Natural Earth 1:110m (public domain), via the `nvkelso/natural-earth-vector`
GeoJSON mirror. Natural Earth's terms: "All versions of Natural Earth raster +
vector map data found on this website are in the public domain."

The coastline layer (`coastline-110m.json`) predates this script and comes from
the same 1:110m physical coastline release; it is left alone so a regeneration
can't silently reshape the overlay that already ships. Pass `--coastline` to
rebuild it too.

Output shape matches the existing coastline asset, which `extension/src/overlay.ts`
reads: `{"source": "...", "lines": [[lon, lat, lon, lat, ...], ...]}` — flat
GeoJSON-order pairs per polyline, which the loader swaps to (lat, lon).

Polygon layers (lakes) are emitted as their boundary rings: the overlay pipeline
strokes lines and fills nothing, so a lake is drawn as its outline.
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path
from urllib.request import urlopen

MEDIA = Path(__file__).resolve().parent.parent / "extension/media"

# Coordinates are rounded to this many decimals — ~100 m at the equator, well
# under the 1:110m source's own precision, and it roughly halves the asset size.
PRECISION = 3

# Each source URL is a module-level constant built only from string literals, so
# a static analyzer can prove no user-controlled value ever reaches the fetch.
# The fetch below reads via single-argument `urlopen` and writes the body itself
# rather than using two-argument `urlretrieve`, matching the other download
# scripts in this directory (see `build_oisst_real_fixture.py`).
BASE = "https://raw.githubusercontent.com/nvkelso/natural-earth-vector/master/geojson"
BORDERS_URL = f"{BASE}/ne_110m_admin_0_boundary_lines_land.geojson"
LAKES_URL = f"{BASE}/ne_110m_lakes.geojson"
RIVERS_URL = f"{BASE}/ne_110m_rivers_lake_centerlines.geojson"
COASTLINE_URL = f"{BASE}/ne_110m_coastline.geojson"

# asset name → (source URL, human description for the asset's `source` field)
LAYERS = {
    "borders": (
        BORDERS_URL,
        "Natural Earth 1:110m admin-0 country boundary lines, land (public domain)",
    ),
    "lakes": (
        LAKES_URL,
        "Natural Earth 1:110m lakes (public domain)",
    ),
    "rivers": (
        RIVERS_URL,
        "Natural Earth 1:110m rivers and lake centerlines (public domain)",
    ),
    "coastline": (
        COASTLINE_URL,
        "Natural Earth 1:110m physical coastline (public domain)",
    ),
}


def fetch(url: str) -> dict:
    with urlopen(url) as response:
        return json.loads(response.read())


def rings(geometry: dict) -> list[list[list[float]]]:
    """Every coordinate ring in a geometry, as lists of [lon, lat] pairs.

    Lines contribute themselves; polygons contribute their boundary rings
    (exterior and any holes), since the overlay strokes outlines and fills
    nothing.
    """
    kind = geometry["type"]
    coords = geometry["coordinates"]
    if kind == "LineString":
        return [coords]
    if kind == "MultiLineString":
        return list(coords)
    if kind == "Polygon":
        return list(coords)
    if kind == "MultiPolygon":
        return [ring for polygon in coords for ring in polygon]
    raise SystemExit(f"unhandled geometry type {kind!r}")


def build(layer: str) -> None:
    url, description = LAYERS[layer]
    geojson = fetch(url)
    lines: list[list[float]] = []
    for feature in geojson["features"]:
        for ring in rings(feature["geometry"]):
            flat: list[float] = []
            for lon, lat, *_ in ring:
                flat.extend((round(lon, PRECISION), round(lat, PRECISION)))
            # A single point cannot be stroked; drop it rather than ship it.
            if len(flat) >= 4:
                lines.append(flat)
    out = MEDIA / f"{layer}-110m.json"
    # Trailing newline: the repo's end-of-file hook adds one, so emitting it here
    # keeps a fresh run byte-identical to what is committed.
    body = json.dumps({"source": description, "lines": lines}, separators=(",", ":"))
    out.write_text(body + "\n")
    points = sum(len(line) for line in lines) // 2
    print(f"{out.name}: {len(lines)} lines, {points} points, {out.stat().st_size} bytes")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--coastline",
        action="store_true",
        help="also rebuild coastline-110m.json (already committed; regenerating "
        "it reshapes the overlay that ships today)",
    )
    args = parser.parse_args()
    for layer in ("borders", "lakes", "rivers"):
        build(layer)
    if args.coastline:
        build("coastline")


if __name__ == "__main__":
    main()
