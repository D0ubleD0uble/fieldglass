#!/usr/bin/env python3
"""Unit tests for tools/check_js_api.py.

    python3 tools/test_check_js_api.py

What is tested is *when it fails*, over synthetic declaration files: a checker
that never reported anything would pass the real builds just as well. Each
contract rule gets a file that breaks it alone, beside a file that keeps them
all, so a rule that stopped firing is a failing test here.
"""

from __future__ import annotations

import importlib.util
import unittest
from pathlib import Path

spec = importlib.util.spec_from_file_location(
    "check_js_api", Path(__file__).resolve().parent / "check_js_api.py"
)
chk = importlib.util.module_from_spec(spec)
spec.loader.exec_module(chk)


def napi_file(*, extra: str = "", drop: str | None = None, swap: tuple[str, str] | None = None,
              functions: str = "") -> str:
    """A napi `index.d.ts` that keeps the contract, unless told otherwise."""
    lines = []
    for name, params in chk.SHARED.items():
        if name == drop:
            continue
        if swap and name == swap[0]:
            params = swap[1]
        lines.append(f"  {name}({params}): unknown")
    for name in chk.NAPI_ONLY:
        lines.append(f"  {name}(): unknown")
    fns = [f"export declare function {n}({p}): unknown" for n, p in chk.SHARED_FUNCTIONS.items()]
    fns += [f"export declare function {n}(): unknown" for n in chk.NAPI_ONLY_FUNCTIONS]
    fns.append("export declare function openStore(path: string): Handle")
    body = "\n".join(lines) + "\n" + extra
    return (
        "/** A handle. */\nexport declare class Handle {\n" + body + "\n}\n" + "\n".join(fns) + "\n" + functions
    )


class Rules(unittest.TestCase):
    def test_a_file_that_keeps_the_contract_passes(self):
        self.assertEqual(chk.check("napi", napi_file()), [])

    def test_a_missing_shared_method_fails(self):
        problems = chk.check("napi", napi_file(drop="decodeSlice"))
        self.assertTrue(any("decodeSlice is shared and missing" in p for p in problems), problems)

    def test_a_shared_method_with_other_parameters_fails(self):
        problems = chk.check(
            "napi", napi_file(swap=("decodeSlice", "variable: number, yDim: number, xDim: number"))
        )
        self.assertTrue(any("decodeSlice(" in p and "differs" in p for p in problems), problems)

    def test_an_optional_parameter_spelt_with_undefined_is_the_same_parameter(self):
        problems = chk.check(
            "napi", napi_file(swap=("decode", "index: number, options?: DecodeOptions | null | undefined"))
        )
        self.assertEqual(problems, [])

    def test_the_other_hosts_operation_under_its_name_fails(self):
        problems = chk.check("napi", napi_file(extra="  probe(field: unknown, lat: number, lon: number): unknown"))
        self.assertTrue(any("Handle.probe is the other host's" in p for p in problems), problems)

    def test_an_unclassified_method_fails(self):
        problems = chk.check("napi", napi_file(extra="  somethingNew(): unknown"))
        self.assertTrue(any("somethingNew is in neither" in p for p in problems), problems)

    def test_a_snake_case_parameter_fails(self):
        problems = chk.check("napi", napi_file(extra="  metadata(y_dim: number): unknown"))
        self.assertTrue(any("`y_dim` is not camelCase" in p for p in problems), problems)

    def test_a_rust_doc_link_fails(self):
        for doc in ["/** see [`Grib1Handle`] */", "/** see [`decode`](Self::decode_grid) */"]:
            problems = chk.check("napi", doc + "\n" + napi_file())
            self.assertTrue(any("Rust doc link" in p for p in problems), (doc, problems))

    def test_a_web_link_is_not_a_doc_link(self):
        self.assertEqual(chk.check("napi", "/** [spec](https://example.org) */\n" + napi_file()), [])

    def test_the_reserved_async_names_fail(self):
        for extra, functions in [
            ("  decodeAsync(index: number): unknown", ""),
            ("", "export declare function load(source: string): Promise<Handle>"),
        ]:
            problems = chk.check("napi", napi_file(extra=extra, functions=functions))
            self.assertTrue(any("reserved for the async layer" in p for p in problems), problems)

    def test_no_handle_class_fails(self):
        self.assertEqual(chk.check("napi", "export declare function open(): void"), ["no `Handle` class is declared"])

    def test_the_lists_do_not_overlap(self):
        shared = set(chk.SHARED)
        self.assertFalse(shared & set(chk.NAPI_ONLY), "a shared method listed as napi's own")
        self.assertFalse(shared & set(chk.WASM_ONLY), "a shared method listed as wasm's own")
        self.assertFalse(set(chk.NAPI_ONLY) & set(chk.WASM_ONLY), "one name, two operations")
        for table in (chk.NAPI_ONLY, chk.WASM_ONLY, chk.NAPI_ONLY_FUNCTIONS, chk.WASM_ONLY_FUNCTIONS):
            for name, why in table.items():
                self.assertTrue(why, f"{name} is listed with no reason")


if __name__ == "__main__":
    unittest.main()
