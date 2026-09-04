#!/usr/bin/env python3
"""Unit tests for tools/check_wasm_bundle_size.py.

The checker is the only thing standing between the wasm bundle and a silent
doubling, so what it must *catch* is pinned alongside what it must let through —
a size gate that passes everything is worse than none. Run:

    python3 tools/test_check_wasm_bundle_size.py
"""

from __future__ import annotations

import importlib.util
import tempfile
import unittest
from pathlib import Path

_spec = importlib.util.spec_from_file_location(
    "check_wasm_bundle_size",
    Path(__file__).resolve().parent / "check_wasm_bundle_size.py",
)
assert _spec and _spec.loader
chk = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(chk)

TABLE = f"""# whatever

{chk.TABLE_MARKER}

| Build | `.wasm` bytes | gzipped bytes |
|---|---:|---:|
| baseline | 1,000,000 | 400,000 |
| `+simd128` | 999,000 | 399,000 |

Prose after the table.

| Build | `.wasm` bytes | gzipped bytes |
|---|---:|---:|
| decoy | 1 | 1 |
"""


class ParseTable(unittest.TestCase):
    def readme(self, text: str) -> Path:
        path = Path(self.tmp.name) / "README.md"
        path.write_text(text, encoding="utf-8")
        return path

    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)

    def test_reads_names_and_strips_separators(self) -> None:
        rows = chk.parse_readme_table(self.readme(TABLE))
        # Backticks off the name, thousands separators off the numbers.
        self.assertEqual(rows["baseline"], (1_000_000, 400_000))
        self.assertEqual(rows["+simd128"], (999_000, 399_000))

    def test_stops_at_the_end_of_the_first_table(self) -> None:
        # A later table in the same README must not be swept up: its rows would
        # become names the gate silently accepts.
        self.assertNotIn("decoy", chk.parse_readme_table(self.readme(TABLE)))

    def test_missing_marker_is_a_failure(self) -> None:
        with self.assertRaises(chk.Failure):
            chk.parse_readme_table(self.readme(TABLE.replace(chk.TABLE_MARKER, "")))

    def test_marker_without_byte_rows_is_a_failure(self) -> None:
        # The header and the `|---|` rule are not rows. A table whose numbers
        # were reworded into prose must fail rather than gate on nothing.
        gutted = f"{chk.TABLE_MARKER}\n\n| Build | size |\n|---|---|\n| baseline | small |\n"
        with self.assertRaises(chk.Failure):
            chk.parse_readme_table(self.readme(gutted))

    def test_the_real_readme_parses(self) -> None:
        # Guards the marker and the table shape in the file CI actually reads.
        rows = chk.parse_readme_table(chk.DEFAULT_README)
        self.assertEqual(set(rows), {"baseline", "+simd128"})


class Gzip(unittest.TestCase):
    def test_is_deterministic(self) -> None:
        # `mtime=0`: without it the gzip header carries the clock and the same
        # bytes measure differently on two runs.
        data = b"fieldglass" * 5000
        self.assertEqual(chk.gzipped_size(data), chk.gzipped_size(data))

    def test_is_smaller_than_the_input(self) -> None:
        self.assertLess(chk.gzipped_size(b"a" * 100_000), 100_000)


class Check(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.dir = Path(self.tmp.name)
        self.readme = self.dir / "README.md"

    def write_readme(self, raw: int, gz: int) -> None:
        self.readme.write_text(
            f"{chk.TABLE_MARKER}\n\n| Build | a | b |\n|---|---|---|\n"
            f"| baseline | {raw} | {gz} |\n",
            encoding="utf-8",
        )

    def wasm(self, data: bytes) -> Path:
        path = self.dir / "fieldglass_wasm_bg.wasm"
        path.write_bytes(data)
        return path

    def test_passes_within_tolerance(self) -> None:
        data = b"x" * 10_000
        path = self.wasm(data)
        self.write_readme(len(data), chk.gzipped_size(data))
        _, failures = chk.check([("baseline", path)], self.readme, 0.05)
        self.assertEqual(failures, [])

    def test_fails_when_it_grew(self) -> None:
        data = b"x" * 10_000
        path = self.wasm(data)
        self.write_readme(9_000, chk.gzipped_size(data))
        _, failures = chk.check([("baseline", path)], self.readme, 0.05)
        self.assertEqual(len(failures), 1)
        self.assertIn("over the 5% tolerance", failures[0])

    def test_fails_when_it_shrank(self) -> None:
        # Both directions: an unrecorded win leaves the next regression measured
        # against a stale, generous number.
        data = b"x" * 10_000
        path = self.wasm(data)
        self.write_readme(20_000, chk.gzipped_size(data))
        _, failures = chk.check([("baseline", path)], self.readme, 0.05)
        self.assertEqual(len(failures), 1)

    def test_fails_when_only_the_gzipped_figure_drifted(self) -> None:
        # Raw within tolerance is not enough: gzipped is what a browser
        # downloads, so it is checked on its own.
        data = b"x" * 10_000
        path = self.wasm(data)
        self.write_readme(len(data), chk.gzipped_size(data) * 3)
        _, failures = chk.check([("baseline", path)], self.readme, 0.05)
        self.assertEqual(len(failures), 1)

    def test_missing_build_fails_rather_than_measuring_zero(self) -> None:
        self.write_readme(10_000, 100)
        _, failures = chk.check([("baseline", self.dir / "absent.wasm")], self.readme, 0.05)
        self.assertEqual(len(failures), 1)
        self.assertIn("does not exist", failures[0])

    def test_unknown_build_name_fails(self) -> None:
        data = b"x" * 10_000
        path = self.wasm(data)
        self.write_readme(len(data), chk.gzipped_size(data))
        _, failures = chk.check([("nosuchvariant", path)], self.readme, 0.05)
        self.assertEqual(len(failures), 1)
        self.assertIn("no row named", failures[0])

    def test_no_builds_fails(self) -> None:
        self.write_readme(10_000, 100)
        _, failures = chk.check([], self.readme, 0.05)
        self.assertEqual(len(failures), 1)
        self.assertIn("nothing was measured", failures[0])


class ParseBuild(unittest.TestCase):
    def test_splits_on_the_first_equals(self) -> None:
        name, path = chk.parse_build("+simd128=pkg/web-simd/a.wasm")
        self.assertEqual(name, "+simd128")
        self.assertEqual(path, Path("pkg/web-simd/a.wasm"))

    def test_rejects_a_bare_path(self) -> None:
        with self.assertRaises(chk.Failure):
            chk.parse_build("pkg/web/a.wasm")


if __name__ == "__main__":
    unittest.main(verbosity=2)
