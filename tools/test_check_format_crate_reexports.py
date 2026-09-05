#!/usr/bin/env python3
"""Unit tests for tools/check_format_crate_reexports.py.

The checker exists because adding a `pub fn` that returns a `fieldglass-core`
type compiles fine and says nothing. A checker that is equally silent about it
would be worse than none — it would read as a passing gate — so every failure
it is supposed to raise is pinned here against a synthetic crate built in a temp
directory, and the pure text functions are pinned against the Rust spellings
rustfmt actually produces. Run:

    python3 tools/test_check_format_crate_reexports.py
"""

from __future__ import annotations

import importlib.util
import tempfile
import unittest
from pathlib import Path

_spec = importlib.util.spec_from_file_location(
    "check_format_crate_reexports",
    Path(__file__).resolve().parent / "check_format_crate_reexports.py",
)
assert _spec and _spec.loader
chk = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(chk)

CRATE = "fieldglass-fake"

# A crate that names core's error type in a public signature and re-exports it:
# the shape all three real format crates have.
CLEAN_LIB = """\
pub mod reader;

pub use fieldglass_core::FieldglassError;
"""

CLEAN_READER = """\
use fieldglass_core::FieldglassError;

/// A reader over some bytes.
pub struct Reader {
    /// The bytes.
    pub bytes: Vec<u8>,
}

impl Reader {
    /// Parse.
    pub fn from_bytes(data: Vec<u8>) -> Result<Self, FieldglassError> {
        Ok(Self { bytes: data })
    }
}
"""


class Fixture:
    """A synthetic one-crate workspace, mounted under the checker's roots."""

    def __init__(self, files: dict[str, str]):
        self._tmp = tempfile.TemporaryDirectory()
        # Resolved: on macOS the temp root is a symlink, and a message built
        # with `relative_to` against the unresolved path would raise.
        self.root = Path(self._tmp.name).resolve()
        for relative, body in files.items():
            path = self.root / "crates" / CRATE / "src" / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(body, encoding="utf-8")

    def run(self, allowed=None) -> int:
        saved = (chk.REPO_ROOT, chk.CRATES_DIR, chk.FORMAT_CRATES, chk.ALLOWED_UNEXPORTED)
        chk.REPO_ROOT = self.root
        chk.CRATES_DIR = self.root / "crates"
        chk.FORMAT_CRATES = (CRATE,)
        chk.ALLOWED_UNEXPORTED = {} if allowed is None else {CRATE: allowed}
        try:
            return chk.main()
        finally:
            (chk.REPO_ROOT, chk.CRATES_DIR, chk.FORMAT_CRATES, chk.ALLOWED_UNEXPORTED) = saved

    def close(self) -> None:
        self._tmp.cleanup()

    @classmethod
    def clean(cls, **overrides: str) -> "Fixture":
        files = {"lib.rs": CLEAN_LIB, "reader.rs": CLEAN_READER}
        files.update(overrides)
        return cls(files)


class ARequiredNameIsChecked(unittest.TestCase):
    def test_clean_crate_passes(self):
        fx = Fixture.clean()
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(), 0)

    def test_missing_re_export_fails(self):
        fx = Fixture.clean(**{"lib.rs": "pub mod reader;\n"})
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(), 1)

    def test_a_new_signature_needs_its_own_re_export(self):
        # The case `tests/crate-independence` cannot see: the crate compiles,
        # and a consumer is the one who discovers the name is unwritable.
        fx = Fixture.clean(
            **{
                "reader.rs": CLEAN_READER.replace(
                    "use fieldglass_core::FieldglassError;",
                    "use fieldglass_core::{FieldglassError, GridGeometry};",
                )
                + "\nimpl Reader {\n    pub fn geometry(&self) -> GridGeometry {\n"
                "        unimplemented!()\n    }\n}\n"
            }
        )
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(), 1)

    def test_a_trait_impl_naming_a_core_type_counts(self):
        # `GridGeometry` reaches both GRIB crates' API through a `From` impl
        # and no `pub fn` at all, so impl headers have to be in the scan.
        fx = Fixture.clean(
            **{
                "reader.rs": CLEAN_READER.replace(
                    "use fieldglass_core::FieldglassError;",
                    "use fieldglass_core::{FieldglassError, GridGeometry};",
                )
                + "\nimpl From<&Reader> for GridGeometry {\n"
                "    fn from(_: &Reader) -> Self {\n        unimplemented!()\n    }\n}\n"
            }
        )
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(), 1)

    def test_an_associated_type_in_a_trait_impl_counts(self):
        # `type Error = FieldglassError;` is how a core name most often reaches
        # a consumer through a conversion, and it is not a `pub fn` anywhere.
        fx = Fixture.clean(
            **{
                "lib.rs": "pub mod reader;\n",
                "reader.rs": "use fieldglass_core::FieldglassError;\n\npub struct Reader;\n\n"
                "impl TryFrom<&[u8]> for Reader {\n    type Error = FieldglassError;\n\n"
                "    fn try_from(_: &[u8]) -> Result<Self, Self::Error> {\n"
                "        unimplemented!()\n    }\n}\n",
            }
        )
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(), 1)

    def test_a_trait_impl_method_return_type_counts(self):
        fx = Fixture.clean(
            **{
                "lib.rs": "pub mod reader;\n",
                "reader.rs": "use fieldglass_core::Metadata;\n\npub struct Reader;\n\n"
                "impl Describe for Reader {\n    fn describe(&self) -> Metadata {\n"
                "        unimplemented!()\n    }\n}\n",
            }
        )
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(), 1)

    def test_a_private_helper_in_an_inherent_impl_does_not(self):
        # The counterpart: items in an inherent `impl` are only public when
        # they say `pub`, so a private helper must not be reported.
        fx = Fixture.clean(
            **{
                "lib.rs": "pub mod reader;\n",
                "reader.rs": "use fieldglass_core::GridGeometry;\n\npub struct Reader;\n\n"
                "impl Reader {\n    fn helper(&self) -> GridGeometry {\n"
                "        unimplemented!()\n    }\n}\n",
            }
        )
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(), 0)

    def test_a_method_on_a_type_re_exported_out_of_a_private_module_counts(self):
        # Reachability turns on the *type's* name, not the method's: grib2
        # publishes `LocalTableCentre` out of a private `tables_local`.
        fx = Fixture(
            {
                "lib.rs": "mod tables;\n\npub use tables::Table;\n",
                "tables.rs": "use fieldglass_core::GridGeometry;\n\npub struct Table;\n\n"
                "impl Table {\n    pub fn geometry(&self) -> GridGeometry {\n"
                "        unimplemented!()\n    }\n}\n",
            }
        )
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(), 1)

    def test_a_renamed_re_export_still_names_the_declared_item(self):
        fx = Fixture(
            {
                "lib.rs": "mod tables;\n\npub use tables::Table as Centre;\n",
                "tables.rs": "use fieldglass_core::GridGeometry;\n\npub struct Table;\n\n"
                "impl Table {\n    pub fn geometry(&self) -> GridGeometry {\n"
                "        unimplemented!()\n    }\n}\n",
            }
        )
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(), 1)

    def test_a_glob_re_export_publishes_the_whole_module(self):
        fx = Fixture(
            {
                "lib.rs": "mod tables;\n\npub use tables::*;\n",
                "tables.rs": "use fieldglass_core::GridGeometry;\n\n"
                "pub fn geometry() -> GridGeometry {\n    unimplemented!()\n}\n",
            }
        )
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(), 1)

    def test_a_public_struct_field_counts(self):
        fx = Fixture.clean(
            **{
                "reader.rs": "use fieldglass_core::ByteRange;\n\n"
                "pub struct Plan {\n    pub ranges: Vec<ByteRange>,\n}\n"
            }
        )
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(), 1)

    def test_a_private_struct_field_does_not(self):
        fx = Fixture.clean(
            **{
                "reader.rs": "use fieldglass_core::ByteRange;\n\n"
                "pub struct Plan {\n    ranges: Vec<ByteRange>,\n}\n"
            }
        )
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(), 0)

    def test_a_public_enum_payload_counts(self):
        fx = Fixture.clean(
            **{
                "reader.rs": "use fieldglass_core::ByteRange;\n\n"
                "pub enum Backing {\n    Ranged(ByteRange),\n}\n"
            }
        )
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(), 1)

    def test_a_public_trait_method_counts(self):
        fx = Fixture.clean(
            **{
                "reader.rs": "use fieldglass_core::ByteSource;\n\n"
                "pub trait Backing {\n    fn source(&self) -> &dyn ByteSource;\n}\n"
            }
        )
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(), 1)

    def test_a_name_used_only_in_a_body_does_not_count(self):
        fx = Fixture.clean(
            **{
                "reader.rs": "use fieldglass_core::GaussianParams;\n\n"
                "pub fn count() -> u32 {\n    let p = GaussianParams::default();\n"
                "    p.ni\n}\n"
            }
        )
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(), 0)

    def test_a_path_written_out_instead_of_imported_counts(self):
        fx = Fixture.clean(
            **{
                "lib.rs": "pub mod reader;\n",
                "reader.rs": "pub fn geometry() -> fieldglass_core::GridGeometry {\n"
                "    unimplemented!()\n}\n",
            }
        )
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(), 1)

    def test_a_type_alias_names_the_type_after_its_equals_sign(self):
        fx = Fixture.clean(
            **{
                "lib.rs": "pub mod reader;\n",
                "reader.rs": "use fieldglass_core::ByteRange;\n\npub type Span = ByteRange;\n",
            }
        )
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(), 1)

    def test_a_pub_item_in_a_private_module_does_not_count(self):
        fx = Fixture(
            {
                "lib.rs": "mod tables;\n",
                "tables.rs": "use fieldglass_core::GridGeometry;\n\n"
                "pub fn geometry() -> GridGeometry {\n    unimplemented!()\n}\n",
            }
        )
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(), 0)

    def test_a_private_module_item_re_exported_by_name_does(self):
        fx = Fixture(
            {
                "lib.rs": "mod tables;\n\npub use tables::geometry;\n",
                "tables.rs": "use fieldglass_core::GridGeometry;\n\n"
                "pub fn geometry() -> GridGeometry {\n    unimplemented!()\n}\n",
            }
        )
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(), 1)

    def test_a_re_export_from_a_public_submodule_satisfies_the_rule(self):
        # `tables_cct` re-exports the shared sub-centre lookup from a public
        # module rather than from lib.rs; a consumer can still write the name.
        fx = Fixture(
            {
                "lib.rs": "pub mod inner;\npub mod reader;\n",
                "inner.rs": "pub use fieldglass_core::FieldglassError;\n",
                "reader.rs": CLEAN_READER,
            }
        )
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(), 0)


class TheAllowlist(unittest.TestCase):
    MISSING = {"lib.rs": "pub mod reader;\n"}

    def test_an_allowlisted_name_passes(self):
        fx = Fixture.clean(**self.MISSING)
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(allowed={"FieldglassError": "why"}), 0)

    def test_an_allowlisted_name_no_signature_names_fails(self):
        fx = Fixture.clean()
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(allowed={"GridGeometry": "stale"}), 1)

    def test_an_allowlisted_name_that_is_re_exported_fails(self):
        # Both halves are satisfied, so the entry has nothing left to excuse.
        fx = Fixture.clean()
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(allowed={"FieldglassError": "redundant"}), 1)

    def test_the_allowlist_is_per_name_not_per_shape(self):
        # The distinction the rule turns on: `GridGeometry`'s payload structs
        # are excusable, the parameter structs grib2 returns are not, and both
        # are `…Params`. Allowlisting one must not excuse the other.
        fx = Fixture.clean(
            **{
                "lib.rs": "pub mod reader;\n",
                "reader.rs": "use fieldglass_core::{LatLonParams, TransverseMercatorParams};\n\n"
                "pub fn a() -> LatLonParams {\n    unimplemented!()\n}\n\n"
                "pub fn b() -> TransverseMercatorParams {\n    unimplemented!()\n}\n",
            }
        )
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(allowed={"LatLonParams": "reached by destructuring"}), 1)


class GatesThatWouldOtherwiseFailOpen(unittest.TestCase):
    def test_a_glob_import_is_rejected(self):
        fx = Fixture.clean(**{"reader.rs": "use fieldglass_core::*;\n\npub struct Reader;\n"})
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(), 1)

    def test_a_renamed_crate_import_is_rejected(self):
        # Every path through the new name would be invisible to the scan.
        fx = Fixture.clean(**{"reader.rs": "use fieldglass_core as core;\n\npub struct Reader;\n"})
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(), 1)

    def test_the_same_rename_spelled_as_self_is_rejected_too(self):
        fx = Fixture.clean(
            **{
                "reader.rs": "use fieldglass_core::{self as fc, FieldglassError};\n\n"
                "pub fn geometry() -> fc::GridGeometry {\n    unimplemented!()\n}\n"
                "pub struct Reader { pub e: Option<FieldglassError> }\n"
            }
        )
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(), 1)

    def test_a_where_clause_does_not_hide_a_public_field(self):
        fx = Fixture.clean(
            **{
                "lib.rs": "pub mod reader;\n",
                "reader.rs": "use fieldglass_core::ByteRange;\n\n"
                "pub struct Plan<T>\nwhere\n    T: Clone,\n{\n"
                "    pub ranges: Vec<ByteRange>,\n    pub other: T,\n}\n",
            }
        )
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(), 1)

    def test_a_reported_line_number_is_the_line_in_the_file(self):
        # Line numbers indexed into a list with test items *removed* would be
        # off by the size of every test module above the finding.
        text = (
            "#[cfg(test)]\nmod tests {\n    fn a() {}\n    fn b() {}\n}\n\n"
            "use fieldglass_core::GridGeometry;\n\n"
            "pub fn geometry() -> GridGeometry {\n    unimplemented!()\n}\n"
        )
        signatures = chk.public_signatures(text, module_is_public=True, reexported=set())
        self.assertEqual([line for line, _ in signatures], [9])
        self.assertEqual(chk.core_names_from_text(text)[0], {"GridGeometry": "GridGeometry"})

    def test_a_source_file_the_module_walk_never_reaches_fails(self):
        fx = Fixture.clean(**{"orphan.rs": "use fieldglass_core::GridGeometry;\n"})
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(), 1)

    def test_a_unit_struct_does_not_swallow_the_rest_of_the_file(self):
        # `pub struct X;` opens no block. Scanning for a closing brace anyway
        # would skip to end of file and hide every signature below it.
        fx = Fixture.clean(
            **{
                "lib.rs": "pub mod reader;\n",
                "reader.rs": "use fieldglass_core::GridGeometry;\n\n"
                "pub struct Marker;\n\n"
                "pub fn geometry() -> GridGeometry {\n    unimplemented!()\n}\n",
            }
        )
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(), 1)

    def test_a_test_module_is_not_public_api(self):
        fx = Fixture.clean(
            **{
                "lib.rs": "pub mod reader;\n",
                "reader.rs": "#[cfg(test)]\nmod tests {\n"
                "    use fieldglass_core::GridGeometry;\n\n"
                "    pub fn mock() -> GridGeometry {\n        unimplemented!()\n    }\n}\n",
            }
        )
        self.addCleanup(fx.close)
        self.assertEqual(fx.run(), 0)

    def test_a_missing_crate_fails(self):
        fx = Fixture({"lib.rs": CLEAN_LIB})
        self.addCleanup(fx.close)
        (fx.root / "crates" / CRATE / "src" / "lib.rs").unlink()
        (fx.root / "crates" / CRATE / "src").rmdir()
        self.assertEqual(fx.run(), 1)


class UseTrees(unittest.TestCase):
    def test_a_plain_name(self):
        self.assertEqual(chk.flatten_use_tree("FieldglassError"), [("FieldglassError", "FieldglassError")])

    def test_a_module_path(self):
        self.assertEqual(chk.flatten_use_tree("bits::ibm_float_to_f64"), [("ibm_float_to_f64", "ibm_float_to_f64")])

    def test_a_nested_tree(self):
        self.assertEqual(
            sorted(chk.flatten_use_tree("{FieldglassError, bits::{BitReader, bits_to_bytes}}")),
            [
                ("BitReader", "BitReader"),
                ("FieldglassError", "FieldglassError"),
                ("bits_to_bytes", "bits_to_bytes"),
            ],
        )

    def test_a_rename_binds_the_alias(self):
        self.assertEqual(
            chk.flatten_use_tree("{FieldglassError as CoreError}"),
            [("CoreError", "FieldglassError")],
        )

    def test_self_binds_a_module_not_a_name(self):
        self.assertEqual(chk.flatten_use_tree("projection::{self, GridGeometry}"), [("GridGeometry", "GridGeometry")])

    def test_a_statement_wrapped_across_lines_is_joined(self):
        lines = [
            "use fieldglass_core::{",
            "    FieldglassError, GeostationaryParams, GridGeometry,",
            "};",
        ]
        statements = chk.core_use_statements(lines)
        self.assertEqual(len(statements), 1)
        self.assertEqual(
            sorted(name for _, name in chk.flatten_use_tree(statements[0][2])),
            ["FieldglassError", "GeostationaryParams", "GridGeometry"],
        )

    def test_pub_use_is_reported_as_a_re_export(self):
        _, re_exported, globs = chk.core_names_from_text("pub use fieldglass_core::FieldglassError;\n")
        self.assertEqual(re_exported, {"FieldglassError"})
        self.assertEqual(globs, [])

    def test_a_glob_is_reported_with_its_line(self):
        _, _, globs = chk.core_names_from_text("// a note\nuse fieldglass_core::*;\n")
        self.assertEqual(globs, [2])


class LogicalHeaders(unittest.TestCase):
    def test_a_wrapped_signature_is_joined(self):
        lines = [
            "pub fn variable_plan(",
            "    header: &ClassicHeader,",
            "    var_index: usize,",
            ") -> Result<Vec<ByteRange>, FieldglassError> {",
        ]
        header, end, terminator = chk._logical_header(lines, 0)
        self.assertEqual((end, terminator), (3, "{"))
        self.assertIn("ByteRange", header)

    def test_generic_commas_do_not_end_a_header(self):
        header, _, terminator = chk._logical_header(["pub fn f<A, B>(a: A) -> Result<B, FieldglassError> {"], 0)
        self.assertEqual(terminator, "{")
        self.assertIn("FieldglassError", header)

    def test_a_field_ends_at_its_comma(self):
        lines = ["    pub ranges: Vec<ByteRange>,", "    pub other: u32,"]
        header, end, terminator = chk._logical_header(lines, 0)
        self.assertEqual((end, terminator), (0, ","))
        self.assertNotIn("other", header)

    def test_a_declaration_ends_at_its_semicolon(self):
        _, end, terminator = chk._logical_header(["pub struct Marker;", "pub fn after() {}"], 0)
        self.assertEqual((end, terminator), (0, ";"))

    def test_a_doc_link_is_not_source(self):
        # `strip_noise` blanks comments, so `/// see [`GridGeometry`]` above an
        # item does not put the name into the signature below it.
        self.assertEqual(chk.strip_noise("    // [`GridGeometry`]").strip(), "")


class TheRepoItselfPasses(unittest.TestCase):
    """The checker's verdict on the real workspace, so the hook cannot rot."""

    def test_no_offenders(self):
        self.assertEqual(chk.main(), 0)

    def test_it_finds_every_name_the_real_crates_re_export_for_a_signature(self):
        """Not just "green": the names it is supposed to require, it requires.

        A checker that scanned nothing would also return 0, so the positive
        finding is asserted too. `expand_reduced_to_regular` and
        `lookup_sub_centre` are deliberately absent — they are functions, no
        signature names them, and they are re-exported for reasons this rule
        cannot express.
        """
        expected = {
            "fieldglass-grib1": {
                "CornerPair",
                "FieldglassError",
                "GridGeometry",
                "StoredRuns",
            },
            "fieldglass-grib2": {
                "CornerPair",
                "FieldglassError",
                "GeostationaryParams",
                "GridGeometry",
                "LambertAzimuthalParams",
                "StoredRuns",
                "TransverseMercatorParams",
            },
            "fieldglass-netcdf": {"ByteRange", "ByteSource", "FieldglassError"},
        }
        for crate, names in expected.items():
            with self.subTest(crate=crate):
                self.assertEqual(required_names(crate), names)


def required_names(crate: str) -> set[str]:
    """Core names the real crate's public signatures name, by the same route
    :func:`check_format_crate_reexports.check_crate` takes."""
    src_dir = chk.CRATES_DIR / crate / "src"
    files, reexported_items = chk.module_files(src_dir)
    found: set[str] = set()
    for path, is_public in files:
        text = path.read_text(encoding="utf-8")
        in_scope, _, _ = chk.core_names_from_text(text)
        if not in_scope:
            continue
        for _, signature in chk.public_signatures(
            text, module_is_public=is_public, reexported=reexported_items
        ):
            found.update(
                in_scope[ident] for ident in chk.IDENT_RE.findall(signature) if ident in in_scope
            )
    return found


if __name__ == "__main__":
    unittest.main()
