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
