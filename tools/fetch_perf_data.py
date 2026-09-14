#!/usr/bin/env python3
"""Fetch the performance report's real data into a capped, content-addressed cache.

    python3 tools/fetch_perf_data.py              # fetch what is missing, verify everything
    python3 tools/fetch_perf_data.py --offline    # verify only; touching the network is an error
    python3 tools/fetch_perf_data.py --clean      # remove the cache and everything in it
    python3 tools/fetch_perf_data.py --print-dir  # where the cache is

The manifest (`crates/fieldglass-perf/manifests/era5.json`) names each object by
the SHA-256 of its bytes, with the URL and byte range it comes from. This script
is the only thing that touches the network for the performance harness — the
readers never do, as ADR-0005 requires — and it is deliberately strict:

* **One copy on disk.** An object is stored once, as `<cache>/<sha256>`, and the
  harness reads it from there straight into memory. Nothing is extracted, and
  nothing is ever written inside the repository: a cache directory under the
  repository root is refused.
* **Fail, never skip, never quietly re-fetch.** A cached object whose size or
  hash is wrong fails the run and names the file; it is not replaced behind the
  user's back, because an object that changed on disk is a fact someone should
  see. A fetched object whose hash is not the manifest's fails too — the
  upstream changed — and leaves no partial file behind.
* **`--offline` proves the second run is free.** It verifies every object and
  fails if any is missing, so a CI job keyed on the manifest's hash can show it
  read the cache and nothing else.
* **Capped, least recently used first.** Every object this run uses has its
  modification time refreshed; when the cache is over its cap, the objects no
  manifest entry needs are removed oldest first. A manifest that needs more than
  the cap on its own is refused before anything is fetched.

URLs are limited to `ALLOWED_PREFIXES`. A manifest is a file in the repository,
but `urllib` also opens `file://` and `ftp://`, and the fetcher is exactly where
a doctored manifest would be turned into a read of something else.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shutil
import sys
import urllib.request
from collections.abc import Callable
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
DEFAULT_MANIFEST = REPO / "crates" / "fieldglass-perf" / "manifests" / "era5.json"

ALLOWED_PREFIXES = ("https://storage.googleapis.com/gcp-public-data-arco-era5/",)

SHA256 = re.compile(r"[0-9a-f]{64}")
PARTIAL_SUFFIX = ".partial"

# (url, offset or None, length) -> bytes
Fetcher = Callable[[str, "int | None", int], bytes]


class Failure(Exception):
    """A failure reported to the user rather than as a traceback."""


def default_cache() -> Path:
    if os.environ.get("FIELDGLASS_PERF_CACHE"):
        return Path(os.environ["FIELDGLASS_PERF_CACHE"])
    base = os.environ.get("XDG_CACHE_HOME") or str(Path.home() / ".cache")
    return Path(base) / "fieldglass-perf"


def http_fetch(url: str, offset: int | None, length: int) -> bytes:
    request = urllib.request.Request(url)
    if offset is not None:
        request.add_header("Range", f"bytes={offset}-{offset + length - 1}")
    with urllib.request.urlopen(request, timeout=120) as response:  # prefix-checked in `load_manifest`
        return response.read()


def load_manifest(path: Path) -> dict:
    try:
        manifest = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        raise Failure(f"{path}: {exc}") from exc
    objects = manifest.get("objects")
    if not isinstance(objects, dict) or not objects:
        raise Failure(f"{path}: the manifest lists no objects")
    for digest, entry in objects.items():
        if not SHA256.fullmatch(digest):
            raise Failure(f"{path}: object key {digest!r} is not a lowercase SHA-256")
        url = entry.get("url", "")
        if not url.startswith(ALLOWED_PREFIXES):
            raise Failure(f"{path}: {url!r} is not under an allowed prefix ({', '.join(ALLOWED_PREFIXES)})")
        length = entry.get("length")
        if not isinstance(length, int) or length <= 0:
            raise Failure(f"{path}: object {digest[:12]} has no positive length")
        offset = entry.get("offset")
        if offset is not None and (not isinstance(offset, int) or offset < 0):
            raise Failure(f"{path}: object {digest[:12]} has a bad offset {offset!r}")
    cap = manifest.get("cache_cap_bytes")
    if not isinstance(cap, int) or cap <= 0:
        raise Failure(f"{path}: the manifest names no positive cache_cap_bytes")
    return manifest


def check_cache_location(cache: Path, repo: Path) -> None:
    resolved = cache.resolve()
    if resolved == repo.resolve() or repo.resolve() in resolved.parents:
        raise Failure(f"the cache {cache} is inside the repository; fetched data never lands in the tree")


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def ensure(
    manifest: dict,
    cache: Path,
    *,
    offline: bool,
    fetch: Fetcher = http_fetch,
    cap: int | None = None,
    log: Callable[[str], None] = print,
) -> dict[str, int]:
    """Make every manifest object present and verified. Returns what it did."""
    objects: dict[str, dict] = manifest["objects"]
    cap = cap if cap is not None else manifest["cache_cap_bytes"]
    needed = sum(entry["length"] for entry in objects.values())
    if needed > cap:
        raise Failure(f"the manifest needs {needed:,} bytes, over the cache cap of {cap:,}")

    cache.mkdir(parents=True, exist_ok=True)
    fetched = fetched_bytes = 0
    missing: list[str] = []
    for digest, entry in objects.items():
        path = cache / digest
        if path.is_file():
            size = path.stat().st_size
            if size != entry["length"] or sha256_file(path) != digest:
                raise Failure(
                    f"cached object {path} is {size:,} bytes and does not hash to its name; "
                    "it changed on disk. Not re-fetching over it: remove it (or run --clean) and fetch again"
                )
            os.utime(path)
            continue
        if offline:
            missing.append(digest)
            continue
        where = f" @{entry['offset']}" if entry.get("offset") is not None else ""
        log(f"  fetch {entry['length']:>12,}  {entry['url']}{where}")
        data = fetch(entry["url"], entry.get("offset"), entry["length"])
        partial = cache / f"{digest}{PARTIAL_SUFFIX}"
        try:
            if len(data) != entry["length"]:
                raise Failure(f"{entry['url']}{where}: fetched {len(data):,} bytes, the manifest says {entry['length']:,}")
            actual = hashlib.sha256(data).hexdigest()
            if actual != digest:
                raise Failure(
                    f"{entry['url']}{where}: fetched bytes hash to {actual}, the manifest says {digest}; "
                    "the upstream object changed, so the manifest needs rebuilding"
                )
            partial.write_bytes(data)
            partial.replace(path)
        finally:
            partial.unlink(missing_ok=True)
        fetched += 1
        fetched_bytes += len(data)

    if missing:
        raise Failure(f"--offline, and {len(missing)} object(s) are not cached, e.g. {missing[0]}; run without --offline first")

    evicted = evict(cache, set(objects), cap)
    if fetched:
        log(f"network: fetched {fetched} object(s), {fetched_bytes:,} bytes")
    else:
        log(f"network: none — all {len(objects)} objects were already cached and verified")
    return {"fetched": fetched, "fetched_bytes": fetched_bytes, "evicted": evicted}


def evict(cache: Path, keep: set[str], cap: int) -> int:
    """Remove unneeded objects, least recently used first, until under `cap`."""
    entries = [p for p in cache.iterdir() if p.is_file() and SHA256.fullmatch(p.name)]
    total = sum(p.stat().st_size for p in entries)
    evicted = 0
    for path in sorted((p for p in entries if p.name not in keep), key=lambda p: p.stat().st_mtime):
        if total <= cap:
            break
        total -= path.stat().st_size
        path.unlink()
        evicted += 1
    if total > cap:
        raise Failure(f"the cache holds {total:,} bytes the manifest needs, over the cap of {cap:,}")
    return evicted


def clean(cache: Path) -> int:
    """Remove the cache directory: objects, partial files, and the directory."""
    if not cache.exists():
        return 0
    stray = [p for p in cache.iterdir() if not (SHA256.fullmatch(p.name.removesuffix(PARTIAL_SUFFIX)) and p.is_file())]
    if stray:
        # Only what this script writes is removed, so a mistyped --cache cannot
        # empty some other directory.
        raise Failure(f"{cache} holds files this script did not write (e.g. {stray[0].name}); not removing it")
    count = len(list(cache.iterdir()))
    shutil.rmtree(cache)
    return count


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--manifest", type=Path, default=DEFAULT_MANIFEST)
    parser.add_argument("--cache", type=Path, default=None, help="default: $FIELDGLASS_PERF_CACHE, else $XDG_CACHE_HOME/fieldglass-perf")
    parser.add_argument("--offline", action="store_true", help="verify only; fail if anything is missing")
    parser.add_argument("--clean", action="store_true", help="remove the cache and everything in it")
    parser.add_argument("--print-dir", action="store_true", help="print the cache directory and exit")
    args = parser.parse_args(argv)
    cache = args.cache or default_cache()
    try:
        check_cache_location(cache, REPO)
        if args.print_dir:
            print(cache)
            return 0
        if args.clean:
            print(f"removed {cache} ({clean(cache)} files)")
            return 0
        ensure(load_manifest(args.manifest), cache, offline=args.offline)
    except Failure as exc:
        print(f"error: {exc}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
