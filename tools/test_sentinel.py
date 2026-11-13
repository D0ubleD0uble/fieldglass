#!/usr/bin/env python3
"""Self-test for tools/sentinel.py (#1031).

    python3 tools/test_sentinel.py

Three halves. The census: a packing taken out of the committed baseline is
flagged, on real `message()` metadata from the built addon over committed
fixtures when the addon is in extension/bin (the nightly workflow builds it
before running this), and on recorded metadata otherwise. The tracking issue:
against a fake GitHub, a dead source raises it only on its second failing run
in a row, a later change updates it rather than opening another, it names the
producer's announcement channel, and it closes once the source is back. The
fetch: retention-limited sources found on the other date are not failures, and
`--break` really sends curl to a dead proxy.

No network and no GitHub: every seam is replaced or reads committed files.
"""
from __future__ import annotations

import datetime as dt
import importlib.util
import json
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path
from types import SimpleNamespace
from unittest import mock

_spec = importlib.util.spec_from_file_location("sentinel", Path(__file__).resolve().parent / "sentinel.py")
assert _spec and _spec.loader
sentinel = importlib.util.module_from_spec(_spec)
sys.modules[_spec.name] = sentinel
_spec.loader.exec_module(sentinel)

REPO = sentinel.REPO
ADDON = REPO / "extension" / "bin" / "fieldglass.linux-x64-gnu.node"
# Committed fixtures that are also sample files (the `reduced` source copies
# them into samples/), so the committed baseline has entries for them.
FIXTURES = {
    "octahedral_gaussian_o32.grib2": REPO / "crates/fieldglass-grib2/tests/fixtures/octahedral_gaussian_o32.grib2",
    "reduced_gg_n32_smooth.grib1": REPO / "crates/fieldglass-grib1/tests/fixtures/reduced_gg_n32_smooth.grib1",
}
HAVE_ADDON = ADDON.is_file() and shutil.which("node") is not None


def run_args(**kw):
    base = dict(state_in=None, report=None, today="20261112", break_source="", skip_sweep=True,
                no_fetch=False, write_baseline=False, sources=[])
    base.update(kw)
    return SimpleNamespace(**base)


def grib2_doc(file, packing="complex_spatial_diff", grid=30, product=0, local=None):
    return {"file": file, "format": "grib2", "failed": False, "error": None, "preflight": [],
            "messages": [{"packing": packing, "fingerprint": {
                "edition": "grib2", "gridTemplate": grid, "productTemplate": product, "localCentre": local}}]}


class Census(unittest.TestCase):
    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.tmp)

    def test_tokens_name_packing_templates_and_table(self):
        toks = sentinel.census_tokens(grib2_doc("x.grib2", local=78))
        self.assertEqual(toks, {
            "grib2 packing complex_spatial_diff",
            "grib2 grid template 3.30",
            "grib2 product template 4.0",
            "grib2 parameter table local, center 78",
        })

    def test_removing_a_packing_from_the_baseline_flags_it(self):
        doc = grib2_doc("nbm.grib2")
        base = {"nbm.grib2": sentinel.census_tokens(doc)}
        self.assertEqual(sentinel.census_findings([doc], base), {})
        base["nbm.grib2"].discard("grib2 packing complex_spatial_diff")
        found = sentinel.census_findings([doc], base)
        self.assertEqual(list(found), ["census:nbm.grib2:grib2 packing complex_spatial_diff"])

    def test_a_new_packing_on_a_known_file_is_flagged(self):
        base = {"nbm.grib2": sentinel.census_tokens(grib2_doc("nbm.grib2"))}
        found = sentinel.census_findings([grib2_doc("nbm.grib2", packing="ccsds")], base)
        self.assertIn("census:nbm.grib2:grib2 packing ccsds", found)

    def test_a_failed_preflight_is_a_finding(self):
        doc = dict(grib2_doc("hrrr.grib2"), failed=True, preflight=["  hrrr.grib2: RENDER FAILED: boom"])
        found = sentinel.census_findings([doc], {"hrrr.grib2": sentinel.census_tokens(doc)})
        self.assertEqual(list(found), ["preflight:hrrr.grib2"])
        self.assertIn("RENDER FAILED: boom", found["preflight:hrrr.grib2"])

    def test_baseline_round_trips_and_keeps_unseen_files(self):
        path = self.tmp / "b.txt"
        sentinel.write_baseline({"a.grib2": {"t1", "t2"}, "b.grib1": {"t3"}}, path)
        self.assertEqual(sentinel.read_baseline(path), {"a.grib2": {"t1", "t2"}, "b.grib1": {"t3"}})

    def test_committed_baseline_parses_and_covers_every_grib_sample(self):
        base = sentinel.read_baseline()
        for src in sentinel.SOURCES.values():
            for f in src.files:
                if Path(f).suffix in {".grib2", ".grib1"} and src.name != "nam":
                    self.assertTrue(base.get(f), f"no baseline entry for {f}")

    @unittest.skipUnless(HAVE_ADDON, "needs the native addon in extension/bin")
    def test_real_addon_census_against_committed_baseline(self):
        """The real path: sentinel_census.js over committed fixtures, through
        cmd_run, against the committed baseline and then one without a packing."""
        samples = self.tmp / "samples"
        samples.mkdir()
        for name, src in FIXTURES.items():
            shutil.copy(src, samples / name)
        report_path = self.tmp / "report.json"
        baseline = self.tmp / "baseline.txt"
        shutil.copy(sentinel.BASELINE, baseline)
        args = run_args(no_fetch=True, report=report_path)
        with mock.patch("sys.stdout"), mock.patch("sys.stderr"):
            self.assertEqual(sentinel.cmd_run(args, samples=samples, baseline=baseline), 0)
        report = json.loads(report_path.read_text(encoding="utf-8"))
        self.assertEqual(report["findings"], {}, "the committed baseline should hold the committed fixtures")
        o32 = report["census"]["octahedral_gaussian_o32.grib2"]
        packing = next(t for t in o32 if t.startswith("grib2 packing "))

        kept = [l for l in baseline.read_text(encoding="utf-8").splitlines() if l != f"octahedral_gaussian_o32.grib2 | {packing}"]
        self.assertEqual(len(kept), len(baseline.read_text(encoding="utf-8").splitlines()) - 1, "the packing line was not in the baseline")
        baseline.write_text("\n".join(kept) + "\n", encoding="utf-8")
        with mock.patch("sys.stdout"), mock.patch("sys.stderr"):
            sentinel.cmd_run(args, samples=samples, baseline=baseline)
        report = json.loads(report_path.read_text(encoding="utf-8"))
        self.assertEqual(list(report["findings"]), [f"census:octahedral_gaussian_o32.grib2:{packing}"])


class FakeGitHub:
    def __init__(self):
        self.issues: dict[int, dict] = {}
        self.comments: dict[int, list[str]] = {}
        self.calls: list[str] = []

    def find_open_issue(self):
        for n, it in sorted(self.issues.items()):
            if it["state"] == "open" and f"<!-- {sentinel.MARKER}" in it["body"]:
                return {"number": n, "body": it["body"]}
        return None

    def create_issue(self, title, body):
        n = len(self.issues) + 1
        self.issues[n] = {"title": title, "body": body, "state": "open"}
        self.calls.append("create")
        return n

    def update_issue(self, number, body=None, state=None):
        if body is not None:
            self.issues[number]["body"] = body
        if state is not None:
            self.issues[number]["state"] = state
        self.calls.append(f"update {state or 'body'}")

    def comment(self, number, body):
        self.comments.setdefault(number, []).append(body)
        self.calls.append("comment")


def night(failed, prev, findings=None):
    """A report as cmd_run writes it, for sources in `failed`."""
    return {
        "fetch": {n: {"ok": False, "date": None, "missing": list(sentinel.SOURCES[n].files)} for n in failed},
        "fetch_failed": sorted(failed),
        "previous_fetch_failed": sorted(prev),
        "findings": findings or {},
    }


class TrackingIssue(unittest.TestCase):
    def test_dead_source_raises_on_second_run_updates_then_clears(self):
        gh = FakeGitHub()
        # Night 1: NAM fails for the first time. Nothing is raised.
        self.assertEqual(sentinel.sync_issue(gh, night({"nam"}, set())), "quiet")
        self.assertEqual(gh.issues, {})
        # Night 2: it fails again. One issue, naming the NWS channel.
        self.assertEqual(sentinel.sync_issue(gh, night({"nam"}, {"nam"}), "https://run/2"), "opened")
        self.assertEqual(len(gh.issues), 1)
        body = gh.issues[1]["body"]
        self.assertIn("`nam` failed to fetch on two runs in a row (missing `nam.grib2`)", body)
        self.assertIn("NWS Service Change Notices", body)
        self.assertIn("https://run/2", body)
        # Night 3: unchanged. No edit and no comment.
        gh.calls.clear()
        self.assertEqual(sentinel.sync_issue(gh, night({"nam"}, {"nam"})), "unchanged")
        self.assertEqual(gh.calls, [])
        # Night 4: ECCC's packing changes as well. The same issue is updated.
        census = {"census:eccc.grib2:grib2 packing ccsds": "`eccc.grib2` has `grib2 packing ccsds`, ..."}
        self.assertEqual(sentinel.sync_issue(gh, night({"nam"}, {"nam"}, census)), "updated")
        self.assertEqual(len(gh.issues), 1)
        self.assertIn("MSC Datamart `dd_info`", gh.issues[1]["body"])
        self.assertIn("dd_info", gh.comments[1][-1])
        # Night 5: both are fixed. The issue is closed with a comment.
        self.assertEqual(sentinel.sync_issue(gh, night(set(), {"nam"})), "cleared")
        self.assertEqual(gh.issues[1]["state"], "closed")
        self.assertIn("`fetch:nam`", gh.comments[1][-1])
        # Night 6: quiet again; a new problem would open issue 2, not reopen 1.
        self.assertEqual(sentinel.sync_issue(gh, night(set(), set())), "quiet")

    def test_one_failure_between_successes_never_raises(self):
        gh = FakeGitHub()
        for failed, prev in (({"icon"}, set()), (set(), {"icon"}), ({"icon"}, set()), (set(), {"icon"})):
            self.assertEqual(sentinel.sync_issue(gh, night(failed, prev)), "quiet")
        self.assertEqual(gh.calls, [])

    def test_lost_state_keeps_a_listed_failure(self):
        gh = FakeGitHub()
        sentinel.sync_issue(gh, night({"nam"}, {"nam"}))
        # The previous report expired: the issue's own list keeps NAM raised.
        self.assertEqual(sentinel.sync_issue(gh, night({"nam"}, set())), "unchanged")
        self.assertEqual(gh.issues[1]["state"], "open")

    def test_each_listed_channel_is_named(self):
        cases = {"gfs": "NWS Service Change Notices", "ecmwf": "forecast_changes", "eccc": "dd_info",
                 "icon": "DWD Open Data news feed"}
        for src, needle in cases.items():
            body = sentinel.render_body(sentinel.raised_findings(night({src}, {src}), set()), None)
            self.assertIn(needle, body, src)
        self.assertIn("open-data-community", sentinel.SOURCES["ecmwf"].channel)

    def test_a_census_finding_names_its_source_channel(self):
        self.assertIn("DWD", sentinel.channel_for("census:icon.grib2:grib2 packing x"))
        self.assertIn("NWS", sentinel.channel_for("sweep:nbm.grib2"))
        self.assertIsNone(sentinel.channel_for("sentinel:census"))

    def test_listed_keys_round_trip(self):
        findings = {"fetch:nam": "a", "sweep:nbm.grib2": "b"}
        self.assertEqual(sentinel.listed_keys(sentinel.render_body(findings, None)), set(findings))
        self.assertEqual(sentinel.listed_keys("no marker"), set())


class Fetch(unittest.TestCase):
    def setUp(self):
        self.samples = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.samples)

    def fake(self, posted: set[str | None]):
        calls = []

        def fetch(name, date, broken):
            calls.append((name, date, broken))
            if date in posted and not broken:
                for f in sentinel.SOURCES[name].files:
                    (self.samples / f).write_bytes(b"GRIB")
        return fetch, calls

    def test_retention_limited_source_found_on_the_other_date_is_ok(self):
        # ICON-D2 keeps about 24 h: one of the two dates is always there.
        fetch, calls = self.fake({"20261112"})
        r = sentinel.fetch_source(sentinel.SOURCES["icon"], dt.date(2026, 11, 13), self.samples, False, fetch)
        self.assertEqual(r, {"ok": True, "date": "20261112", "missing": []})
        self.assertEqual([c[1] for c in calls], ["20261113", "20261112"])

    def test_source_with_neither_date_fails(self):
        fetch, _ = self.fake(set())
        r = sentinel.fetch_source(sentinel.SOURCES["nam"], dt.date(2026, 11, 13), self.samples, False, fetch)
        self.assertEqual(r, {"ok": False, "date": None, "missing": ["nam.grib2"]})

    def test_a_stale_file_is_not_mistaken_for_a_fetch(self):
        (self.samples / "gfs.grib2").write_bytes(b"old")
        fetch, _ = self.fake(set())
        r = sentinel.fetch_source(sentinel.SOURCES["gfs"], dt.date(2026, 11, 13), self.samples, False, fetch)
        self.assertFalse(r["ok"])

    def test_partial_multi_file_source_fails_naming_the_missing(self):
        def fetch(name, date, broken):
            (self.samples / "szip2_h5repack.h5").write_bytes(b"x")
        r = sentinel.fetch_source(sentinel.SOURCES["szip2"], dt.date(2026, 11, 13), self.samples, False, fetch)
        self.assertEqual(r["missing"], ["szip2_le_data.h5"])

    def test_undated_source_is_fetched_once(self):
        fetch, calls = self.fake({None})
        r = sentinel.fetch_source(sentinel.SOURCES["oisst"], dt.date(2026, 11, 13), self.samples, False, fetch)
        self.assertTrue(r["ok"])
        self.assertEqual(calls, [("oisst", None, False)])

    def test_break_points_curl_at_a_dead_proxy(self):
        with mock.patch.object(sentinel.subprocess, "run") as run:
            sentinel.run_fetch("gfs", "20261112", broken=True)
        env = run.call_args.kwargs["env"]
        self.assertEqual(env["https_proxy"], sentinel.DEAD_PROXY)
        self.assertEqual(env["DATE"], "20261112")
        self.assertNotIn("no_proxy", env)

    @unittest.skipUnless(shutil.which("curl"), "needs curl")
    def test_dead_proxy_really_refuses(self):
        env = {"PATH": "/usr/bin:/bin", "https_proxy": sentinel.DEAD_PROXY}
        p = subprocess.run(["curl", "-fsS", "--max-time", "10", "-o", "/dev/null", "https://example.com/"],
                           env=env, capture_output=True, check=False)
        self.assertNotEqual(p.returncode, 0)

    def test_run_with_break_reports_the_source_failed_and_keeps_state(self):
        fetch, calls = self.fake({"20261112", "20261111", None})
        state = Path(self.samples) / "prev.json"
        state.write_text(json.dumps({"date": "2026-11-11", "fetch_failed": ["gfs"]}), encoding="utf-8")
        report = Path(self.samples) / "report.json"
        args = run_args(state_in=state, report=report, break_source="gfs", sources=["gfs", "hrrr"])
        with mock.patch("sys.stdout"), mock.patch("sys.stderr"):
            sentinel.cmd_run(args, fetch=fetch, census=lambda files: [], samples=self.samples,
                             baseline=self.samples / "none.txt")
        r = json.loads(report.read_text(encoding="utf-8"))
        self.assertEqual(r["fetch_failed"], ["gfs"])
        self.assertEqual(r["previous_fetch_failed"], ["gfs"])
        self.assertIn(("gfs", "20261112", True), calls)
        self.assertTrue(r["fetch"]["hrrr"]["ok"])
        gh = FakeGitHub()
        self.assertEqual(sentinel.sync_issue(gh, r), "opened")


    def test_a_report_from_before_a_gap_is_not_the_previous_night(self):
        fetch, _ = self.fake(set())
        state = self.samples / "prev.json"
        report = self.samples / "report.json"
        for date, counted in (("2026-11-10", False), ("2026-11-11", True), ("2026-11-12", True), (None, False)):
            state.write_text(json.dumps({"date": date, "fetch_failed": ["nam"]}), encoding="utf-8")
            args = run_args(state_in=state, report=report, sources=["nam"])
            with mock.patch("sys.stdout"), mock.patch("sys.stderr"):
                sentinel.cmd_run(args, fetch=fetch, census=lambda files: [], samples=self.samples,
                                 baseline=self.samples / "none.txt")
            prev = json.loads(report.read_text(encoding="utf-8"))["previous_fetch_failed"]
            self.assertEqual(prev, ["nam"] if counted else [], date)

    def test_only_fetched_files_are_checked(self):
        (self.samples / "mine.grib2").write_bytes(b"left here by hand")
        fetch, _ = self.fake({"20261112"})
        seen = []
        args = run_args(sources=["gfs"], skip_sweep=False)
        with mock.patch("sys.stdout"), mock.patch("sys.stderr"):
            sentinel.cmd_run(args, fetch=fetch, census=lambda files: seen.extend(f.name for f in files) or [],
                             sweep=lambda p: (0, ""), samples=self.samples, baseline=self.samples / "none.txt")
        self.assertEqual(seen, ["gfs.grib2"])


class Sweep(unittest.TestCase):
    def test_pass_is_no_finding(self):
        self.assertIsNone(sentinel.sweep_finding("a.grib2", 0, "all good"))

    def test_failures_block_is_quoted(self):
        out = "table\n\nFAILURES:\n  nbm.grib2 msg#0: 12 cell disagreements\n      at (1, 2)\n"
        msg = sentinel.sweep_finding("nbm.grib2", 1, out)
        self.assertIn("12 cell disagreements", msg)

    def test_a_sweep_that_did_not_run_is_a_finding(self):
        msg = sentinel.sweep_finding("x.nc", 2, "probe_parity_sweep: x.nc: boom")
        self.assertIn("could not check", msg)
        self.assertIn("boom", msg)

    def test_sweep_findings_reach_the_report(self):
        tmp = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, tmp)
        (tmp / "nbm.grib2").write_bytes(b"x")
        report = tmp / "r.json"
        args = run_args(no_fetch=True, skip_sweep=False, report=report)
        with mock.patch("sys.stdout"), mock.patch("sys.stderr"):
            sentinel.cmd_run(args, census=lambda files: [], sweep=lambda p: (1, "\nFAILURES:\n  nbm.grib2 msg#0: bad\n"),
                             samples=tmp, baseline=tmp / "none.txt")
        self.assertIn("sweep:nbm.grib2", json.loads(report.read_text(encoding="utf-8"))["findings"])


if __name__ == "__main__":
    unittest.main(verbosity=2)
