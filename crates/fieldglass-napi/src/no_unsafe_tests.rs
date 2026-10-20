//! The crate holds no hand-written `unsafe` (ADR-0008, #1242).
//!
//! The crate root's `deny` of the lint does the work, but a `deny` is lifted by
//! one `allow` written beside the code, and the `#[napi]` glue's own allowance
//! means the root cannot be a `forbid`. So this reads every source file and
//! fails on a hand-written allowance of the lint, on a Semgrep suppression, and
//! on the `deny` going missing. The needles are spelt in pieces so this file
//! does not match itself.

use std::fs;
use std::path::{Path, PathBuf};

/// The lint's name.
const LINT: &str = concat!("unsafe", "_code");
/// A Semgrep suppression comment.
const SUPPRESSION: &str = concat!("no", "semgrep");
/// The lint levels that would let hand-written `unsafe` compile.
const LIFTING_LEVELS: [&str; 3] = ["allow", "expect", "warn"];

/// Every `.rs` file under `src/`.
fn sources() -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(dir).expect("src/ is readable") {
            let path = entry.expect("a directory entry").path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }
    let mut out = Vec::new();
    walk(&Path::new(env!("CARGO_MANIFEST_DIR")).join("src"), &mut out);
    assert!(out.len() > 1, "found the crate's sources");
    out
}

/// Each place in `text` that sets the lint to a level that lifts the `deny`:
/// the lint's name inside `allow(…)`, `expect(…)` or `warn(…)`, other lints in
/// the same list and line breaks included.
fn lifted_levels(text: &str) -> Vec<String> {
    text.match_indices(LINT)
        .filter_map(|(at, _)| {
            let open = text[..at].rfind('(')?;
            let level = text[..open].trim_end();
            let level = level
                .rsplit(|c: char| !(c.is_alphanumeric() || c == '_'))
                .next()?;
            LIFTING_LEVELS
                .contains(&level)
                .then(|| format!("{level}(… {LINT}"))
        })
        .collect()
}

#[test]
fn the_crate_root_denies_unsafe_code() {
    let lib = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("src/lib.rs"))
        .expect("lib.rs is readable");
    let deny = format!("#![deny({LINT})]");
    assert!(
        lib.lines().any(|line| line.trim() == deny),
        "src/lib.rs must keep `{deny}`: the crate takes no hand-written unsafe (ADR-0008)"
    );
}

#[test]
fn no_source_lifts_the_lint_or_suppresses_semgrep() {
    let mut problems = Vec::new();
    for path in sources() {
        let text = fs::read_to_string(&path).expect("a source file is readable");
        for found in lifted_levels(&text) {
            problems.push(format!("{}: `{found}`", path.display()));
        }
        for (n, line) in text.lines().enumerate() {
            if line.contains(SUPPRESSION) {
                problems.push(format!(
                    "{}:{}: a Semgrep suppression",
                    path.display(),
                    n + 1
                ));
            }
        }
    }
    assert!(
        problems.is_empty(),
        "the napi crate takes no hand-written unsafe and no Semgrep suppression \
         (ADR-0008):\n{}",
        problems.join("\n")
    );
}

#[test]
fn the_scan_sees_each_way_of_lifting_the_lint() {
    for level in LIFTING_LEVELS {
        assert_eq!(
            lifted_levels(&format!("#[{level}({LINT})]")).len(),
            1,
            "{level}"
        );
        assert_eq!(
            lifted_levels(&format!(
                "#[cfg_attr(test, {level}(dead_code,\n    {LINT}))]"
            ))
            .len(),
            1,
            "{level} in a list"
        );
    }
    assert!(lifted_levels(&format!("#![deny({LINT})]")).is_empty());
    assert!(lifted_levels(&format!("#![forbid({LINT})]")).is_empty());
}
