#!/usr/bin/env python3
"""Fail if a script under `tools/` reads or writes text without saying in what.

    python3 tools/check_generator_encoding.py

Python's text I/O defaults to `locale.getpreferredencoding()`. On this repo's
CI, and on any developer machine with a UTF-8 or `C`/`POSIX` locale, that is
UTF-8 — since PEP 540, CPython turns UTF-8 mode on for `C` and `POSIX` — so the
default is right and nothing goes wrong. On a machine whose locale is neither,
most obviously Windows with an ANSI codepage, it is not, and the failure is
silent:

    $ PYTHONIOENCODING=cp1252 python3 -c "print('helicity - right moving storm')"
    helicity ? right moving storm

The en-dash is replaced, not rejected. Every generated table in this repo is
supposed to be byte-reproducible from a pinned upstream — that is what lets a
reviewer read a regeneration diff as "upstream changed" rather than "something
moved" — and a substituted character breaks that guarantee without breaking the
build (#451).

So the rule is: **every text read and every text write states its encoding**,
whether or not the file has non-ASCII in it today. A table that is pure ASCII
now gains an en-dash the next time WMO edits a parameter name, and by then
nobody is looking.

Child processes count. `subprocess.run(..., text=True)` decodes the child's
output with the locale encoding too, and the generators that drive `grib_get`
read parameter names straight out of it — under an ASCII locale that is a
`UnicodeDecodeError` on the first umlaut, and under a mismatched one it is the
same silent substitution as above.

Binary I/O is exempt, since there is no encoding to get wrong. `tarfile.open`
and friends are not text I/O and are exempt too.
"""
from __future__ import annotations

import ast
import sys
from pathlib import Path

TOOLS = Path(__file__).resolve().parent

# Calls that open a text stream. `reconfigure` is here because it is the fix
# for a stream you did not open (`sys.stdout.reconfigure(encoding="utf-8")`).
TEXT_IO = {"write_text", "read_text", "open", "reconfigure"}

# Calls that decode a *child process's* output. They are only text I/O when
# asked to be, which is what `TEXT_MODE_FLAGS` detects.
SUBPROCESS = {"run", "Popen", "check_output", "getoutput", "getstatusoutput"}
TEXT_MODE_FLAGS = {"text", "universal_newlines"}

# Attribute calls that share a name with text I/O but are not it.
NOT_TEXT_IO = {
    ("tarfile", "open"),
    ("zipfile", "open"),
    ("gzip", "open"),
    ("io", "open"),
}


def mode_of(node: ast.Call) -> str | None:
    """The `mode` argument, however it was passed.

    `open(path, "rb")` puts it at index 1; `Path.open("rb")` at index 0. Getting
    that wrong is how a binary write gets reported as a missing encoding, so
    both shapes are read.
    """
    for keyword in node.keywords:
        if keyword.arg == "mode" and isinstance(keyword.value, ast.Constant):
            return keyword.value.value
    index = 1 if isinstance(node.func, ast.Name) else 0
    if len(node.args) > index and isinstance(node.args[index], ast.Constant):
        value = node.args[index].value
        return value if isinstance(value, str) else None
    return None


def qualifier(node: ast.Call) -> tuple[str, str] | None:
    """`(module, attr)` for a call like `tarfile.open(...)`."""
    func = node.func
    if isinstance(func, ast.Attribute) and isinstance(func.value, ast.Name):
        return (func.value.id, func.attr)
    return None


def offenders(path: Path) -> list[tuple[int, str]]:
    tree = ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
    found = []
    for node in ast.walk(tree):
        if not isinstance(node, ast.Call):
            continue
        func = node.func
        name = func.attr if isinstance(func, ast.Attribute) else getattr(func, "id", None)
        if name not in TEXT_IO:
            continue
        if qualifier(node) in NOT_TEXT_IO:
            continue
        mode = mode_of(node)
        if mode is not None and "b" in mode:
            continue
        if any(keyword.arg == "encoding" for keyword in node.keywords):
            continue
        found.append((node.lineno, name))

    for node in ast.walk(tree):
        if not isinstance(node, ast.Call) or not asks_for_text(node):
            continue
        if any(keyword.arg == "encoding" for keyword in node.keywords):
            continue
        found.append((node.lineno, "subprocess(text=True)"))
    return sorted(found)


def asks_for_text(node: ast.Call) -> bool:
    """Whether this is a `subprocess` call that decodes the child's output."""
    func = node.func
    name = func.attr if isinstance(func, ast.Attribute) else getattr(func, "id", None)
    if name not in SUBPROCESS:
        return False
    return any(
        keyword.arg in TEXT_MODE_FLAGS
        and (not isinstance(keyword.value, ast.Constant) or bool(keyword.value.value))
        for keyword in node.keywords
    )


def main() -> int:
    failures = []
    for path in sorted(TOOLS.glob("*.py")):
        if path.name == Path(__file__).name:
            continue
        for line, name in offenders(path):
            failures.append(f"  {path.relative_to(TOOLS.parent)}:{line}: {name} has no encoding=")
    if failures:
        print("Text I/O without an explicit encoding (see #451):", file=sys.stderr)
        print("\n".join(failures), file=sys.stderr)
        print(
            "\nPass `encoding=\"utf-8\"`. For a stream you did not open, "
            'reconfigure it: `sys.stdout.reconfigure(encoding="utf-8")`.',
            file=sys.stderr,
        )
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
