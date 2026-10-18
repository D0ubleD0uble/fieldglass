//! `verify_message` over real sidecar records and the real bytes they point at
//! (#1093).
//!
//! Every pair here is a committed, unmodified sidecar line and the message a
//! producer published at that line's offset, fetched by range from the object
//! the sidecar describes (provenance in
//! `crates/fieldglass-fetchplan/tests/fixtures/NOTICE.md`). Nothing is built
//! from the decoder's own output: a sidecar written from what the decoder says
//! agrees with the decoder by construction, which is how the level check came to
//! refuse every real wgrib2 record with a level (`2 mb` against the bare
//! pascals `200`) while `fetch_plan_slices_a_message.rs` stayed green.
//!
//! Each accepted pair is matched by a refused one from the same sidecar, a
//! neighbouring record promising another level or parameter, so a
//! `verify_message` that accepted everything fails here too.

use fieldglass::Session;
use fieldglass::api::{MessageIdentity, MessageInfo};
use fieldglass::fetchplan::{
    EcmwfIndex, Manifest, MessageManifest, Mismatch, NoResolver, PlanItem, Query, Wgrib2Idx,
    verify_envelope, verify_message,
};

/// Relative to the crate directory, not built from `CARGO_MANIFEST_DIR`: this
/// suite also runs under `wasmtime --dir=. --dir=..`, which cannot open an
/// absolute host path.
const FIXTURES: &str = "../fieldglass-fetchplan/tests/fixtures";

fn read(name: &str) -> Vec<u8> {
    let path = format!("{FIXTURES}/{name}");
    std::fs::read(&path).unwrap_or_else(|e| panic!("reading {path}: {e}"))
}

fn text(name: &str) -> String {
    String::from_utf8(read(name)).expect("a sidecar is text")
}

/// The one message in `bytes`, after the envelope check a host runs first.
fn message(item: &PlanItem, bytes: Vec<u8>) -> MessageInfo {
    verify_envelope(item, &bytes).expect("the bytes are the message the range promised");
    let session = Session::open(bytes).expect("the message opens");
    assert_eq!(session.count(), 1);
    session.message(0).expect("message 0")
}

/// The single record a query selects, which the test relies on being unique.
fn only(manifest: &dyn MessageManifest, query: &Query) -> PlanItem {
    let hits = manifest.select(query, &NoResolver);
    assert_eq!(hits.len(), 1, "{query:?} should select one record");
    hits.into_iter().next().unwrap()
}

fn refused_field(result: Result<(), Mismatch>) -> (&'static str, String) {
    match result {
        Err(Mismatch::Field {
            field, expected, ..
        }) => (field, expected),
        other => panic!("expected a field mismatch, got {other:?}"),
    }
}

fn gfs() -> Wgrib2Idx {
    Wgrib2Idx::parse("gfs.t00z.pgrb2.0p25.f000", &text("gfs_0p25.idx"))
        .expect("the committed sidecar parses")
}

fn gefs_chem() -> Wgrib2Idx {
    Wgrib2Idx::parse(
        "gefs.chem.t00z.a2d_0p25.f000.grib2",
        &text("gefs_chem_a2d_0p25.idx"),
    )
    .expect("the committed sidecar parses")
}

fn ecmwf() -> EcmwfIndex {
    EcmwfIndex::parse(
        "20260904000000-0h-oper-fc.grib2",
        &text("ecmwf_ifs_0p25_oper.index"),
    )
    .expect("the committed sidecar parses")
}

/// GFS record 107, `RH:2 mb:anl`, against the 210 bytes at its offset. The
/// message states its level as isobaric surface 200 Pa; the line says `2 mb`.
#[test]
fn gfs_record_107_is_what_its_idx_line_promised() {
    let idx = gfs();
    let item = idx.items()[106].clone();
    assert_eq!(item.expect.abbreviation.as_deref(), Some("RH"));
    assert_eq!(item.expect.level.as_deref(), Some("2 mb"));

    let info = message(&item, read("gfs_0p25_rh_2mb.grib2"));
    verify_message(&item.expect, &info).expect("record 107 is the message at its offset");
}

/// The same bytes against the record for another level of the same
/// parameter: the check still refuses a wrong level.
#[test]
fn gfs_record_107_is_not_relative_humidity_at_3_mb() {
    let idx = gfs();
    let info = message(&idx.items()[106], read("gfs_0p25_rh_2mb.grib2"));

    let other = only(&idx, &Query::abbreviation("RH").at_level_text("3 mb"));
    assert_eq!(
        refused_field(verify_message(&other.expect, &info)),
        ("level", "3 mb".to_string())
    );
}

/// …and against the record for another parameter at the same level.
#[test]
fn gfs_record_107_is_not_temperature_at_2_mb() {
    let idx = gfs();
    let info = message(&idx.items()[106], read("gfs_0p25_rh_2mb.grib2"));

    let other = only(&idx, &Query::abbreviation("TMP").at_level_text("2 mb"));
    assert_eq!(
        refused_field(verify_message(&other.expect, &info)),
        ("abbreviation", "TMP".to_string())
    );
}

/// ECMWF `r` on `pl` `50`, against the bytes at its `_offset`. The message's
/// level is 5000 Pa and its parameter decodes to WMO `RH`, not the MARS `r`
/// the sidecar names.
#[test]
fn an_ecmwf_isobaric_record_is_what_its_index_line_promised() {
    let index = ecmwf();
    let item = only(&index, &Query::abbreviation("r").at_level_text("pl 50"));
    let info = message(&item, read("ecmwf_ifs_0p25_r_50hpa.grib2"));
    verify_message(&item.expect, &info).expect("the record is the message at its offset");
}

/// ECMWF `mx2t3` on `sfc`, against its 224 bytes. The message is on WMO
/// surface 103 at 2 m: MARS `sfc` is every single-level field, so this is the
/// pair that a reading of `sfc` as "the ground" would refuse.
#[test]
fn an_ecmwf_single_level_record_is_what_its_index_line_promised() {
    let index = ecmwf();
    let item = only(&index, &Query::abbreviation("mx2t3"));
    assert_eq!(item.expect.level.as_deref(), Some("sfc"));
    let info = message(&item, read("ecmwf_ifs_0p25_mx2t3.grib2"));
    verify_message(&item.expect, &info).expect("the record is the message at its offset");
}

/// The ECMWF isobaric bytes against the records for another pressure level and
/// for a single-level field: both are refused on the level, since a MARS name
/// cannot be checked against a WMO table's.
#[test]
fn an_ecmwf_record_for_another_level_is_refused() {
    let index = ecmwf();
    let r50 = only(&index, &Query::abbreviation("r").at_level_text("pl 50"));
    let info = message(&r50, read("ecmwf_ifs_0p25_r_50hpa.grib2"));

    let r10 = only(&index, &Query::abbreviation("r").at_level_text("pl 10"));
    assert_eq!(
        refused_field(verify_message(&r10.expect, &info)),
        ("level", "pl 10".to_string())
    );

    let mx2t3 = only(&index, &Query::abbreviation("mx2t3"));
    assert_eq!(
        refused_field(verify_message(&mx2t3.expect, &info)),
        ("level", "sfc".to_string())
    );
}

/// GEFS-aerosol record 22, `PMTC:surface:anl:aerosol=Dust dry:…`, against the
/// 216,548 bytes at its offset (#1126). Its §4 is template 4.48 (optical
/// properties of aerosol), which the reader does not parse, so the message's
/// surfaces are unknown and the line's level has nothing to be compared with.
#[test]
fn a_gefs_aerosol_record_is_what_its_idx_line_promised() {
    let idx = gefs_chem();
    let item = idx.items()[21].clone();
    assert_eq!(item.expect.abbreviation.as_deref(), Some("PMTC"));
    assert_eq!(item.expect.level.as_deref(), Some("surface"));

    let info = message(&item, read("gefs_chem_0p25_pmtc_sfc.grib2"));
    let MessageIdentity::Grib2 {
        product_template,
        parameter_number,
        first_surface,
        ..
    } = &info.identity
    else {
        panic!("a GRIB2 message");
    };
    // The reader does not parse 4.48, so the parameter and the surfaces are
    // unread and only the reference time is left to check.
    assert_eq!(*product_template, 48);
    assert_eq!(*parameter_number, None);
    assert_eq!(info.abbreviation, None);
    assert_eq!(*first_surface, None);

    verify_message(&item.expect, &info).expect("record 22 is the message at its offset");
}

/// What a message on an unparsed template is still checked on: the reference
/// time. Record 22's line with another cycle's date is refused.
#[test]
fn a_gefs_aerosol_record_from_another_cycle_is_refused() {
    let idx = gefs_chem();
    let item = idx.items()[21].clone();
    let info = message(&item, read("gefs_chem_0p25_pmtc_sfc.grib2"));

    let mut stale = item.expect.clone();
    stale.reference_time = Some("2026090312".to_string());
    assert_eq!(
        refused_field(verify_message(&stale, &info)),
        ("referenceTime", "2026090312".to_string())
    );
}
