#!/usr/bin/env python3
"""The `fieldglass-aec` oracle job (#763): libaec regenerated and compared in full.

    python3 tools/aec_oracle.py                   # download the pinned tarball
    python3 tools/aec_oracle.py --tarball PATH    # use a local copy of it

`.github/workflows/aec-oracle.yml` runs this; it runs the same way locally.
It needs what `tools/build_aec_fixtures.py` needs (cmake 3.26 or newer, a C
compiler, network access or `--tarball`) plus cargo. It is not a pre-commit or
pre-push hook, for exactly those reasons.

Three steps, each of which must pass:

1. **The committed corpus is libaec's.** Regenerate it into a scratch directory
   and `diff -r` it against `crates/fieldglass-aec/tests/fixtures`. The crate's
   tests only check the decoder against the manifest, so a manifest edited by
   hand would pass them; it cannot pass this. The regenerated corpus, szip
   cases included, is then decoded by `examples/oracle.rs`.
2. **The full matrix.** `build_aec_fixtures.py --full` (about 5,000 cases, not
   committed), decoded by `examples/oracle.rs`, every case.
3. **The CCSDS 121.0-B-2 sample data** in the tarball's `data/121B2TestData`,
   with the parameters libaec's own `tests/sampledata.sh` passes, read from
   that script rather than copied here. Exactly `SAMPLE_STREAMS` streams must
   be compared, and a missing file fails.

`examples/oracle.rs` holds the one allowance for libaec's recorded divergences
(ADR-0012 decision 4), keyed on the kind the generator gives each case. It
fails on anything else.

Scratch files go under `target/aec-oracle/`. It is emptied first, removed at the
end unless `--keep`, and left in place when a step fails, for reading.
"""
from __future__ import annotations

import argparse
import io
import os
import shutil
import subprocess
import sys
import tarfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import build_aec_fixtures as builder  # noqa: E402

REPO = builder.REPO
FIXTURES = builder.FIXTURES
WORK = REPO / "target" / "aec-oracle"
ORACLE = REPO / "target" / "release" / "examples" / "oracle"

# `aec_cases` and `sz_cases` in the committed corpus, the counts
# `crates/fieldglass-aec/tests/common/mod.rs` pins. Step 1's diff already holds
# them; pinning them here too makes a count change fail with a plain message.
CORPUS_AEC_CASES = 540
CORPUS_SZ_CASES = 78
# `aec_cases` in the `--full` matrix. Pinned so a matrix that shrinks, or
# comes back empty, fails rather than passing on fewer cases. A change to the
# matrix in `build_aec_fixtures.py` changes this number in the same commit.
FULL_AEC_CASES = 4819
# The streams `tests/sampledata.sh` decodes: 28 in AllOptions, 36 in
# LowEntropyOptions and 2 in ExtendedParameters. AllOptions also holds eight
# streams (n25 to n32) the script never names, so they are not compared.
SAMPLE_STREAMS = 66

SAMPLE_DIR = f"libaec-{builder.LIBAEC_VERSION}/data/121B2TestData/"
SAMPLE_SCRIPT = f"libaec-{builder.LIBAEC_VERSION}/tests/sampledata.sh"

# Replaces `sampledata.sh`'s `decode`, `codec` and `cosdec` (each of which
# ends in a decode) with one that prints its arguments: the stream, the
# expected output and the graec options. The script's own loops and variables
# then list every comparison it would make, with its own parameters.
SAMPLE_LISTER = """
decode () { printf '%s\\t%s\\t%s\\n' "$1" "$2" "$3"; }
codec () { decode "$@"; }
cosdec () { decode "$@"; }
"""


def step(title: str) -> None:
    print(f"\n== {title}", flush=True)


def oracle(mode: str, path: Path, **expect: int) -> None:
    """Run `examples/oracle.rs` in `mode` over `path`.

    It takes its inputs from the environment, not arguments (see its docs).
    Each `expect` keyword names an `AEC_ORACLE_*` count, e.g.
    `EXPECT_AEC=4819` sets `AEC_ORACLE_EXPECT_AEC`.
    """
    env = {k: v for k, v in os.environ.items() if not k.startswith("AEC_ORACLE_")}
    settings = {"AEC_ORACLE_MODE": mode, "AEC_ORACLE_INPUT": str(path)}
    settings.update({f"AEC_ORACLE_{key}": str(value) for key, value in expect.items()})
    env.update(settings)
    print("$ " + " ".join(f"{k}={v}" for k, v in settings.items()) + f" {ORACLE}", flush=True)
    code = subprocess.run([str(ORACLE)], env=env).returncode
    if code != 0:
        raise SystemExit(f"oracle {mode} exited with {code}")


def run(cmd: list[str], **kw) -> None:
    """Run `cmd`; stop the job, without a traceback, if it fails."""
    print("$ " + " ".join(str(c) for c in cmd), flush=True)
    code = subprocess.run(cmd, **kw).returncode
    if code != 0:
        raise SystemExit(f"{' '.join(Path(c).name for c in cmd[:2])} exited with {code}")


def generate(tarball: Path, *args: str) -> None:
    run([sys.executable, str(REPO / "tools" / "build_aec_fixtures.py"), "--tarball", str(tarball), *args])


def extract_samples(source: bytes, dest: Path) -> None:
    """The sample data and `sampledata.sh`, regular files only, inside `dest`."""
    with tarfile.open(fileobj=io.BytesIO(source), mode="r:gz") as tar:
        for member in tar.getmembers():
            if not member.isfile():
                continue
            if not (member.name.startswith(SAMPLE_DIR) or member.name == SAMPLE_SCRIPT):
                continue
            rel = Path(member.name)
            if rel.is_absolute() or ".." in rel.parts:
                raise SystemExit(f"libaec tarball member escapes the work directory: {member.name}")
            handle = tar.extractfile(member)
            if handle is None:
                continue
            out = dest / rel
            out.parent.mkdir(parents=True, exist_ok=True)
            out.write_bytes(handle.read())


def sample_list(root: Path) -> list[str]:
    """Every (stream, expected, options) `sampledata.sh` decodes, one per line.

    The overrides go after the script's last function definition (its last
    line that is a lone `}`), so the loops that follow call them.
    """
    script = (root / SAMPLE_SCRIPT).read_text(encoding="utf-8")
    lines = script.split("\n")
    closes = [i for i, line in enumerate(lines) if line == "}"]
    if not closes:
        raise SystemExit("sampledata.sh: no function definitions found")
    at = closes[-1] + 1
    lister = root / "sampledata-list.sh"
    lister.write_text("\n".join(lines[:at]) + SAMPLE_LISTER + "\n".join(lines[at:]), encoding="utf-8")
    out = subprocess.run(
        ["sh", str(lister), str((root / SAMPLE_SCRIPT).parent)],
        check=True,
        capture_output=True,
        encoding="utf-8",
    ).stdout
    return [line for line in out.splitlines() if line.count("\t") == 2]


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--tarball", type=Path, help="use this copy of the libaec tarball instead of downloading")
    ap.add_argument("--keep", action="store_true", help=f"keep {WORK.relative_to(REPO)} afterwards")
    args = ap.parse_args(argv)
    started = time.monotonic()

    if WORK.exists():
        shutil.rmtree(WORK)
    WORK.mkdir(parents=True)
    # Fetched and digest-checked once; both generator runs read this copy.
    source = builder.fetch_source(args.tarball)
    tarball = WORK / "libaec.tar.gz"
    tarball.write_bytes(source)

    step("build examples/oracle.rs")
    run(["cargo", "build", "--locked", "--release", "-p", "fieldglass-aec", "--example", "oracle"], cwd=REPO)

    step("1. regenerate the committed corpus and diff it")
    corpus = WORK / "corpus"
    generate(tarball, "--out", str(corpus))
    # NOTICE.md is written by hand; the generator writes everything else.
    diff = subprocess.run(["diff", "-r", "--exclude=NOTICE.md", str(FIXTURES), str(corpus)])
    if diff.returncode != 0:
        raise SystemExit(
            "the committed corpus is not what libaec writes: regenerate it with "
            "`python3 tools/build_aec_fixtures.py`, never edit it by hand"
        )
    print("identical")
    oracle("corpus", corpus, EXPECT_AEC=CORPUS_AEC_CASES, EXPECT_SZ=CORPUS_SZ_CASES)

    step("2. the full matrix")
    full = WORK / "full"
    generate(tarball, "--full", str(full))
    oracle("corpus", full, EXPECT_AEC=FULL_AEC_CASES, EXPECT_SZ=0)

    step("3. CCSDS 121.0-B-2 sample data, with sampledata.sh's parameters")
    samples = WORK / "samples"
    extract_samples(source, samples)
    rows = sample_list(samples)
    if len(rows) != SAMPLE_STREAMS:
        raise SystemExit(f"sampledata.sh names {len(rows)} streams, expected {SAMPLE_STREAMS}")
    listing = WORK / "sampledata.tsv"
    listing.write_text("\n".join(rows) + "\n", encoding="utf-8")
    oracle("sampledata", listing, EXPECT=SAMPLE_STREAMS)

    if not args.keep:
        shutil.rmtree(WORK)
    print(f"\naec oracle: all three steps passed in {time.monotonic() - started:.0f} s")
    return 0


if __name__ == "__main__":
    sys.exit(main())
