#!/usr/bin/env python3
"""Self-test for `semver_ci.py`.

    python3 tools/test_semver_ci.py

`semver.yml` runs this on every PR, over the PR's copy of the script. The
guard cases are the ways a PR could change the Public API check (#1699): a
guarded file edited, renamed away, deleted, or hidden past the first page of
a long file list. The baseline cases run against a real git repository with
real tags, since that is what the job reads.
"""

from __future__ import annotations

import contextlib
import io
import json
import os
import re
import subprocess
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from semver_ci import (  # noqa: E402
    GUARDED,
    LABEL,
    BaselineError,
    baseline_main,
    fetch_pr_files,
    guard,
    guard_main,
    latest_tag,
    read_baseline_file,
)

# A git hook exports these, and they would point the baseline cases' `git`
# at the outer repository instead of the temporary one.
for var in ("GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE"):
    os.environ.pop(var, None)

failures = 0


def check(name: str, condition: bool, detail: str = "") -> None:
    global failures
    if condition:
        print(f"  ok   {name}")
    else:
        print(f"  FAIL {name}{': ' + detail if detail else ''}")
        failures += 1


def entry(path: str, status: str = "modified", previous: str | None = None) -> dict:
    e = {"filename": path, "status": status}
    if previous is not None:
        e["previous_filename"] = previous
    return e


def pages_of(files: list[dict], size: int = 100) -> list[list[dict]]:
    """Split a file list the way `gh api --paginate --slurp` returns it."""
    return [files[i : i + size] for i in range(0, len(files), size)] or [[]]


def fake_gh(files: list[dict], seen: list[list[str]] | None = None):
    def runner(cmd: list[str]) -> str:
        if seen is not None:
            seen.append(cmd)
        return json.dumps(pages_of(files))

    return runner


def run_guard_main(files: list[dict], labels: list[str], count: int | None = None):
    env = {
        "REPO": "owner/repo",
        "PR_NUMBER": "7",
        "LABELS": json.dumps(labels),
        "CHANGED_FILES": str(len(files) if count is None else count),
    }
    seen: list[list[str]] = []
    out = io.StringIO()
    with contextlib.redirect_stdout(out):
        rc = guard_main(env, fake_gh(files, seen))
    return rc, out.getvalue(), seen


print("guard")

rc, out, seen = run_guard_main([entry("crates/fieldglass-core/src/lib.rs")], [])
check("an unrelated file passes", rc == 0, out)
check("it says no guarded file was touched", "no guarded file" in out, out)
check(
    "the file list is fetched from every page, 100 at a time",
    seen
    and seen[0][:4] == ["gh", "api", "--paginate", "--slurp"]
    and seen[0][4] == "repos/owner/repo/pulls/7/files?per_page=100",
    repr(seen),
)

for path in GUARDED:
    rc, out, _ = run_guard_main([entry("README.md"), entry(path)], [])
    check(f"{path} without the label fails", rc == 1, out)
    check(f"{path}: the failure names the file", path in out and "::error::" in out, out)
    rc, out, _ = run_guard_main([entry(path)], ["breaking", LABEL])
    check(f"{path} with the label passes", rc == 0, out)
    check(f"{path}: a labeled pass is flagged for review", "::warning::" in out, out)

rc, _, _ = run_guard_main([entry("tools/semver-release-type")], ["breaking"])
check("the 'breaking' label alone does not satisfy the guard", rc == 1)

rc, out, _ = run_guard_main(
    [entry("tools/old-setting", "renamed", previous="tools/semver-release-type")], []
)
check("a rename away from a guarded path fails", rc == 1, out)
check("the rename's guarded end is named", "tools/semver-release-type" in out, out)

rc, out, _ = run_guard_main(
    [entry(".github/workflows/semver.yml", "renamed", previous=".github/workflows/x.yml")],
    [],
)
check("a rename onto a guarded path fails", rc == 1, out)

rc, out, _ = run_guard_main([entry("tools/semver-baseline", "removed")], [])
check("deleting a guarded file fails", rc == 1, out)

rc, out, _ = run_guard_main([entry("tools/semver-baseline", "added")], [])
check("adding tools/semver-baseline fails", rc == 1, out)

rc, _, _ = run_guard_main([entry(".github/workflows/semver.yml.bak")], [])
check("a path that only starts like a guarded one passes", rc == 0)

rc, _, _ = run_guard_main([entry("sub/tools/semver-release-type")], [])
check("a guarded name in another directory passes", rc == 0)

many = [entry(f"crates/c/src/f{i}.rs") for i in range(250)]
rc, out, _ = run_guard_main(many + [entry("tools/semver_ci.py")], [])
check("a guarded file on the third page of 251 fails", rc == 1, out)
check("all 251 files were counted", "251 file(s)" in out, out)
rc, _, _ = run_guard_main(many, [])
check("250 unrelated files pass", rc == 0)

rc, out, _ = run_guard_main(many[:100], [], count=3500)
check("a PR with more files than the API lists fails closed", rc == 1, out)
check("the truncation is explained", "100 of the PR's 3500" in out, out)
rc, _, _ = run_guard_main(many[:100], [LABEL], count=3500)
check("the label clears a truncated listing", rc == 0)

ok, _ = guard([entry("tools/semver-baseline")], [], None)
check("no event count: the listing is taken as complete", ok is False)

out = io.StringIO()
with contextlib.redirect_stdout(out):
    rc = guard_main({"REPO": "o/r"}, fake_gh([]))
check("a missing PR number is an error", rc == 1, out.getvalue())

out = io.StringIO()
with contextlib.redirect_stdout(out):
    rc = guard_main(
        {"REPO": "o/r", "PR_NUMBER": "1", "LABELS": "{}", "CHANGED_FILES": "0"},
        fake_gh([]),
    )
check("labels that are not a list are an error", rc == 1, out.getvalue())


def failing_gh(cmd: list[str]) -> str:
    raise subprocess.CalledProcessError(1, cmd)


out = io.StringIO()
with contextlib.redirect_stdout(out):
    rc = guard_main(
        {"REPO": "o/r", "PR_NUMBER": "1", "LABELS": "[]", "CHANGED_FILES": "1"},
        failing_gh,
    )
check("an API failure fails the guard", rc == 1, out.getvalue())

check(
    "fetch_pr_files flattens pages in order",
    [e["filename"] for e in fetch_pr_files("o/r", "1", fake_gh(many[:205]))]
    == [e["filename"] for e in many[:205]],
)


def guard_env(changed: str | None) -> dict[str, str]:
    env = {"REPO": "o/r", "PR_NUMBER": "1", "LABELS": "[]"}
    if changed is not None:
        env["CHANGED_FILES"] = changed
    return env


for changed, why in [(None, "missing"), ("", "empty"), ("lots", "not a number"), ("-1", "negative")]:
    out = io.StringIO()
    with contextlib.redirect_stdout(out):
        rc = guard_main(guard_env(changed), fake_gh([entry("README.md")]))
    check(
        f"an event file count that is {why} fails closed",
        rc == 1 and "::error::" in out.getvalue(),
        out.getvalue(),
    )

for body, why in [
    ({"message": "Not Found"}, "an error object"),
    ([{"message": "page"}], "a page that is an object"),
    ([["README.md"]], "an entry that is not an object"),
]:
    out = io.StringIO()
    with contextlib.redirect_stdout(out):
        try:
            rc = guard_main(guard_env("1"), lambda cmd, b=body: json.dumps(b))
        except Exception as err:  # noqa: BLE001 - a traceback is the failure
            rc = f"raised {type(err).__name__}"
    check(
        f"an API reply that is {why} is a clean error",
        rc == 1 and "::error::" in out.getvalue(),
        f"{rc!r} {out.getvalue()}",
    )


print("workflows")

# The checkout the semver.yml jobs run is the PR's, so how they call Python
# and cargo is part of what the guard protects. Pin the parts the #1699
# review showed a PR could subvert from an unguarded file.
REPO_ROOT = Path(__file__).resolve().parent.parent
WORKFLOWS = REPO_ROOT / ".github" / "workflows"
for name in ("semver.yml", "semver-guard.yml"):
    text = (WORKFLOWS / name).read_text(encoding="utf-8")
    calls = [
        line.strip()
        for line in text.splitlines()
        if re.search(r"\bpython3?\b", line) and not line.lstrip().startswith("#")
    ]
    check(f"{name} runs Python", bool(calls), name)
    for call in calls:
        check(
            f"{name}: Python runs isolated (-I): {call}",
            re.search(r"\bpython3 -I tools/", call) is not None,
        )

semver_yml = (WORKFLOWS / "semver.yml").read_text(encoding="utf-8")
jobs = re.split(r"\n  (?=[a-z][a-z0-9-]*:\n)", semver_yml.split("\njobs:\n", 1)[1])
check_job = next((j for j in jobs if "name: Public API vs last tag" in j), "")
check("semver.yml has the Public API job", bool(check_job))
check(
    "the Public API job does not run the (unguarded) self-test",
    "test_semver_ci" not in check_job,
)
check("the Public API job has no needs: link", "needs:" not in check_job)
selftest_job = next((j for j in jobs if "test_semver_ci.py" in j), "")
check(
    "the self-test runs in a job of its own",
    bool(selftest_job) and selftest_job is not check_job and "needs:" not in selftest_job,
)
code_lines = [line for line in check_job.splitlines() if not line.lstrip().startswith("#")]
check(
    "cargo-semver-checks is called by its binary name, not via a cargo alias",
    any("cargo-semver-checks semver-checks" in line for line in code_lines)
    and not any(re.search(r"\bcargo semver-checks\b", line) for line in code_lines),
)
pinned = re.search(r"toolchain: (\S+)", check_job)
check(
    "the check step pins RUSTUP_TOOLCHAIN to the installed toolchain",
    pinned is not None and f"RUSTUP_TOOLCHAIN: {pinned.group(1)}" in check_job,
)
guard_yml = (WORKFLOWS / "semver-guard.yml").read_text(encoding="utf-8")
check(
    "the guard cancels an older run for the same PR",
    "cancel-in-progress: true" in guard_yml
    and "group: semver-guard-${{ github.event.pull_request.number }}" in guard_yml,
)


print("baseline")

check("latest tag compares numbers", latest_tag(["v0.9.0", "v0.10.0", "v0.2.1"]) == "v0.10.0")
check(
    "latest tag ignores other tag shapes",
    latest_tag(["v1.0.0-rc1", "v0.6.0", "vscode-1", "v9"]) == "v0.6.0",
)
check("no tag at all", latest_tag([]) is None)
check(
    "the file may carry comments",
    read_baseline_file("# last good release\nv0.6.0\n\n") == "v0.6.0",
)
for junk, why in [
    ("", "empty"),
    ("# only a comment\n", "comment only"),
    ("v0.6", "not X.Y.Z"),
    ("0.6.0", "no v"),
    ("v0.6.0 v0.5.0", "two names on a line"),
    ("v0.6.0\nv0.5.0\n", "two lines"),
    ("master", "a branch"),
    ("v0.6.0; rm -rf /", "shell text"),
]:
    try:
        read_baseline_file(junk)
        check(f"junk refused: {why}", False, repr(junk))
    except BaselineError:
        check(f"junk refused: {why}", True)


def git(root: Path, *args: str) -> None:
    subprocess.run(
        ["git", "-C", str(root), *args],
        check=True,
        capture_output=True,
        env={
            "GIT_AUTHOR_NAME": "t",
            "GIT_AUTHOR_EMAIL": "t@example.invalid",
            "GIT_COMMITTER_NAME": "t",
            "GIT_COMMITTER_EMAIL": "t@example.invalid",
            "GIT_CONFIG_NOSYSTEM": "1",
            "HOME": str(root),
            "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
        },
    )


def run_baseline(root: Path) -> tuple[int, str, str]:
    out, err = io.StringIO(), io.StringIO()
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
        rc = baseline_main(root)
    return rc, out.getvalue(), err.getvalue()


with tempfile.TemporaryDirectory() as tmp:
    root = Path(tmp)
    git(root, "init", "-q")
    rc, out, err = run_baseline(root)
    check("a repository with no tag is an error", rc == 1 and out == "", err)

    git(root, "commit", "-q", "--allow-empty", "-m", "one")
    git(root, "tag", "v0.5.0")
    git(root, "tag", "-a", "v0.6.0", "-m", "annotated")
    git(root, "tag", "v0.10.0")
    git(root, "tag", "v0.11.0-rc1")

    rc, out, err = run_baseline(root)
    check("no file: the latest X.Y.Z tag", rc == 0 and out == "v0.10.0\n", out + err)
    check("no file: the baseline is printed", "Baseline v0.10.0 (the latest" in err, err)

    (root / "tools").mkdir()
    baseline = root / "tools" / "semver-baseline"
    baseline.write_text("# 0.10.0 broke X; measure the fix against 0.6.0\nv0.6.0\n", encoding="utf-8")
    rc, out, err = run_baseline(root)
    check("file: the tag it names, annotated or not", rc == 0 and out == "v0.6.0\n", out + err)
    check(
        "file: the baseline and its source are printed",
        "Baseline v0.6.0 (named in tools/semver-baseline" in err,
        err,
    )

    baseline.write_text("v0.7.0\n", encoding="utf-8")
    rc, out, err = run_baseline(root)
    check("file naming a tag that does not exist fails", rc == 1 and out == "", out)
    check("the failure names the tag", "::error::" in err and "v0.7.0" in err, err)

    baseline.write_text("v0.11.0-rc1\n", encoding="utf-8")
    rc, out, err = run_baseline(root)
    check("file naming an existing non-X.Y.Z tag fails", rc == 1 and out == "", err)

    baseline.write_text("garbage\n", encoding="utf-8")
    rc, out, err = run_baseline(root)
    check("junk in the file fails, with no fallback", rc == 1 and out == "", err)

    baseline.write_text("", encoding="utf-8")
    rc, out, err = run_baseline(root)
    check("an empty file fails, with no fallback", rc == 1 and out == "", err)

    rc, out, err = run_baseline(root / "missing")
    check("a path that is not a repository fails", rc == 1 and out == "", err)


if failures:
    print(f"{failures} failure(s)")
    sys.exit(1)
print("all passed")
