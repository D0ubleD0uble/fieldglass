#!/usr/bin/env python3
"""Unit tests for tools/fetch_perf_data.py.

No network: every test hands `ensure` a fake fetcher that counts its calls, so
"the second run touches no network" is an assertion about a number rather than
an impression. Run:

    python3 tools/test_fetch_perf_data.py
"""

from __future__ import annotations

import hashlib
import importlib.util
import os
import tempfile
import unittest
from pathlib import Path

_spec = importlib.util.spec_from_file_location(
    "fetch_perf_data", Path(__file__).resolve().parent / "fetch_perf_data.py"
)
assert _spec and _spec.loader
fetch = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(fetch)

PREFIX = fetch.ALLOWED_PREFIXES[0]
BLOBS = {"a": b"alpha" * 100, "b": b"bravo" * 50}


def manifest(blobs=BLOBS, cap=10_000):
    objects = {}
    for name, data in blobs.items():
        objects[hashlib.sha256(data).hexdigest()] = {"url": f"{PREFIX}{name}", "length": len(data)}
    return {"objects": objects, "cache_cap_bytes": cap}


class FakeRemote:
    def __init__(self, blobs=BLOBS):
        self.blobs = dict(blobs)
        self.calls = 0

    def __call__(self, url, offset, length):
        self.calls += 1
        data = self.blobs[url.removeprefix(PREFIX)]
        return data if offset is None else data[offset : offset + length]


class FetchTest(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.cache = Path(self._tmp.name) / "cache"
        self.remote = FakeRemote()

    def tearDown(self):
        self._tmp.cleanup()

    def ensure(self, m=None, **kwargs):
        kwargs.setdefault("offline", False)
        kwargs.setdefault("fetch", self.remote)
        return fetch.ensure(m or manifest(), self.cache, log=lambda _: None, **kwargs)

    def test_first_run_fetches_and_second_run_touches_nothing(self):
        self.assertEqual(self.ensure()["fetched"], 2)
        self.assertEqual(self.remote.calls, 2)
        self.assertEqual(self.ensure()["fetched"], 0)
        self.assertEqual(self.remote.calls, 2)
        self.assertEqual(self.ensure(offline=True)["fetched"], 0)
        self.assertEqual(self.remote.calls, 2)

    def test_objects_are_stored_under_their_hash_and_nothing_else(self):
        self.ensure()
        names = sorted(p.name for p in self.cache.iterdir())
        self.assertEqual(names, sorted(manifest()["objects"]))

    def test_offline_with_a_missing_object_fails(self):
        with self.assertRaises(fetch.Failure):
            self.ensure(offline=True)
        self.assertEqual(self.remote.calls, 0)

    def test_a_changed_cached_object_fails_and_is_not_refetched(self):
        self.ensure()
        victim = next(self.cache.iterdir())
        victim.write_bytes(b"tampered")
        with self.assertRaises(fetch.Failure) as caught:
            self.ensure()
        self.assertIn(victim.name, str(caught.exception))
        self.assertEqual(self.remote.calls, 2)
        self.assertEqual(victim.read_bytes(), b"tampered")

    def test_a_truncated_cached_object_fails(self):
        self.ensure()
        victim = next(self.cache.iterdir())
        victim.write_bytes(victim.read_bytes()[:-1])
        with self.assertRaises(fetch.Failure):
            self.ensure(offline=True)

    def test_an_upstream_change_fails_and_leaves_no_partial(self):
        self.remote.blobs["a"] = b"ALPHA" * 100
        written = []
        original = Path.write_bytes

        def spy(path, data):
            written.append(path.name)
            return original(path, data)

        Path.write_bytes = spy
        try:
            with self.assertRaises(fetch.Failure) as caught:
                self.ensure()
        finally:
            Path.write_bytes = original
        self.assertIn("upstream object changed", str(caught.exception))
        # The partial was written (the hash is checked from disk) and removed.
        self.assertTrue(any(name.endswith(fetch.PARTIAL_SUFFIX) for name in written), written)
        self.assertFalse(any(p.name.endswith(fetch.PARTIAL_SUFFIX) for p in self.cache.iterdir()))

    def test_a_partial_left_by_a_killed_run_is_removed(self):
        self.cache.mkdir()
        stale = self.cache / f"{'9' * 64}{fetch.PARTIAL_SUFFIX}"
        stale.write_bytes(b"half")
        self.ensure()
        self.assertFalse(stale.exists())

    def test_a_short_response_fails(self):
        self.remote.blobs["b"] = BLOBS["b"][:-3]
        with self.assertRaises(fetch.Failure):
            self.ensure()

    def test_a_ranged_object_is_fetched_by_its_range(self):
        whole = b"0123456789" * 10
        part = whole[20:30]
        m = {"objects": {hashlib.sha256(part).hexdigest(): {"url": f"{PREFIX}w", "offset": 20, "length": 10}}, "cache_cap_bytes": 100}
        self.remote.blobs["w"] = whole
        self.assertEqual(self.ensure(m)["fetched"], 1)

    def test_a_manifest_over_the_cap_is_refused_before_fetching(self):
        with self.assertRaises(fetch.Failure):
            self.ensure(manifest(cap=10))
        self.assertEqual(self.remote.calls, 0)

    def test_least_recently_used_unneeded_objects_are_evicted_first(self):
        self.cache.mkdir()
        # The older file sorts *last* by name, so eviction by name order would
        # remove the wrong one and fail this test.
        old, new = "2" * 64, "1" * 64
        (self.cache / old).write_bytes(b"x" * 400)
        (self.cache / new).write_bytes(b"y" * 400)
        os.utime(self.cache / old, (1, 1))
        # 500 + 250 needed, 800 already there, cap 1200: one must go, the old one.
        result = self.ensure(manifest(cap=1200))
        self.assertEqual(result["evicted"], 1)
        self.assertFalse((self.cache / old).exists())
        self.assertTrue((self.cache / new).exists())

    def test_needed_objects_are_never_evicted(self):
        self.ensure()
        result = self.ensure(manifest(cap=750))
        self.assertEqual(result["evicted"], 0)
        self.assertEqual(len(list(self.cache.iterdir())), 2)

    def test_clean_leaves_nothing_behind(self):
        self.ensure()
        self.assertEqual(fetch.clean(self.cache), 2)
        self.assertFalse(self.cache.exists())
        self.assertEqual(fetch.clean(self.cache), 0)

    def test_clean_refuses_a_directory_it_did_not_write(self):
        self.cache.mkdir()
        (self.cache / "notes.txt").write_text("mine", encoding="utf-8")
        with self.assertRaises(fetch.Failure):
            fetch.clean(self.cache)
        self.assertTrue((self.cache / "notes.txt").exists())

    def test_a_cache_inside_the_repository_is_refused(self):
        with self.assertRaises(fetch.Failure):
            fetch.check_cache_location(fetch.REPO / "target" / "perf-cache", fetch.REPO)
        fetch.check_cache_location(self.cache, fetch.REPO)

    def test_a_url_outside_the_allowed_prefixes_is_refused(self):
        path = Path(self._tmp.name) / "m.json"
        for url in ("file:///etc/passwd", "https://example.com/x", "ftp://storage.googleapis.com/gcp-public-data-arco-era5/x"):
            path.write_text(
                f'{{"objects": {{"{"0" * 64}": {{"url": "{url}", "length": 1}}}}, "cache_cap_bytes": 10}}',
                encoding="utf-8",
            )
            with self.assertRaises(fetch.Failure, msg=url):
                fetch.load_manifest(path)

    def test_the_committed_manifest_loads_and_fits_its_cap(self):
        m = fetch.load_manifest(fetch.DEFAULT_MANIFEST)
        total = sum(entry["length"] for entry in m["objects"].values())
        self.assertLessEqual(total, m["cache_cap_bytes"])
        self.assertEqual(total, m["total_bytes"])


if __name__ == "__main__":
    unittest.main()
