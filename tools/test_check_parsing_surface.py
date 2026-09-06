#!/usr/bin/env python3
"""Unit tests for tools/check_parsing_surface.py.

    python3 tools/test_check_parsing_surface.py

The checker compares three sets, and every one of them can come back empty from a
parse that has stopped working — at which point equality holds and the gate is a
green no-op. So each way that can happen has a test named for it, alongside the
two directions of drift the gate exists to catch: a module used and not listed
(`lead_time`, #545), and a module listed and not used (`detect`, #558).

The trees are synthetic. A real `fieldglass-core` is not needed to test set
arithmetic, and building one here would make the tests re-derive the thing they
are checking; the last class runs the checker against the real crate instead.
"""

from __future__ import annotations

import importlib.util
import shutil
import tempfile
import unittest
from pathlib import Path

_spec = importlib.util.spec_from_file_location(
    "check_parsing_surface",
    Path(__file__).resolve().parent / "check_parsing_surface.py",
)
assert _spec and _spec.loader
chk = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(chk)

# Enough `pub mod` and `pub use` lines to clear the checker's sanity floors, so a
# test that means to exercise one rule is not stopped by another.
MODULES = [
    "bits",
    "bytes",
    "cct_tables",
    "detect",
    "error",
    "global_grid",
    "healpix",
    "lead_time",
    "matrix",
    "projection",
    "scan",
    "sht",
    "spatial_index",
    "units",
]
SURFACE = ["bits", "bytes", "cct_tables", "error", "global_grid", "healpix", "lead_time"]


def lib_rs(
    surface: list[str] | None,
    *,
    gated: dict[str, str] | None = None,
    extra: str = "",
) -> str:
    """A synthetic core `lib.rs`. `surface=None` omits the region markers."""
    gated = gated or {"warp": "render", "contour": "analysis"}
    head = ""
    if surface is not None:
        listed = ", ".join(f"[`{m}`]" for m in surface)
        head = f"//! <!-- parsing-surface -->\n//! {listed}\n//! <!-- /parsing-surface -->\n"
    mods = "".join(f"pub mod {m};\n" for m in MODULES)
    mods += "".join(f'#[cfg(feature = "{f}")]\npub mod {m};\n' for m, f in gated.items())
    mods += extra
    # Two-per-module re-exports, enough to clear MIN_REEXPORTS, and the shapes
    # that matter: a brace group, a single name, and a rename.
    uses = "".join(f"pub use {m}::{{Thing{i}, thing{i} as t{i}}};\n" for i, m in enumerate(MODULES))
    uses += "pub use error::FieldglassError;\n"
    uses += "pub use global_grid::{GlobalGrid, SynthesisedField};\n"
    return head + mods + uses


def readme(surface: list[str] | None) -> str:
    if surface is None:
        return "# core\n\nNo region here.\n"
    listed = ", ".join(f"`{m}`" for m in surface)
    return f"# core\n\n<!-- parsing-surface -->\n{listed}\n<!-- /parsing-surface -->\n"


def tree(root: Path, surface: list[str], uses: dict[str, str], **kwargs) -> tuple[Path, Path]:
    """A synthetic core plus the three format crates, each naming `uses[crate]`."""
    readme_surface = kwargs.pop("readme_surface", surface)
    core = root / "crates" / "fieldglass-core"
    (core / "src").mkdir(parents=True)
    (core / "src" / "lib.rs").write_text(lib_rs(surface, **kwargs), encoding="utf-8")
    (core / "README.md").write_text(readme(readme_surface), encoding="utf-8")
    for name in chk.FORMAT_CRATES:
        src = root / "crates" / name / "src"
        src.mkdir(parents=True)
        (src / "lib.rs").write_text(uses[name], encoding="utf-8")
    return core, root / "crates"


def three(body: str) -> dict[str, str]:
    """The same source in all three format crates."""
    return dict.fromkeys(chk.FORMAT_CRATES, body)


USES_SURFACE = "\n".join(f"use fieldglass_core::{m}::X;" for m in SURFACE)


class Catches(unittest.TestCase):
    def test_a_module_used_but_not_listed(self):
        # `lead_time` (#545): it arrived in core, both GRIB libraries call it, and
        # neither copy of the list mentioned it. Every other gate stayed green.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            core, crates = tree(root, SURFACE, three(USES_SURFACE + "\nuse fieldglass_core::scan::Y;"))
            problems = chk.check(core, crates)
        self.assertEqual(len(problems), 2)  # once per copy
        self.assertTrue(all("does not list" in p and "scan" in p for p in problems))

    def test_a_module_listed_but_used_by_nothing(self):
        # `detect` (#558): on both lists, named by no library. A containment
        # check calls this clean, which is why the invariant is equality.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            core, crates = tree(root, SURFACE + ["detect"], three(USES_SURFACE))
            problems = chk.check(core, crates)
        self.assertEqual(len(problems), 2)
        self.assertTrue(all("which no format crate library names" in p for p in problems))

    def test_the_two_copies_are_checked_separately(self):
        # Fixing one copy and leaving the other is how this got wrong the first
        # time, so a disagreement between them has to be reported.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            core, crates = tree(
                root, SURFACE, three(USES_SURFACE), readme_surface=SURFACE + ["units"]
            )
            problems = chk.check(core, crates)
        self.assertEqual(len(problems), 1)
        self.assertIn("README.md", problems[0])
        self.assertIn("units", problems[0])

    def test_a_gated_module_cannot_be_part_of_the_surface(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            core, crates = tree(root, SURFACE, three(USES_SURFACE + "\nuse fieldglass_core::warp::W;"))
            problems = chk.check(core, crates)
        self.assertTrue(any("behind the `render` feature" in p for p in problems))

    def test_a_missing_region_is_a_failure_not_an_empty_list(self):
        # An empty list would match an empty used-set and pass vacuously.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            core, crates = tree(root, SURFACE, three(USES_SURFACE), readme_surface=None)
            problems = chk.check(core, crates)
        self.assertEqual(len(problems), 1)
        self.assertIn("no `<!-- parsing-surface -->` region", problems[0])

    def test_a_region_naming_almost_nothing_is_a_failure(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            core, crates = tree(root, ["bits"], three("use fieldglass_core::bits::X;"))
            problems = chk.check(core, crates)
        self.assertEqual(len(problems), 2)
        self.assertTrue(all("names only 1 core modules" in p for p in problems))

    def test_a_lib_rs_that_stops_parsing_is_a_failure_not_an_empty_surface(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            core, crates = tree(root, SURFACE, three(USES_SURFACE))
            (core / "src" / "lib.rs").write_text("// nothing here\n", encoding="utf-8")
            problems = chk.check(core, crates)
        self.assertTrue(any("`pub mod` found" in p for p in problems))
        self.assertTrue(any("re-exports found" in p for p in problems))

    def test_a_format_crate_that_names_nothing_is_a_failure(self):
        # Its modules would silently drop out of the measured surface, which
        # then reads as "the docs list too much".
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            uses = three(USES_SURFACE)
            uses["fieldglass-netcdf"] = "// no core here\n"
            core, crates = tree(root, SURFACE, uses)
            problems = chk.check(core, crates)
        self.assertTrue(any("names no `fieldglass_core::` path at all" in p for p in problems))

    def test_a_missing_format_crate_is_a_failure(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            core, crates = tree(root, SURFACE, three(USES_SURFACE))
            shutil.rmtree(crates / "fieldglass-netcdf")
            problems = chk.check(core, crates)
        self.assertTrue(any("no library to scan" in p for p in problems))

    def test_an_unattributable_use_is_reported_rather_than_dropped(self):
        # A name that is neither a module nor a re-export means the re-export map
        # is incomplete, which makes the measured surface silently too small.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            body = USES_SURFACE + "\nuse fieldglass_core::MysteryType;"
            core, crates = tree(root, SURFACE, three(body))
            problems = chk.check(core, crates)
        self.assertTrue(any("resolves to no core module" in p for p in problems))


    def test_a_gate_the_exact_cfg_pattern_would_miss_is_still_a_gate(self):
        # `#[cfg(any(feature = ...))]`, and a `#[doc(hidden)]` between the cfg
        # and the declaration. Reading either as ungated would make this gate
        # tell someone to document a feature-gated module as part of a surface
        # defined by nothing in it being gated.
        extra = (
            '#[cfg(any(feature = "render", feature = "analysis"))]\npub mod fancy;\n'
            '#[cfg(feature = "render")]\n#[doc(hidden)]\npub mod fancy2;\n'
        )
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            body = USES_SURFACE + "\nuse fieldglass_core::fancy::A;\nuse fieldglass_core::fancy2::B;"
            core, crates = tree(root, SURFACE, three(body), extra=extra)
            problems = chk.check(core, crates)
        self.assertTrue(any("`render, analysis` feature" in p for p in problems))
        self.assertTrue(any("`fieldglass_core::fancy2`" in p and "`render` feature" in p for p in problems))

    def test_a_cfg_test_module_inside_src_is_not_library_use(self):
        # It compiles with the crate's dev-dependency features on, which for
        # `fieldglass-grib1` means `analysis`. Counting it would admit a gated
        # module to the surface — the very thing excluding `tests/` avoids.
        body = USES_SURFACE + (
            "\n#[cfg(test)]\nmod tests {\n"
            "    use fieldglass_core::contour::C;\n"
            "    fn inner() { let _ = fieldglass_core::units::U; }\n"
            "}\n"
        )
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            core, crates = tree(root, SURFACE, three(body))
            self.assertEqual(chk.check(core, crates), [])

    def test_a_cfg_test_module_in_its_own_file_is_reported_not_ignored(self):
        body = USES_SURFACE + "\n#[cfg(test)]\nmod tests;\n"
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            core, crates = tree(root, SURFACE, three(body))
            problems = chk.check(core, crates)
        self.assertTrue(any("this scan does not follow" in p for p in problems))

    def test_a_glob_import_is_reported_rather_than_read_as_no_use(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            core, crates = tree(root, SURFACE, three(USES_SURFACE + "\nuse fieldglass_core::*;"))
            problems = chk.check(core, crates)
        self.assertTrue(any("hides which modules are used" in p for p in problems))

    def test_an_alias_of_the_crate_is_reported_rather_than_read_as_no_use(self):
        body = "use fieldglass_core as fgc;\nfn f() { let _ = fgc::units::U; }\n"
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            core, crates = tree(root, SURFACE, three(USES_SURFACE + "\n" + body))
            problems = chk.check(core, crates)
        self.assertTrue(any("renames the crate" in p for p in problems))

    def test_a_reexport_of_a_dependency_type_does_not_crash_the_checker(self):
        # `pub use half::f16;` at core's root: `half` is not a core module, so
        # looking it up in the module map raised `KeyError` and killed the hook
        # with a traceback instead of a diagnostic.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            core, crates = tree(
                root,
                SURFACE,
                three(USES_SURFACE + "\nuse fieldglass_core::f16;"),
                extra="pub use half::f16;\n",
            )
            problems = chk.check(core, crates)
        self.assertTrue(any("is not a module of core" in p for p in problems))

    def test_a_name_reexported_from_two_modules_is_reported(self):
        # Last-wins would attribute the use to whichever `pub use` came last and
        # could move a module in or out of the surface with no diagnostic.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            core, crates = tree(
                root, SURFACE, three(USES_SURFACE), extra="pub use units::Thing0;\n"
            )
            problems = chk.check(core, crates)
        self.assertTrue(any("re-exported from both" in p for p in problems))


class LetsThrough(unittest.TestCase):
    def test_a_tree_whose_three_sets_agree(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            core, crates = tree(root, SURFACE, three(USES_SURFACE))
            self.assertEqual(chk.check(core, crates), [])

    def test_a_crate_root_reexport_counts_as_its_module(self):
        # A format crate writes `fieldglass_core::GlobalGrid`, never the module
        # path; without resolution that use is invisible. Every module here is
        # reached through the crate root, and `Thing<i>` is the synthetic
        # re-export `lib_rs` gives each one.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            through_root = ", ".join(f"Thing{MODULES.index(m)}" for m in SURFACE)
            body = (
                f"use fieldglass_core::{{{through_root}}};\n"
                "use fieldglass_core::{FieldglassError, GlobalGrid, SynthesisedField};"
            )
            core, crates = tree(root, SURFACE, three(body))
            problems = chk.check(core, crates)
        self.assertEqual(problems, [])

    def test_naming_a_module_in_a_comment_or_a_string_is_not_using_it(self):
        body = (
            USES_SURFACE
            + '\n// see fieldglass_core::units for the conversion\n'
            + 'const D: &str = "fieldglass_core::spatial_index";\n'
            + 'const R: &str = r#"fieldglass_core::detect"#;\n'
            + "const C: char = '\\\\';\n"
            + 'const S: &str = c"fieldglass_core::warp";\n'
        )
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            core, crates = tree(root, SURFACE, three(body))
            self.assertEqual(chk.check(core, crates), [])

    def test_backticked_prose_in_the_region_that_is_not_a_module_is_ignored(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            surface = SURFACE + ["some_type", "default_features"]
            core, crates = tree(root, surface, three(USES_SURFACE))
            self.assertEqual(chk.check(core, crates), [])


    def test_a_multi_segment_reexport_resolves_to_its_module(self):
        # `pub use projection::grid::GridGeometry;` is a legal refactor of core.
        # A pattern that could not read it would report the *use site* as
        # unattributable and point the reader at this checker.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            core, crates = tree(
                root,
                SURFACE,
                three(USES_SURFACE + "\nuse fieldglass_core::Nested;"),
                extra="pub use bits::inner::deeper::Nested;\n",
            )
            self.assertEqual(chk.check(core, crates), [])

class TheRepoItselfPasses(unittest.TestCase):
    """The real check, against the real crate."""

    def test_the_documented_surface_is_the_one_in_use(self):
        self.assertEqual(chk.check(), [])

    def test_the_surface_is_the_eleven_modules_measured(self):
        # Pinned so that widening the surface comes past a reviewer here as well
        # as in the two doc regions.
        lib = (chk.CORE / "src" / "lib.rs").read_text(encoding="utf-8")
        modules = chk.core_modules(lib)
        listed = sorted(n for n in chk.documented_surface(lib) if n in modules)
        self.assertEqual(
            listed,
            [
                "bits",
                "bytes",
                "cct_tables",
                "error",
                "global_grid",
                "healpix",
                "lead_time",
                "matrix",
                "projection",
                "scan",
                "sht",
            ],
        )

    def test_the_three_ungated_modules_left_out_are_left_out_on_purpose(self):
        # `detect`, `spatial_index` and `units` are ungated and unused by any
        # format crate library. If one of them starts being used, the gate above
        # fires; this asserts the reason the doc gives for excluding them.
        lib = (chk.CORE / "src" / "lib.rs").read_text(encoding="utf-8")
        modules = chk.core_modules(lib)
        ungated = {m for m, feature in modules.items() if feature is None}
        listed = {n for n in chk.documented_surface(lib) if n in modules}
        self.assertEqual(sorted(ungated - listed), ["detect", "spatial_index", "units"])


if __name__ == "__main__":
    unittest.main()
