#!/usr/bin/env python3
"""Unit tests for tools/check_unused_dependencies.py.

    python3 tools/test_check_unused_dependencies.py

The checker's whole value is that it says something cargo does not, so a
checker that quietly said nothing would be worse than no checker: it would
pass, look like a gate, and let the next unused line through. Each verdict is
pinned here against a synthetic package built in a temp directory.

Two of these matter more than the rest, and they are not the same mechanism.
`serde_json` must not count as a use of `serde` — that is the exact pair the
check was written for, and a substring match would have found `serde` in every
file of all three format crates and reported nothing; that is the identifier
tokenisation, not the stripper. The stripper's own risk runs the other way: it
is the only part that can *hide* a use and fail a correct commit, so
`Stripper` asserts on its output directly rather than trusting an exit code to
distinguish "found nothing to strip" from "stripped the file".
"""

from __future__ import annotations

import contextlib
import importlib.util
import io
import tempfile
import textwrap
import unittest
from pathlib import Path

_spec = importlib.util.spec_from_file_location(
    "check_unused_dependencies",
    Path(__file__).resolve().parent / "check_unused_dependencies.py",
)
assert _spec and _spec.loader
chk = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(chk)

MANIFEST = """\
[package]
name = "a"

[dependencies]
serde = "1"
"""


class Fixture:
    """A synthetic package on disk, mounted under the checker's ROOT."""

    def __init__(self, manifest: str = MANIFEST, **files: str) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.root = Path(self._tmp.name)
        self.write("Cargo.toml", manifest)
        for name, body in files.items():
            self.write(name.replace("__", "/"), body)

    def write(self, rel: str, body: str) -> None:
        path = self.root / rel
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(textwrap.dedent(body), encoding="utf-8")

    def run(self, skipped=None, min_packages: int = 1) -> int:
        return self.run_reporting(skipped, min_packages)[0]

    def run_reporting(self, skipped=None, min_packages: int = 1) -> tuple[int, str]:
        """The exit code and what was printed, for the tests that need both.

        An exit code alone cannot tell a guard that fired from the ordinary
        finding it exists to pre-empt: both are 1.
        """
        saved = chk.ROOT, chk.SKIPPED, chk.MIN_PACKAGES
        chk.ROOT, chk.SKIPPED = self.root, {} if skipped is None else skipped
        chk.MIN_PACKAGES = min_packages
        errors = io.StringIO()
        try:
            with contextlib.redirect_stderr(errors):
                return chk.main(), errors.getvalue()
        finally:
            chk.ROOT, chk.SKIPPED, chk.MIN_PACKAGES = saved

    def close(self) -> None:
        self._tmp.cleanup()


class Usage(unittest.TestCase):
    def test_a_used_dependency_passes(self):
        fx = Fixture(**{"src__lib.rs": "use serde::Serialize;\n"})
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(), 0)

    def test_an_unused_dependency_fails(self):
        fx = Fixture(**{"src__lib.rs": "pub fn f() {}\n"})
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(), 1)

    def test_serde_json_is_not_serde(self):
        # #538 in one assertion. All three format crates use `serde_json` in
        # their tests and none used `serde`; a substring search would have
        # called every one of them clean.
        fx = Fixture(**{"tests__t.rs": "use serde_json::Value;\n"})
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(), 1)

    def test_a_hyphenated_key_is_looked_up_as_an_identifier(self):
        manifest = '[package]\nname = "a"\n\n[dependencies]\nrust-aec = "1"\n'
        fx = Fixture(manifest, **{"src__lib.rs": "use rust_aec::decode;\n"})
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(), 0)

    def test_a_renamed_dependency_is_looked_up_by_its_key(self):
        # With `key = { package = "other" }` the code spells the key, so the
        # key is what has to appear.
        manifest = '[package]\nname = "a"\n\n[dependencies]\nalias = { package = "serde" }\n'
        fx = Fixture(manifest, **{"src__lib.rs": "use alias::Serialize;\n"})
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(), 0)


class WhereItLooks(unittest.TestCase):
    def test_a_dev_dependency_used_in_tests_passes(self):
        manifest = '[package]\nname = "a"\n\n[dev-dependencies]\ntempfile = "3"\n'
        fx = Fixture(manifest, **{"src__lib.rs": "", "tests__t.rs": "use tempfile::tempdir;\n"})
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(), 0)

    def test_a_build_dependency_used_in_build_rs_passes(self):
        manifest = '[package]\nname = "a"\n\n[build-dependencies]\nnapi-build = "2"\n'
        fx = Fixture(manifest, **{"src__lib.rs": "", "build.rs": "fn main() { napi_build::setup(); }\n"})
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(), 0)

    def test_a_target_specific_dependency_is_checked(self):
        manifest = (
            '[package]\nname = "a"\n\n'
            "[target.'cfg(unix)'.dependencies]\nlibc = \"0.2\"\n"
        )
        fx = Fixture(manifest, **{"src__lib.rs": "pub fn f() {}\n"})
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(), 1)

    def test_a_nested_package_does_not_lend_its_sources_to_the_parent(self):
        # `crates/fieldglass-grib1/fuzz` is its own package. A dependency of
        # the parent named only inside it is not a use by the parent.
        fx = Fixture(**{"src__lib.rs": ""})
        self.addCleanup(fx.close)
        fx.write("fuzz/Cargo.toml", '[package]\nname = "a-fuzz"\n')
        fx.write("fuzz/fuzz_targets/f.rs", "use serde::Serialize;\n")
        self.assertEqual(fx.run(), 1)

    def test_a_nested_package_is_checked_in_its_own_right(self):
        fx = Fixture(**{"src__lib.rs": "use serde::Serialize;\n"})
        self.addCleanup(fx.close)
        fx.write("fuzz/Cargo.toml", '[package]\nname = "a-fuzz"\n\n[dependencies]\nlibfuzzer-sys = "0.4"\n')
        fx.write("fuzz/fuzz_targets/f.rs", "fn main() {}\n")
        self.assertEqual(fx.run(), 1)


class NamingIsNotUsing(unittest.TestCase):
    """A dependency mentioned in prose or in a literal is still unused."""

    def test_a_line_comment_is_not_a_use(self):
        fx = Fixture(**{"src__lib.rs": "// serde is not used here.\n"})
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(), 1)

    def test_a_doc_comment_is_not_a_use(self):
        fx = Fixture(**{"src__lib.rs": "//! Parses with serde one day.\n"})
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(), 1)

    def test_a_block_comment_is_not_a_use(self):
        fx = Fixture(**{"src__lib.rs": "/* serde */\npub fn f() {}\n"})
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(), 1)

    def test_a_nested_block_comment_is_not_a_use(self):
        # Rust block comments nest; stopping at the first `*/` would leave the
        # tail of the outer comment looking like code.
        fx = Fixture(**{"src__lib.rs": "/* outer /* inner */ serde */\npub fn f() {}\n"})
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(), 1)

    def test_a_string_literal_is_not_a_use(self):
        fx = Fixture(**{"src__lib.rs": 'pub const D: &str = "https://docs.rs/serde";\n'})
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(), 1)

    def test_a_raw_string_literal_is_not_a_use(self):
        fx = Fixture(**{"src__lib.rs": 'pub const D: &str = r#"serde"#;\n'})
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(), 1)

    def test_a_byte_string_literal_is_not_a_use(self):
        fx = Fixture(**{"src__lib.rs": 'pub const D: &[u8] = b"serde";\n'})
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(), 1)

    def test_a_raw_byte_string_literal_is_not_a_use(self):
        fx = Fixture(**{"src__lib.rs": 'pub const D: &[u8] = br#"serde"#;\n'})
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(), 1)

    def test_a_c_string_literal_is_not_a_use(self):
        # `c"…"` (Rust 1.77). An unhandled prefix is worse than a missed
        # literal: the opening quote survives, the closing one is then read as
        # an opening quote, and everything up to the next quote in the file is
        # swallowed — which can hide a real use rather than invent one.
        fx = Fixture(**{"src__lib.rs": 'pub const D: &core::ffi::CStr = c"serde.";\n'})
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(), 1)

    def test_a_raw_c_string_literal_is_not_a_use(self):
        fx = Fixture(**{"src__lib.rs": 'pub const D: &core::ffi::CStr = cr#"serde."#;\n'})
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(), 1)


class StripperDoesNotEatCode(unittest.TestCase):
    """The other direction: stripping must not hide a real use."""

    def test_a_slash_slash_inside_a_string_does_not_eat_the_line(self):
        src = 'pub fn f() { let _ = "https://example.com"; serde::run(); }\n'
        fx = Fixture(**{"src__lib.rs": src})
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(), 0)

    def test_a_quote_inside_a_line_comment_does_not_open_a_string(self):
        # An apostrophe in prose would otherwise be read as a char literal and
        # swallow the code after it.
        src = "// don't read this as a literal\nuse serde::Serialize;\n"
        fx = Fixture(**{"src__lib.rs": src})
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(), 0)

    def test_a_lifetime_does_not_open_a_char_literal(self):
        src = "pub fn f<'a>(x: &'a str) -> &'a str { serde::id(x) }\n"
        fx = Fixture(**{"src__lib.rs": src})
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(), 0)

    def test_an_escaped_quote_in_a_char_literal_does_not_run_on(self):
        src = "pub fn f() { let _ = '\\''; serde::run(); }\n"
        fx = Fixture(**{"src__lib.rs": src})
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(), 0)

    def test_an_escaped_backslash_char_does_not_run_on(self):
        # `'\\'` is the escape the first cut got wrong: scanning from `i + 2`
        # landed on the *second* backslash, read it as an escape of its own,
        # and skipped the closing quote — so everything to the next `'` in the
        # file went, taking the only use of the dependency with it. There is no
        # later `'` here on purpose, so the swallow runs to end of file.
        src = "pub fn f() { let sep = '\\\\'; serde::run(); }\n"
        fx = Fixture(**{"src__lib.rs": src})
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(), 0)

    def test_an_escaped_backslash_byte_char_does_not_run_on(self):
        src = "pub fn f() { let sep = b'\\\\'; serde::run(); }\n"
        fx = Fixture(**{"src__lib.rs": src})
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(), 0)

    def test_a_string_beside_a_for_loop_does_not_hide_the_use(self):
        src = 'pub fn f() { for _ in 0..1 { let _ = "x"; } serde::run(); }\n'
        fx = Fixture(**{"src__lib.rs": src})
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(), 0)

    def test_a_c_string_does_not_swallow_the_code_after_it(self):
        src = 'pub fn f() { let _ = c"a."; serde::run(); let _ = "b"; }\n'
        fx = Fixture(**{"src__lib.rs": src})
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(), 0)

    def test_a_raw_string_holding_a_quote_ends_at_its_own_hashes(self):
        src = 'pub fn f() { let _ = r#"say "no" here"#; serde::run(); }\n'
        fx = Fixture(**{"src__lib.rs": src})
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(), 0)

    def test_a_block_comment_opener_inside_a_string_is_not_a_comment(self):
        src = 'pub fn f() { let _ = "/*"; serde::run(); }\n'
        fx = Fixture(**{"src__lib.rs": src})
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(), 0)


class Stripper(unittest.TestCase):
    """`strip_comments_and_literals` on its own, asserted in both directions.

    Every test in `NamingIsNotUsing` expects exit 1, so a stripper that
    returned the empty string would pass all of them. These assert on what
    survives and what does not, which no single exit code can express.
    """

    def kept(self, src: str) -> set[str]:
        import re

        return set(re.findall(r"[A-Za-z_][A-Za-z0-9_]*", chk.strip_comments_and_literals(src)))

    def test_code_survives_and_prose_does_not(self):
        src = '// wants serde\nuse only::this; /* and serde */ let _ = "serde";\n'
        self.assertEqual(self.kept(src), {"use", "only", "this", "let", "_"})

    def test_a_nested_block_comment_closes_once(self):
        src = "/* a /* b */ c */ kept"
        self.assertEqual(self.kept(src), {"kept"})

    def test_an_unterminated_block_comment_takes_the_rest(self):
        # Not valid Rust, but a truncated file must not make the stripper loop
        # or throw; running to EOF is the safe answer.
        self.assertEqual(self.kept("kept /* on and on"), {"kept"})

    def test_an_unterminated_string_takes_the_rest(self):
        self.assertEqual(self.kept('kept "on and on'), {"kept"})

    def test_a_char_literal_at_the_very_end_of_file_terminates(self):
        self.assertEqual(self.kept("kept 'x'"), {"kept"})

    def test_raw_string_hashes_are_counted(self):
        self.assertEqual(self.kept('r##"a"# b"## kept'), {"kept"})

    def test_a_raw_identifier_is_not_a_raw_string(self):
        self.assertEqual(self.kept("r#fn kept"), {"r", "fn", "kept"})

    def test_a_label_is_not_a_char_literal(self):
        self.assertEqual(self.kept("'outer: loop { kept }"), {"outer", "loop", "kept"})

    def test_everything_removed_leaves_a_separator(self):
        # Blanking to "" rather than " " would splice `ser` onto `de`.
        self.assertEqual(self.kept('ser/* x */de'), {"ser", "de"})


class Escapes(unittest.TestCase):
    def test_skipped_silences_a_finding(self):
        fx = Fixture(**{"src__lib.rs": "pub fn f() {}\n"})
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(skipped={("a", "serde"): "reason"}), 0)

    def test_a_stale_skipped_entry_fails(self):
        fx = Fixture(**{"src__lib.rs": "use serde::Serialize;\n"})
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(skipped={("a", "gone"): "reason"}), 1)

    def test_a_skipped_entry_that_became_redundant_fails(self):
        # The dependency is declared *and* used now, so the entry excuses
        # nothing — and left in place it would silence that pair for good, the
        # next time someone deletes the last use of it.
        fx = Fixture(**{"src__lib.rs": "use serde::Serialize;\n"})
        self.addCleanup(fx.close)
        code, printed = fx.run_reporting(skipped={("a", "serde"): "reason"})
        self.assertEqual(code, 1)
        self.assertIn("it *is* named", printed)

    def test_a_skipped_entry_on_a_sourceless_package_is_not_also_called_stale(self):
        # Two guards could fire on one package and one of the messages would be
        # wrong: the entry is not stale, the package just has nothing to read.
        fx = Fixture()
        self.addCleanup(fx.close)
        code, printed = fx.run_reporting(skipped={("a", "serde"): "reason"})
        self.assertEqual(code, 1)
        self.assertIn("has no `.rs` file", printed)
        self.assertNotIn("not a declared dependency any more", printed)


class FailingOpen(unittest.TestCase):
    """The ways this check could pass by checking nothing."""

    def test_too_few_packages_fails(self):
        fx = Fixture(**{"src__lib.rs": "use serde::Serialize;\n"})
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(min_packages=2), 1)

    def test_a_package_with_dependencies_and_no_sources_fails(self):
        # The exit code alone does not test this guard: with no sources every
        # dependency also reports through the ordinary path, so deleting the
        # guard leaves the code at 1. What is being asserted is that the reason
        # given is the walk, not the manifest.
        fx = Fixture()
        self.addCleanup(fx.close)
        code, printed = fx.run_reporting()
        self.assertEqual(code, 1)
        self.assertIn("has no `.rs` file", printed)

    def test_a_package_with_no_dependencies_and_no_sources_passes(self):
        # `fieldglass-verify`'s shape if its one dependency went: nothing to
        # check is not the same as failing to check.
        fx = Fixture('[package]\nname = "a"\n')
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(), 0)


class TheRepoItselfPasses(unittest.TestCase):
    """The checker's verdict on the real tree, so the hook cannot rot."""

    def test_no_offenders(self):
        self.assertEqual(chk.main(), 0)

    def test_the_walk_finds_every_package(self):
        # The floor is a smoke alarm, not the count. This is the count: nine
        # crates, three `fuzz/` packages, one test harness. A new package that
        # is not being checked shows up here.
        found = {p.relative_to(chk.ROOT).as_posix() for p in chk.package_dirs(chk.ROOT)}
        self.assertEqual(
            found,
            {
                "crates/fieldglass",
                "crates/fieldglass-core",
                "crates/fieldglass-fetchplan",
                "crates/fieldglass-grib1",
                "crates/fieldglass-grib1/fuzz",
                "crates/fieldglass-grib2",
                "crates/fieldglass-grib2/fuzz",
                "crates/fieldglass-napi",
                "crates/fieldglass-netcdf",
                "crates/fieldglass-netcdf/fuzz",
                "crates/fieldglass-verify",
                "crates/fieldglass-wasm",
                "tests/crate-independence",
            },
            "the set of packages this check walks has changed — add the new one "
            "here (or remove the deleted one) so the walk stays pinned",
        )


if __name__ == "__main__":
    unittest.main()
