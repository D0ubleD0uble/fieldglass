#!/usr/bin/env python3
"""Self-test for `semver_ci.py`.

    python3 tools/test_semver_ci.py

`semver.yml` runs this on every PR, over the PR's copy of the script. The
guard cases are the ways a PR could change the Public API check (#1699): a
guarded file edited, renamed away, deleted, or hidden past the first page of
a long file list. The workflow job-name cases (#1825) run the real
`guard` command, with a stand-in `gh` serving the PR's files and blobs: a job
that takes a required check's name, spelled every way YAML and `${{ }}`
expressions allow, and workflow files the guard must refuse to read. They
need PyYAML, as the guard does. The baseline cases run against a real git repository with
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
from collections.abc import Callable
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
    if path.startswith(".github/workflows/"):
        # A workflow is read as well, so it needs the stand-in API below;
        # those cases run further down, through the real command.
        continue
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


print("guard: workflow job names")

# The real command, `python3 -I tools/semver_ci.py guard`, run as the guard
# job runs it, with a stand-in `gh` first on the PATH. The stand-in serves the
# PR's file listing and each workflow's blob, the two API calls the guard
# makes, from a JSON file, and fails any other call (#1825).
GUARD_SCRIPT = Path(__file__).resolve().parent / "semver_ci.py"
REPO_ROOT = Path(__file__).resolve().parent.parent
WORKFLOWS = REPO_ROOT / ".github" / "workflows"
PUBLIC_API = "Public API vs last tag"
GUARD_NAME = "Semver files need the semver-change label"
SEMVER_YML = ".github/workflows/semver.yml"
GUARD_YML = ".github/workflows/semver-guard.yml"

FAKE_GH = """#!{python} -I
import json, sys
state = json.load(open({state!r}, encoding="utf-8"))
args = sys.argv[1:]
files = "repos/owner/repo/pulls/7/files?per_page=100"
if args == ["api", "--paginate", "--slurp", files]:
    print(json.dumps(state["pages"]))
elif args == ["api", "repos/owner/repo/pulls/7"]:
    if state["pull"] is None:
        sys.exit("gh: Server Error (HTTP 502)")
    print(json.dumps(state["pull"]))
elif len(args) == 2 and args[0] == "api" and args[1].startswith("repos/owner/repo/git/trees/"):
    tree = state["trees"].get(args[1].rsplit("/", 1)[1])
    if tree is None:
        sys.exit("gh: Not Found (HTTP 404)")
    print(json.dumps(tree))
elif len(args) == 2 and args[0] == "api" and args[1].startswith("repos/owner/repo/git/blobs/"):
    blob = state["blobs"].get(args[1].rsplit("/", 1)[1])
    if blob is None:
        sys.exit("gh: Not Found (HTTP 404)")
    print(json.dumps(blob))
else:
    sys.exit("unexpected gh call: " + repr(args))
"""


def blob_sha(data: bytes) -> str:
    """A blob id for `data`, in the shape of a SHA-256 repository's."""
    import hashlib

    return hashlib.sha256(b"blob %d\0" % len(data) + data).hexdigest()


def run_guard(
    workflows: dict[str, bytes | str | None],
    labels: list[str] = (),
    others: list[dict] = (),
    blobs: dict[str, dict] | None = None,
    flags: tuple[str, ...] = (),
    modes: dict[str, str] | None = None,
    tamper: Callable[[dict], None] | None = None,
) -> tuple[int, str]:
    """Run the real guard over a PR that changes `workflows` (path -> new
    content; None is a deletion) and the file entries in `others`.

    The PR's head commit holds every file it adds or changes, under the
    blob SHA its entry lists, as a regular file unless `modes` says
    otherwise. `tamper` may change the stand-in API's state first."""
    files = list(others)
    served = dict(blobs or {})
    for path, content in workflows.items():
        if content is None:
            files.append({"filename": path, "status": "removed", "sha": "0" * 40})
            continue
        data = content.encode("utf-8") if isinstance(content, str) else content
        sha = blob_sha(data)
        files.append({"filename": path, "status": "added", "sha": sha})
        served.setdefault(
            sha,
            {
                "sha": sha,
                "size": len(data),
                "encoding": "base64",
                "content": __import__("base64").encodebytes(data).decode("ascii"),
            },
        )
    head = "1" * 40
    root: dict = {}
    for f in files:
        if f.get("status") == "removed" or not isinstance(f.get("filename"), str):
            continue
        *dirs, leaf = f["filename"].split("/")
        node = root
        for d in dirs:
            node = node.setdefault(d, {})
        node[leaf] = ((modes or {}).get(f["filename"], "100644"), f.get("sha"))
    trees: dict[str, dict] = {}

    def tree_of(node: dict, prefix: str) -> str:
        import hashlib

        sha = head if not prefix else hashlib.sha256(prefix.encode("utf-8")).hexdigest()
        entries = []
        for name, child in sorted(node.items()):
            if isinstance(child, dict):
                entries.append({"path": name, "mode": "040000", "type": "tree", "sha": tree_of(child, f"{prefix}/{name}")})
            else:
                mode, blob = child
                entries.append({"path": name, "mode": mode, "type": "blob", "sha": blob})
        trees[sha] = {"sha": sha, "tree": entries, "truncated": False}
        return sha

    tree_of(root, "")
    state_data = {"pages": pages_of(files), "blobs": served, "pull": {"head": {"sha": head}}, "trees": trees}
    if tamper is not None:
        tamper(state_data)
    with tempfile.TemporaryDirectory() as tmp:
        state = Path(tmp) / "state.json"
        state.write_text(json.dumps(state_data), encoding="utf-8")
        gh = Path(tmp) / "gh"
        gh.write_text(FAKE_GH.format(python=sys.executable, state=str(state)), encoding="utf-8")
        gh.chmod(0o755)
        env = {
            "PATH": f"{tmp}{os.pathsep}{os.environ.get('PATH', '')}",
            "GH_TOKEN": "unused",
            "REPO": "owner/repo",
            "PR_NUMBER": "7",
            "CHANGED_FILES": str(len(files)),
            "LABELS": json.dumps(list(labels)),
        }
        proc = subprocess.run(
            [sys.executable, "-I", *flags, str(GUARD_SCRIPT), "guard"],
            env=env,
            capture_output=True,
            text=True,
            encoding="utf-8",
            timeout=120,
        )
    return proc.returncode, proc.stdout + proc.stderr


def workflow(jobs: str, top: str = "") -> str:
    return f"name: CI\non: [pull_request]\n{top}jobs:\n{jobs}"


def job(job_id: str, name: str | None = None) -> str:
    named = f"    name: {name}\n" if name is not None else ""
    return f"  {job_id}:\n{named}    runs-on: ubuntu-latest\n    steps:\n      - run: true\n"


def check_clash(why: str, result: tuple[int, str], *needles: str) -> None:
    rc, out = result
    check(f"{why}: fails", rc == 1, out)
    check(
        f"{why}: the error names the file and the check",
        all(n in out for n in needles) and "::error::" in out,
        out,
    )


NEW = ".github/workflows/new.yml"

# The guard job runs the runner's system python3 with -I and installs
# nothing, so PyYAML must come with it. This job runs on the same image.
yaml_check = subprocess.run(
    [sys.executable, "-I", "-c", "import yaml"], capture_output=True, text=True, encoding="utf-8"
)
check("PyYAML imports under python3 -I, as the guard runs it", yaml_check.returncode == 0, yaml_check.stderr)

# Acceptance: a job named like either required check, in a new workflow.
for name in (PUBLIC_API, GUARD_NAME):
    for labels in ([], [LABEL]):
        with_label = " with the label" if labels else ""
        check_clash(
            f"a new workflow with a job named '{name}'{with_label}",
            run_guard({NEW: workflow(job("stand-in", name))}, labels),
            NEW,
            name,
        )

# Acceptance: a matrix or expression name that could render to either.
for name, renders in [
    ("${{ matrix.check }}", PUBLIC_API),
    ("'${{ matrix.a }} ${{ matrix.b }}'", GUARD_NAME),
    ("Public ${{ matrix.word }} last tag", PUBLIC_API),
    ("Semver files need the ${{ inputs.what }}", GUARD_NAME),
    ("\"${{ format('{0} API vs last tag', 'Public') }}\"", PUBLIC_API),
    ("'Public API vs last tag${{ matrix.empty }}'", PUBLIC_API),
    ("Public API vs ${{ unclosed", PUBLIC_API),
    ("\"${{ '}}' }}Public API vs last tag\"", PUBLIC_API),
    ("\"${{ '}}' && 'Public API vs last tag' }}\"", PUBLIC_API),
    ("'${{ \"}}\" }}Public API vs last tag'", PUBLIC_API),
    ("\"${{ 'it''s }}' }}Public API vs last tag\"", PUBLIC_API),
]:
    rc, out = run_guard({NEW: workflow(job("stand-in", name))})
    check_clash(f"an expression name {name}", (rc, out), NEW, renders)
    check(f"{name}: called an expression", "could render to" in out, out)

stand_in = workflow(job("stand-in", PUBLIC_API).replace(
    "    runs-on:", "    strategy:\n      matrix:\n        os: [a, b]\n    runs-on:"
))
check_clash("a matrix job named like the check", run_guard({NEW: stand_in}), NEW, PUBLIC_API)

# The same name spelled every way YAML and GitHub allow.
for why, text in [
    ("a job id with no name", workflow(job(f'"{PUBLIC_API}"'))),
    ("double quotes with escapes", workflow(job("x", '"Public\\x20API\\u0020vs last\\ttag"'))),
    ("single quotes", workflow(job("x", f"'{PUBLIC_API}'"))),
    ("a folded scalar", workflow(job("x", ">-\n      Public API\n      vs last tag"))),
    ("a literal scalar", workflow(job("x", "|\n      Public API vs last tag"))),
    ("another case", workflow(job("x", PUBLIC_API.upper()))),
    ("extra whitespace", workflow(job("x", "'  Public  API vs last tag '"))),
    ("a `Name:` key", workflow(job("x").replace("    runs-on", f"    Name: {PUBLIC_API}\n    runs-on"))),
    ("a `Jobs:` key", workflow(job("x", PUBLIC_API)).replace("\njobs:", "\nJobs:")),
    (
        "an alias",
        workflow(job("x", "*n"), top=f"env:\n  N: &n {PUBLIC_API}\n"),
    ),
    ("flow style", f'on: push\njobs: {{x: {{name: "{PUBLIC_API}", runs-on: u, steps: []}}}}\n'),
    (
        "JSON",
        json.dumps({"on": "push", "jobs": {"x": {"name": PUBLIC_API, "runs-on": "u"}}}),
    ),
    ("a !!str tag", workflow(job("x", f"!!str {PUBLIC_API}"))),
    ("a UTF-16 file", workflow(job("x", PUBLIC_API)).encode("utf-16")),
]:
    check_clash(f"{why}", run_guard({NEW: text}), NEW, PUBLIC_API)

for path in (
    ".github/workflows/new.yaml",
    ".github/workflows/NEW.YML",
    ".github/workflows/sub/dir.yml",
    ".GitHub/Workflows/new.yml",
):
    check_clash(f"a workflow at {path}", run_guard({path: workflow(job("x", PUBLIC_API))}), path)

# The owner workflows: each name once, in its own file. Both are guarded, so
# the label clears an ordinary change to them, and only that.
semver_text = (WORKFLOWS / "semver.yml").read_text(encoding="utf-8")
guard_text = (WORKFLOWS / "semver-guard.yml").read_text(encoding="utf-8")
rc, out = run_guard({SEMVER_YML: semver_text, GUARD_YML: guard_text}, [LABEL])
check("the owner workflows as they are pass with the label", rc == 0, out)
check("...and only for the label", "::error::" not in out, out)
rc, out = run_guard({SEMVER_YML: semver_text}, [])
check("the owner workflow still needs the label", rc == 1 and "semver-change" in out, out)
check_clash(
    "semver.yml taking the guard's name, even with the label",
    run_guard({SEMVER_YML: semver_text + job("second", GUARD_NAME)}, [LABEL]),
    SEMVER_YML,
    GUARD_NAME,
)
check_clash(
    "a second job named like the check in its own workflow",
    run_guard({SEMVER_YML: semver_text + job("second", PUBLIC_API)}, [LABEL]),
    SEMVER_YML,
    "`second`",
)
check_clash(
    "a rename of the owner workflow keeps its job",
    run_guard(
        {".github/workflows/api.yml": semver_text},
        [LABEL],
        others=[],
    ),
    ".github/workflows/api.yml",
    PUBLIC_API,
)

# Acceptance: ordinary workflow changes pass with no label.
ci_text = (WORKFLOWS / "ci.yml").read_text(encoding="utf-8")
bumped = ci_text.replace(
    "actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1 # v7.0.1",
    "actions/checkout@0123456789abcdef0123456789abcdef01234567 # v7.0.2",
)
check("the Dependabot-style bump changes ci.yml", bumped != ci_text)
rc, out = run_guard({".github/workflows/ci.yml": bumped})
check("a Dependabot-style `uses:` pin bump passes with no label", rc == 0, out)
check("...with nothing flagged", "::error::" not in out and "::warning::" not in out, out)
rc, out = run_guard({NEW: workflow(job("lint", "Lint the docs") + job("other"))})
check("a new unrelated job passes with no label", rc == 0, out)
for name in (
    "Public API vs last tag (extra)",
    "Build (${{ matrix.target }})",
    "${{ matrix.os }} / ${{ matrix.target }}",
    "Public API",
    "'Public API vs last tag ${{ matrix.os }} x'",
):
    rc, out = run_guard({NEW: workflow(job("x", name))})
    check(f"a name that cannot render to a check passes: {name}", rc == 0, out)
rc, out = run_guard({NEW: None})
check("deleting a workflow passes; it is not fetched", rc == 0, out)
rc, out = run_guard({".github/workflows/notes.md": "Public API vs last tag"})
check("a non-YAML file beside the workflows is not read", rc == 0, out)
everything = {
    f".github/workflows/{p.name}": p.read_bytes()
    for p in sorted(WORKFLOWS.iterdir())
    if p.suffix in (".yml", ".yaml") and p.name not in ("semver.yml", "semver-guard.yml")
}
check("the repository has other workflows", len(everything) >= 5, repr(everything.keys()))
rc, out = run_guard(everything)
check("every other workflow in the repository passes as a change", rc == 0, out)

# A workflow the guard cannot read fails closed, and the label clears it.
deep = "on: push\njobs:\n  x:\n    name: " + "[" * 20000 + "]" * 20000 + "\n"
for why, content, blobs in [
    ("not UTF-8", workflow(job("x", "caf\xe9")).encode("latin-1"), None),
    ("a NUL byte", workflow(job("x", "a\0b")), None),
    ("two documents", workflow(job("x")) + "---\n" + workflow(job("y")), None),
    ("a name key twice", workflow(job("x", "ok").replace("    runs-on", f"    name: {PUBLIC_API}\n    runs-on")), None),
    ("a name key twice, differently cased", workflow(job("x", "ok").replace("    runs-on", f"    NAME: {PUBLIC_API}\n    runs-on")), None),
    ("a job id twice", workflow(job("x", PUBLIC_API) + job("x", "ok")), None),
    ("not a mapping", "- just\n- a list\n", None),
    ("nothing in it", "", None),
    ("only a comment", "# jobs:\n", None),
    ("no jobs", "on: push\n", None),
    ("jobs that are a list", "on: push\njobs:\n  - x\n", None),
    ("a job that is a string", "on: push\njobs:\n  x: hi\n", None),
    ("a name that is a list", workflow(job("x", "[a, b]")), None),
    ("a Python tag", workflow(job("x", "!!python/object:os.system x")), None),
    ("a Python tuple", workflow(job("x", "!!python/tuple [a, b]")), None),
    ("an unknown tag", workflow(job("x", f"!custom {PUBLIC_API}")), None),
    ("unclosed quotes", workflow(job("x", '"Public API')), None),
    ("deep nesting", deep, None),
    (
        "a merge key, which GitHub refuses",
        workflow(
            "  x:\n    <<: *base\n    steps:\n      - run: true\n",
            top=f"x-base: &base\n  name: {PUBLIC_API}\n  runs-on: ubuntu-latest\n",
        ),
        None,
    ),
    ("a key that is not text", workflow(job("x").replace("    runs-on", "    !!binary bmFtZQ==: x\n    runs-on")), None),
    ("a name that is bytes", workflow(job("x", "!!binary UHVibGljIEFQSQ==")), None),
    ("257 jobs", workflow("".join(job(f"j{i}") for i in range(257))), None),
    ("over 1 MiB", workflow(job("x")) + "#" * (1024 * 1024) + "\n", None),
]:
    rc, out = run_guard({NEW: content})
    check(f"a workflow with {why} fails closed", rc == 1 and NEW in out and "::error::" in out, out)
    check(f"{why}: it says it could not read it", "could not read the workflow" in out, out)
    rc, out = run_guard({NEW: content}, [LABEL])
    check(f"{why}: the label clears it, with a warning", rc == 0 and "::warning::" in out, out)

# Each reply below would read as a clean workflow if the guard skipped the
# check that refuses it.
clean = workflow(job("x", "Lint")).encode("utf-8")
clean_b64 = __import__("base64").b64encode(clean).decode("ascii")
for why, entry_, blobs in [
    ("no blob SHA", {"filename": NEW, "status": "added"}, {}),
    ("a blob the API does not have", {"filename": NEW, "status": "added", "sha": "a" * 40}, {}),
    (
        "a blob that is not marked base64",
        {"filename": NEW, "status": "modified", "sha": "b" * 40},
        {"b" * 40: {"size": len(clean), "encoding": "utf-8", "content": clean_b64}},
    ),
    (
        "a blob whose size is wrong",
        {"filename": NEW, "status": "modified", "sha": "c" * 40},
        {"c" * 40: {"size": len(clean) + 1, "encoding": "base64", "content": clean_b64}},
    ),
    (
        "a blob with no size",
        {"filename": NEW, "status": "modified", "sha": "e" * 40},
        {"e" * 40: {"size": True, "encoding": "base64", "content": clean_b64}},
    ),
    (
        "a blob the API says is too large",
        {"filename": NEW, "status": "modified", "sha": "d" * 40},
        {"d" * 40: {"size": 10**8, "encoding": "base64", "content": ""}},
    ),
    (
        "a SHA that is not hex",
        {"filename": NEW, "status": "modified", "sha": "../../pulls"},
        {"pulls": {"size": len(clean), "encoding": "base64", "content": clean_b64}},
    ),
]:
    # A listed SHA that is no SHA can't match the head commit's, so those
    # fail at the mode check, before any blob is fetched.
    rc, out = run_guard({}, others=[entry_], blobs=blobs)
    refused = "could not read the workflow" in out or "cannot tell whether" in out
    check(f"{why} fails closed", rc == 1 and refused, out)

# Many aliases of aliases are built by reference, not copied out.
laughs = "on: push\nx-0: &a0 [lol]\n" + "".join(
    f"x-{i}: &a{i} [" + ", ".join([f"*a{i - 1}"] * 9) + "]\n" for i in range(1, 12)
)
import time  # noqa: E402

started = time.monotonic()
rc, out = run_guard({NEW: laughs + workflow(job("x", PUBLIC_API)).split("\n", 2)[2]})
check("a billion-laughs file is read quickly", time.monotonic() - started < 60, out)
check_clash("...and its clash is still found", (rc, out), NEW, PUBLIC_API)

# A long name of many expressions is matched in linear time, not by
# backtracking over every way to split the check's name among them.
started = time.monotonic()
long_name = "'" + "${{ a }} a " * 40000 + "${{ b }}'"
rc, out = run_guard({NEW: workflow(job("x", long_name))})
check("a long name of many expressions is matched quickly", time.monotonic() - started < 60, out)
check("...and passes: no check has 40,000 a's", rc == 0, out)
started = time.monotonic()
rc, out = run_guard({NEW: workflow(job("x", "'" + "${{ a }} " * 40000 + "'"))})
check("a long name of only expressions is matched quickly", time.monotonic() - started < 60, out)
check_clash("...and is a clash", (rc, out), NEW, PUBLIC_API)

# One anchor with many keys, aliased as every job: refused past GitHub's 256
# jobs, and read once per mapping below it, so the run time stays bounded.
anchor = "x-a: &a {" + ", ".join(f"k{i}: 1" for i in range(50000)) + "}\n"
for jobs, passes in [(40000, False), (256, True)]:
    started = time.monotonic()
    fanned = anchor + "on: push\njobs:\n" + "".join(f"  j{i}: *a\n" for i in range(jobs))
    rc, out = run_guard({NEW: fanned})
    check(f"{jobs} jobs aliasing one 50,000-key mapping run quickly", time.monotonic() - started < 120, out)
    if passes:
        check(f"...and {jobs} jobs are read: no clash", rc == 0, out)
    else:
        check(f"...and {jobs} jobs are refused", rc == 1 and "256 jobs" in out, out)

# A file that is one plain value, as a symlink's blob often is, is refused
# even where the head commit calls it a regular file: the second line of
# defense behind the mode check below. No label clears it.
for labels in ([], [LABEL]):
    rc, out = run_guard({NEW: "../../shared/ci.yml"}, labels)
    with_label = " with the label" if labels else ""
    check(f"a workflow that is one plain value fails{with_label}", rc == 1 and "single value" in out, out)
    check(f"...and it says the label does not clear it{with_label}", "does not clear" in out, out)

# The guard reads the mode from the head commit's tree, not the blob: a
# symlink's target can be any text, even `jobs: {}`, which parses as a valid
# empty workflow while GitHub would read the file it points to.
for why, content, mode, passes in [
    ("a symlink whose target reads as a workflow", "jobs: {}", "120000", False),
    ("a symlink to a plain path", "../../shared/ci.yml", "120000", False),
    ("a submodule", "jobs: {}", "160000", False),
    ("a regular file", "jobs: {}", "100644", True),
    ("an executable file", "jobs: {}", "100755", True),
]:
    for labels in ([], [LABEL]):
        with_label = " with the label" if labels else ""
        rc, out = run_guard({NEW: content}, labels, modes={NEW: mode})
        if passes:
            check(f"{why} passes{with_label}", rc == 0, out)
        else:
            check(
                f"{why} fails{with_label}",
                rc == 1 and NEW in out and "regular file" in out and "does not clear" in out,
                out,
            )
rc, out = run_guard({NEW: "jobs: {}"}, modes={NEW: "120000"})
check("a symlink is named as one", "it is a symlink" in out, out)

# When the guard can't read the mode, or the head holds another blob, it
# fails closed, and the label does not clear that either.
def drop_pull(state: dict) -> None:
    state["pull"] = None


def bad_head(state: dict) -> None:
    # The stand-in serves the head's tree under the bad SHA too, so only the
    # SHA check can refuse it.
    state["pull"] = {"head": {"sha": "1" * 40 + "\n"}}
    state["trees"]["1" * 40 + "\n"] = state["trees"]["1" * 40]


def truncate(state: dict) -> None:
    for tree in state["trees"].values():
        tree["truncated"] = True


def lose_trees(state: dict) -> None:
    state["trees"] = {}


def other_blob(state: dict) -> None:
    for tree in state["trees"].values():
        for e in tree["tree"]:
            if e["type"] == "blob":
                e["sha"] = "9" * 64


def symlinked_dir(state: dict) -> None:
    for tree in state["trees"].values():
        for e in tree["tree"]:
            if e["type"] == "tree":
                e["mode"], e["type"] = "120000", "blob"


for why, tamper in [
    ("the API can't give the PR's head", drop_pull),
    ("a head SHA that is not a SHA", bad_head),
    ("a truncated tree", truncate),
    ("a tree the API does not have", lose_trees),
    ("a head blob other than the listed one", other_blob),
    ("a directory on the path that is not a directory", symlinked_dir),
]:
    for labels in ([], [LABEL]):
        with_label = " with the label" if labels else ""
        rc, out = run_guard({NEW: workflow(job("x", "Lint"))}, labels, tamper=tamper)
        check(f"{why} fails closed{with_label}", rc == 1 and NEW in out and "does not clear" in out, out)

# A blob SHA with a trailing newline is not a SHA, even where the API would
# serve something under it.
rc, out = run_guard(
    {},
    others=[{"filename": NEW, "status": "modified", "sha": "f" * 40 + "\n"}],
    blobs={"f" * 40 + "\n": {"size": len(clean), "encoding": "base64", "content": clean_b64}},
)
check("a blob SHA with a trailing newline fails closed", rc == 1 and "cannot tell whether" in out, out)

# Without PyYAML the guard cannot read any workflow. That is the runner's
# fault, so it fails every PR that changes one, and the label does not clear
# it. `-S` leaves site-packages off the path, so PyYAML can't be imported.
no_yaml = subprocess.run(
    [sys.executable, "-I", "-S", "-c", "import yaml"], capture_output=True, text=True, encoding="utf-8"
)
check("python3 -I -S cannot import PyYAML (so the next cases mean something)", no_yaml.returncode != 0)
for labels in ([], [LABEL]):
    rc, out = run_guard({NEW: workflow(job("x", PUBLIC_API))}, labels, flags=("-S",))
    with_label = " with the label" if labels else ""
    check(
        f"no PyYAML: a workflow change fails{with_label}",
        rc == 1 and "no working PyYAML" in out and "does not clear" in out,
        out,
    )
    rc, out = run_guard({NEW: workflow(job("x", "Lint"))}, labels, flags=("-S",))
    check(f"no PyYAML: even a clean workflow change fails{with_label}", rc == 1, out)
rc, out = run_guard({}, others=[entry("README.md")], flags=("-S",))
check("no PyYAML: a PR with no workflow change still passes", rc == 0, out)


print("workflows")

# The checkout the semver.yml jobs run is the PR's, so how they call Python
# and cargo is part of what the guard protects. Pin the parts the #1699
# review showed a PR could subvert from an unguarded file.
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
# One Rust pin for the whole workflow, at the top level, so the Public API
# job (rust-cache runs cargo too) and the self-test cannot drift apart (#1825).
pin = re.search(r"\nenv:\n  RUSTUP_TOOLCHAIN: (\d+\.\d+\.\d+)\n", semver_yml.split("\njobs:\n", 1)[0])
check("semver.yml pins RUSTUP_TOOLCHAIN workflow-wide", pin is not None)
check(
    "nothing else in semver.yml sets RUSTUP_TOOLCHAIN",
    len(re.findall(r"^\s*RUSTUP_TOOLCHAIN:", semver_yml, re.M)) == 1,
)
setup = re.compile(
    r"- name: Set up Rust\n(?:\s+if: .*\n)?"
    r"\s+uses: dtolnay/rust-toolchain@([0-9a-f]{40}) # .*\n\s+with:\n(?:\s*#.*\n)*"
    r"\s+toolchain: \$\{\{ env\.RUSTUP_TOOLCHAIN \}\}\n"
)
setups = [setup.search(j) for j in (check_job, selftest_job)]
check(
    "both jobs install the pinned Rust, from the same action pin",
    all(setups) and setups[0].group(1) == setups[1].group(1),
)
check(
    "the self-test sets up Rust before it runs",
    bool(setups[1]) and selftest_job.index("Set up Rust") < selftest_job.index("test_semver_ci.py"),
)
check("the self-test prints the Rust it uses", "run: rustc -vV" in selftest_job)
if os.environ.get("GITHUB_JOB") == "semver-ci-selftest" and pin:
    # In the job itself: the Rust on the path is the pinned one.
    rustc = subprocess.run(["rustc", "-vV"], capture_output=True, text=True, encoding="utf-8")
    check(
        f"the self-test job runs Rust {pin.group(1)}",
        f"\nrelease: {pin.group(1)}\n" in rustc.stdout,
        rustc.stdout + rustc.stderr,
    )
check(
    "no step sets its own toolchain or rustc override",
    all(
        len(re.findall(rf"^\s+{var}:", check_job, re.M)) == 1
        for var in (
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
        git(root, "reset", "-q", "--hard")
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
    check("publish = false on a published crate fails", rc == 1 and out == "", out + err)
    check(
        "publish = false: the failure names the crate and the owner's call",
        "::error::b is published on the base branch but `publish = false`" in err
        and "release decision for the owner" in err,
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
        (base + "\n", "a SHA with a trailing newline"),
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
    for rev in (base + "\n", base + "0"):
        rc, out, err = run_packages(root, rev)
        check(
            f"a full SHA with more after it is refused as a SHA: {rev!r}",
            rc == 1 and "the base must be a full commit SHA" in err,
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
    # the PR's tree unpublishing everything, the old base still lists a and b,
    # and un-publishing them fails.
    rc, out, err = run_packages(root, base)
    check(
        "the list comes from the base commit, not the checkout",
        rc == 1 and "all checked: a, b" in err and "::error::a is published" in err,
        out + err,
    )
    git(root, "reset", "-q", "--hard", base)

    # A tracked manifest that is a symlink can change through its target,
    # which the job's path filter never sees.
    (root / "crates/b/Cargo.toml").rename(root / "crates/b/manifest.toml")
    (root / "crates/b/Cargo.toml").symlink_to("manifest.toml")
    git(root, "add", "-A")
    rc, out, err = run_packages(root, base)
    check("a symlinked Cargo.toml fails", rc == 1 and out == "", out + err)
    check(
        "the symlinked manifest is named",
        "::error::crates/b/Cargo.toml is a symlink" in err,
        err,
    )
    reset()
    rc, out, err = run_packages(root, base)
    check("a regular manifest again passes", rc == 0 and out == "a\nb\n", out + err)


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
