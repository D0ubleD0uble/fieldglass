#!/usr/bin/env python3
"""Self-test for `semver_ci.py`.

    python3 tools/test_semver_ci.py

`semver.yml` runs this on every PR, over the PR's copy of the script. The
guard cases are the ways a PR could change the Public API check (#1699): a
guarded file edited, renamed away, deleted, or hidden past the first page of
a long file list. The baseline cases run against a real git repository with
real tags, since that is what the job reads. The packages cases (#1712) run
the script itself on a real git repository holding a small cargo workspace:
the base commit's manifests against a PR's, for each way a PR's manifests
could turn the check off. They need `cargo` on the path.
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
rc, out, _ = run_guard_main(many[:99], [], count=100)
check("a listing exactly one file short fails closed", rc == 1, out)
rc, out, _ = run_guard_main(many[:100], [], count=100)
check("a listing exactly as long as the event's count passes", rc == 0, out)
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

# Job-level env sits between the job's keys and `steps:`, at four spaces.
job_env_block = re.search(r"\n    env:\n((?:      .*\n|\s*#.*\n|\s*\n)*?)    steps:", check_job)
job_env = job_env_block.group(1) if job_env_block else ""
check(
    "the Public API job pins RUSTUP_TOOLCHAIN job-wide (rust-cache runs cargo too)",
    re.search(r"^      RUSTUP_TOOLCHAIN: \d+\.\d+\.\d+$", job_env, re.M) is not None,
    job_env,
)
check(
    "the Public API job empties RUSTC_WRAPPER job-wide",
    re.search(r'^      RUSTC_WRAPPER: ""$', job_env, re.M) is not None,
    job_env,
)
check(
    "the Public API job empties RUSTC_WORKSPACE_WRAPPER job-wide",
    re.search(r'^      RUSTC_WORKSPACE_WRAPPER: ""$', job_env, re.M) is not None,
    job_env,
)
check(
    "the Public API job sets RUSTC to the rustup proxy job-wide",
    re.search(r"^      RUSTC: rustc$", job_env, re.M) is not None,
    job_env,
)
check(
    "the Public API job sets RUSTDOC to the rustup proxy job-wide",
    re.search(r"^      RUSTDOC: rustdoc$", job_env, re.M) is not None,
    job_env,
)
check(
    "the Public API job empties RUSTDOCFLAGS job-wide",
    re.search(r'^      RUSTDOCFLAGS: ""$', job_env, re.M) is not None,
    job_env,
)
check(
    "Set up Rust installs the toolchain the job pins",
    "toolchain: ${{ env.RUSTUP_TOOLCHAIN }}" in check_job,
)
check(
    "no step sets its own toolchain or rustc override",
    all(
        len(re.findall(rf"^\s+{var}:", check_job, re.M)) == 1
        for var in (
            "RUSTUP_TOOLCHAIN",
            "RUSTC_WRAPPER",
            "RUSTC_WORKSPACE_WRAPPER",
            "RUSTC",
            "RUSTDOC",
            "RUSTDOCFLAGS",
        )
    ),
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


print("packages")

SCRIPT = Path(__file__).resolve().parent / "semver_ci.py"

ROOT_MANIFEST = """[workspace]
members = ["crates/a", "crates/b", "crates/private"]
resolver = "2"

[workspace.package]
version = "0.1.0"
edition = "2021"
"""


def crate_manifest(name: str, extra: str = "") -> str:
    return (
        f'[package]\nname = "{name}"\nversion.workspace = true\n'
        f"edition.workspace = true\n{extra}"
    )


def write(root: Path, rel: str, text: str) -> None:
    path = root / rel
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def rev_parse(root: Path, rev: str) -> str:
    return subprocess.run(
        ["git", "-C", str(root), "rev-parse", rev],
        check=True,
        capture_output=True,
        text=True,
        encoding="utf-8",
    ).stdout.strip()


PUBLISH_FALSE = ("edition.workspace = true\n", "edition.workspace = true\npublish = false\n")
ALLOW = '[package.metadata.cargo-semver-checks.lints]\nfunction_missing = "allow"\n'
WS_ALLOW = '[workspace.metadata.cargo-semver-checks.lints]\nfunction_missing = "allow"\n'


def run_packages(root: Path, *args: str) -> tuple[int, str, str]:
    """The script as the job runs it: `-I`, from the PR's checkout."""
    proc = subprocess.run(
        [sys.executable, "-I", str(SCRIPT), "packages", *args],
        cwd=root,
        capture_output=True,
        text=True,
        encoding="utf-8",
    )
    return proc.returncode, proc.stdout, proc.stderr


with tempfile.TemporaryDirectory() as tmp:
    root = Path(tmp)
    git(root, "init", "-q")
    write(root, "Cargo.toml", ROOT_MANIFEST)
    docs_rs = "\n[package.metadata.docs.rs]\nall-features = true\n"
    write(root, "crates/a/Cargo.toml", crate_manifest("a", docs_rs))
    write(root, "crates/b/Cargo.toml", crate_manifest("b"))
    write(root, "crates/private/Cargo.toml", crate_manifest("private", "publish = false\n"))
    for crate in ("a", "b", "private"):
        write(root, f"crates/{crate}/src/lib.rs", "pub fn f() {}\n")
    git(root, "add", "-A")
    git(root, "commit", "-q", "-m", "base")
    base = rev_parse(root, "HEAD")

    def reset() -> None:
        git(root, "checkout", "-q", "--", ".")
        git(root, "clean", "-qfdx")

    def edit(rel: str, old: str, new: str) -> None:
        path = root / rel
        text = path.read_text(encoding="utf-8")
        assert old in text, (rel, old)
        path.write_text(text.replace(old, new, 1), encoding="utf-8")

    rc, out, err = run_packages(root, base)
    check(
        "unchanged: the base's published crates, one per line",
        rc == 0 and out == "a\nb\n",
        out + err,
    )
    check("unchanged: the list is explained", "all checked: a, b" in err, err)
    check("other [package.metadata] tables are fine", rc == 0, err)

    edit("crates/b/Cargo.toml", *PUBLISH_FALSE)
    rc, out, err = run_packages(root, base)
    check(
        "publish = false on a published crate keeps it on the list",
        rc == 0 and out == "a\nb\n",
        out + err,
    )
    check(
        "publish = false: the log says it is checked anyway",
        "b is `publish = false` in this PR" in err,
        err,
    )
    reset()

    edit("Cargo.toml", '"crates/b", ', "")
    rc, out, err = run_packages(root, base)
    check("a published crate dropped from members fails", rc == 1 and out == "", out + err)
    check(
        "the dropped crate is named",
        "::error::b is published on the base branch" in err,
        err,
    )
    reset()

    # Cargo keeps a path dependency inside the workspace as a member, so the
    # tool still checks it; the script reads the same metadata and agrees.
    edit("Cargo.toml", '"crates/b", ', "")
    path_dep = '\n[dependencies]\nb = { path = "../b" }\n\n[package.metadata'
    edit("crates/a/Cargo.toml", "\n[package.metadata", path_dep)
    rc, out, err = run_packages(root, base)
    check(
        "a crate left out of members but still a path dependency passes",
        rc == 0 and out == "a\nb\n",
        out + err,
    )
    reset()

    edit("Cargo.toml", 'resolver = "2"\n', 'resolver = "2"\nexclude = ["crates/b"]\n')
    edit("Cargo.toml", '"crates/b", ', "")
    rc, out, err = run_packages(root, base)
    check("a published crate moved to exclude fails", rc == 1 and "::error::b " in err, err)
    reset()

    edit("crates/b/Cargo.toml", 'name = "b"', 'name = "b2"')
    rc, out, err = run_packages(root, base)
    check("a published crate renamed fails", rc == 1 and "::error::b " in err, err)
    reset()

    (root / "crates/b/src/lib.rs").rename(root / "crates/b/src/main.rs")
    rc, out, err = run_packages(root, base)
    check("a published crate that loses its library target fails", rc == 1, out + err)
    check(
        "the lost library is named",
        "b has a library target on the base branch" in err,
        err,
    )
    reset()

    pkg, ws = "[package.metadata.cargo-semver-checks]", "[workspace.metadata.cargo-semver-checks]"
    for rel, table, who in [
        ("crates/a/Cargo.toml", ALLOW, f"{pkg} in a "),
        (
            "crates/a/Cargo.toml",
            "[package.metadata.cargo-semver-checks.lints]\nworkspace = true\n",
            f"{pkg} in a ",
        ),
        ("crates/private/Cargo.toml", ALLOW, f"{pkg} in private "),
        ("Cargo.toml", WS_ALLOW, ws),
        ("Cargo.toml", "[workspace.metadata.cargo-semver-checks]\n", ws),
    ]:
        path = root / rel
        path.write_text(path.read_text(encoding="utf-8") + "\n" + table, encoding="utf-8")
        rc, out, err = run_packages(root, base)
        label = " ".join(table.splitlines())
        check(f"{rel}: {label} fails", rc == 1 and out == "", out + err)
        check(f"{rel}: {label}: the manifest is named", f"::error::{who}" in err, err)
        reset()

    edit("Cargo.toml", '"crates/private"]', '"crates/private", "crates/c"]')
    write(root, "crates/c/Cargo.toml", crate_manifest("c"))
    write(root, "crates/c/src/lib.rs", "pub fn f() {}\n")
    rc, out, err = run_packages(root, base)
    check(
        "a crate the PR adds passes; --workspace checks it",
        rc == 0 and out == "a\nb\n",
        out + err,
    )
    reset()

    write(root, "Cargo.toml", "this is not toml [")
    rc, out, err = run_packages(root, base)
    check(
        "a PR manifest cargo cannot read fails",
        rc == 1 and "::error::cargo metadata failed" in err,
        err,
    )
    reset()

    for rev, why in [
        ("HEAD", "a ref name"),
        (base[:12], "an abbreviated SHA"),
        ("--output=/tmp/x", "an option"),
        ("0" * 40, "a SHA that is not in the repository"),
    ]:
        rc, out, err = run_packages(root, rev)
        check(
            f"a base that is {why} fails",
            rc == 1 and out == "" and "::error::" in err,
            err,
        )
    rc, out, err = run_packages(root, rev_parse(root, "HEAD^{tree}"))
    check("a base that is a tree, not a commit, fails", rc == 1 and out == "", err)
    rc, out, err = run_packages(root)
    check("no base at all is a usage error", rc == 2 and out == "", err)

    edit("crates/a/Cargo.toml", *PUBLISH_FALSE)
    edit("crates/b/Cargo.toml", *PUBLISH_FALSE)
    git(root, "commit", "-q", "-am", "unpublish everything")
    rc, out, err = run_packages(root, rev_parse(root, "HEAD"))
    check("a base that publishes nothing fails closed", rc == 1 and out == "", err)

    # The base is read from the base commit, never from the PR's tree: with
    # the PR's tree unpublishing everything, the old base still lists a and b.
    rc, out, err = run_packages(root, base)
    check(
        "the list comes from the base commit, not the checkout",
        rc == 0 and out == "a\nb\n",
        out + err,
    )


print("packages: the workflow")

# The step that runs the check must take its crates from the script, keyed
# on the base commit, and pass them next to --workspace.
code = "\n".join(code_lines)
check(
    "the check step reads the crate list from the base commit",
    'crates=$(python3 -I tools/semver_ci.py packages "$BASE_SHA")' in code,
)
check(
    "BASE_SHA is the PR's base commit, or the pushed commit on master",
    "BASE_SHA: ${{ github.event.pull_request.base.sha || github.sha }}" in code,
)
tool_runs = [
    line.strip() for line in code_lines if line.strip().startswith("cargo-semver-checks ")
]
check(
    "every cargo-semver-checks run passes the base crates next to --workspace",
    bool(tool_runs)
    and all(
        run.startswith('cargo-semver-checks semver-checks --workspace "${package_args[@]}"')
        for run in tool_runs
    ),
    repr(tool_runs),
)
check(
    "each crate becomes a --package argument",
    'package_args+=(--package "$crate")' in code,
)

# The gate's path filter, run through grep -E as the step runs it: a change
# to any Cargo.toml must start the job, since any workspace manifest may
# declare cargo-semver-checks metadata.
gate = re.search(r"if ! grep -Eq '([^']+)' <<<\"\$changed\"", semver_yml)
check("the gate's path filter is found", gate is not None)
if gate:
    for path, runs in [
        ("tests/crate-independence/Cargo.toml", True),
        ("crates/fieldglass-aec/Cargo.toml", True),
        ("Cargo.toml", True),
        ("Cargo.lock", True),
        ("docs/notCargo.toml.md", False),
        ("README.md", False),
    ]:
        hit = subprocess.run(
            ["grep", "-Eq", gate.group(1)], input=path, text=True, encoding="utf-8"
        )
        verb = "starts" if runs else "does not start"
        check(f"a change to {path} {verb} the job", (hit.returncode == 0) == runs)

if failures:
    print(f"{failures} failure(s)")
    sys.exit(1)
print("all passed")
