#!/usr/bin/env python3
"""Nightly sentinel over the live sample feeds (#1031).

The release checks run `tools/fetch_samples.sh`, `tools/preflight_samples.js`
and the probe-parity sweep by hand, so a feed that moved or a producer that
changed its packing is found only when someone prepares a release. This runs
the same three nightly (`.github/workflows/sentinel.yml`) and raises one
tracking issue when something needs a person.

    python3 tools/sentinel.py run [--state-in F] [--report F] [--break SOURCE]
        [--skip-sweep] [--no-fetch] [--write-baseline] [SOURCE ...]
    python3 tools/sentinel.py track --report F [--run-url URL]

`run` fetches each source on its own, preflights and censuses every file
(`tools/sentinel_census.js`), sweeps each GRIB and NetCDF file, and writes a
report. It touches no GitHub. `track` reads that report and opens, updates or
closes the tracking issue through `gh api` (`GH_TOKEN`, `GITHUB_REPOSITORY`).
The report is also the next run's `--state-in`.

What raises the issue:

* A source that fails to fetch on two runs in a row. The first failure is only
  recorded (in the report, which the workflow keeps as the next run's
  `--state-in`). Each dated source is tried for today's 00Z run and then
  yesterday's, so a run that is not posted yet, or a file past a short
  retention (ICON-D2 keeps about 24 hours, the MiRS bucket a few days), is
  found on the other date rather than reported.
* A sample whose open, decode or render fails in the preflight.
* A file the probe-parity sweep fails, or could not check.
* A GRIB message whose packing, grid or product template, or parameter table
  is not in the committed baseline for its file (`tools/sentinel_baseline.txt`).
  After deciding a new one is fine, regenerate the baseline with
  `run --write-baseline` and commit it.

The issue is written only when the set of findings changes, never on every run.
It names, for each finding of a source, where that source's producer
announces changes. When the findings clear, the issue gets a closing comment
and is closed; the next finding opens a new one.

`--break SOURCE` is the mutation check: it fetches that source through a proxy
that refuses every connection, which is what a dead URL looks like to curl.
"""
from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import re
import resource
import signal
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
SAMPLES = REPO / "samples"
FETCH = REPO / "tools" / "fetch_samples.sh"
CENSUS_JS = REPO / "tools" / "sentinel_census.js"
SWEEP = REPO / "tools" / "probe_parity_sweep.py"
BASELINE = REPO / "tools" / "sentinel_baseline.txt"

# The address in the proxy variables `--break` sets. Nothing listens on port 9
# (discard) on a runner, so curl's connection is refused.
DEAD_PROXY = "http://127.0.0.1:9"

# The probe-parity sweep's own memory cap (RELEASING.md), in bytes.
SWEEP_MEMORY = 12_000_000 * 1024

# Where each producer announces changes, named in the tracking issue.
NWS = ("NWS Service Change Notices, https://www.weather.gov/notification/ "
       "(also an Atom feed and the PNS-SCN mail list)")
ECMWF = ("ECMWF `forecast_changes` list "
         "(https://confluence.ecmwf.int/display/FCST/Changes+to+the+forecasting+system) "
         "and `open-data-community` list (open-data-community@lists.ecmwf.int)")
MSC = ("MSC Datamart `dd_info` list, "
       "https://comm.collab.science.gc.ca/mailman3/hyperkitty/list/dd_info@comm.collab.science.gc.ca/")
DWD = ("DWD Open Data news feed, "
       "https://www.dwd.de/SiteGlobals/Functions/RSSFeed/DE/opendata/opendata_lsb.xml")
NESDIS = "NOAA OSPO satellite product notices, https://www.ospo.noaa.gov/Operations/messages.html"
NODD = ("a pinned object on the NOAA Open Data Dissemination buckets; no producer channel "
        "announces its removal. See https://www.noaa.gov/nodd")
ECCODES = ("a pinned eccodes test-data file (checksum in tools/fetch_samples.sh); "
           "ECMWF announces eccodes changes at https://confluence.ecmwf.int/display/ECC/Releases")
PINNED = "a pinned file at a fixed commit or in this repository; no producer channel applies"


@dataclass(frozen=True)
class Source:
    name: str
    files: tuple[str, ...]
    channel: str
    dated: bool = True


SOURCES = {s.name: s for s in (
    Source("gfs", ("gfs.grib2",), NWS),
    Source("hrrr", ("hrrr.grib2",), NWS),
    Source("rrfs", ("rrfs.grib2",), NWS),
    Source("rap", ("rap.grib2",), NWS),
    Source("nbm", ("nbm.grib2",), NWS),
    Source("mrms", ("mrms.grib2",), NWS),
    Source("ecmwf", ("ecmwf.grib2",), ECMWF),
    Source("eccc", ("eccc.grib2",), MSC),
    Source("icon", ("icon.grib2",), DWD),
    Source("goes", ("goes.nc",), NESDIS),
    Source("oisst", ("oisst.nc",), NODD, dated=False),
    Source("wrf", ("wrf.nc",), PINNED, dated=False),
    Source("rtofs", ("rtofs_ice.nc",), NODD, dated=False),
    Source("mirs", ("mirs_swath.nc",), NESDIS),
    Source("reduced", ("reduced_gg_n32_smooth.grib1", "octahedral_gaussian_o32.grib2",
                       "octahedral_o1280.grib1", "reduced_ll_wave.grib1",
                       "reduced_gaussian_surface.grib2"), ECCODES, dated=False),
    Source("szip2", ("szip2_h5repack.h5", "szip2_le_data.h5"), PINNED, dated=False),
)}

# Days back from today a dated source is tried for, in order. Today's 00Z is
# posted for every model by the time the job runs; yesterday's covers a late
# run, and is still inside every retention window but ICON-D2's.
DAY_OFFSETS = (0, -1)

# The files the preflight and the sweep read: preflight_samples.js's own set.
SAMPLE_SUFFIXES = {".grib2", ".grib1", ".grib", ".grb2", ".nc", ".nc4"}
GRIB_FORMATS = {"grib1", "grib2"}

MARKER = "fieldglass-sentinel"
TITLE = "Nightly sentinel: a live sample feed needs attention"
LABEL = "github-actions"


def source_of(file: str) -> Source | None:
    return next((s for s in SOURCES.values() if file in s.files), None)


# ---------------------------------------------------------------------------
# Fetch
# ---------------------------------------------------------------------------


# A source's fetch is stopped after this long. curl itself gives up only on a
# stalled connection (fetch_samples.sh), so this bounds one that crawls.
FETCH_TIMEOUT = 1800


def run_fetch(source: str, date: str | None, broken: bool, samples: Path) -> None:
    """Run fetch_samples.sh for one source into `samples`. Its exit status says
    nothing per source (it warns and goes on), so the caller checks the files."""
    env = dict(os.environ)
    env["SAMPLES_OUT"] = str(samples)
    if date is not None:
        env["DATE"], env["CYCLE"] = date, "00"
    if broken:
        for k in ("http_proxy", "https_proxy", "HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "all_proxy"):
            env[k] = DEAD_PROXY
        env.pop("no_proxy", None)
        env.pop("NO_PROXY", None)
    # Its own process group, so a timeout stops curl too and not only bash:
    # an orphaned curl would go on writing the file the caller then checks.
    p = subprocess.Popen(["bash", str(FETCH), source], env=env, stdout=sys.stderr, stderr=sys.stderr,
                         start_new_session=True)
    try:
        p.wait(timeout=FETCH_TIMEOUT)
    except subprocess.TimeoutExpired:
        os.killpg(p.pid, signal.SIGKILL)
        p.wait()
        print(f"sentinel: fetching {source} timed out", file=sys.stderr)


def fetch_source(src: Source, today: dt.date, samples: Path, broken: bool, fetch=run_fetch) -> dict:
    """Fetch one source, trying each date in DAY_OFFSETS. Returns
    {"ok": bool, "date": str|None, "missing": [files]}."""
    dates = [(today + dt.timedelta(days=d)).strftime("%Y%m%d") for d in DAY_OFFSETS] if src.dated else [None]
    missing = list(src.files)
    for date in dates:
        for f in src.files:
            (samples / f).unlink(missing_ok=True)
        fetch(src.name, date, broken, samples)
        missing = [f for f in src.files if not ((samples / f).is_file() and (samples / f).stat().st_size > 0)]
        if not missing:
            return {"ok": True, "date": date, "missing": []}
    return {"ok": False, "date": None, "missing": missing}


# ---------------------------------------------------------------------------
# Census
# ---------------------------------------------------------------------------


def message_tokens(edition: str, msg: dict) -> set[str]:
    """What the census records for one message: its packing, its templates
    and its parameter table. A parameter of the WMO tables is one table; a
    parameter the center defines for itself is named with that center."""
    fp = msg.get("fingerprint") or {}
    toks = {f"{edition} packing {msg.get('packing')}"}
    if edition == "grib2":
        toks.add(f"grib2 grid template 3.{fp.get('gridTemplate')}")
        toks.add(f"grib2 product template 4.{fp.get('productTemplate')}")
        local = fp.get("localCentre")
        toks.add("grib2 parameter table WMO" if local is None else f"grib2 parameter table local, center {local}")
    else:
        toks.add(f"grib1 grid type {fp.get('gridType')}")
        local = fp.get("localCentre")
        table = f"grib1 parameter table 2 version {fp.get('tableVersion')}"
        toks.add(table if local is None else f"{table}, center {local}")
    return toks


def census_tokens(doc: dict) -> set[str]:
    return set().union(*(message_tokens(doc["format"], m) for m in doc.get("messages") or []))


def read_baseline(path: Path = BASELINE) -> dict[str, set[str]]:
    out: dict[str, set[str]] = {}
    if not path.is_file():
        return out
    for raw in path.read_text(encoding="utf-8").splitlines():
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        file, sep, token = line.partition(" | ")
        if not sep:
            raise ValueError(f"{path.name}: not 'file | token': {raw!r}")
        out.setdefault(file.strip(), set()).add(token.strip())
    return out


def write_baseline(seen: dict[str, set[str]], path: Path = BASELINE) -> None:
    lines = [
        "# Census baseline for tools/sentinel.py (#1031): per sample file, every",
        "# packing, grid and product template and parameter table its GRIB messages",
        "# have been seen with. The sentinel raises one it finds that is not here.",
        "# Regenerate with `python3 tools/sentinel.py run --write-baseline` after",
        "# deciding a new one is fine. Format: `file | token`.",
    ]
    for file in sorted(seen):
        lines.extend(f"{file} | {t}" for t in sorted(seen[file]))
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")


def census_findings(docs: list[dict], baseline: dict[str, set[str]]) -> dict[str, str]:
    """The census and preflight findings, keyed so that the same problem on two
    nights is the same finding."""
    out: dict[str, str] = {}
    for doc in docs:
        f = doc["file"]
        if doc.get("failed") or doc.get("error"):
            lines = [l.strip() for l in doc.get("preflight") or []]
            if doc.get("error"):
                lines.append(doc["error"])
            out[f"preflight:{f}"] = f"`{f}` failed the preflight: " + " / ".join(lines)
        if doc.get("format") in GRIB_FORMATS:
            for t in sorted(census_tokens(doc) - baseline.get(f, set())):
                out[f"census:{f}:{t}"] = f"`{f}` has `{t}`, which is not in the census baseline"
    return out


def run_census(files: list[Path]) -> list[dict]:
    if not files:
        return []
    p = subprocess.run(["node", str(CENSUS_JS), *map(str, files)], capture_output=True, text=True,
                       encoding="utf-8", check=False)
    if p.returncode != 0:
        raise RuntimeError(f"sentinel_census.js exited {p.returncode}: {p.stderr.strip()[-2000:]}")
    return json.loads(p.stdout)


# ---------------------------------------------------------------------------
# Probe-parity sweep
# ---------------------------------------------------------------------------


def _cap_memory():  # pragma: no cover - runs in the child
    resource.setrlimit(resource.RLIMIT_AS, (SWEEP_MEMORY, SWEEP_MEMORY))


def run_sweep(path: Path) -> tuple[int, str]:
    try:
        p = subprocess.run([sys.executable, "-I", str(SWEEP), str(path)], capture_output=True, text=True,
                           encoding="utf-8", check=False, preexec_fn=_cap_memory, timeout=1800)
    except subprocess.TimeoutExpired:
        return 2, "the sweep timed out after 30 minutes"
    return p.returncode, p.stdout + p.stderr


def sweep_finding(file: str, rc: int, output: str) -> str | None:
    """Read one file's sweep: None when it passed, else what to report."""
    if rc == 0:
        return None
    if rc == 1 and "\nFAILURES:\n" in output:
        block = output.split("\nFAILURES:\n", 1)[1].strip().splitlines()
        return f"`{file}` failed the probe-parity sweep: " + " / ".join(l.strip() for l in block[:6])
    tail = " / ".join(l.strip() for l in output.strip().splitlines()[-3:])
    return f"the probe-parity sweep could not check `{file}` (exit {rc}): {tail}"


# ---------------------------------------------------------------------------
# run
# ---------------------------------------------------------------------------


def load_json(path: Path | None) -> dict:
    if path is None or not path.is_file():
        return {}
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return {}


def is_previous_night(date: str | None, today: dt.date) -> bool:
    """Whether a report dated `date` is from the run before one on `today`:
    the same day (a manual run) or the day before."""
    try:
        return 0 <= (today - dt.date.fromisoformat(date or "")).days <= 1
    except ValueError:
        return False


def cmd_run(args, fetch=run_fetch, census=run_census, sweep=run_sweep, samples: Path = SAMPLES,
            baseline: Path = BASELINE) -> int:
    """`fetch`, `census`, `sweep`, `samples` and `baseline` are the seams the
    self-test replaces."""
    samples.mkdir(exist_ok=True)
    today = dt.datetime.strptime(args.today, "%Y%m%d").date() if args.today else dt.datetime.now(dt.timezone.utc).date()
    unknown = [n for n in [*args.sources, args.break_source] if n and n not in SOURCES]
    if unknown:
        print(f"sentinel: unknown source(s) {', '.join(unknown)}; known: {', '.join(SOURCES)}", file=sys.stderr)
        return 2
    prev = load_json(args.state_in)
    if not is_previous_night(prev.get("date"), today):
        # A run that died before keeping its report leaves an older one behind:
        # two failures with a gap between them are not "in a row".
        prev = {}

    fetched = {}
    for name in [] if args.no_fetch else (args.sources or SOURCES):
        src = SOURCES[name]
        fetched[name] = fetch_source(src, today, samples, broken=(name == args.break_source), fetch=fetch)
        state = "ok" if fetched[name]["ok"] else "FAILED, missing " + ", ".join(fetched[name]["missing"])
        print(f"fetch {name}: {state} {fetched[name]['date'] or ''}", file=sys.stderr)

    # Only what this run fetched: a file left in samples/ by hand is not a feed.
    # With --no-fetch, everything there.
    wanted = None if args.no_fetch else {f for n, r in fetched.items() if r["ok"] for f in SOURCES[n].files}
    files = sorted(p for p in samples.iterdir()
                   if p.is_file() and p.suffix.lower() in SAMPLE_SUFFIXES and (wanted is None or p.name in wanted))
    findings: dict[str, str] = {}
    try:
        docs = census(files)
    except Exception as e:  # the addon or node is missing: the census did not run
        docs = []
        findings["sentinel:census"] = f"the preflight and census could not run: {e}"
    seen = {d["file"]: census_tokens(d) for d in docs if d.get("format") in GRIB_FORMATS}
    if args.write_baseline:
        # Files this run did not see keep their entries.
        write_baseline({**read_baseline(baseline), **seen}, baseline)
        print(f"sentinel: wrote {baseline}", file=sys.stderr)
    findings.update(census_findings(docs, read_baseline(baseline)))

    swept: dict[str, str] = {}
    if not args.skip_sweep:
        for f in files:
            print(f"sweep {f.name}", file=sys.stderr, flush=True)
            rc, output = sweep(f)
            summary = next((l for l in output.splitlines() if " item(s) passed, " in l), "")
            swept[f.name] = f"exit {rc}: {summary}".strip()
            msg = sweep_finding(f.name, rc, output)
            if msg:
                findings[f"sweep:{f.name}"] = msg

    report = {
        "date": today.isoformat(),
        "fetch": fetched,
        "fetch_failed": sorted(n for n, r in fetched.items() if not r["ok"]),
        "previous_fetch_failed": sorted(prev.get("fetch_failed", [])),
        "findings": findings,
        "census": {f: sorted(t) for f, t in seen.items()},
        "preflight": {d["file"]: d.get("preflight") or [] for d in docs},
        "sweep": swept,
    }
    text = json.dumps(report, indent=2, sort_keys=True)
    if args.report:
        args.report.write_text(text + "\n", encoding="utf-8")
    print(text)
    return 0


# ---------------------------------------------------------------------------
# track
# ---------------------------------------------------------------------------


class GitHub:
    """The four calls the tracker makes, through the `gh` CLI (which reads
    GH_TOKEN, and is on every GitHub-hosted runner)."""

    def __init__(self, repo: str):
        self.repo = repo

    def _call(self, method: str, path: str, body: dict | None = None):
        cmd = ["gh", "api", "--method", method, f"repos/{self.repo}{path}"]
        if body is not None:
            cmd += ["--input", "-"]
        p = subprocess.run(cmd, input=None if body is None else json.dumps(body), capture_output=True,
                           text=True, encoding="utf-8", check=False, timeout=120)
        if p.returncode != 0:
            raise RuntimeError(f"gh api {method} {path}: {p.stderr.strip()[:500]}")
        return json.loads(p.stdout or "null")

    def _find(self, state: str) -> dict | None:
        """The newest issue in `state` that carries the sentinel's marker."""
        for page in range(1, 11):
            batch = self._call(
                "GET", f"/issues?state={state}&labels={LABEL}&sort=created&direction=desc&per_page=100&page={page}")
            for it in batch:
                if "pull_request" not in it and f"<!-- {MARKER}" in (it.get("body") or ""):
                    return {"number": it["number"], "body": it["body"]}
            if len(batch) < 100:
                return None
        return None

    def find_open_issue(self) -> dict | None:
        return self._find("open")

    def find_closed_issue(self) -> dict | None:
        return self._find("closed")

    def create_issue(self, title: str, body: str) -> int:
        return self._call("POST", "/issues", {"title": title, "body": body, "labels": [LABEL]})["number"]

    def update_issue(self, number: int, body: str | None = None, state: str | None = None) -> None:
        patch = {k: v for k, v in (("body", body), ("state", state)) if v is not None}
        self._call("PATCH", f"/issues/{number}", patch)

    def comment(self, number: int, body: str) -> None:
        self._call("POST", f"/issues/{number}/comments", {"body": body})


STATE_RE = re.compile(r"<!-- " + MARKER + r" (\{.*?\}) -->", re.S)


def listed_keys(body: str | None) -> set[str]:
    m = STATE_RE.search(body or "")
    if not m:
        return set()
    try:
        return set(json.loads(m.group(1)).get("findings", []))
    except ValueError:
        return set()


def raised_findings(report: dict, listed: set[str]) -> dict[str, str]:
    """Every finding that belongs in the issue tonight. A fetch failure counts
    on its second consecutive failing run, or when the issue already lists it
    (so a lost state file cannot clear an open finding for a night)."""
    out = dict(report.get("findings", {}))
    prev = set(report.get("previous_fetch_failed", []))
    for name in report.get("fetch_failed", []):
        key = f"fetch:{name}"
        if name in prev or key in listed:
            missing = ", ".join(f"`{f}`" for f in report["fetch"][name]["missing"])
            out[key] = f"`{name}` failed to fetch on two runs in a row (missing {missing})"
    return out


SECTIONS = (
    ("fetch:", "Fetch"),
    ("preflight:", "Open, decode or render"),
    ("sweep:", "Probe parity"),
    ("census:", "Census: a packing, template or table not seen before"),
    ("sentinel:", "The sentinel itself"),
)


def channel_for(key: str) -> str | None:
    """The announcement channel for the source a finding is about."""
    kind, _, rest = key.partition(":")
    if kind == "fetch":
        src = SOURCES.get(rest)
    else:
        src = source_of(rest.split(":", 1)[0])
    return src.channel if src else None


def render_body(findings: dict[str, str], run_url: str | None) -> str:
    parts = [
        "The nightly sentinel (`.github/workflows/sentinel.yml`, `tools/sentinel.py`) found "
        "something in the live sample feeds that needs a person. This issue is rewritten when "
        "the set of findings changes, and closed when they clear.",
    ]
    if run_url:
        parts.append(f"Latest change seen in {run_url}.")
    for prefix, title in SECTIONS:
        keys = sorted(k for k in findings if k.startswith(prefix))
        if not keys:
            continue
        parts.append(f"## {title}")
        lines = []
        for k in keys:
            line = f"- {findings[k]}."
            ch = channel_for(k)
            if ch:
                line += f" Announcements: {ch}."
            lines.append(line)
        parts.append("\n".join(lines))
    parts.append("<!-- " + MARKER + " " + json.dumps({"findings": sorted(findings)}) + " -->")
    return "\n\n".join(parts) + "\n"


def change_comment(new: list[str], cleared: list[str], findings: dict[str, str], run_url: str | None) -> str:
    lines = []
    if new:
        lines.append("New:")
        for k in new:
            ch = channel_for(k)
            lines.append(f"- {findings[k]}." + (f" Announcements: {ch}." if ch else ""))
    if cleared:
        lines.append("Cleared: " + ", ".join(f"`{k}`" for k in cleared) + ".")
    if run_url:
        lines.append(f"Run: {run_url}")
    return "\n".join(lines)


def sync_issue(gh, report: dict, run_url: str | None = None) -> str:
    """Bring the tracking issue in line with tonight's report. Returns what it
    did: "opened", "reopened", "updated", "cleared", "unchanged" or "quiet".

    A finding that comes back after the issue was closed reopens the newest
    closed sentinel issue, so a problem that flaps keeps one history rather
    than a new issue each time."""
    issue = gh.find_open_issue()
    listed = listed_keys(issue["body"]) if issue else set()
    findings = raised_findings(report, listed)
    if set(findings) == listed:
        return "unchanged" if issue else "quiet"
    if not findings:
        gh.comment(issue["number"], change_comment([], sorted(listed), {}, run_url) + "\n\nEverything is clear again; closing.")
        gh.update_issue(issue["number"], state="closed")
        return "cleared"
    body = render_body(findings, run_url)
    if issue is None:
        closed = gh.find_closed_issue()
        if closed is None:
            gh.create_issue(TITLE, body)
            return "opened"
        gh.update_issue(closed["number"], body=body, state="open")
        gh.comment(closed["number"], "Reopened.\n\n" + change_comment(sorted(findings), [], findings, run_url))
        return "reopened"
    gh.update_issue(issue["number"], body=body)
    gh.comment(issue["number"], change_comment(sorted(set(findings) - listed), sorted(listed - set(findings)), findings, run_url))
    return "updated"


def cmd_track(args, gh=None) -> int:
    report = load_json(args.report)
    if not report:
        print(f"sentinel: no report at {args.report}", file=sys.stderr)
        return 2
    if gh is None:
        repo = os.environ.get("GITHUB_REPOSITORY")
        if not repo or not os.environ.get("GH_TOKEN"):
            print("sentinel: track needs GITHUB_REPOSITORY and GH_TOKEN", file=sys.stderr)
            return 2
        gh = GitHub(repo)
    try:
        print(f"sentinel: tracking issue {sync_issue(gh, report, args.run_url)}")
    except RuntimeError as e:
        print(f"sentinel: {e}", file=sys.stderr)
        return 2
    return 0


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    r = sub.add_parser("run", help="fetch, preflight, census and sweep; write a report")
    r.add_argument("--state-in", type=Path, default=None, help="the previous run's report")
    r.add_argument("--report", type=Path, default=None, help="where to write this run's report")
    r.add_argument("--today", default=None, help="YYYYMMDD to fetch as today (default: the UTC date)")
    r.add_argument("--break", dest="break_source", default="", help="fetch this source through a dead proxy")
    r.add_argument("--skip-sweep", action="store_true")
    r.add_argument("--no-fetch", action="store_true", help="check what is already in samples/")
    r.add_argument("--write-baseline", action="store_true", help="rewrite the census baseline from this run")
    r.add_argument("sources", nargs="*", default=[], metavar="SOURCE", help="only these sources (default: all)")
    t = sub.add_parser("track", help="open, update or close the tracking issue from a report")
    t.add_argument("--report", type=Path, required=True)
    t.add_argument("--run-url", default=None)
    args = ap.parse_args(argv)
    return cmd_run(args) if args.cmd == "run" else cmd_track(args)


if __name__ == "__main__":
    sys.exit(main())
