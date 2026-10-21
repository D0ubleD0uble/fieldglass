//! `verify_message` over real sidecar records and the real bytes they point at
//! (#1093).
//!
//! Every pair here is a committed, unmodified sidecar line and the message a
//! producer published at that line's offset, fetched by range from the object
//! the sidecar describes (provenance in the `NOTICE.md` beside each: the sidecars
//! in `crates/fieldglass-fetchplan/tests/fixtures/`, the messages in
//! `crates/fieldglass/tests/fixtures/`). Nothing is built
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
const SIDECARS: &str = "../fieldglass-fetchplan/tests/fixtures";

/// The messages the sidecars point at. They live here, not beside the
/// sidecars, because only this crate reads them (#1132).
const MESSAGES: &str = "tests/fixtures";

fn read_from(dir: &str, name: &str) -> Vec<u8> {
    let path = format!("{dir}/{name}");
    std::fs::read(&path).unwrap_or_else(|e| panic!("reading {path}: {e}"))
}

fn read(name: &str) -> Vec<u8> {
    read_from(MESSAGES, name)
}

fn text(name: &str) -> String {
    String::from_utf8(read_from(SIDECARS, name)).expect("a sidecar is text")
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
/// for a single-level field: the first is refused on the level, and the second
/// on its MARS name, which is checked before the level (#1116).
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
        ("abbreviation", "mx2t3".to_string())
    );
    // With the name out of the way, `sfc` still does not hold for 50 hPa.
    let mut unnamed = mx2t3.expect.clone();
    unnamed.abbreviation = None;
    assert_eq!(
        refused_field(verify_message(&unnamed, &info)),
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

fn nbm() -> Wgrib2Idx {
    Wgrib2Idx::parse("blend.t00z.core.f001.co.grib2", &text("nbm_core_co.idx"))
        .expect("the committed sidecar parses")
}

fn hrrr() -> Wgrib2Idx {
    Wgrib2Idx::parse("hrrr.t00z.wrfsfcf00.grib2", &text("hrrr_conus_wrfsfc.idx"))
        .expect("the committed sidecar parses")
}

/// The one ECMWF record for `param` at `level` (`sfc`, `pl 50`, `sol 3`).
fn ecmwf_record(param: &str, level: &str) -> PlanItem {
    only(&ecmwf(), &Query::abbreviation(param).at_level_text(level))
}

/// ECMWF `mx2t3` and `mn2t3` (#1116): both 224 bytes, `sfc`, 2 m, step 3 of
/// one run, and both discipline 0, category 0, number 0. Only the statistical
/// process (maximum 2, minimum 3) tells them apart, and each line is refused
/// on the other's bytes.
#[test]
fn ecmwf_mx2t3_and_mn2t3_are_refused_on_each_others_bytes() {
    let mx2t3 = ecmwf_record("mx2t3", "sfc");
    let mn2t3 = ecmwf_record("mn2t3", "sfc");
    let max_bytes = message(&mx2t3, read("ecmwf_ifs_0p25_mx2t3.grib2"));
    let min_bytes = message(&mn2t3, read("ecmwf_ifs_0p25_mn2t3.grib2"));

    verify_message(&mn2t3.expect, &min_bytes).expect("line 13 is the message at its offset");
    assert_eq!(
        refused_field(verify_message(&mx2t3.expect, &min_bytes)),
        ("abbreviation", "mx2t3".to_string())
    );
    assert_eq!(
        refused_field(verify_message(&mn2t3.expect, &max_bytes)),
        ("abbreviation", "mn2t3".to_string())
    );
}

/// ECMWF `t` on `pl 50` against the bytes of `r` on `pl 50`: one level, one
/// run, another parameter (#1116).
#[test]
fn ecmwf_temperature_is_refused_on_relative_humidity_bytes() {
    let r50 = ecmwf_record("r", "pl 50");
    let info = message(&r50, read("ecmwf_ifs_0p25_r_50hpa.grib2"));
    let t50 = ecmwf_record("t", "pl 50");
    assert_eq!(
        refused_field(verify_message(&t50.expect, &info)),
        ("abbreviation", "t".to_string())
    );
}

/// ECMWF `sot` on `sol 3` against its 625,288 bytes, which code the layer
/// between soil levels 2 and 3 (surface type 151 twice). eccodes, and so
/// MARS, numbers such a layer by its bottom (#1116). The `sol 1` line is
/// refused, and so is `vsw` on `sol 3`.
#[test]
fn ecmwf_soil_levels_are_compared_by_number() {
    let sol3 = ecmwf_record("sot", "sol 3");
    let info = message(&sol3, read("ecmwf_ifs_0p25_sot_sol3.grib2"));
    verify_message(&sol3.expect, &info).expect("line 4 is the message at its offset");

    let sol1 = ecmwf_record("sot", "sol 1");
    assert_eq!(
        refused_field(verify_message(&sol1.expect, &info)),
        ("level", "sol 1".to_string())
    );
    let vsw3 = ecmwf_record("vsw", "sol 3");
    assert_eq!(
        refused_field(verify_message(&vsw3.expect, &info)),
        ("abbreviation", "vsw".to_string())
    );
}

/// ECMWF `vsw` on `sol 1`, the top soil layer: the bytes code it as the layer
/// between soil levels 0 and 1, so it too is numbered by its bottom. The
/// `sol 2` record is refused on the level, and `sot` on `sol 1` on the name.
#[test]
fn the_top_ecmwf_soil_layer_is_level_1() {
    let sol1 = ecmwf_record("vsw", "sol 1");
    let info = message(&sol1, read("ecmwf_ifs_0p25_vsw_sol1.grib2"));
    verify_message(&sol1.expect, &info).expect("line 69 is the message at its offset");

    let sol2 = ecmwf_record("vsw", "sol 2");
    assert_eq!(
        refused_field(verify_message(&sol2.expect, &info)),
        ("level", "sol 2".to_string())
    );
    let sot1 = ecmwf_record("sot", "sol 1");
    assert_eq!(
        refused_field(verify_message(&sot1.expect, &info)),
        ("abbreviation", "sot".to_string())
    );
}

fn gefs_member() -> Wgrib2Idx {
    Wgrib2Idx::parse("gep01.t00z.pgrb2a.0p50.f000", &text("gefs_p01_0p50.idx"))
        .expect("the committed sidecar parses")
}

/// GEFS member 1's `ICETK:surface:anl:ENS=+1` (record 63) against its 24,114
/// bytes, template 4.1, perturbed member 1 (#1117). The same line naming
/// member 2, or the control, is refused; so is the line without its `ENS=`
/// qualifier, which wgrib2 prints for every member forecast.
#[test]
fn a_gefs_member_is_compared_by_its_ens_qualifier() {
    let item = gefs_member().items()[62].clone();
    assert_eq!(item.expect.abbreviation.as_deref(), Some("ICETK"));
    assert_eq!(item.expect.qualifiers, ["ENS=+1"]);
    let info = message(&item, read("gefs_p01_0p50_icetk.grib2"));
    verify_message(&item.expect, &info).expect("record 63 is the message at its offset");

    for other in ["ENS=+2", "ENS=-1", "ENS=low-res ctl"] {
        let mut expect = item.expect.clone();
        expect.qualifiers = vec![other.to_string()];
        assert_eq!(
            refused_field(verify_message(&expect, &info)),
            ("ensemble", other.to_string())
        );
    }
    let mut bare = item.expect.clone();
    bare.qualifiers.clear();
    assert_eq!(
        refused_field(verify_message(&bare, &info)),
        ("ensemble", "none".to_string())
    );
}

/// Every parameter the committed ECMWF index names is one the MARS table
/// checks (#1116): each record is refused on a message whose codes no MARS
/// name has, which a name missing from the table would pass.
#[test]
fn every_ecmwf_parameter_in_the_index_is_checked() {
    let index = ecmwf();
    let r50 = ecmwf_record("r", "pl 50");
    let mut nameless = message(&r50, read("ecmwf_ifs_0p25_r_50hpa.grib2"));
    let MessageIdentity::Grib2 {
        discipline,
        parameter_category,
        parameter_number,
        ..
    } = &mut nameless.identity
    else {
        panic!("a GRIB2 message");
    };
    *discipline = 3;
    *parameter_category = Some(190);
    *parameter_number = Some(190);

    let items = index.items();
    assert_eq!(items.len(), 187);
    for item in items {
        let name = item
            .expect
            .abbreviation
            .clone()
            .expect("every record names its param");
        assert_eq!(
            refused_field(verify_message(&item.expect, &nameless)),
            ("abbreviation", name)
        );
    }
}

/// NBM lines 51 and 68 (#1117): `APCP:surface:0-1 hour acc fcst`, one a
/// probability (`prob >0.254`, template 4.9) and one the amount (template
/// 4.8). Name, level, window and reference time agree; each is refused on the
/// other's bytes, and accepted on its own.
#[test]
fn nbm_probability_and_amount_are_refused_on_each_others_bytes() {
    let idx = nbm();
    let prob = idx.items()[50].clone();
    let amount = idx.items()[67].clone();
    assert_eq!(prob.expect.qualifiers, ["prob >0.254", "prob fcst 255/255"]);
    assert!(amount.expect.qualifiers.is_empty());
    assert_eq!(prob.expect.forecast, amount.expect.forecast);

    let prob_bytes = message(&prob, read("nbm_core_co_apcp_prob.grib2"));
    let amount_bytes = message(&amount, read("nbm_core_co_apcp.grib2"));
    verify_message(&prob.expect, &prob_bytes).expect("line 51 is the message at its offset");
    verify_message(&amount.expect, &amount_bytes).expect("line 68 is the message at its offset");

    assert_eq!(
        refused_field(verify_message(&prob.expect, &amount_bytes)),
        ("probability", "prob >0.254".to_string())
    );
    assert_eq!(
        refused_field(verify_message(&amount.expect, &prob_bytes)),
        ("probability", "none".to_string())
    );

    // Another threshold of the same probability is another message.
    let mut other = prob.expect.clone();
    other.qualifiers[0] = "prob >0.1016".to_string();
    assert_eq!(
        refused_field(verify_message(&other, &prob_bytes)),
        ("probability", "prob >0.1016".to_string())
    );
}

/// HRRR's `APCP:surface:0-0 day acc fcst` line against NBM's one-hour
/// accumulation (#1117): the same name, surface and cycle over another
/// window.
#[test]
fn a_forecast_window_mismatch_is_refused() {
    let amount = nbm().items()[67].clone();
    let info = message(&amount, read("nbm_core_co_apcp.grib2"));

    let hrrr = only(
        &hrrr(),
        &Query::abbreviation("APCP").at_level_text("surface"),
    );
    assert_eq!(hrrr.expect.forecast.as_deref(), Some("0-0 day acc fcst"));
    assert_eq!(hrrr.expect.reference_time, amount.expect.reference_time);
    assert_eq!(
        refused_field(verify_message(&hrrr.expect, &info)),
        ("forecast", "0-0 day acc fcst".to_string())
    );
}

/// The ECMWF `step` is the end of a window: `mx2t3` at step 3 is the
/// maximum over hours 0 to 3, and is not the record for step 6 (#1117).
#[test]
fn an_ecmwf_step_is_the_end_of_the_window() {
    let mx2t3 = ecmwf_record("mx2t3", "sfc");
    assert_eq!(mx2t3.expect.forecast.as_deref(), Some("3"));
    let info = message(&mx2t3, read("ecmwf_ifs_0p25_mx2t3.grib2"));
    let mut later = mx2t3.expect.clone();
    later.forecast = Some("6".to_string());
    assert_eq!(
        refused_field(verify_message(&later, &info)),
        ("forecast", "6".to_string())
    );
}

/// Forecast wording the grammar does not read is not compared (#1117): line
/// 68's bytes against its expectation with the wording NBM's
/// `0-1 hour acc@(fcst,dt=1 hour),missing=0` lines carry.
#[test]
fn unparsed_forecast_wording_is_accepted() {
    let idx = nbm();
    let amount = idx.items()[67].clone();
    let info = message(&amount, read("nbm_core_co_apcp.grib2"));
    let wording = idx
        .items()
        .iter()
        .filter_map(|i| i.expect.forecast.clone())
        .find(|f| f.contains('@'))
        .expect("the sidecar has a nested window");
    assert_eq!(wording, "0-1 hour acc@(fcst,dt=1 hour),missing=0");

    let mut expect = amount.expect.clone();
    expect.forecast = Some(wording);
    verify_message(&expect, &info).expect("unparsed wording is not compared");
}
