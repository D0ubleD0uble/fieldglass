//! The packages' `NOTICE` files carry every notice the third-party data in
//! them owes (#1563).
//!
//! The list is `fieldglass-core`'s `src/data_notices.rs`, kept beside the
//! colormap tables and held to the registry by that crate's own tests. This
//! half reads the `NOTICE` files, which sit outside every published crate, so
//! it lives here: the umbrella's `tests/` is excluded from its crates.io
//! package (`exclude` in `Cargo.toml`), and `fieldglass-core`'s published
//! tests stay buildable from the tarball.

#[path = "../../fieldglass-core/src/data_notices.rs"]
mod data_notices;

use std::path::{Path, PathBuf};

use data_notices::{DATA_NOTICES, EXTENSION, NPM, X11_COLOR_NAMES};

/// Every package that ships the tables, by the path `DATA_NOTICES` names it
/// with, and the `NOTICE` it carries.
const NOTICES: &[(&str, &str)] = &[
    (NPM, include_str!("../../fieldglass-wasm/npm/NOTICE")),
    (EXTENSION, include_str!("../../../extension/NOTICE")),
];

/// Collapse every run of whitespace to one space, so a credit wrapped to a
/// file's line width still matches the list's one-line form.
fn words(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn notice_of(package: &str) -> &'static str {
    NOTICES
        .iter()
        .find(|(path, _)| *path == package)
        .map(|(_, text)| *text)
        .unwrap_or_else(|| panic!("DATA_NOTICES names `{package}`, which has no NOTICE here"))
}

/// A credit line is compared up to line wrapping; a license text byte for
/// byte, since its terms ask for it as written.
#[test]
fn every_package_notice_carries_its_credits_and_license_texts() {
    for notice in DATA_NOTICES {
        for &package in notice.packages {
            let text = notice_of(package);
            if let Some(credit) = notice.credit {
                assert!(
                    words(text).contains(&words(credit)),
                    "{package} is missing the credit for {:?}:\n{credit}",
                    notice.tables,
                );
            }
            if let Some(terms) = notice.terms {
                assert!(
                    text.contains(terms),
                    "{package} does not carry the license text for {:?} verbatim:\n{terms}",
                    notice.tables,
                );
            }
        }
    }
}

/// `LICENSE-UNICODE` from unicode-ident 1.0.24, verbatim. unicode-ident is in
/// the wasm module's dependency graph (through `wasm-bindgen-shared`), which
/// is how the NOTICE builds its crate list; it is not linked into the module
/// itself. It is published under (MIT OR Apache-2.0) AND Unicode-3.0, and the
/// Unicode License v3 asks that this notice appear with copies of the
/// software.
const UNICODE_LICENSE_V3: &str = r#"UNICODE LICENSE V3

COPYRIGHT AND PERMISSION NOTICE

Copyright © 1991-2023 Unicode, Inc.

NOTICE TO USER: Carefully read the following legal agreement. BY
DOWNLOADING, INSTALLING, COPYING OR OTHERWISE USING DATA FILES, AND/OR
SOFTWARE, YOU UNEQUIVOCALLY ACCEPT, AND AGREE TO BE BOUND BY, ALL OF THE
TERMS AND CONDITIONS OF THIS AGREEMENT. IF YOU DO NOT AGREE, DO NOT
DOWNLOAD, INSTALL, COPY, DISTRIBUTE OR USE THE DATA FILES OR SOFTWARE.

Permission is hereby granted, free of charge, to any person obtaining a
copy of data files and any associated documentation (the "Data Files") or
software and any associated documentation (the "Software") to deal in the
Data Files or Software without restriction, including without limitation
the rights to use, copy, modify, merge, publish, distribute, and/or sell
copies of the Data Files or Software, and to permit persons to whom the
Data Files or Software are furnished to do so, provided that either (a)
this copyright and permission notice appear with all copies of the Data
Files or Software, or (b) this copyright and permission notice appear in
associated Documentation.

THE DATA FILES AND SOFTWARE ARE PROVIDED "AS IS", WITHOUT WARRANTY OF ANY
KIND, EXPRESS OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT OF
THIRD PARTY RIGHTS.

IN NO EVENT SHALL THE COPYRIGHT HOLDER OR HOLDERS INCLUDED IN THIS NOTICE
BE LIABLE FOR ANY CLAIM, OR ANY SPECIAL INDIRECT OR CONSEQUENTIAL DAMAGES,
OR ANY DAMAGES WHATSOEVER RESULTING FROM LOSS OF USE, DATA OR PROFITS,
WHETHER IN AN ACTION OF CONTRACT, NEGLIGENCE OR OTHER TORTIOUS ACTION,
ARISING OUT OF OR IN CONNECTION WITH THE USE OR PERFORMANCE OF THE DATA
FILES OR SOFTWARE.

Except as contained in this notice, the name of a copyright holder shall
not be used in advertising or otherwise to promote the sale, use or other
dealings in these Data Files or Software without prior written
authorization of the copyright holder.
"#;

/// The npm package carries unicode-ident's Unicode License v3 byte for
/// byte. It is a crate's license, not a data table's, so it is checked here
/// rather than through `DATA_NOTICES`.
#[test]
fn the_npm_notice_carries_the_unicode_license() {
    assert!(
        notice_of(NPM).contains(UNICODE_LICENSE_V3),
        "{NPM} does not carry unicode-ident's LICENSE-UNICODE verbatim"
    );
}

/// A generated table the list names still exists, so a renamed or removed
/// table can't leave a stale entry behind. Relative to `crates/`, which the
/// wasm32 test runner preopens as `..`.
#[test]
fn every_listed_table_file_exists() {
    for notice in DATA_NOTICES {
        for table in notice.tables.iter().filter(|t| t.ends_with(".rs")) {
            let path = Path::new("..").join(table);
            assert!(
                path.is_file(),
                "DATA_NOTICES lists {table}, which is not a file"
            );
        }
    }
}

/// Files named like a generated table that hold no third-party data: the
/// hand-written dispatch over the generated tables. Each must exist and must
/// not carry the generated marker, so this list can't hide a real table.
const NOT_DATA: &[&str] = &["fieldglass-grib2/src/tables_local.rs"];

/// Generated tables `DATA_NOTICES` names by what they hold rather than by
/// path. The colormaps are also held to the registry by `fieldglass-core`'s
/// tests, and below through the public `colormaps()`.
const LISTED_BY_ID: &[(&str, &str)] = &[
    ("fieldglass-core/src/color_names.rs", X11_COLOR_NAMES),
    ("fieldglass-core/src/colormap_tables.rs", "viridis"),
];

/// A generator's output says "do not edit" near its top, in a `//!` or `//`
/// line. Every `tools/gen_*.py` writes one.
fn has_generated_marker(text: &str) -> bool {
    text.lines()
        .take(20)
        .any(|line| line.to_ascii_lowercase().contains("do not edit"))
}

/// A file named like a generated data table.
fn named_like_a_table(name: &str) -> bool {
    (name.starts_with("tables_") && name.ends_with(".rs"))
        || matches!(
            name,
            "center_tables.rs" | "mars_short_names.rs" | "colormap_tables.rs" | "color_names.rs"
        )
}

/// Every `.rs` file under `dir`, recursively. Panics rather than skipping a
/// directory it cannot read, so a sandbox without the files fails the test
/// instead of passing it empty.
fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries =
        std::fs::read_dir(dir).unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()));
    for entry in entries {
        let path = entry.expect("directory entry").path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

fn listed(table: &str) -> bool {
    DATA_NOTICES
        .iter()
        .any(|notice| notice.tables.contains(&table))
}

/// The other direction from the tests above: every generated data file in a
/// crate's `src/` is in `DATA_NOTICES`, so a new table can't ship without its
/// terms recorded. Walks `crates/*/src` relative to this crate, which the
/// wasm32 test runner preopens as `..`.
#[test]
fn every_generated_table_is_listed() {
    let crates = Path::new("..");
    let mut members: Vec<PathBuf> = std::fs::read_dir(crates)
        .unwrap_or_else(|e| panic!("cannot read crates/: {e}"))
        .map(|entry| entry.expect("directory entry").path().join("src"))
        .filter(|src| src.is_dir())
        .collect();
    members.sort();

    let mut found = Vec::new();
    for src in &members {
        let mut files = Vec::new();
        rust_files(src, &mut files);
        for file in files {
            let name = file
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            let text = std::fs::read_to_string(&file)
                .unwrap_or_else(|e| panic!("cannot read {}: {e}", file.display()));
            if !(has_generated_marker(&text) || named_like_a_table(name)) {
                continue;
            }
            let rel = file
                .strip_prefix(crates)
                .expect("under crates/")
                .to_str()
                .expect("UTF-8 path")
                .replace('\\', "/");
            found.push(rel);
        }
    }

    // Fail closed: a walk that found nothing, or missed tables every build
    // has, proves nothing.
    for known in [
        "fieldglass-core/src/colormap_tables.rs",
        "fieldglass-grib2/src/tables_wmo.rs",
    ] {
        assert!(
            found.iter().any(|f| f == known),
            "the walk of crates/*/src did not find {known}; found {found:?}",
        );
    }

    for exempt in NOT_DATA {
        let text = std::fs::read_to_string(crates.join(exempt))
            .unwrap_or_else(|e| panic!("NOT_DATA lists {exempt}, which can't be read: {e}"));
        assert!(
            !has_generated_marker(&text),
            "{exempt} is generated, so it can't be exempt from DATA_NOTICES",
        );
    }

    for file in &found {
        if NOT_DATA.contains(&file.as_str()) {
            continue;
        }
        let id = LISTED_BY_ID
            .iter()
            .find(|(path, _)| path == file)
            .map_or(file.as_str(), |(_, id)| *id);
        assert!(
            listed(id),
            "{file} is a generated data table, and DATA_NOTICES has no entry for it \
             (add one to crates/fieldglass-core/src/data_notices.rs)",
        );
    }
}

/// Every colormap the renderer offers is in `DATA_NOTICES`, read through the
/// public registry rather than the generated file.
#[cfg(feature = "render")]
#[test]
fn every_colormap_is_listed() {
    for colormap in fieldglass::colormaps() {
        assert!(
            listed(colormap.name()),
            "colormap `{}` has no DATA_NOTICES entry",
            colormap.name(),
        );
    }
}

#[cfg(not(target_family = "wasm"))]
type Crates = std::collections::BTreeSet<(String, String)>;

/// One `cargo tree --prefix none` line as `(name, version)`, or `None` for a
/// workspace crate (printed with its path, `name vX (/path)`, and covered by a
/// sentence in each NOTICE). `(*)` marks a repeat, which is still a crate.
/// Any other parenthesized source, such as a URL, panics: it is third-party
/// code the NOTICE must list, so dropping it would hide it.
#[cfg(not(target_family = "wasm"))]
fn tree_line(line: &str) -> Option<(String, String)> {
    let mut parts = line.split_whitespace();
    let name = parts.next()?;
    let version = parts.next()?.strip_prefix('v')?;
    match parts.next() {
        None | Some("(*)") => Some((name.to_string(), version.to_string())),
        Some(source) => {
            let path = source.strip_prefix('(').unwrap_or_default();
            let is_path = path.starts_with('/')
                || path.starts_with('\\')
                || path.as_bytes().get(1..3) == Some(b":\\")
                || path.as_bytes().get(1..3) == Some(b":/");
            assert!(
                is_path,
                "cargo tree names `{name}` from a source this test does not handle: {line}"
            );
            None
        }
    }
}

/// The crates a `cargo tree` run names, as `(name, version)`. The workspace's
/// own crates are left out (`cargo tree` prints their path); each NOTICE
/// covers them in a sentence. Native only: a wasm32 test runner can't spawn
/// `cargo`.
#[cfg(not(target_family = "wasm"))]
fn tree_crates(args: &[&str]) -> Crates {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let output = std::process::Command::new(cargo)
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .arg("tree")
        .args(args)
        .args(["-e", "normal,no-proc-macro", "--prefix", "none", "--locked"])
        .output()
        .expect("run cargo tree");
    assert!(
        output.status.success(),
        "cargo tree {args:?} failed:\n{}",
        String::from_utf8_lossy(&output.stderr),
    );
    String::from_utf8(output.stdout)
        .expect("UTF-8 output")
        .lines()
        .filter_map(tree_line)
        .collect()
}

/// The `(name, version)` rows of a NOTICE's crate lists: a line indented
/// exactly two spaces whose second word is a dotted version. Continuation
/// lines, bullets and prose never match.
#[cfg(not(target_family = "wasm"))]
fn listed_crates(notice: &str) -> Crates {
    notice
        .lines()
        .filter(|line| line.starts_with("  ") && !line.starts_with("   "))
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let name = parts.next()?;
            let version = parts.next()?;
            let dotted = version.split('.').count() == 3
                && version
                    .split('.')
                    .all(|n| !n.is_empty() && n.chars().all(|c| c.is_ascii_digit()));
            dotted.then(|| (name.to_string(), version.to_string()))
        })
        .collect()
}

#[cfg(not(target_family = "wasm"))]
fn assert_same_crates(package: &str, listed: &Crates, graph: &Crates) {
    let missing: Vec<_> = graph.difference(listed).collect();
    let stale: Vec<_> = listed.difference(graph).collect();
    assert!(
        missing.is_empty() && stale.is_empty(),
        "{package}'s NOTICE crate list has drifted from `cargo tree`; regenerate it.\n\
         in the graph, not the NOTICE: {missing:?}\n\
         in the NOTICE, not the graph: {stale:?}",
    );
}

/// "Regenerated when that graph changes" is held by this test: the npm NOTICE
/// lists exactly the crates in the wasm module's graph, across both entries
/// (the default one, and `full/` with the fetch planner and Zarr).
#[cfg(not(target_family = "wasm"))]
#[test]
fn the_npm_notice_lists_the_wasm_module_s_crates() {
    let wasm = [
        "-p",
        "fieldglass-wasm",
        "--target",
        "wasm32-unknown-unknown",
    ];
    let default = tree_crates(&wasm);
    let mut full_args = wasm.to_vec();
    full_args.extend(["--features", "fetchplan,zarr"]);
    let full = tree_crates(&full_args);
    assert!(
        default.iter().any(|(name, _)| name == "wasm-bindgen"),
        "the wasm graph is missing wasm-bindgen: {default:?}",
    );
    assert!(
        full.is_superset(&default),
        "`full/` links less than the default entry"
    );
    assert_same_crates(NPM, &listed_crates(notice_of(NPM)), &full);
}

/// The same for the extension's NOTICE and the native addon, across every
/// target the addon ships for (`--target all` adds `windows-link`).
#[cfg(not(target_family = "wasm"))]
#[test]
fn the_extension_notice_lists_the_addon_s_crates() {
    let graph = tree_crates(&["-p", "fieldglass-napi", "--target", "all"]);
    assert!(
        graph.iter().any(|(name, _)| name == "napi"),
        "the addon graph is missing napi: {graph:?}",
    );
    assert_same_crates(EXTENSION, &listed_crates(notice_of(EXTENSION)), &graph);
}

#[cfg(not(target_family = "wasm"))]
#[test]
fn tree_line_keeps_registry_crates_and_skips_workspace_paths() {
    let krate = |n: &str, v: &str| Some((n.to_string(), v.to_string()));
    assert_eq!(tree_line("slab v0.4.12"), krate("slab", "0.4.12"));
    assert_eq!(tree_line("serde v1.0.229 (*)"), krate("serde", "1.0.229"));
    assert_eq!(
        tree_line("fieldglass v0.6.0 (/repo/crates/fieldglass)"),
        None
    );
    assert_eq!(
        tree_line(r"fieldglass v0.6.0 (C:\repo\crates\fieldglass)"),
        None
    );
    assert_eq!(tree_line(""), None);
}

#[cfg(not(target_family = "wasm"))]
#[test]
#[should_panic(expected = "source this test does not handle")]
fn tree_line_refuses_a_url_source() {
    tree_line("foo v1.2.3 (https://example.com/x/foo#abcdef0)");
}
