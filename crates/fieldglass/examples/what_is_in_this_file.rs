//! What is in this weather file?
//!
//! Lists what a GRIB, NetCDF or Zarr file holds, in plain words. For GRIB
//! that is its fields (messages); for NetCDF it is its variables and their
//! axes. Start here if you have never opened one of these files before.
//!
//!     cargo run -p fieldglass --example what_is_in_this_file
//!
//! To look at your own file, pass its path:
//!
//!     cargo run -p fieldglass --example what_is_in_this_file -- path/to/forecast.grib2

use std::error::Error;

use fieldglass::{Addressing, Session};

// A real ECMWF forecast field, sea-level pressure, that ships with this crate.
const TEST_FILE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/ecmwf_ifs_0p25_msl.grib2"
);

fn main() -> Result<(), Box<dyn Error>> {
    let path = std::env::args().nth(1).unwrap_or(TEST_FILE.into());
    let session = Session::open(std::fs::read(&path)?)?;
    println!("Format: {:?}", session.format());

    match session.addressing() {
        // GRIB: a list of fields, one after another.
        Addressing::Messages => {
            for index in 0..session.count() {
                let message = session.message(index)?;
                println!(
                    "  field {index}: {} ({}) at {}, from the forecast run of {}{}",
                    message.parameter.as_deref().unwrap_or("unknown parameter"),
                    message.units.as_deref().unwrap_or("no units"),
                    message.level.as_deref().unwrap_or("an unnamed level"),
                    message.reference_time,
                    message
                        .forecast
                        .map(|f| format!(", {f}"))
                        .unwrap_or_default(),
                );
            }
        }
        // NetCDF and Zarr: named variables, each over a few axes.
        _ => {
            for variable in session.variables() {
                let axes: Vec<String> = variable
                    .dims
                    .iter()
                    .map(|dim| format!("{} ({})", dim.name, dim.length))
                    .collect();
                println!(
                    "  {}: over {}{}",
                    variable.name,
                    axes.join(", "),
                    variable
                        .units
                        .map(|u| format!(", in {u}"))
                        .unwrap_or_default(),
                );
            }
        }
    }
    Ok(())
}
