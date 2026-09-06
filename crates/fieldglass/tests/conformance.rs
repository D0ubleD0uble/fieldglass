//! The reference runner for the ADR-0006 conformance suite (#573).
//!
//! Three runners drive `conformance/suite.json`. This is the first and the
//! one that records it:
//!
//! | runner | binding under test | where |
//! |---|---|---|
//! | this file | `fieldglass::Session` itself | `cargo test --workspace`, and `cargo test --target wasm32-wasip1 -p fieldglass` in CI |
//! | `fieldglass-napi`'s `conformance_host` module | the napi handles | `cargo test --workspace` |
//! | `crates/fieldglass-wasm/tests/node/conformance.mjs` | the built browser bundle, from Node | the `wasm32 build` CI job |
//!
//! The first two are Rust and the third is JavaScript, which is the point:
//! ADR-0006 decision 3 asks for expectations that live in the crate as *data*,
//! so a host in another language reads the same file rather than a translated
//! copy of it.
//!
//! # Recording
//!
//! ```sh
//! FIELDGLASS_UPDATE_CONFORMANCE=1 cargo test -p fieldglass --test conformance
//! ```
//!
//! The run fails afterwards on purpose: a run that re-recorded has verified
//! nothing, and an environment with the variable left set must not be able to
//! turn the suite into a no-op that re-baselines every diff it exists to catch.

use fieldglass::conformance::{
    self, Case, RecordedCase, Suite, Tolerance, cases, compare, error_codes, observe,
};

/// Set to re-record rather than compare.
const UPDATE_ENV: &str = "FIELDGLASS_UPDATE_CONFORMANCE";

/// The recording, relative to this crate's manifest directory (which cargo
/// makes the working directory of a test).
const SUITE_PATH: &str = "conformance/suite.json";

/// Read a fixture named the way the suite names one: relative to `crates/`.
fn read(fixture: &str) -> Result<Vec<u8>, String> {
    let path = format!("../{fixture}");
    std::fs::read(&path).map_err(|e| format!("{path}: {e}"))
}

/// The shipped suite, parsed, with a message that says what to do when it is
/// the file rather than the code that is wrong.
fn shipped() -> Suite {
    conformance::suite().unwrap_or_else(|e| {
        panic!("{SUITE_PATH} does not parse as the current `Suite` type: {e}\nre-record with {UPDATE_ENV}=1")
    })
}

/// Rewrite the recording from a fresh run, then fail.
fn rerecord() {
    let suite = conformance::record(&read).expect("every fixture reads");
    let json = conformance::to_pretty_json(&suite).expect("the suite serialises");
    std::fs::write(SUITE_PATH, json).expect("the recording is writable");
    panic!(
        "{SUITE_PATH} rewritten from this run ({} cases). \
         Unset {UPDATE_ENV} and run again to verify it.",
        suite.cases.len()
    );
}

/// The suite, case by case, against `fieldglass::Session`.
///
/// This is the recording run as well: with `FIELDGLASS_UPDATE_CONFORMANCE` set
/// it rewrites the file and then fails, so re-recording can never be mistaken
/// for a green run.
#[test]
fn every_conformance_case_matches_its_recording() {
    if std::env::var_os(UPDATE_ENV).is_some() {
        rerecord();
    }
    let suite = shipped();
    assert!(
        !suite.cases.is_empty(),
        "the conformance suite is empty — it would pass while checking nothing"
    );

    let mut failures = Vec::new();
    for RecordedCase { case, expect } in &suite.cases {
        let bytes = read(&case.fixture).expect("fixture");
        let observed = observe(&bytes, case);
        for line in compare(expect, &observed, suite.tolerance) {
            failures.push(format!("{}: {line}", case.id));
        }
    }
    assert!(
        failures.is_empty(),
        "{} conformance disagreement(s):\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// The file must still describe exactly the cases the code names, in order.
///
/// Without this, adding a case and forgetting to re-record would leave the new
/// case simply absent — the suite would stay green while covering less, which
/// is the failure mode a data-driven gate is most prone to.
#[test]
fn the_recording_covers_exactly_the_cases_the_code_names() {
    let suite = shipped();
    let recorded: Vec<Case> = suite.cases.iter().map(|r| r.case.clone()).collect();
    let named = cases();
    assert_eq!(
        recorded.len(),
        named.len(),
        "the recording holds {} cases and the code names {} — re-record with {UPDATE_ENV}=1",
        recorded.len(),
        named.len()
    );
    for (r, n) in recorded.iter().zip(&named) {
        assert_eq!(r, n, "recorded case differs from the one the code names");
    }
}

/// Every code in the shipped list is produced by a real call in the suite, and
/// every code a suite case produced is in the list.
///
/// This is what "`Error::code()` values are pinned by the suite" means in
/// practice. A list on its own would pass while naming codes nothing can
/// reach; the cases on their own would pass while a variant quietly stopped
/// being reachable at all.
#[test]
fn every_error_code_is_both_listed_and_reachable() {
    let suite = shipped();
    assert_eq!(
        suite.error_codes,
        error_codes(),
        "the recorded error codes differ from this build's — re-record with {UPDATE_ENV}=1"
    );

    let mut seen: Vec<String> = suite
        .cases
        .iter()
        .filter_map(|r| {
            r.expect
                .get("error")
                .and_then(|e| e.get("code"))
                .and_then(|c| c.as_str())
                .map(str::to_string)
        })
        .collect();
    seen.sort();
    seen.dedup();
    assert_eq!(
        seen,
        error_codes(),
        "every error code must be produced by at least one conformance case"
    );
}

/// Exactly these cases record a failure; every other one produced an answer.
///
/// The five `error/…` cases exist to prove each [`fieldglass::Error`] code is
/// reachable. The two beside them are the `degenerate` subject — a §3.20 grid
/// stating `Dx = Dy = 0`, which decodes fine and has no extent to warp onto,
/// which is why that fixture is in the suite. `degenerate/warp/window` is not
/// here: a *manual* window gives the warp a box even when the grid states none,
/// and that difference is worth having recorded.
///
/// `error/unsupported` is that same refusal, on purpose: since #580 no fixture
/// in the corpus produces `unsupported` at `decode`, because both families that
/// used to — spectral and HEALPix — are synthesised onto a lat/lon grid now.
/// It is kept as a named case rather than left to `degenerate/warp/bilinear`
/// so that the code's reachability is proved by a case whose whole job that is.
const CASES_THAT_RECORD_A_FAILURE: &[&str] = &[
    "degenerate/warp/bilinear",
    "degenerate/warp/nearest",
    "error/decode",
    "error/invalid_option",
    "error/no_such_message",
    "error/unsupported",
    "error/unsupported_format",
];

/// A case that stopped exercising its operation records a failure instead, and
/// passes for ever after.
///
/// This is the hole in a recorded suite: [`observe`] turns *any* failure into an
/// observation, so an argument dropped in a refactor, a fixture that no longer
/// decodes, or an option an operation newly rejects re-records as
/// `{"error": …}` and nothing says so. The recording still matches, the case
/// count is still right, and `every_error_code_is_both_listed_and_reachable` is
/// satisfied by the deliberate cases whatever else joins them.
///
/// Asserted in both directions, because half of it would be the same fail-open:
/// a case that starts failing has to be added here on purpose, and one that
/// stops failing — a refusal quietly becoming an answer — is a change to the
/// contract too.
#[test]
fn exactly_the_expected_cases_record_a_failure() {
    let suite = shipped();
    let mut failing: Vec<&str> = suite
        .cases
        .iter()
        .filter(|r| r.expect.get("error").is_some())
        .map(|r| r.case.id.as_str())
        .collect();
    failing.sort_unstable();
    assert_eq!(
        failing, CASES_THAT_RECORD_A_FAILURE,
        "a case that records a failure proves nothing about the operation it \
         names; add it here deliberately or find out why it stopped answering"
    );
}

/// The recorded tolerance is the one this build chose.
///
/// It is read out of the JSON and used by all three runners, so editing that
/// one line to `1e-1` would loosen every comparison in the suite at once and
/// leave nothing red — including the perturbation check below, which is
/// supposed to be the thing that proves the numbers are pinned.
#[test]
fn the_recorded_tolerance_is_the_one_this_build_chose() {
    assert_eq!(
        shipped().tolerance,
        Tolerance::default(),
        "the suite's tolerance has been edited away from this build's"
    );
}

/// The comparator rejects a recording perturbed by more than its tolerance —
/// for every case that has a real-valued leaf to perturb.
///
/// Without this the suite could be green because the comparison is loose rather
/// than because the answers agree.
#[test]
fn the_suite_would_notice_if_the_answers_moved() {
    let suite = shipped();
    // The suite's own tolerance, not this build's default: a recording that
    // widened it has to fail *here*, which is what the test above pins.
    let tol = suite.tolerance;

    // Perturb every real-valued leaf of every recorded observation by a
    // thousand times the tolerance and check each case notices.
    let mut unmoved = Vec::new();
    for RecordedCase { case, expect } in &suite.cases {
        let mut perturbed = expect.clone();
        let moved = perturb(&mut perturbed);
        if !moved {
            // A case whose observation is entirely discrete (an error code, a
            // count) is pinned by exact comparison already; nothing to check.
            continue;
        }
        if compare(expect, &perturbed, tol).is_empty() {
            unmoved.push(case.id.clone());
        }
    }
    assert!(
        unmoved.is_empty(),
        "these cases accept a perturbed recording, so their numbers are not pinned: {unmoved:?}"
    );
}

/// Move every real-valued leaf well outside the tolerance. Returns whether
/// anything moved.
fn perturb(value: &mut serde_json::Value) -> bool {
    match value {
        serde_json::Value::Number(n) if n.is_f64() => {
            let Some(x) = n.as_f64() else { return false };
            let bumped = x + 1e-6 + x.abs() * 1e-6;
            match serde_json::Number::from_f64(bumped) {
                Some(m) => {
                    *n = m;
                    true
                }
                None => false,
            }
        }
        // A plain loop rather than `any`, which short-circuits: every leaf has
        // to move, or the check would only prove the *first* number is pinned.
        serde_json::Value::Array(a) => {
            let mut moved = false;
            for v in a {
                moved |= perturb(v);
            }
            moved
        }
        serde_json::Value::Object(o) => {
            let mut moved = false;
            for (_, v) in o {
                moved |= perturb(v);
            }
            moved
        }
        _ => false,
    }
}
