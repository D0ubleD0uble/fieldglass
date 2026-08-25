#!/usr/bin/env python3
"""Build the GRIB2 octahedral reduced Gaussian fixture and its oracle line.

eccodes 2.34.1 ships **no octahedral sample** — every `reduced_gg_*` template in
`samples/` answers `isOctahedral = 0`, checked rather than assumed — and the
crate's own `reduced_gaussian_pressure_level.grib2` is a classic `N32`. So the
`O` half of the grid naming in `GridDefinition::size_label` had no fixture to be
wrong against, which is the half a test can pass without exercising (#500).

This builds one: take the stock classic `N32` sample, replace its `PL` list with
the octahedral row widths, and set a value per point so the message stays
self-consistent (`numberOfDataPoints` and the values array both follow the new
`PL` sum, which is 5248 rather than the classic 6114).

**The octahedral rule, and why the fixture is what it is.** Row widths rise from
20 at the pole by exactly four per row to the equator, then fall by four again:
`20, 24, ... 144, 144, 140, ... 20` for `N = 32`. That sequence is what eccodes'
`is_pl_octahedral` recognises — every step `+4`, one `0` plateau at the equator,
then every step `-4` — so a fixture built this way exercises the plateau case as
well as the two slopes, which a formula comparing only the equatorial row would
not.

**Encode with the wheel, verify with the pin.** Setting `pl` needs
`codes_set_array`, which the CLI has no equivalent for, so the PyPI wheel writes
the message; eccodes 2.34.1 on PATH is then the oracle that says what was
written, exactly as `NOTICE.md` requires. The pin is not too old here — it has
answered `isOctahedral` since grib_api 1.14.0 and `gridName` since 1.14.4.

Usage:  python3 tools/build_grib2_octahedral_fixture.py
Needs:  the `eccodes` PyPI wheel (encoding) and eccodes 2.34.1 on PATH (oracle).
"""

from __future__ import annotations

import pathlib
import subprocess
import sys

import eccodes as ec
import numpy as np

FIXTURES = pathlib.Path(__file__).resolve().parent.parent / (
    "crates/fieldglass-grib2/tests/fixtures"
)

# The stock classic sample this is derived from, and the resolution. N = 32
# keeps the message to about 16 kB — small enough to commit whole — while still
# giving 64 rows, enough for the step ordering to mean something.
SAMPLE = pathlib.Path("/usr/share/eccodes/samples/reduced_gg_pl_32_grib2.tmpl")
N_PARALLELS = 32
OUT = FIXTURES / "octahedral_gaussian_o32.grib2"

# What the pin must say about the result. Asserted rather than printed: a
# builder that quietly writes a classic grid would leave the `O` branch
# untested while looking like it had covered it.
EXPECTED = {"gridName": "O32", "isOctahedral": "1", "N": "32", "Nj": "64"}


def octahedral_row_widths(n_parallels: int) -> list[int]:
    """`20, 24, ... , 20 + 4(N-1)` down to the equator, then mirrored."""
    northern = [20 + 4 * row for row in range(n_parallels)]
    return northern + northern[::-1]


def oracle(path: pathlib.Path, key: str) -> str:
    """Ask the *pinned* eccodes CLI what it reads back."""
    return subprocess.run(
        ["grib_get", "-p", key, str(path)],
        check=True,
        capture_output=True,
        text=True,
        encoding="utf-8",
    ).stdout.strip()


def main() -> int:
    if not SAMPLE.is_file():
        raise SystemExit(f"{SAMPLE} is missing; install eccodes' sample files")
    widths = octahedral_row_widths(N_PARALLELS)
    total = sum(widths)

    with SAMPLE.open("rb") as source:
        handle = ec.codes_grib_new_from_file(source)
    try:
        ec.codes_set(handle, "numberOfDataPoints", total)
        ec.codes_set_array(handle, "pl", np.array(widths, dtype=np.int64))
        # A ramp rather than a constant: a decoder that loses track of which row
        # it is in produces a picture that is wrong in a visible way.
        ec.codes_set_values(handle, np.arange(total, dtype=np.float64) % 50.0)
        OUT.write_bytes(ec.codes_get_message(handle))
    finally:
        ec.codes_release(handle)

    for key, want in EXPECTED.items():
        got = oracle(OUT, key)
        if got != want:
            raise SystemExit(f"{OUT.name}: eccodes reads {key}={got!r}, expected {want!r}")
    print(
        f"{OUT.name}: {len(widths)} rows, {total} points, "
        f"gridName={EXPECTED['gridName']} (verified against the pin)",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
