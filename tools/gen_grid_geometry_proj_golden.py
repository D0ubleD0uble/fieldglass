#!/usr/bin/env python3
"""Regenerate the PROJ golden for `GridGeometry`'s projected families.

`GridGeometry::proj4` claims that a browser map library handed that string
places the grid where Fieldglass places it. This script is what checks the
claim, using PROJ as an oracle that shares no code with `core`:

    x0, y0 = PROJ_forward(lon_first, lat_first)      # the grid origin
    x,  y  = x0 + i·dx, y0 + j·dy                    # step out in metres
    lat, lon = PROJ_inverse(x, y)                    # and come back

which is `PlanarGridProjector::grid_point_lonlat` written in PROJ instead of
Rust. The test asserts `GridGeometry::forward(i, j)` agrees. It also asserts
the proj4 string itself matches, so changing `proj4()` fails the test until
this script is re-run and PROJ has had its say about the new string.

The grids are the real ones the round-trip test already uses (see
`crates/fieldglass-core/tests/grid_round_trip.rs`), not convenient synthetic
ones — #488 is what that lesson cost.

Usage:  python3 tools/gen_grid_geometry_proj_golden.py
Needs:  PROJ's `proj` and `projinfo` on PATH (9.4.0 pinned in the golden).
"""

from __future__ import annotations

import json
import pathlib
import subprocess
import sys

GOLDEN = (
    pathlib.Path(__file__).resolve().parent.parent
    / "crates/fieldglass-core/tests/grid_geometry_proj.golden.json"
)

# Every 7th point in each direction, plus the far edges: enough to catch a
# wrong cone constant or a flipped sign without committing 12,825 rows.
STRIDE = 7

CASES = [
    {
        "name": "NCEP Eta 93x65 Lambert conformal",
        "geometry": {
            "kind": "lambert",
            "earth_radius_m": 6371229.0,
            "ni": 93,
            "nj": 65,
            "lat_first": 12.19,
            "lon_first": -133.459,
            "lad": 25.0,
            "lov": -95.0,
            "dx_metres": 81271.0,
            "dy_metres": 81271.0,
            "latin1": 25.0,
            "latin2": 25.0,
        },
    },
    {
        # The CMC regional grid behind cmc_wind_300_2010052400_p012.grib. Its
        # far corner reaches 4.7 degS, which is what #488 was about.
        "name": "CMC regional 135x95 polar stereographic",
        "geometry": {
            "kind": "polar_stereo",
            "earth_radius_m": 6371229.0,
            "ni": 135,
            "nj": 95,
            "lat_first": 11.43,
            "lon_first": -110.27,
            "lov": 247.0,
            "lad": 60.0,
            "dx_metres": 60000.0,
            "dy_metres": 60000.0,
            "south_pole": False,
        },
    },
    {
        # The southern mirror. Polar stereographic is where a sign convention
        # goes wrong silently, so both hemispheres are pinned.
        "name": "south polar stereographic reaching north",
        "geometry": {
            "kind": "polar_stereo",
            "earth_radius_m": 6371229.0,
            "ni": 135,
            "nj": 95,
            "lat_first": -11.43,
            "lon_first": -110.27,
            "lov": 247.0,
            "lad": 60.0,
            "dx_metres": 60000.0,
            "dy_metres": 60000.0,
            "south_pole": True,
        },
    },
]


def proj4_for(g: dict) -> str:
    """Mirror `GridGeometry::proj4`. The test asserts the two agree, so a
    change on either side has to be made on both, deliberately."""
    if g["kind"] == "lambert":
        return (
            f"+proj=lcc +lat_1={fmt(g['latin1'])} +lat_2={fmt(g['latin2'])} "
            f"+lat_0={fmt(g['lad'])} +lon_0={fmt(g['lov'])} "
            f"+R={fmt(g['earth_radius_m'])} +units=m +no_defs"
        )
    if g["kind"] == "polar_stereo":
        lat0 = -90.0 if g["south_pole"] else 90.0
        lat_ts = -abs(g["lad"]) if g["south_pole"] else abs(g["lad"])
        return (
            f"+proj=stere +lat_0={fmt(lat0)} +lat_ts={fmt(lat_ts)} "
            f"+lon_0={fmt(g['lov'])} +R={fmt(g['earth_radius_m'])} "
            "+units=m +no_defs"
        )
    raise ValueError(f"no projected CRS for {g['kind']!r}")


def fmt(v: float) -> str:
    """Rust's `{}` for f64: an integral value prints without a fraction."""
    return str(int(v)) if float(v).is_integer() else repr(float(v))


def run_proj(crs: str, rows: list[tuple[float, float]], inverse: bool) -> list:
    """One `proj` invocation for the whole batch. `-f %.9f` keeps sub-micron
    precision on metres and sub-millimetre on degrees."""
    argv = ["proj", "-f", "%.9f"] + (["-I"] if inverse else []) + crs.split()
    stdin = "".join(f"{a} {b}\n" for a, b in rows)
    out = subprocess.run(
        argv,
        input=stdin,
        capture_output=True,
        text=True,
        encoding="utf-8",
        check=True,
    ).stdout
    parsed = []
    for line in out.strip().splitlines():
        fields = line.replace("\t", " ").split()
        if any(f in ("*", "inf", "-inf", "nan") for f in fields[:2]):
            raise SystemExit(f"PROJ could not transform a point: {line!r}")
        parsed.append((float(fields[0]), float(fields[1])))
    return parsed


def proj_version() -> str:
    """`proj` with no arguments prints "Rel. 9.4.0, ..." to stderr and exits
    non-zero, which is why this neither checks the status nor reads stdout."""
    out = subprocess.run(
        ["proj"], capture_output=True, text=True, encoding="utf-8"
    ).stderr
    for tok in out.replace(",", " ").split():
        if tok[0].isdigit() and "." in tok:
            return tok
    return "unknown"


def main() -> int:
    cases = []
    for case in CASES:
        g = case["geometry"]
        crs = proj4_for(g)
        ni, nj = g["ni"], g["nj"]

        # The grid origin in projected metres, from the stated first point.
        (x0, y0), = run_proj(crs, [(g["lon_first"], g["lat_first"])], inverse=False)

        idx = [
            (i, j)
            for j in sorted({*range(0, nj, STRIDE), nj - 1})
            for i in sorted({*range(0, ni, STRIDE), ni - 1})
        ]
        xy = [(x0 + i * g["dx_metres"], y0 + j * g["dy_metres"]) for i, j in idx]
        lonlat = run_proj(crs, xy, inverse=True)

        cases.append(
            {
                "name": case["name"],
                "geometry": g,
                "proj4": crs,
                "origin_xy": [x0, y0],
                "points": [
                    {"i": i, "j": j, "lat": lat, "lon": lon}
                    for (i, j), (lon, lat) in zip(idx, lonlat)
                ],
            }
        )

    GOLDEN.write_text(
        json.dumps(
            {
                "_comment": (
                    "Generated by tools/gen_grid_geometry_proj_golden.py. "
                    "PROJ is the oracle; do not hand-edit."
                ),
                "proj_version": proj_version(),
                "stride": STRIDE,
                "cases": cases,
            },
            indent=2,
        )
        + "\n",
        encoding="utf-8",
    )
    total = sum(len(c["points"]) for c in cases)
    print(f"wrote {GOLDEN.relative_to(pathlib.Path.cwd())}: "
          f"{len(cases)} grids, {total} points, PROJ {proj_version()}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
