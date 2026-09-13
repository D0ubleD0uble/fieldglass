//! A cross-section's axes are not a map (#171).
//!
//! The 1-D coordinate step of `cf::slice_placement` used to read whatever
//! coordinate arrays the two chosen axes had and build a lat/lon grid from
//! them. A time–latitude plane was therefore placed on the Earth with time as
//! longitude, and a file whose only axes are time and depth failed on the
//! depth axis's fill value instead of drawing.
//!
//! So: latitude down the rows and longitude across the columns is a map, and
//! nothing else is. An unplaced plane still decodes and still renders in its
//! own source projection — it is a plot, not an error.

use fieldglass::{DecodeOptions, Session};

const NETCDF: &str = "../fieldglass-netcdf/tests/fixtures";

fn session(name: &str) -> Session {
    Session::open(std::fs::read(format!("{NETCDF}/{name}")).expect(name)).expect("opens")
}

fn variable(session: &Session, name: &str) -> u32 {
    session
        .variables()
        .iter()
        .position(|v| v.name == name)
        .map(|i| i as u32)
        .unwrap_or_else(|| panic!("{name} is renderable"))
}

/// The family `place_slice` reports for one axis pair.
fn family(session: &Session, variable: u32, y_dim: u32, x_dim: u32) -> String {
    session
        .place_slice(variable, y_dim, x_dim)
        .expect("the slice places")
        .family()
        .to_string()
}

#[test]
fn only_latitude_by_longitude_is_a_map() {
    let session = session("ersst_v5_187001_cdf1.nc");
    let sst = variable(&session, "sst"); // sst(time, lev, lat, lon)

    assert_eq!(family(&session, sst, 2, 3), "latlon", "lat by lon is a map");
    // Every cross-section through it is not, including the transposed pair:
    // longitude down the rows and latitude across them is a plot of the two
    // against each other, and placing it would draw a transposed map.
    for (y, x, what) in [
        (0, 3, "time by lon"),
        (0, 2, "time by lat"),
        (1, 3, "level by lon"),
        (3, 2, "lon by lat (transposed)"),
        (0, 1, "time by level"),
    ] {
        assert_ne!(family(&session, sst, y, x), "latlon", "{what}");
    }
}

/// The slice still decodes and still has its own raster: a cross-section is
/// drawn, just not on a map.
#[test]
fn an_unplaced_cross_section_still_decodes() {
    let session = session("ersst_v5_187001_cdf1.nc");
    let sst = variable(&session, "sst");
    let field = session
        .decode_slice(sst, 0, 2, &[0, 0, 0, 0], &DecodeOptions::default())
        .expect("a time–latitude plane decodes");
    assert_eq!((field.ni, field.nj), (89, 1), "one time step, 89 latitudes");
    assert_ne!(field.georef.label, "latlon");
}

/// The file that only has cross-sections: a time-series whose variables are
/// `(time, z)`, with a depth axis carrying fill values. Reading that axis as a
/// longitude failed every render of the file with "coordinate variable contains
/// a fill value"; it now renders in index space.
#[test]
fn a_time_series_file_renders_instead_of_failing_on_its_depth_axis() {
    let session = session("netcdf_classic_dummy.nc");
    let u = variable(&session, "u_1205"); // u_1205(time, z)
    assert_ne!(family(&session, u, 0, 1), "latlon");
    let field = session
        .decode_slice(u, 0, 1, &[0, 0], &DecodeOptions::default())
        .expect("the plane decodes rather than erroring on the z axis");
    // The time axis is an unlimited dimension with no records in this file, so
    // the plane is 8 columns of depth by no rows at all.
    assert_eq!((field.ni, field.nj), (8, 0));
}
