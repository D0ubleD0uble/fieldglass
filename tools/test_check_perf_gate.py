#!/usr/bin/env python3
"""Unit tests for tools/check_perf_gate.py.

A gate that passes everything is worse than none, so what the checker must
*catch* is pinned beside what it must let through: an exact metric that moves by
one, an instruction count past its tolerance, a scenario that appears or
disappears, a partial Gungraun run, a corpus that changed under the table. Run:

    python3 tools/test_check_perf_gate.py
"""

from __future__ import annotations

import argparse
import importlib.util
import json
import tempfile
import unittest
from pathlib import Path

_spec = importlib.util.spec_from_file_location(
    "check_perf_gate", Path(__file__).resolve().parent / "check_perf_gate.py"
)
assert _spec and _spec.loader
chk = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(chk)

DIGEST = "ab" * 32

DOC = f"""# Performance

Corpus digest: `{"00" * 32}`

{chk.TABLE_BEGIN}
{chk.TABLE_END}

{chk.BOUNDS_BEGIN}
{chk.BOUNDS_END}

Prose after.
"""

ORDER = [
    "grib2-5.0-L/decode",
    "grib2-5.0-S/decode",
    "netcdf-classic-D/slice",
    "netcdf-classic-S/slice",
    "netcdf-classic-L/slice",
    "grib2-5.42-S/codec",
]


def scenario(cells, bytes_, bound, blocks, peak, requests=1, width=4):
    return {
        "cells": cells, "width": width, "bytes": bytes_, "requests": requests, "bound_bytes": bound,
        "input_bytes": 0, "blocks": blocks, "peak": peak,
    }


def measured():
    return {
        "digest": DIGEST,
        "order": ORDER,
        "scenarios": {
            "grib2-5.0-L/decode": scenario(64, 1000, 1100, 17, 2000),
            "grib2-5.0-S/decode": scenario(16, 300, 400, 17, 600),
            "netcdf-classic-D/slice": scenario(16, 9000, 700, 120, 8000),
            "netcdf-classic-S/slice": scenario(16, 3000, 700, 120, 2400),
            "netcdf-classic-L/slice": scenario(64, 9000, 2600, 120, 9000),
            "grib2-5.42-S/codec": scenario(16, 0, None, 500, 30),
        },
    }


def write_gungraun(root: Path, counts: dict[str, int]) -> None:
    for index, name in enumerate(ORDER):
        if name not in counts:
            continue
        folder = root / f"run.scenario_{index}"
        folder.mkdir(parents=True)
        callgrind = {
            "Ir": {"metrics": {"Left": {"Int": counts[name]}}},
            "EstimatedCycles": {"metrics": {"Both": [{"Int": counts[name] * 2}, {"Int": 1}]}},
        }
        doc = {"id": f"scenario_{index}", "profiles": [{"summaries": {"total": {"summary": {"Callgrind": callgrind}}}}]}
        (folder / "summary.json").write_text(json.dumps(doc), encoding="utf-8")


COUNTS = {name: 1_000_000 + 1000 * i for i, name in enumerate(ORDER)}


def wasm(after=4_000_000):
    rows = {name: {"before": 1, "after": after, "ms": 1.0} for name in ORDER if "codec" not in name}
    return {"digest": DIGEST, "scenarios": rows}


class Workspace:
    """A temporary doc and a full set of inputs, which a test then perturbs."""

    def __init__(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name)
        self.doc = self.root / "performance.md"
        self.doc.write_text(DOC, encoding="utf-8")
        self.measured = self.root / "measured.json"
        self.gungraun = self.root / "gungraun"
        self.wasm = self.root / "wasm.json"
        self.wasm_simd = self.root / "wasm-simd.json"
        self.set(measured(), COUNTS, wasm())

    def set(self, m, counts, w) -> None:
        self.measured.write_text(json.dumps(m), encoding="utf-8")
        if self.gungraun.exists():
            for path in sorted(self.gungraun.rglob("*"), reverse=True):
                path.unlink() if path.is_file() else path.rmdir()
        write_gungraun(self.gungraun, counts)
        self.wasm.write_text(json.dumps(w), encoding="utf-8")
        self.wasm_simd.write_text(json.dumps(w), encoding="utf-8")

    def args(self, **overrides) -> argparse.Namespace:
        values = {
            "measured": self.measured, "instructions": self.gungraun,
            "wasm": [f"baseline={self.wasm}", f"+simd128={self.wasm_simd}"],
            "doc": self.doc, "only": None,
            "instruction_tolerance": chk.INSTRUCTION_TOLERANCE, "write": False,
        }
        values.update(overrides)
        return argparse.Namespace(**values)

    def record(self) -> None:
        chk.run(self.args(write=True))

    def close(self) -> None:
        self._tmp.cleanup()


class GateTest(unittest.TestCase):
    def setUp(self) -> None:
        self.ws = Workspace()
        self.ws.record()

    def tearDown(self) -> None:
        self.ws.close()

    def failures(self, **overrides) -> list[str]:
        return chk.run(self.ws.args(**overrides))[1]

    def test_a_fresh_recording_passes(self):
        self.assertEqual(self.failures(), [])

    def test_the_table_round_trips(self):
        rows = chk.measured_rows(measured(), {n: {"instructions": 5, "cycles": 6} for n in ORDER}, None)
        parsed = chk.parse_table(chk.render_table(rows), self.ws.doc)
        for name, row in rows.items():
            for key, _, _ in chk.COLUMNS:
                self.assertEqual(parsed[name][key], row.get(key), (name, key))

    def test_one_more_allocation_fails(self):
        m = measured()
        m["scenarios"]["grib2-5.0-S/decode"]["blocks"] += 1
        self.ws.set(m, COUNTS, wasm())
        failures = self.failures()
        self.assertEqual(len(failures), 1, failures)
        self.assertIn("grib2-5.0-S/decode: Allocations 18, recorded 17", failures[0])

    def test_a_smaller_number_fails_too(self):
        m = measured()
        m["scenarios"]["grib2-5.0-L/decode"]["peak"] -= 1
        self.ws.set(m, COUNTS, wasm())
        self.assertTrue(any("Peak heap" in f for f in self.failures()))

    def test_instructions_inside_the_tolerance_pass(self):
        counts = dict(COUNTS)
        counts["grib2-5.0-L/decode"] = int(counts["grib2-5.0-L/decode"] * 1.015)
        self.ws.set(measured(), counts, wasm())
        self.assertEqual(self.failures(), [])

    def test_instructions_past_the_tolerance_fail(self):
        counts = dict(COUNTS)
        counts["grib2-5.0-L/decode"] = int(counts["grib2-5.0-L/decode"] * 1.03)
        self.ws.set(measured(), counts, wasm())
        self.assertTrue(any("grib2-5.0-L/decode: Instructions" in f for f in self.failures()))

    def test_wasm_memory_gets_one_page(self):
        self.ws.set(measured(), COUNTS, wasm(4_000_000 + chk.WASM_PAGE))
        self.assertEqual(self.failures(), [])
        self.ws.set(measured(), COUNTS, wasm(4_000_000 + 2 * chk.WASM_PAGE))
        self.assertTrue(any("wasm memory" in f for f in self.failures()))

    def test_a_new_scenario_fails_until_recorded(self):
        m = measured()
        m["order"] = ORDER + ["grib2-5.0-S/place"]
        m["scenarios"]["grib2-5.0-S/place"] = scenario(0, 0, 10, 4, 100)
        counts = dict(COUNTS)
        self.ws.set(m, counts, wasm())
        with self.assertRaises(chk.Failure):
            # Gungraun has no summary for the new position: a partial run.
            self.failures()
        failures = self.failures(only="io,heap")
        self.assertTrue(any("grib2-5.0-S/place: in the measurement but not the table" in f for f in failures), failures)

    def test_a_scenario_missing_from_the_measurement_fails(self):
        m = measured()
        m["order"] = ORDER[:-1]
        del m["scenarios"]["grib2-5.42-S/codec"]
        self.ws.set(m, {name: COUNTS[name] for name in ORDER[:-1]}, wasm())
        failures = self.failures()
        self.assertTrue(any("grib2-5.42-S/codec: in the table but not the measurement" in f for f in failures), failures)

    def test_a_stale_summary_past_the_catalogue_is_refused(self):
        write_gungraun(self.ws.gungraun / "extra", {})
        folder = self.ws.gungraun / f"run.scenario_{len(ORDER)}"
        folder.mkdir()
        (folder / "summary.json").write_text(json.dumps({"id": f"scenario_{len(ORDER)}"}), encoding="utf-8")
        with self.assertRaises(chk.Failure):
            self.failures()

    def test_a_changed_corpus_is_named(self):
        m = measured()
        m["digest"] = "cd" * 32
        w = wasm()
        w["digest"] = m["digest"]
        self.ws.set(m, COUNTS, w)
        self.assertTrue(any("corpus digest" in f for f in self.failures()))

    def test_wasm_over_another_corpus_is_refused(self):
        w = wasm()
        w["digest"] = "ef" * 32
        self.ws.set(measured(), COUNTS, w)
        with self.assertRaises(chk.Failure):
            self.failures()

    def test_a_hand_edited_bounds_section_fails(self):
        text = self.ws.doc.read_text(encoding="utf-8").replace("holds", "grows +0")
        self.ws.doc.write_text(text, encoding="utf-8")
        self.assertTrue(any("bounds section" in f for f in self.failures()))

    def test_only_checks_the_named_tiers(self):
        m = measured()
        m["scenarios"]["grib2-5.0-S/decode"]["blocks"] += 1
        self.ws.set(m, COUNTS, wasm())
        self.assertEqual(self.failures(only="io,instructions,wasm"), [])
        self.assertNotEqual(self.failures(only="heap"), [])

    def test_only_still_checks_cells(self):
        m = measured()
        m["scenarios"]["grib2-5.0-S/decode"]["cells"] += 1
        self.ws.set(m, COUNTS, wasm())
        self.assertTrue(any("Cells" in f for f in self.failures(only="io")))

    def test_the_instructions_tier_without_its_input_is_refused(self):
        with self.assertRaises(chk.Failure):
            self.failures(instructions=None)

    def test_a_duplicated_row_is_refused(self):
        text = self.ws.doc.read_text(encoding="utf-8")
        row = next(line for line in text.splitlines() if line.startswith("| `grib2-5.0-S/decode`"))
        self.ws.doc.write_text(text.replace(row, f"{row}\n{row}"), encoding="utf-8")
        with self.assertRaises(chk.Failure):
            self.failures()

    def test_write_refuses_a_partial_tier_set(self):
        with self.assertRaises(chk.Failure):
            chk.run(self.ws.args(write=True, only="io"))


class BoundsTest(unittest.TestCase):
    def rows(self):
        return chk.measured_rows(measured(), {n: {"instructions": 4_000 if "-L" in n else 1_000, "cycles": 1} for n in ORDER}, None)

    def test_a_read_over_its_bound_is_listed(self):
        text = chk.render_bounds(self.rows(), set(chk.TIERS))
        self.assertIn("| `netcdf-classic-S/slice` | 3,000 | 700 | 4.3× |", text)
        self.assertNotIn("`grib2-5.0-S/decode` | 300", text)

    def test_allocations_that_do_not_move_hold(self):
        text = chk.render_bounds(self.rows(), set(chk.TIERS))
        self.assertIn("| `grib2-5.0/decode` | 17 | 17 | holds |", text)

    def test_the_per_cell_slope_cancels_the_fixed_cost(self):
        # peak(S) = 600 at 16 cells, peak(L) = 2000 at 64: B = 1400/48.
        text = chk.render_bounds(self.rows(), set(chk.TIERS))
        self.assertIn(f"| `grib2-5.0/decode` | {1400 / 48:.1f} B |", text)

    def test_the_floor_follows_the_decoded_width(self):
        text = chk.render_bounds(self.rows(), set(chk.TIERS))
        # 5.0 decodes to width 4: floor 5 B, and 1400/48 = 29.2 B is 5.8x it.
        self.assertIn(f"| `grib2-5.0/decode` | {1400 / 48:.1f} B | ", text)
        self.assertIn("| 5 B | 5.8× | output value (4) + mask byte (1)", text)
        # A classic slice reads in place: no chunk element in its floor.
        peak = text.split("#### Peak heap per cell")[1]
        self.assertIn("| 5 B |", peak.split("`netcdf-classic/slice`")[1].splitlines()[0])

    def test_the_variable_ratio_uses_the_deep_input(self):
        text = chk.render_bounds(self.rows(), set(chk.TIERS))
        self.assertIn("| `netcdf-classic/slice` | 3.33 | 1.00 | 1.00 |", text)


if __name__ == "__main__":
    unittest.main()
