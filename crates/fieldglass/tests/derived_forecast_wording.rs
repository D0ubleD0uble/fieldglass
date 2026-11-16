//! `MessageInfo::derived_forecast` and the listing's copy of it (#1850): which
//! product of a whole ensemble a GRIB2 template 4.2 or 4.12 message is, worded
//! from Code Table 4.7, so a message list can tell an ensemble's mean, spread
//! and a derived field of unstated kind apart.
//!
//! Read on the wire, as a host reads it, over `uv_derived.grib2` (see
//! `fixtures/NOTICE.md`): a u mean, then a v mean, a v member, a v spread, a v
//! whose kind is coded missing, a 4.12 v mean and a u member. A code the table
//! does not name is that file's spread with §4 octet 35 rewritten in memory.

use fieldglass::Session;
use serde_json::Value;

const DERIVED: &[u8] = include_bytes!("fixtures/uv_derived.grib2");

/// `message(index)` and `messageListing(index)` as JSON, in that order.
fn wire(s: &Session, index: u32) -> (Value, Value) {
    let info = serde_json::to_value(s.message(index).expect("a message")).expect("serializes");
    let listing =
        serde_json::to_value(s.message_listing(index).expect("a listing")).expect("serializes");
    (info, listing)
}

/// The listing's `derivedForecast`, held equal to the `MessageInfo`'s. Absent
/// reads as a failure, not as `null`: every object this version returns
/// carries the key.
fn derived(s: &Session, index: u32) -> Option<String> {
    let (info, listing) = wire(s, index);
    let key = "derivedForecast";
    assert!(
        listing.get(key).is_some(),
        "#{index}: the listing carries {key}"
    );
    assert!(
        info.get(key).is_some(),
        "#{index}: the message carries {key}"
    );
    assert_eq!(info[key], listing[key], "#{index}: the two calls agree");
    listing[key].as_str().map(str::to_string)
}

#[test]
fn a_mean_a_spread_and_an_unstated_kind_read_apart() {
    let s = Session::open(DERIVED.to_vec()).expect("opens");
    assert_eq!(s.count(), 7);
    let mean = "Unweighted mean of all members";
    assert_eq!(derived(&s, 0).as_deref(), Some(mean), "the u mean");
    assert_eq!(derived(&s, 1).as_deref(), Some(mean), "the v mean");
    assert_eq!(
        derived(&s, 3).as_deref(),
        Some("Spread of all members"),
        "the v spread"
    );
    assert_eq!(
        derived(&s, 4).as_deref(),
        Some("Derived forecast, kind missing"),
        "a derived forecast coded missing (255) says it is one, not just `Missing`"
    );
    assert_eq!(
        derived(&s, 5).as_deref(),
        Some(mean),
        "template 4.12 states one too"
    );
    // Members are no derived forecast; their member says what they are.
    for member in [2, 6] {
        assert_eq!(derived(&s, member), None, "#{member} is a member");
    }
    // The three V messages that read alike without it now differ.
    let (one, three, four) = (derived(&s, 1), derived(&s, 3), derived(&s, 4));
    assert!(one != three && three != four && one != four);
}

/// The byte offset of §4 octet 35 of the message that starts at `start`.
fn derived_octet(bytes: &[u8], start: usize) -> usize {
    // §0 is 16 octets; every later section opens with its length (4 octets)
    // and its number (1).
    let mut at = start + 16;
    loop {
        let len = u32::from_be_bytes(bytes[at..at + 4].try_into().expect("4 octets")) as usize;
        if bytes[at + 4] == 4 {
            return at + 34;
        }
        at += len;
    }
}

#[test]
fn a_code_the_table_does_not_name_reads_by_number() {
    let s = Session::open(DERIVED.to_vec()).expect("opens");
    let start = usize::try_from(s.message(3).expect("the spread").offset_bytes).expect("fits");
    for (code, want) in [
        (11, "Derived forecast 11"),
        (192, "Derived forecast 192"),
        (10, "Variance of all ensemble members"),
    ] {
        let mut bytes = DERIVED.to_vec();
        let octet = derived_octet(&bytes, start);
        assert_eq!(bytes[octet], 4, "octet 35 of #3 is the spread's code");
        bytes[octet] = code;
        let s = Session::open(bytes).expect("opens");
        assert_eq!(derived(&s, 3).as_deref(), Some(want), "code {code}");
    }
}

/// Every message of the GRIB corpus that is not a 4.2 or 4.12 derived
/// forecast carries the key as `null`, and one that is carries a name.
#[test]
fn only_a_derived_forecast_is_worded() {
    let mut derived_seen = 0usize;
    for dir in [
        "../fieldglass-grib1/tests/fixtures",
        "../fieldglass-grib2/tests/fixtures",
        "tests/fixtures",
    ] {
        let mut paths: Vec<_> = std::fs::read_dir(dir)
            .unwrap_or_else(|e| panic!("{dir} is the committed corpus: {e}"))
            .map(|e| e.expect("entry").path())
            .filter(|p| {
                matches!(
                    p.extension().and_then(|e| e.to_str()),
                    Some("grib" | "grib1" | "grib2")
                )
            })
            .collect();
        paths.sort();
        for path in paths {
            let Ok(s) = Session::open(std::fs::read(&path).expect("reads")) else {
                continue;
            };
            for index in 0..s.count() {
                let Ok(info) = s.message(index) else { continue };
                let template = match &info.fingerprint {
                    fieldglass::MessageFingerprint::Grib2 {
                        product_template, ..
                    } => Some(*product_template),
                    _ => None,
                };
                let is_derived = matches!(template, Some(2 | 12));
                let worded = derived(&s, index);
                assert_eq!(
                    worded.is_some(),
                    is_derived,
                    "{}#{index}: template {template:?}, worded {worded:?}",
                    path.display()
                );
                derived_seen += usize::from(is_derived);
            }
        }
    }
    assert!(derived_seen >= 5, "the corpus holds derived forecasts");
}
