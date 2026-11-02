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
//! neighboring record promising another level or parameter, so a
//! `verify_message` that accepted everything fails here too.

use fieldglass::Session;
use fieldglass::api::{MessageFingerprint, MessageInfo};
use fieldglass::fetchplan::{
    EcmwfIndex, Expectation, Manifest, MessageManifest, Mismatch, NoResolver, PlanItem, Query,
    Wgrib2Idx, verify_envelope, verify_message,
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
    assert_eq!(item.expectation.abbreviation.as_deref(), Some("RH"));
    assert_eq!(item.expectation.level.as_deref(), Some("2 mb"));

    let info = message(&item, read("gfs_0p25_rh_2mb.grib2"));
    verify_message(&item.expectation, &info).expect("record 107 is the message at its offset");
}

/// The same bytes against the record for another level of the same
/// parameter: the check still refuses a wrong level.
#[test]
fn gfs_record_107_is_not_relative_humidity_at_3_mb() {
    let idx = gfs();
    let info = message(&idx.items()[106], read("gfs_0p25_rh_2mb.grib2"));

    let other = only(&idx, &Query::abbreviation("RH").at_level_text("3 mb"));
    assert_eq!(
        refused_field(verify_message(&other.expectation, &info)),
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
        refused_field(verify_message(&other.expectation, &info)),
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
    verify_message(&item.expectation, &info).expect("the record is the message at its offset");
}

/// ECMWF `mx2t3` on `sfc`, against its 224 bytes. The message is on WMO
/// surface 103 at 2 m: MARS `sfc` is every single-level field, so this is the
/// pair that a reading of `sfc` as "the ground" would refuse.
#[test]
fn an_ecmwf_single_level_record_is_what_its_index_line_promised() {
    let index = ecmwf();
    let item = only(&index, &Query::abbreviation("mx2t3"));
    assert_eq!(item.expectation.level.as_deref(), Some("sfc"));
    let info = message(&item, read("ecmwf_ifs_0p25_mx2t3.grib2"));
    verify_message(&item.expectation, &info).expect("the record is the message at its offset");
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
        refused_field(verify_message(&r10.expectation, &info)),
        ("level", "pl 10".to_string())
    );

    let mx2t3 = only(&index, &Query::abbreviation("mx2t3"));
    assert_eq!(
        refused_field(verify_message(&mx2t3.expectation, &info)),
        ("abbreviation", "mx2t3".to_string())
    );
    // With the name out of the way, `sfc` still does not hold for 50 hPa.
    let mut unnamed = mx2t3.expectation.clone();
    unnamed.abbreviation = None;
    assert_eq!(
        refused_field(verify_message(&unnamed, &info)),
        ("level", "sfc".to_string())
    );
}

/// GEFS-aerosol record 22, `PMTC:surface:anl:aerosol=Dust dry:aerosol_size
/// <1e-05:`, against the 216,548 bytes at its offset (#1126). Its §4 is
/// template 4.48 (optical properties of aerosol), which the reader parses
/// since #1130, so every part of the line is compared: the name, the level,
/// the analysis, the aerosol type and the size range.
///
/// eccodes 2.34.1 (`grib_dump -O`) reads the message as category 13, number
/// 192, `constituentType = 62001`, `typeOfSizeInterval = 0` with first size
/// 10 × 10⁻⁶ m, no wavelength range (`typeOfWavelengthInterval = 255`),
/// `typeOfFirstFixedSurface = 1` and the second surface missing.
#[test]
fn a_gefs_aerosol_record_is_what_its_idx_line_promised() {
    let idx = gefs_chem();
    let item = idx.items()[21].clone();
    assert_eq!(item.expectation.abbreviation.as_deref(), Some("PMTC"));
    assert_eq!(item.expectation.level.as_deref(), Some("surface"));
    assert_eq!(
        item.expectation.qualifiers,
        ["aerosol=Dust dry", "aerosol_size <1e-05"]
    );

    let info = message(&item, read("gefs_chem_0p25_pmtc_sfc.grib2"));
    let MessageFingerprint::Grib2 {
        product_template,
        parameter_category,
        parameter_number,
        first_surface,
        second_surface,
        product_octets,
        ..
    } = &info.fingerprint
    else {
        panic!("a GRIB2 message");
    };
    assert_eq!(*product_template, 48);
    assert_eq!(
        (*parameter_category, *parameter_number),
        (Some(13), Some(192))
    );
    assert_eq!(info.abbreviation.as_deref(), Some("PMTC"));
    assert_eq!(first_surface.map(|s| s.surface_type), Some(1));
    assert_eq!(second_surface.map(|s| s.surface_type), Some(255));
    // §4 octets 12-35 as written: 62001 (0xf231), the size range (type 0,
    // scale 6, value 10, then a second limit of 0 × 10⁰) and the wavelength
    // range of type 255, missing.
    assert_eq!(
        product_octets.as_deref(),
        Some(concat!(
            "f231",
            "00060000000a0000000000",
            "ff00000000000000000000"
        ))
    );

    verify_message(&item.expectation, &info).expect("record 22 is the message at its offset");
}

/// Record 22's bytes against the other lines of its cycle that share its
/// surface: each is another aerosol field the same `.idx` lists, and each is
/// refused on what tells it apart (#1130). Before, a 4.48 message was checked
/// on its reference time alone and every one of these passed.
#[test]
fn a_gefs_aerosol_record_is_refused_for_its_neighbors_lines() {
    let idx = gefs_chem();
    let info = message(&idx.items()[21], read("gefs_chem_0p25_pmtc_sfc.grib2"));
    let refused = |line: usize| {
        let item = &idx.items()[line - 1];
        assert_eq!(
            item.expectation.level.as_deref(),
            Some("surface"),
            "line {line}"
        );
        refused_field(verify_message(&item.expectation, &info))
    };
    // 21 `PMTF:surface:anl:aerosol=Dust dry:aerosol_size <2.5e-06`: the fine
    // particulate matter of the same aerosol.
    assert_eq!(refused(21), ("abbreviation", "PMTF".to_string()));
    // 24 `PMTC:surface:anl:aerosol=Total aerosol:aerosol_size <1e-05`: the same
    // name, level, cycle and size; only the aerosol type differs.
    assert_eq!(
        refused(24),
        ("constituent", "aerosol=Total aerosol".to_string())
    );
    // 23 and 25, `PMTF` of sea salt and of the total.
    assert_eq!(refused(23).0, "abbreviation");
    assert_eq!(refused(25).0, "abbreviation");

    // The size range on its own: line 22 promising line 21's size.
    let mut finer = idx.items()[21].expectation.clone();
    finer.qualifiers = vec!["aerosol=Dust dry".into(), "aerosol_size <2.5e-06".into()];
    assert_eq!(
        refused_field(verify_message(&finer, &info)),
        ("aerosolSize", "<2.5e-06".to_string())
    );
    // A wavelength range the message does not have, as the optical-depth
    // lines carry.
    let mut optical = idx.items()[21].expectation.clone();
    optical
        .qualifiers
        .push("aerosol_wavelength >=5.45e-07,<=5.65e-07".into());
    assert_eq!(
        refused_field(verify_message(&optical, &info)).0,
        "aerosolWavelength"
    );
    // And a column line's level: record 26 is `COLMD:entire atmosphere`.
    let column = &idx.items()[25];
    assert_eq!(
        refused_field(verify_message(&column.expectation, &info)).0,
        "abbreviation"
    );
    // Line 22 with line 26's level in its place: the surface alone refuses.
    let mut level = idx.items()[21].expectation.clone();
    level.level.clone_from(&column.expectation.level);
    level.level_spec.clone_from(&column.expectation.level_spec);
    assert_eq!(level.level.as_deref(), Some("entire atmosphere"));
    assert_eq!(refused_field(verify_message(&level, &info)).0, "level");
}

/// The record's reference time is still compared: record 22's line with
/// another cycle's date is refused.
#[test]
fn a_gefs_aerosol_record_from_another_cycle_is_refused() {
    let idx = gefs_chem();
    let item = idx.items()[21].clone();
    let info = message(&item, read("gefs_chem_0p25_pmtc_sfc.grib2"));

    let mut stale = item.expectation.clone();
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

    verify_message(&mn2t3.expectation, &min_bytes).expect("line 13 is the message at its offset");
    assert_eq!(
        refused_field(verify_message(&mx2t3.expectation, &min_bytes)),
        ("abbreviation", "mx2t3".to_string())
    );
    assert_eq!(
        refused_field(verify_message(&mn2t3.expectation, &max_bytes)),
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
        refused_field(verify_message(&t50.expectation, &info)),
        ("abbreviation", "t".to_string())
    );
}

/// ECMWF `10u` and `100u` (#1298): both `sfc`, step 0, and both discipline 0,
/// category 2, number 2. Only the height of the first fixed surface (10 m or
/// 100 m) tells them apart, which is how eccodes does, so a `100u` line on the
/// `10u` bytes is refused.
#[test]
fn ecmwf_100u_is_refused_on_10u_bytes() {
    let u10 = ecmwf_record("10u", "sfc");
    let info = message(&u10, read("ecmwf_ifs_0p25_10u.grib2"));
    verify_message(&u10.expectation, &info).expect("line 33 is the message at its offset");
    let u100 = ecmwf_record("100u", "sfc");
    assert_eq!(
        refused_field(verify_message(&u100.expectation, &info)),
        ("abbreviation", "100u".to_string())
    );
}

/// ECMWF `msl` and `sp` (#1298): both 0.3.0 at step 0, one at mean sea level
/// (surface type 101) and one on the ground (type 1).
#[test]
fn ecmwf_sp_is_refused_on_msl_bytes() {
    let msl = ecmwf_record("msl", "sfc");
    let info = message(&msl, read("ecmwf_ifs_0p25_msl.grib2"));
    verify_message(&msl.expectation, &info).expect("line 3 is the message at its offset");
    let sp = ecmwf_record("sp", "sfc");
    assert_eq!(
        refused_field(verify_message(&sp.expectation, &info)),
        ("abbreviation", "sp".to_string())
    );
}

/// ECMWF `str` and `ttr` (#1298): both 0.5.5 accumulated from step 0, 224
/// bytes each, one at the ground (surface type 1) and one at the top of the
/// atmosphere (type 8).
#[test]
fn ecmwf_ttr_is_refused_on_str_bytes() {
    let str_ = ecmwf_record("str", "sfc");
    let info = message(&str_, read("ecmwf_ifs_0p25_str.grib2"));
    verify_message(&str_.expectation, &info).expect("line 99 is the message at its offset");
    let ttr = ecmwf_record("ttr", "sfc");
    assert_eq!(
        refused_field(verify_message(&ttr.expectation, &info)),
        ("abbreviation", "ttr".to_string())
    );
}

/// ECMWF `2t` on `mx2t3`'s bytes (#1298). `2t`'s eccodes block (the codes
/// and the 2 m surface) matches a 3-hour maximum at 2 m too, but `mx2t3`'s
/// states the process and the range as well, and eccodes names the message
/// after the block that states the most. The `2t` line at step 3, the end of
/// the window, is refused on its name.
#[test]
fn ecmwf_2t_is_refused_on_mx2t3_bytes() {
    let mx2t3 = ecmwf_record("mx2t3", "sfc");
    let info = message(&mx2t3, read("ecmwf_ifs_0p25_mx2t3.grib2"));
    let mut two_t = ecmwf_record("2t", "sfc").expectation;
    two_t.forecast = Some("3".to_string());
    assert_eq!(
        refused_field(verify_message(&two_t, &info)),
        ("abbreviation", "2t".to_string())
    );
}

/// The ECMWF ensemble probability index line for `tpg100` (one line of the
/// same run's `240h-enfo-ep` index) and its 10,947 bytes, template 4.9 (#1298):
/// the probability that 24-hour precipitation exceeds 100 mm. It is 0.1.52
/// accumulated on the ground, as `tp` is; only the probability block (Code
/// Table 4.9 type 3, lower limit 100) makes it `tpg100`. The same line naming
/// `tp`, or another threshold, is refused.
#[test]
fn ecmwf_tp_is_refused_on_a_precipitation_probability() {
    let index = EcmwfIndex::parse(
        "20260904000000-240h-enfo-ep.grib2",
        &text("ecmwf_ifs_0p25_enfo_ep_tpg100.index"),
    )
    .expect("the committed sidecar line parses");
    let tpg100 = index.items()[0].clone();
    assert_eq!(tpg100.expectation.abbreviation.as_deref(), Some("tpg100"));
    let info = message(&tpg100, read("ecmwf_ifs_0p25_tpg100.grib2"));
    verify_message(&tpg100.expectation, &info).expect("the line is the message at its offset");

    for other in ["tp", "tpg50", "tpg25"] {
        let mut expect = tpg100.expectation.clone();
        expect.abbreviation = Some(other.to_string());
        assert_eq!(
            refused_field(verify_message(&expect, &info)),
            ("abbreviation", other.to_string())
        );
    }
}

/// The ECMWF wave ensemble probability line for `swhg8` (one line of the same
/// run's `240h-waef-ep` index) and its 137,974 bytes: template 4.5, wave height
/// 10.0.3 at mean sea level (surface 101), above 8 m. It pins the surface
/// ECMWF writes today, which eccodes 2.48 no longer requires (#1298). Another
/// threshold, and `swh` itself, are refused.
#[test]
fn an_ecmwf_wave_height_probability_is_what_its_line_promised() {
    let index = EcmwfIndex::parse(
        "20260904000000-240h-waef-ep.grib2",
        &text("ecmwf_ifs_0p25_waef_ep_swhg8.index"),
    )
    .expect("the committed sidecar line parses");
    let swhg8 = index.items()[0].clone();
    assert_eq!(swhg8.expectation.abbreviation.as_deref(), Some("swhg8"));
    let info = message(&swhg8, read("ecmwf_ifs_0p25_waef_swhg8.grib2"));
    verify_message(&swhg8.expectation, &info).expect("the line is the message at its offset");

    for other in ["swhg2", "swh"] {
        let mut expect = swhg8.expectation.clone();
        expect.abbreviation = Some(other.to_string());
        assert_eq!(
            refused_field(verify_message(&expect, &info)),
            ("abbreviation", other.to_string())
        );
    }
}

/// ECMWF `sot` on `sol 3` against its 625,288 bytes, which code the layer
/// between soil levels 2 and 3 (surface type 151 twice). eccodes, and so
/// MARS, numbers such a layer by its bottom (#1116). The `sol 1` line is
/// refused, and so is `vsw` on `sol 3`.
#[test]
fn ecmwf_soil_levels_are_compared_by_number() {
    let sol3 = ecmwf_record("sot", "sol 3");
    let info = message(&sol3, read("ecmwf_ifs_0p25_sot_sol3.grib2"));
    verify_message(&sol3.expectation, &info).expect("line 4 is the message at its offset");

    let sol1 = ecmwf_record("sot", "sol 1");
    assert_eq!(
        refused_field(verify_message(&sol1.expectation, &info)),
        ("level", "sol 1".to_string())
    );
    let vsw3 = ecmwf_record("vsw", "sol 3");
    assert_eq!(
        refused_field(verify_message(&vsw3.expectation, &info)),
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
    verify_message(&sol1.expectation, &info).expect("line 69 is the message at its offset");

    let sol2 = ecmwf_record("vsw", "sol 2");
    assert_eq!(
        refused_field(verify_message(&sol2.expectation, &info)),
        ("level", "sol 2".to_string())
    );
    let sot1 = ecmwf_record("sot", "sol 1");
    assert_eq!(
        refused_field(verify_message(&sot1.expectation, &info)),
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
    assert_eq!(item.expectation.abbreviation.as_deref(), Some("ICETK"));
    assert_eq!(item.expectation.qualifiers, ["ENS=+1"]);
    let info = message(&item, read("gefs_p01_0p50_icetk.grib2"));
    verify_message(&item.expectation, &info).expect("record 63 is the message at its offset");

    for other in ["ENS=+2", "ENS=-1", "ENS=low-res ctl"] {
        let mut expect = item.expectation.clone();
        expect.qualifiers = vec![other.to_string()];
        assert_eq!(
            refused_field(verify_message(&expect, &info)),
            ("ensemble", other.to_string())
        );
    }
    let mut bare = item.expectation.clone();
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
    let MessageFingerprint::Grib2 {
        discipline,
        parameter_category,
        parameter_number,
        ..
    } = &mut nameless.fingerprint
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
            .expectation
            .abbreviation
            .clone()
            .expect("every record names its param");
        assert_eq!(
            refused_field(verify_message(&item.expectation, &nameless)),
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
    assert_eq!(
        prob.expectation.qualifiers,
        ["prob >0.254", "prob fcst 255/255"]
    );
    assert!(amount.expectation.qualifiers.is_empty());
    assert_eq!(prob.expectation.forecast, amount.expectation.forecast);

    let prob_bytes = message(&prob, read("nbm_core_co_apcp_prob.grib2"));
    let amount_bytes = message(&amount, read("nbm_core_co_apcp.grib2"));
    verify_message(&prob.expectation, &prob_bytes).expect("line 51 is the message at its offset");
    verify_message(&amount.expectation, &amount_bytes)
        .expect("line 68 is the message at its offset");

    assert_eq!(
        refused_field(verify_message(&prob.expectation, &amount_bytes)),
        ("probability", "prob >0.254".to_string())
    );
    assert_eq!(
        refused_field(verify_message(&amount.expectation, &prob_bytes)),
        ("probability", "none".to_string())
    );

    // Another threshold of the same probability is another message.
    let mut other = prob.expectation.clone();
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
    assert_eq!(
        hrrr.expectation.forecast.as_deref(),
        Some("0-0 day acc fcst")
    );
    assert_eq!(
        hrrr.expectation.reference_time,
        amount.expectation.reference_time
    );
    assert_eq!(
        refused_field(verify_message(&hrrr.expectation, &info)),
        ("forecast", "0-0 day acc fcst".to_string())
    );
}

/// The ECMWF `step` is the end of a window: `mx2t3` at step 3 is the
/// maximum over hours 0 to 3, and is not the record for step 6 (#1117).
#[test]
fn an_ecmwf_step_is_the_end_of_the_window() {
    let mx2t3 = ecmwf_record("mx2t3", "sfc");
    assert_eq!(mx2t3.expectation.forecast.as_deref(), Some("3"));
    let info = message(&mx2t3, read("ecmwf_ifs_0p25_mx2t3.grib2"));
    let mut later = mx2t3.expectation.clone();
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
        .filter_map(|i| i.expectation.forecast.clone())
        .find(|f| f.contains('@'))
        .expect("the sidecar has a nested window");
    assert_eq!(wording, "0-1 hour acc@(fcst,dt=1 hour),missing=0");

    let mut expect = amount.expectation.clone();
    expect.forecast = Some(wording);
    verify_message(&expect, &info).expect("unparsed wording is not compared");
}

/// The one record of a single committed AIFS `.index` line, and the message it
/// points at.
fn aifs_record(object: &str, sidecar: &str, bytes: &str) -> (PlanItem, MessageInfo) {
    let index = EcmwfIndex::parse(object, &text(sidecar)).expect("the committed line parses");
    let item = index.items()[0].clone();
    let info = message(&item, read(bytes));
    (item, info)
}

/// AIFS-single `mcc` (#1323): its `.index` line says `levtype` `sfc`, and its
/// 1,299,114 bytes code the layer from isobaric 80000 Pa to isobaric 45000 Pa.
/// eccodes calls that layer `sfc` (its `marsLevtype` block for it states more
/// conditions than `pl`'s), and so must the level check. Before, the line was
/// refused because 800 hPa is on a surface ECMWF lists levels of.
#[test]
fn an_aifs_medium_cloud_cover_record_is_what_its_index_line_promised() {
    let (mcc, info) = aifs_record(
        "20260904000000-6h-oper-fc.grib2",
        "ecmwf_aifs_single_0p25_oper_mcc.index",
        "ecmwf_aifs_single_0p25_mcc.grib2",
    );
    assert_eq!(mcc.expectation.abbreviation.as_deref(), Some("mcc"));
    assert_eq!(mcc.expectation.level.as_deref(), Some("sfc"));
    verify_message(&mcc.expectation, &info).expect("the line is the message at its offset");

    // The same line naming the high cloud cover is refused on the name.
    let mut hcc = mcc.expectation.clone();
    hcc.abbreviation = Some("hcc".to_string());
    assert_eq!(
        refused_field(verify_message(&hcc, &info)),
        ("abbreviation", "hcc".to_string())
    );
    // And promising a pressure level of the layer, on the level.
    let pl = EcmwfIndex::parse(
        "o",
        r#"{"param": "mcc", "levtype": "pl", "levelist": "800", "_offset": 0, "_length": 1}"#,
    )
    .expect("one record")
    .items()[0]
        .expectation
        .clone();
    assert_eq!(refused_field(verify_message(&pl, &info)).0, "level");
}

/// AIFS-ens control `hcc` (#1323): `sfc` on its line, and its 1,206,307 bytes
/// code the layer from isobaric 45000 Pa to the nominal top of the atmosphere
/// (surface 8), which eccodes calls `sfc` too. The `mcc` name is refused on
/// it, and the line, its name cleared, still does not hold on a plain isobaric
/// level: the IFS `r` on 50 hPa of the same run.
#[test]
fn an_aifs_high_cloud_cover_record_is_what_its_index_line_promised() {
    let (hcc, info) = aifs_record(
        "20260904000000-6h-enfo-cf.grib2",
        "ecmwf_aifs_ens_0p25_cf_hcc.index",
        "ecmwf_aifs_ens_0p25_cf_hcc.grib2",
    );
    assert_eq!(hcc.expectation.abbreviation.as_deref(), Some("hcc"));
    assert_eq!(hcc.expectation.level.as_deref(), Some("sfc"));
    verify_message(&hcc.expectation, &info).expect("the line is the message at its offset");

    let mut mcc = hcc.expectation.clone();
    mcc.abbreviation = Some("mcc".to_string());
    assert_eq!(
        refused_field(verify_message(&mcc, &info)),
        ("abbreviation", "mcc".to_string())
    );

    let r50 = ecmwf_record("r", "pl 50");
    let isobaric = message(&r50, read("ecmwf_ifs_0p25_r_50hpa.grib2"));
    let mut unnamed = hcc.expectation.clone();
    unnamed.abbreviation = None;
    unnamed.forecast.clone_from(&r50.expectation.forecast);
    assert_eq!(
        refused_field(verify_message(&unnamed, &isobaric)),
        ("level", "sfc".to_string())
    );
}

/// The record of `sidecar` (two lines of one ECMWF index) whose `type` is
/// `kind`, and the message `bytes` decode to.
fn ecmwf_typed(sidecar: &str, kind: &str, bytes: Vec<u8>) -> (PlanItem, MessageInfo) {
    let index = EcmwfIndex::parse("o", &text(sidecar)).expect("the committed lines parse");
    let item = index
        .items()
        .into_iter()
        .find(|item| {
            item.expectation
                .qualifiers
                .contains(&format!("type={kind}"))
        })
        .expect("the sidecar has a record of that type");
    let info = message(&item, bytes);
    (item, info)
}

/// `expect` with its `type` qualifier replaced by `kind`, and its `number`
/// replaced by `number` (dropped for `None`).
fn retyped(expect: &Expectation, kind: &str, number: Option<&str>) -> Expectation {
    let mut expect = expect.clone();
    expect
        .qualifiers
        .retain(|q| !q.starts_with("type=") && !q.starts_with("number="));
    expect.qualifiers.push(format!("type={kind}"));
    if let Some(n) = number {
        expect.qualifiers.push(format!("number={n}"));
    }
    expect
}

/// `bytes`, one GRIB2 message, with §4 octet `octet` (1-based) set to `value`:
/// a member's Code Table 4.6 type is octet 35 and its perturbation number
/// octet 36. Every other byte stays as published.
fn with_section4_octet(mut bytes: Vec<u8>, octet: usize, value: u8) -> Vec<u8> {
    let mut at = 16;
    loop {
        let length = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
        if bytes[at + 4] == 4 {
            bytes[at + octet - 1] = value;
            return bytes;
        }
        at += length;
    }
}

/// The product template, Code Table 4.6 type and member of a GRIB2 member
/// forecast, as the message's identity carries them.
fn ensemble_block(info: &MessageInfo) -> Option<(u16, u8, u8)> {
    match &info.fingerprint {
        MessageFingerprint::Grib2 {
            product_template,
            ensemble_type: Some(kind),
            ensemble_member: Some(member),
            ..
        } => Some((*product_template, *kind, *member)),
        _ => None,
    }
}

/// The one message in `bytes`, with no sidecar range to check them against.
fn open(bytes: Vec<u8>) -> MessageInfo {
    Session::open(bytes)
        .and_then(|s| s.message(0))
        .expect("the message opens")
}

fn ifs_members() -> EcmwfIndex {
    EcmwfIndex::parse(
        "20260904000000-6h-enfo-ef.grib2",
        &text("ecmwf_ifs_0p25_enfo_ef_pf.index"),
    )
    .expect("the committed lines parse")
}

/// IFS perturbed members (#1304): `asn` member 40 at step 6 (template 4.1)
/// and `ro` member 48 over hours 0 to 6 (4.11). ECMWF writes both with the
/// Code Table 4.6 type missing (255) and the member as the perturbation
/// number. Each passes its own `type=pf` line, and the same line calling it
/// the control, a mean, a standard deviation or a probability is refused on
/// its type.
#[test]
fn ecmwf_perturbed_members_are_compared_by_type() {
    for (param, bytes) in [
        ("asn", "ecmwf_ifs_0p25_pf_asn_40.grib2"),
        ("ro", "ecmwf_ifs_0p25_pf_ro_48.grib2"),
    ] {
        let item = only(&ifs_members(), &Query::abbreviation(param));
        let info = message(&item, read(bytes));
        verify_message(&item.expectation, &info).expect("the line is the message at its offset");

        for other in ["cf", "em", "es", "ep"] {
            assert_eq!(
                refused_field(verify_message(
                    &retyped(&item.expectation, other, None),
                    &info
                )),
                ("type", format!("type={other}")),
                "{param} as {other}"
            );
        }
        // A type this does not read is not compared.
        verify_message(&retyped(&item.expectation, "cm", None), &info)
            .expect("an unread type is not compared");
    }
}

/// The IFS control of `ro` over hours 0 to 6 of the 2025-01-01 00Z run, the
/// record of its committed `.index` line, and that line.
fn ifs_control() -> (PlanItem, MessageInfo) {
    aifs_record(
        "20250101000000-6h-enfo-ef.grib2",
        "ecmwf_ifs_0p25_enfo_ef_cf.index",
        "ecmwf_ifs_0p25_cf_ro.grib2",
    )
}

/// The control (#1304). AIFS-ens writes `hcc` on template 4.1 with Code
/// Table 4.6 type 5 (unperturbed) and member 0. IFS writes its control as it
/// writes its members, type missing (255), with perturbation number 0: the
/// published `ro` control (template 4.11, #1552). Each is a `cf` and not a
/// `pf`, and the `asn` member 40 bytes as published are not a `cf`.
#[test]
fn an_ecmwf_control_is_compared_by_type() {
    let (hcc, info) = aifs_record(
        "20260904000000-6h-enfo-cf.grib2",
        "ecmwf_aifs_ens_0p25_cf_hcc.index",
        "ecmwf_aifs_ens_0p25_cf_hcc.grib2",
    );
    assert!(hcc.expectation.qualifiers.contains(&"type=cf".to_string()));
    verify_message(&hcc.expectation, &info).expect("the line is the message at its offset");
    for other in ["pf", "em", "ep"] {
        assert_eq!(
            refused_field(verify_message(
                &retyped(&hcc.expectation, other, None),
                &info
            )),
            ("type", format!("type={other}"))
        );
    }

    let (ro, control) = ifs_control();
    assert!(ro.expectation.qualifiers.contains(&"type=cf".to_string()));
    assert_eq!(
        ensemble_block(&control),
        Some((11, 255, 0)),
        "the published control is template 4.11, type missing, member 0"
    );
    verify_message(&ro.expectation, &control).expect("type 255, member 0, is the control");
    for other in ["em", "es", "ep"] {
        assert_eq!(
            refused_field(verify_message(
                &retyped(&ro.expectation, other, None),
                &control
            )),
            ("type", format!("type={other}"))
        );
    }
    assert_eq!(
        refused_field(verify_message(
            &retyped(&ro.expectation, "pf", Some("0")),
            &control
        )),
        ("type", "type=pf".to_string())
    );

    let asn = only(&ifs_members(), &Query::abbreviation("asn"));
    let cf = retyped(&asn.expectation, "cf", None);
    let member = message(&asn, read("ecmwf_ifs_0p25_pf_asn_40.grib2"));
    assert_eq!(
        refused_field(verify_message(&cf, &member)),
        ("type", "type=cf".to_string())
    );
}

/// IFS ensemble mean and standard deviation of 250 hPa wind speed at step
/// 228 (#1304), both template 4.2: the mean is Code Table 4.7 code 0 and the
/// standard deviation code 2, where eccodes would encode 4. Each is refused
/// on the other's bytes, which differ in nothing else the line states.
#[test]
fn ecmwf_ensemble_mean_and_standard_deviation_are_refused_on_each_others_bytes() {
    let sidecar = "ecmwf_ifs_0p25_enfo_ep_ws250.index";
    let (em, mean) = ecmwf_typed(sidecar, "em", read("ecmwf_ifs_0p25_em_ws250.grib2"));
    let (es, spread) = ecmwf_typed(sidecar, "es", read("ecmwf_ifs_0p25_es_ws250.grib2"));
    verify_message(&em.expectation, &mean).expect("the em line is its message");
    verify_message(&es.expectation, &spread).expect("the es line is its message");

    assert_eq!(
        refused_field(verify_message(&em.expectation, &spread)),
        ("type", "type=em".to_string())
    );
    assert_eq!(
        refused_field(verify_message(&es.expectation, &mean)),
        ("type", "type=es".to_string())
    );
    for other in ["cf", "pf", "ep"] {
        assert_eq!(
            refused_field(verify_message(
                &retyped(&em.expectation, other, None),
                &mean
            )),
            ("type", format!("type={other}"))
        );
    }
}

/// The `tpg100` event probability (template 4.9) passes its `type=ep` line
/// and is refused as a control, a member, a mean or a standard deviation
/// (#1304).
#[test]
fn an_ecmwf_event_probability_is_compared_by_type() {
    let index = EcmwfIndex::parse(
        "20260904000000-240h-enfo-ep.grib2",
        &text("ecmwf_ifs_0p25_enfo_ep_tpg100.index"),
    )
    .expect("the committed line parses");
    let item = index.items()[0].clone();
    let info = message(&item, read("ecmwf_ifs_0p25_tpg100.grib2"));
    verify_message(&item.expectation, &info).expect("the line is the message at its offset");
    for other in ["cf", "pf", "em", "es"] {
        assert_eq!(
            refused_field(verify_message(
                &retyped(&item.expectation, other, None),
                &info
            )),
            ("type", format!("type={other}"))
        );
    }
}

/// A one-record wgrib2 `.idx` line, in the form wgrib2 prints, for an IFS
/// ensemble message at step 6 (the `asn` member) or over hours 0 to 6 (the
/// `ro` control), ending in the qualifier `q`. wgrib2 itself was not
/// available to run, so the line is written by hand.
fn wgrib2_line(info: &MessageInfo, q: &str) -> Expectation {
    let date: String = info
        .reference_time
        .chars()
        .filter(char::is_ascii_digit)
        .take(10)
        .collect();
    let forecast = match ensemble_block(info) {
        Some((11, ..)) => "0-6 hour acc fcst",
        _ => "6 hour fcst",
    };
    // `ro` is ECMWF's local 2.0.201, which wgrib2's tables do not name, so
    // it prints the codes instead.
    let name = match (&info.abbreviation, &info.fingerprint) {
        (Some(name), _) => name.clone(),
        (
            None,
            MessageFingerprint::Grib2 {
                discipline,
                parameter_category: Some(category),
                parameter_number: Some(number),
                ..
            },
        ) => format!(
            "var discipline={discipline} center=98 local_table=0 parmcat={category} parm={number}"
        ),
        _ => panic!("a GRIB2 parameter"),
    };
    let text = format!("1:0:d={date}:{name}:surface:{forecast}:{q}");
    Wgrib2Idx::parse("o", &text).expect("one record").items()[0]
        .expectation
        .clone()
}

/// A wgrib2 `ENS=` over an ECMWF member written with the Code Table 4.6 type
/// missing (#1304). wgrib2's `f_ens` (`src/Ensemble.c`) prints such a member
/// as type 1 (`low-res ctl`) for member 0 and type 3 (`+n`) otherwise, and an
/// older wgrib2 as `ENS=? table4.6=255 pert=n`; any other form is refused.
/// The member is the published `asn` member 40 and the control the published
/// `ro` control (#1552). Another centre's type 255 is printed as it is: GEFS
/// member 1 with its type set to 255 is `ENS=? table4.6=255 pert=1`, not
/// `ENS=+1`.
#[test]
fn a_wgrib2_member_with_the_type_missing_is_compared_as_wgrib2_prints_it() {
    let check = |info: &MessageInfo, q: &str| verify_message(&wgrib2_line(info, q), info);
    let member = open(read("ecmwf_ifs_0p25_pf_asn_40.grib2"));
    let (_, control) = ifs_control();

    for q in ["ENS=+40", "ENS=? table4.6=255 pert=40"] {
        check(&member, q).unwrap_or_else(|e| panic!("{q}: {e}"));
    }
    for q in [
        "ENS=+39",
        "ENS=-40",
        "ENS=low-res ctl",
        "ENS=hi-res ctl",
        "P-ENS=40",
    ] {
        assert_eq!(
            refused_field(check(&member, q)),
            ("ensemble", q.to_string())
        );
    }
    for q in ["ENS=low-res ctl", "ENS=? table4.6=255 pert=0"] {
        check(&control, q).unwrap_or_else(|e| panic!("{q}: {e}"));
    }
    for q in ["ENS=hi-res ctl", "ENS=+0"] {
        assert_eq!(
            refused_field(check(&control, q)),
            ("ensemble", q.to_string())
        );
    }

    let gefs = gefs_member().items()[62].clone();
    let missing = open(with_section4_octet(
        read("gefs_p01_0p50_icetk.grib2"),
        35,
        255,
    ));
    let gefs_line = |q: &str| {
        let mut expect = gefs.expectation.clone();
        expect.qualifiers = vec![q.to_string()];
        expect
    };
    verify_message(&gefs_line("ENS=? table4.6=255 pert=1"), &missing)
        .expect("wgrib2 prints another centre's missing type as it is");
    assert_eq!(
        refused_field(verify_message(&gefs_line("ENS=+1"), &missing)),
        ("ensemble", "ENS=+1".to_string())
    );
}

/// `info` sent through JSON, as a host that keeps a `MessageInfo` holds it,
/// with `edit` applied to the JSON object first.
fn through_json(
    info: &MessageInfo,
    edit: impl FnOnce(&mut serde_json::Map<String, serde_json::Value>),
) -> MessageInfo {
    let mut json = serde_json::to_value(info).expect("a MessageInfo serializes");
    edit(json.as_object_mut().expect("a MessageInfo is an object"));
    serde_json::from_value(json).expect("the edited MessageInfo reads back")
}

/// ECMWF is recognized by its centre number, 98, not by the name a table
/// gives it (#1552). The `asn` member 40 and `ro` control are both written
/// with the Code Table 4.6 type missing, which wgrib2 rewrites only for
/// ECMWF. Under another name, as a later version's table might word it, they
/// still pass the rewritten forms; as centre 7 under ECMWF's name, they do
/// not.
#[test]
fn ecmwf_is_recognized_by_its_centre_number() {
    let check = |info: &MessageInfo, q: &str| verify_message(&wgrib2_line(info, q), info);
    let member = open(read("ecmwf_ifs_0p25_pf_asn_40.grib2"));
    let (_, control) = ifs_control();
    for (info, q) in [(&member, "ENS=+40"), (&control, "ENS=low-res ctl")] {
        assert_eq!(info.originating_centre_code, Some(98), "{q}");
        let renamed = through_json(info, |o| {
            o.insert("originatingCentre".into(), "ECMWF".into());
        });
        check(&renamed, q).unwrap_or_else(|e| panic!("{q} under another name: {e}"));

        let elsewhere = through_json(info, |o| {
            o.insert("originatingCentreCode".into(), 7.into());
        });
        assert_eq!(
            refused_field(check(&elsewhere, q)),
            ("ensemble", q.to_string()),
            "centre 7 under ECMWF's name"
        );
    }
}

/// A `MessageInfo` kept from a version before `originatingCentreCode` reads
/// back with none, and is checked as that version checked it: ECMWF by the
/// centre's name (#1552).
#[test]
fn a_message_info_without_the_centre_number_verifies_as_before() {
    let check = |info: &MessageInfo, q: &str| verify_message(&wgrib2_line(info, q), info);
    let old = |info: &MessageInfo| {
        through_json(info, |o| {
            assert!(o.remove("originatingCentreCode").is_some());
        })
    };
    let member = old(&open(read("ecmwf_ifs_0p25_pf_asn_40.grib2")));
    let (ro, control) = ifs_control();
    let control = old(&control);
    assert_eq!(member.originating_centre_code, None);
    assert_eq!(control.originating_centre_code, None);

    for q in ["ENS=+40", "ENS=? table4.6=255 pert=40"] {
        check(&member, q).unwrap_or_else(|e| panic!("{q}: {e}"));
    }
    assert_eq!(
        refused_field(check(&member, "ENS=+39")),
        ("ensemble", "ENS=+39".to_string())
    );
    check(&control, "ENS=low-res ctl").expect("the control, by ECMWF's name");
    verify_message(&ro.expectation, &control).expect("the cf line is its message");

    // The name is what decides there: under another, the rewritten form is
    // another centre's and is refused.
    let renamed = through_json(&member, |o| {
        o.insert("originatingCentre".into(), "ECMWF".into());
    });
    assert_eq!(
        refused_field(check(&renamed, "ENS=+40")),
        ("ensemble", "ENS=+40".to_string())
    );
}

/// The IFS operational `asn` at step 6 of the 2026-09-04 00Z run, the record
/// of its committed `.index` line (stream `oper`, type `fc`), and that line.
/// Template 4.0, no ensemble block.
fn oper_asn() -> (PlanItem, MessageInfo) {
    aifs_record(
        "20260904000000-6h-oper-fc.grib2",
        "ecmwf_ifs_0p25_oper_fc_asn.index",
        "ecmwf_ifs_0p25_fc_asn.grib2",
    )
}

/// The `tpg100` event probability (template 4.9) and its `type=ep` line.
fn ifs_probability() -> (PlanItem, MessageInfo) {
    aifs_record(
        "20260904000000-240h-enfo-ep.grib2",
        "ecmwf_ifs_0p25_enfo_ep_tpg100.index",
        "ecmwf_ifs_0p25_tpg100.grib2",
    )
}

/// An `fc` is a single forecast, with no ensemble block (#1553). The
/// operational `asn` line at step 6 passes its own bytes and is refused on
/// the same run's ensemble member 40 of `asn` at step 6 (template 4.1), which
/// agrees with it on name, level and step. A line calling a member, the
/// control, a mean, a standard deviation or a probability `fc` is refused
/// too.
#[test]
fn an_ecmwf_forecast_is_refused_on_ensemble_bytes() {
    let (fc, oper) = oper_asn();
    assert!(fc.expectation.qualifiers.contains(&"type=fc".to_string()));
    assert!(
        ensemble_block(&oper).is_none(),
        "the oper message is no member"
    );
    verify_message(&fc.expectation, &oper).expect("the line is the message at its offset");

    let asn = only(&ifs_members(), &Query::abbreviation("asn"));
    let member = message(&asn, read("ecmwf_ifs_0p25_pf_asn_40.grib2"));
    assert_eq!(ensemble_block(&member), Some((1, 255, 40)));
    assert_eq!(
        refused_field(verify_message(&fc.expectation, &member)),
        ("type", "type=fc".to_string())
    );

    let ro = only(&ifs_members(), &Query::abbreviation("ro"));
    let ro_member = message(&ro, read("ecmwf_ifs_0p25_pf_ro_48.grib2"));
    let (cf, control) = ifs_control();
    let ws250 = "ecmwf_ifs_0p25_enfo_ep_ws250.index";
    let (em, mean) = ecmwf_typed(ws250, "em", read("ecmwf_ifs_0p25_em_ws250.grib2"));
    let (es, spread) = ecmwf_typed(ws250, "es", read("ecmwf_ifs_0p25_es_ws250.grib2"));
    let (ep, probability) = ifs_probability();
    for (expect, info, what) in [
        (&ro.expectation, &ro_member, "member 48, template 4.11"),
        (&cf.expectation, &control, "the control, type 255, member 0"),
        (&em.expectation, &mean, "the mean, template 4.2"),
        (
            &es.expectation,
            &spread,
            "the standard deviation, template 4.2",
        ),
        (&ep.expectation, &probability, "a probability, template 4.9"),
    ] {
        assert_eq!(
            refused_field(verify_message(&retyped(expect, "fc", None), info)),
            ("type", "type=fc".to_string()),
            "{what}"
        );
    }
}

/// An `an` is held as an `fc` is (#1553). No committed `.index` line is an
/// analysis, so these lines are published ones retyped `an`: the operational
/// `asn` at step 6 and `r` on 50 hPa at step 0 pass their own bytes, and the
/// `asn` member 40, the `ws250` mean and the `tpg100` probability are
/// refused.
#[test]
fn an_ecmwf_analysis_is_refused_on_ensemble_bytes() {
    let (fc, oper) = oper_asn();
    verify_message(&retyped(&fc.expectation, "an", None), &oper)
        .expect("no ensemble block, so an analysis can be it");
    let r50 = ecmwf_record("r", "pl 50");
    let isobaric = message(&r50, read("ecmwf_ifs_0p25_r_50hpa.grib2"));
    verify_message(&retyped(&r50.expectation, "an", None), &isobaric)
        .expect("no ensemble block, so an analysis can be it");

    let asn = only(&ifs_members(), &Query::abbreviation("asn"));
    let member = message(&asn, read("ecmwf_ifs_0p25_pf_asn_40.grib2"));
    let (em, mean) = ecmwf_typed(
        "ecmwf_ifs_0p25_enfo_ep_ws250.index",
        "em",
        read("ecmwf_ifs_0p25_em_ws250.grib2"),
    );
    let (ep, probability) = ifs_probability();
    for (expect, info, what) in [
        (&asn.expectation, &member, "member 40, template 4.1"),
        (&em.expectation, &mean, "the mean, template 4.2"),
        (&ep.expectation, &probability, "a probability, template 4.9"),
    ] {
        assert_eq!(
            refused_field(verify_message(&retyped(expect, "an", None), info)),
            ("type", "type=an".to_string()),
            "{what}"
        );
    }
}

/// An `fc` or `an` that states a `number` is a member of an ensemble stream
/// MARS types that way, such as the ensemble of data assimilations (`enda`,
/// `elda`), which eccodes encodes on 4.1 or 4.11 (#1553). It is not refused
/// for a member, whose number is compared instead, but is still refused for
/// a probability. The lines are the published `asn` member and `tpg100`
/// lines retyped.
#[test]
fn a_numbered_ecmwf_forecast_or_analysis_can_be_a_member() {
    let asn = only(&ifs_members(), &Query::abbreviation("asn"));
    let member = message(&asn, read("ecmwf_ifs_0p25_pf_asn_40.grib2"));
    for kind in ["fc", "an"] {
        verify_message(&retyped(&asn.expectation, kind, Some("40")), &member)
            .unwrap_or_else(|e| panic!("{kind} member 40: {e}"));
        assert_eq!(
            refused_field(verify_message(
                &retyped(&asn.expectation, kind, Some("39")),
                &member
            )),
            ("ensemble", "number=39".to_string()),
            "{kind}"
        );
        // A number that does not read as a member's makes it no member.
        for unread in ["abc", "300", "40.0"] {
            assert_eq!(
                refused_field(verify_message(
                    &retyped(&asn.expectation, kind, Some(unread)),
                    &member
                )),
                ("type", format!("type={kind}")),
                "{kind} number={unread}"
            );
        }
    }

    // Numbered, on a mean, it is refused on its type before its number.
    let (em, mean) = ecmwf_typed(
        "ecmwf_ifs_0p25_enfo_ep_ws250.index",
        "em",
        read("ecmwf_ifs_0p25_em_ws250.grib2"),
    );
    assert_eq!(
        refused_field(verify_message(
            &retyped(&em.expectation, "fc", Some("40")),
            &mean
        )),
        ("type", "type=fc".to_string())
    );

    let (ep, probability) = ifs_probability();
    assert_eq!(
        refused_field(verify_message(
            &retyped(&ep.expectation, "an", Some("0")),
            &probability
        )),
        ("type", "type=an".to_string())
    );
}
