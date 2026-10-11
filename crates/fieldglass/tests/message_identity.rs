//! What makes a GRIB message the same message in another copy of its file
//! (#959), through the real readers over committed fixtures.
//!
//! A render panel outlives the editor it was opened from, so the file under it
//! can change. The panel keeps the message's `MessageInfo::identity` and looks
//! for it in the new file: one match is the message, none or several is a
//! refusal. These tests hold the identity to that use: a message moved to
//! another position is found there, a file that holds it twice says so, and
//! two different fields never share one.

use fieldglass::{MessageIdentity, Session};

const GRIB2: &str = "../fieldglass-grib2/tests/fixtures";
const GRIB1: &str = "../fieldglass-grib1/tests/fixtures";

fn read(dir: &str, name: &str) -> Vec<u8> {
    let path = format!("{dir}/{name}");
    std::fs::read(&path).unwrap_or_else(|e| panic!("read {path}: {e}"))
}

/// Every message's identity, in file order.
fn identities(bytes: Vec<u8>) -> Vec<MessageIdentity> {
    let session = Session::open(bytes).expect("opens");
    (0..session.count())
        .map(|i| session.message(i).expect("a message").identity)
        .collect()
}

/// The positions in `of` whose identity is `wanted`.
fn matches(of: &[MessageIdentity], wanted: &MessageIdentity) -> Vec<usize> {
    of.iter()
        .enumerate()
        .filter(|(_, id)| *id == wanted)
        .map(|(i, _)| i)
        .collect()
}

fn concat(parts: &[&[u8]]) -> Vec<u8> {
    parts.concat()
}

/// A message moved to another position in a rewritten file is found there, and
/// only there; so is the one that took its place.
#[test]
fn a_message_that_moved_is_found_where_it_now_sits() {
    let surface = read(GRIB2, "regular_latlon_surface.grib2");
    let lambert = read(GRIB2, "eta_lambert_msg0.grib2");
    let before = identities(concat(&[&surface, &lambert]));
    let after = identities(concat(&[&lambert, &surface]));
    assert_eq!(before.len(), 2);
    assert_ne!(before[0], before[1], "two different fields");
    assert_eq!(matches(&after, &before[0]), vec![1]);
    assert_eq!(matches(&after, &before[1]), vec![0]);
}

/// A file that holds the same message twice matches it twice, which a host
/// must read as ambiguous rather than take the first.
#[test]
fn a_message_held_twice_matches_twice() {
    let surface = read(GRIB2, "regular_latlon_surface.grib2");
    let once = identities(surface.clone());
    let twice = identities(concat(&[&surface, &surface]));
    assert_eq!(matches(&twice, &once[0]), vec![0, 1]);
}

/// A message the new file does not hold is not found, even at the position it
/// had, when another field took that position.
#[test]
fn a_message_that_is_gone_matches_nothing() {
    let surface = read(GRIB2, "regular_latlon_surface.grib2");
    let lambert = read(GRIB2, "eta_lambert_msg0.grib2");
    let before = identities(surface);
    let after = identities(lambert);
    assert!(matches(&after, &before[0]).is_empty());
}

/// The same field written with another packing is the same message: the
/// identity says which field a message holds, not how its values are stored.
#[test]
fn the_same_field_packed_another_way_is_the_same_message() {
    let complex = identities(read(GRIB2, "complex_regular_latlon.grib2"));
    let ccsds = identities(read(GRIB2, "ccsds_regular_latlon.grib2"));
    assert_eq!(complex, ccsds);
}

/// The identity is read from the codes, as the message writes them. Each field
/// below is checked against the fixture's own header, so a builder that read
/// the wrong octet would fail here rather than in a panel.
#[test]
fn a_grib2_identity_is_the_messages_codes() {
    let ids = identities(read(GRIB2, "regular_latlon_surface.grib2"));
    let MessageIdentity::Grib2 {
        discipline,
        parameter_category,
        parameter_number,
        first_surface,
        reference_time,
        forecast_time,
        range_end,
        statistics,
        grid_template,
        dimensions,
        data_points,
        ..
    } = &ids[0]
    else {
        panic!("a GRIB2 message gives a GRIB2 identity: {:?}", ids[0]);
    };
    // 2 m temperature (0/0/0) on a regular lat/lon grid (3.0), an analysis.
    assert_eq!(
        (*discipline, *parameter_category, *parameter_number),
        (0, Some(0), Some(0))
    );
    assert_eq!(first_surface.map(|s| s.surface_type), Some(103));
    assert_eq!(reference_time, "2008-02-06T12:00:00Z");
    assert_eq!(*forecast_time, Some(0));
    assert_eq!((range_end, statistics.len()), (&None, 0));
    assert_eq!(*grid_template, 0);
    assert_eq!(*dimensions, Some([16, 31]));
    assert_eq!(*data_points, 16 * 31);
}

/// A GRIB1 parameter from a centre's own table names the centre; one from a
/// WMO table does not, because it means the same from any centre.
#[test]
fn a_grib1_parameter_names_its_centre_only_when_the_table_is_local() {
    for (name, local) in [
        ("cmc_wind_300_2010052400_p012.grib", false),
        ("reduced_gg_n32.grib1", true),
    ] {
        let ids = identities(read(GRIB1, name));
        let MessageIdentity::Grib1 {
            table_version,
            parameter,
            local_centre,
            ..
        } = &ids[0]
        else {
            panic!("{name}: a GRIB1 message gives a GRIB1 identity");
        };
        assert_eq!(
            local_centre.is_some(),
            local,
            "{name}: table {table_version}, parameter {parameter}, centre {local_centre:?}"
        );
    }
}

/// `regular_latlon_surface.grib2` with its §4 rewritten to product template
/// `template`, parameter number `parameter`, and `tail` appended after octet
/// 34, the end of 4.0's layout. §4's length and §0's total length follow.
fn with_template(template: u16, parameter: u8, tail: &[u8]) -> Vec<u8> {
    let bytes = read(GRIB2, "regular_latlon_surface.grib2");
    let mut at = 16;
    loop {
        let len = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
        if bytes[at + 4] == 4 {
            assert_eq!(
                len, 34,
                "the fixture's §4 is template 4.0 with no coordinates"
            );
            let mut section = bytes[at..at + len].to_vec();
            section[7..9].copy_from_slice(&template.to_be_bytes());
            section[10] = parameter;
            section.extend_from_slice(tail);
            let new_len = section.len() as u32;
            section[0..4].copy_from_slice(&new_len.to_be_bytes());
            let mut out = [&bytes[..at], &section[..], &bytes[at + len..]].concat();
            let total = out.len() as u64;
            out[8..16].copy_from_slice(&total.to_be_bytes());
            return out;
        }
        at += len;
    }
}

/// A template with 4.0's layout but no type of its own in the reader (here
/// 4.1, an ensemble member) is read through octet 34 (#959). Before, its
/// parameter, level and step were absent from the identity, so a file whose
/// one message changed parameter at the same time and grid still matched,
/// and a panel drew the new parameter as the old one.
#[test]
fn a_shared_layout_template_tells_parameters_apart() {
    let temperature = identities(with_template(1, 0, &[3, 1, 20]));
    let potential = identities(with_template(1, 2, &[3, 1, 20]));
    assert!(matches!(
        temperature[0],
        MessageIdentity::Grib2 {
            product_template: 1,
            parameter_number: Some(0),
            ..
        }
    ));
    assert!(
        matches(&potential, &temperature[0]).is_empty(),
        "another parameter"
    );
    // The same message, rewritten into another file, is still the message.
    let again = identities(concat(&[
        &with_template(1, 2, &[3, 1, 20]),
        &with_template(1, 0, &[3, 1, 20]),
    ]));
    assert_eq!(matches(&again, &temperature[0]), vec![1]);
}

/// The block a shared-layout template adds after octet 34 is compared as
/// written where it says which field this is: a 50th and a 90th percentile
/// (4.6) of one parameter are two messages.
#[test]
fn a_percentile_is_part_of_the_identity() {
    let median = identities(with_template(6, 0, &[50]));
    let p90 = identities(with_template(6, 0, &[90]));
    assert!(matches(&p90, &median[0]).is_empty());
    assert_eq!(
        matches(&identities(with_template(6, 0, &[50])), &median[0]),
        vec![0]
    );
}

/// A template the reader does not parse at all keeps every octet in the
/// identity, so two of them differing anywhere, the parameter included, do
/// not match, and two identical ones do.
#[test]
fn an_unparsed_template_is_compared_by_its_octets() {
    let a = identities(with_template(99, 0, &[]));
    let b = identities(with_template(99, 2, &[]));
    let MessageIdentity::Grib2 {
        product_octets,
        parameter_category,
        ..
    } = &a[0]
    else {
        panic!("GRIB2");
    };
    assert_eq!(*parameter_category, None, "the reader does not parse 4.99");
    assert!(product_octets.is_some());
    assert!(matches(&b, &a[0]).is_empty());
    assert_eq!(
        matches(&identities(with_template(99, 0, &[])), &a[0]),
        vec![0]
    );
}

/// `bytes` (one GRIB2 message) with `value` written at `octet` (1-based, as
/// the WMO tables count) of section `number`.
fn patched(mut bytes: Vec<u8>, number: u8, octet: usize, value: &[u8]) -> Vec<u8> {
    let mut at = 16;
    loop {
        let len = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap()) as usize;
        if bytes[at + 4] == number {
            bytes[at + octet - 1..at + octet - 1 + value.len()].copy_from_slice(value);
            return bytes;
        }
        at += len;
    }
}

/// The committed eccodes-encoded 4.1 and 4.12 messages (#959): their identity
/// is read from the octets eccodes wrote, the parameter and level of the
/// common block, the member of 4.1 and the derived forecast and interval of
/// 4.12. Those blocks are named fields, not `product_octets`, so nothing is
/// counted twice.
#[test]
fn eccodes_written_shared_layout_messages_read_their_identity() {
    let member = identities(read(GRIB2, "ensemble_member_4_1.grib2"));
    let MessageIdentity::Grib2 {
        product_template,
        product_octets,
        ensemble_type,
        ensemble_member,
        derived_forecast,
        local_centre,
        parameter_category,
        parameter_number,
        first_surface,
        statistics,
        ..
    } = &member[0]
    else {
        panic!("GRIB2");
    };
    assert_eq!(*product_template, 1);
    assert_eq!(
        *product_octets, None,
        "the ensemble block is read into fields"
    );
    assert_eq!((*ensemble_type, *ensemble_member), (Some(3), Some(7)));
    assert_eq!(*derived_forecast, None);
    assert_eq!(*local_centre, None, "temperature is a WMO parameter");
    assert_eq!((*parameter_category, *parameter_number), (Some(0), Some(0)));
    assert_eq!(first_surface.map(|s| s.surface_type), Some(103));
    assert!(statistics.is_empty());

    let derived = identities(read(GRIB2, "derived_max_4_12.grib2"));
    let MessageIdentity::Grib2 {
        product_template,
        product_octets,
        ensemble_member,
        derived_forecast,
        range_end,
        statistics,
        ..
    } = &derived[0]
    else {
        panic!("GRIB2");
    };
    assert_eq!(*product_template, 12);
    assert_eq!(
        *product_octets, None,
        "the derived block is read into a field"
    );
    assert_eq!(*ensemble_member, None);
    assert_eq!(*derived_forecast, Some(0), "an unweighted mean");
    assert_eq!(range_end.as_deref(), Some("2008-02-06T18:00:00Z"));
    assert_eq!(statistics.len(), 1);
    assert_eq!(
        (
            statistics[0].process,
            statistics[0].length_unit,
            statistics[0].length
        ),
        (2, 1, 6),
        "a 6 h maximum"
    );
    assert_ne!(member[0], derived[0]);
}

/// Two members of one ensemble field are two messages (decided on #959,
/// 2026-10-11): member 7 and member 8 of the committed 4.1 message do not
/// match, and member 7 moved behind member 8 in a file is found there.
#[test]
fn two_members_of_one_field_are_two_messages() {
    let seven = read(GRIB2, "ensemble_member_4_1.grib2");
    // §4 octet 36 is the perturbation number.
    let eight = patched(seven.clone(), 4, 36, &[8]);
    let wanted = &identities(seven.clone())[0];
    assert!(
        matches(&identities(eight.clone()), wanted).is_empty(),
        "another member"
    );
    assert_eq!(
        matches(&identities(concat(&[&eight, &seven])), wanted),
        vec![1]
    );
}

/// A mean and a spread of one field are two messages: the committed 4.12
/// message (derived forecast 0, an unweighted mean) and the same with §4
/// octet 35 set to 4 (the spread of all members, Code Table 4.7) do not match.
#[test]
fn a_mean_and_a_spread_are_two_messages() {
    let mean = read(GRIB2, "derived_max_4_12.grib2");
    let spread = patched(mean.clone(), 4, 35, &[4]);
    let wanted = &identities(mean.clone())[0];
    assert!(matches(&identities(spread.clone()), wanted).is_empty());
    assert_eq!(
        matches(&identities(concat(&[&spread, &mean])), wanted),
        vec![1]
    );
}

/// A local parameter (category or number 192-254, Code Tables 4.1 and 4.2)
/// names its centre, so the same codes from two centres are two messages.
/// A WMO parameter means the same from any centre, and matches.
#[test]
fn a_local_parameter_from_two_centres_is_two_messages() {
    // §1 octets 6-7 are the originating centre: 7 is NCEP, 98 ECMWF.
    let from = |centre: u16, parameter: u8| {
        identities(patched(
            with_template(0, parameter, &[]),
            1,
            6,
            &centre.to_be_bytes(),
        ))
    };
    let ncep = from(7, 192);
    let MessageIdentity::Grib2 { local_centre, .. } = &ncep[0] else {
        panic!("GRIB2");
    };
    assert_eq!(*local_centre, Some(7));
    assert!(
        matches(&from(98, 192), &ncep[0]).is_empty(),
        "another centre's 192"
    );
    assert_eq!(
        matches(&from(98, 0), &from(7, 0)[0]),
        vec![0],
        "WMO temperature"
    );
}
