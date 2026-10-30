//! What was the value at one place?
//!
//! Reads a forecast field and looks up the value at a latitude and longitude,
//! with its units.
//!
//!     cargo run -p fieldglass --example what_was_the_value_here
//!
//! To ask about your own file or place, change the constants below.

use std::error::Error;

use fieldglass::Session;

// The file, which field in it, and the place to look (London).
const FILE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/ecmwf_ifs_0p25_msl.grib2"
);
const FIELD: u32 = 0;
const LATITUDE: f64 = 51.5;
const LONGITUDE: f64 = -0.1;

fn main() -> Result<(), Box<dyn Error>> {
    let session = Session::open(std::fs::read(FILE)?)?;
    let info = session.message(FIELD)?;
    let field = session.decode(FIELD, &Default::default())?;

    // Find the grid cell nearest the place and read its value.
    let Some(point) = session.probe(&field, LATITUDE, LONGITUDE) else {
        println!("That place is outside this field's grid.");
        return Ok(());
    };
    let name = info.parameter.as_deref().unwrap_or("Value");
    let units = info.units.as_deref().unwrap_or("");
    match point.value {
        Some(value) => println!("{name} at {LATITUDE}°, {LONGITUDE}°: {value} {units}"),
        None => println!("{name} at {LATITUDE}°, {LONGITUDE}°: no data there"),
    }
    println!("(nearest grid point: {:.2}°, {:.2}°)", point.lat, point.lon);
    Ok(())
}
