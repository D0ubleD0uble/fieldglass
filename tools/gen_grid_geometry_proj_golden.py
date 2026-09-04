#!/usr/bin/env python3
"""Regenerate the PROJ golden for `GridGeometry`'s projected families.

`GridGeometry::proj4` claims that a browser map library handed that string
places the grid where Fieldglass places it. This script is what checks the
claim, using PROJ as an oracle that shares no code with `core`:

    x0, y0 = the grid origin in the plane                (see `affine_for`)
    x,  y  = x0 + i·dx, y0 + j·dy                        # step out in metres
    lat, lon = PROJ_inverse(x, y)                        # and come back

which is `PlanarGridProjector::grid_point_lonlat` written in PROJ instead of
Rust. The test asserts `GridGeometry::forward(i, j)` agrees. It also asserts
the proj4 string and the affine themselves match, so changing either
`proj4()` or `plane_affine()` fails the test until this script is re-run and
PROJ has had its say about the new numbers.

The affine is PROJ's answer wherever PROJ can give one: for the families whose
message states a geographic first point, the origin is what PROJ forward-
projects it to, and for Mercator the steps are PROJ's too, since that family's
plane spacing is not a field of the message but a consequence of its corners.
Transverse Mercator states `X1`/`Y1` in the plane already, and a space-view
grid states scan angles, so those two are mirrored rather than derived — for
them the point-by-point comparison below is what checks the arithmetic.

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

# `fieldglass_core::DEFAULT_EARTH_RADIUS_M` — the WMO mean sphere, which is the
# radius the Mercator CRS states because that family's params carry none.
EARTH_RADIUS_M = 6371229.0

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
    {
        # UKV, the Met Office's 1.5 km UK model, coarsened to 24x30. On Airy
        # 1830 and with a real false easting/northing, which is why this is the
        # one family whose CRS carries `+x_0`/`+y_0`.
        "name": "Met Office UKV 24x30 transverse Mercator",
        "geometry": {
            "kind": "transverse_mercator",
            "semi_major_m": 6377563.0,
            "semi_minor_m": 6356257.0,
            "ni": 24,
            "nj": 30,
            "lat_ref": 49.0,
            "lon_ref": -2.0,
            "scale_factor": 0.9996012449264526,
            "false_easting_m": 400000.0,
            "false_northing_m": -100000.0,
            "x1_metres": -238000.0,
            "y1_metres": 1222000.0,
            "dx_metres": 48000.0,
            "dy_metres": -48000.0,
        },
    },
    {
        # The EFAS domain on ETRS89-LAEA (GRS80). The spheroid is load-bearing:
        # a mean radius puts the far corner 13.5 km out.
        "name": "EFAS 20x16 Lambert azimuthal equal-area",
        "geometry": {
            "kind": "lambert_azimuthal",
            "semi_major_m": 6378137.0,
            "semi_minor_m": 6356752.314,
            "ni": 20,
            "nj": 16,
            "lat_first": 35.0,
            "lon_first": -10.0,
            "standard_parallel": 52.0,
            "central_longitude": 10.0,
            "dx_metres": 200000.0,
            "dy_metres": 200000.0,
        },
    },
    {
        # A Mercator grid over the maritime continent. Its rows are uniform in
        # the Mercator ordinate and not in latitude, which is the whole claim
        # the affine below makes, so PROJ derives both steps rather than
        # reading them off the message.
        "name": "maritime continent 40x40 Mercator",
        "geometry": {
            "kind": "mercator",
            "ni": 40,
            "nj": 40,
            "lat_first": -40.0,
            "lon_first": 100.0,
            "lat_last": 40.0,
            "lon_last": 140.0,
        },
    },
    {
        # The GOES-16 ABI CONUS window, scaled to 250x150. Scan angles, not
        # metres: one radian is `+h` metres along the sight line, and that
        # factor is exactly what the point comparison checks.
        "name": "GOES-16 ABI CONUS 250x150 space view",
        "geometry": {
            "kind": "space_view",
            "ni": 250,
            "nj": 150,
            "h_metres": 42164160.0,
            "r_eq": 6378137.0,
            "r_pol": 6356752.31414,
            "sub_lon_deg": -75.0,
            "sweep_x": True,
            "x0": -0.101332,
            "dx_rad": (0.038612 - -0.101332) / (250 - 1),
            "y0": 0.128212,
            "dy_rad": (0.044268 - 0.128212) / (150 - 1),
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
    if g["kind"] == "mercator":
        return (
            f"+proj=merc +lat_ts=0 +lon_0=0 +R={fmt(EARTH_RADIUS_M)} "
            "+units=m +no_defs"
        )
    if g["kind"] == "transverse_mercator":
        return (
            f"+proj=tmerc +lat_0={fmt(g['lat_ref'])} +lon_0={fmt(g['lon_ref'])} "
            f"+k_0={fmt(g['scale_factor'])} +x_0={fmt(g['false_easting_m'])} "
            f"+y_0={fmt(g['false_northing_m'])} +a={fmt(g['semi_major_m'])} "
            f"+b={fmt(g['semi_minor_m'])} +units=m +no_defs"
        )
    if g["kind"] == "lambert_azimuthal":
        return (
            f"+proj=laea +lat_0={fmt(g['standard_parallel'])} "
            f"+lon_0={fmt(g['central_longitude'])} +a={fmt(g['semi_major_m'])} "
            f"+b={fmt(g['semi_minor_m'])} +units=m +no_defs"
        )
    if g["kind"] == "space_view":
        return (
            f"+proj=geos +h={fmt(g['h_metres'] - g['r_eq'])} "
            f"+lon_0={fmt(g['sub_lon_deg'])} "
            f"+sweep={'x' if g['sweep_x'] else 'y'} "
            f"+a={fmt(g['r_eq'])} +b={fmt(g['r_pol'])} +units=m +no_defs"
        )
    raise ValueError(f"no projected CRS for {g['kind']!r}")


def fmt(v: float) -> str:
    """Rust's `{}` for f64: an integral value prints without a fraction."""
    return str(int(v)) if float(v).is_integer() else repr(float(v))


def run_proj(
    crs: str,
    rows: list[tuple[float, float]],
    inverse: bool,
    allow_invalid: bool = False,
) -> list:
    """One `proj` invocation for the whole batch. `-f %.9f` keeps sub-micron
    precision on metres and sub-millimetre on degrees.

    A point PROJ cannot transform is an error unless `allow_invalid`, where it
    comes back as `None`. Only the space-view grid needs that: its corners look
    past the limb, and "there is no place on Earth here" is an answer the
    geometry has to give too, not a failure of the oracle.
    """
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
            if allow_invalid:
                parsed.append(None)
                continue
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


def eastward_lon_span(lon_first: float, lon_last: float) -> float:
    """`fieldglass_core::eastward_lon_span`: the span going east, so a grid
    published from 180E keeps it instead of collapsing to one cell."""
    span = lon_last - lon_first
    return span + 360.0 if span < 0.0 else span


def affine_for(g: dict, crs: str) -> tuple[float, float, float, float]:
    """`(x0, y0, dx, dy)` in the plane `crs` describes — what
    `GridGeometry::plane_affine` must report for the same grid.

    PROJ supplies whatever it can. The two families whose message already
    states the plane (transverse Mercator's `X1`/`Y1`, a space view's scan
    angles) are mirrored instead; nothing is left for PROJ to say about an
    origin the message spells out, and the per-point comparison still checks
    that the plane they name is the one PROJ lays out.
    """
    kind = g["kind"]
    if kind == "transverse_mercator":
        return g["x1_metres"], g["y1_metres"], g["dx_metres"], g["dy_metres"]
    if kind == "space_view":
        # One radian of scan angle is `+h` metres, and `+h` is the height above
        # the ellipsoid rather than the distance from its centre.
        h = g["h_metres"] - g["r_eq"]
        return h * g["x0"], h * g["y0"], h * g["dx_rad"], h * g["dy_rad"]

    # The rest state a geographic first point, so the origin is where PROJ
    # puts it.
    (x0, y0), = run_proj(crs, [(g["lon_first"], g["lat_first"])], inverse=False)
    if kind == "mercator":
        # No `Di`/`Dj` in the message: the steps follow from the corners, and
        # PROJ is what turns those into the uniform plane spacing this family
        # claims to have.
        span = eastward_lon_span(g["lon_first"], g["lon_last"])
        (x_next, _), (_, y_last) = run_proj(
            crs,
            [
                (g["lon_first"] + span / (g["ni"] - 1), g["lat_first"]),
                (g["lon_first"], g["lat_last"]),
            ],
            inverse=False,
        )
        return x0, y0, x_next - x0, (y_last - y0) / (g["nj"] - 1)
    return x0, y0, g["dx_metres"], g["dy_metres"]


def main() -> int:
    cases = []
    for case in CASES:
        g = case["geometry"]
        crs = proj4_for(g)
        ni, nj = g["ni"], g["nj"]

        x0, y0, dx, dy = affine_for(g, crs)

        idx = [
            (i, j)
            for j in sorted({*range(0, nj, STRIDE), nj - 1})
            for i in sorted({*range(0, ni, STRIDE), ni - 1})
        ]
        xy = [(x0 + i * dx, y0 + j * dy) for i, j in idx]
        # Only a space view has pixels that are not places.
        off_disc = g["kind"] == "space_view"
        lonlat = run_proj(crs, xy, inverse=True, allow_invalid=off_disc)

        cases.append(
            {
                "name": case["name"],
                "geometry": g,
                "proj4": crs,
                # Every projected family measures its plane in metres; the
                # geographic ones have no case here.
                "affine": {"x0": x0, "y0": y0, "dx": dx, "dy": dy, "units": "Metres"},
                # `lat`/`lon` of `null` is PROJ saying the pixel looks past
                # the limb; the test asserts the geometry declines it too.
                "points": [
                    {"i": i, "j": j, "lat": None, "lon": None}
                    if ll is None
                    else {"i": i, "j": j, "lat": ll[1], "lon": ll[0]}
                    for (i, j), ll in zip(idx, lonlat)
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
