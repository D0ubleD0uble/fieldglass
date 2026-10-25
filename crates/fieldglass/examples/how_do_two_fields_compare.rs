//! How do two fields compare?
//!
//! Subtracts one field from another on the same grid. Here that is medium
//! cloud cover minus high cloud cover from the same forecast, which shows
//! where cloud sits mostly lower or mostly higher in the sky.
//!
//!     cargo run -p fieldglass --example how_do_two_fields_compare
//!
//! To compare your own fields, change the constants below. Both must be on the
//! same grid.

use std::error::Error;

use fieldglass::{CombineOp, Session};

const FIRST: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/ecmwf_aifs_single_0p25_mcc.grib2"
);
const SECOND: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/ecmwf_aifs_ens_0p25_cf_hcc.grib2"
);

fn main() -> Result<(), Box<dyn Error>> {
    let first = Session::open(std::fs::read(FIRST)?)?;
    let second = Session::open(std::fs::read(SECOND)?)?;
    let a = first.decode(0, &Default::default())?;
    let b = second.decode(0, &Default::default())?;

    // Cell by cell: first minus second.
    let difference = first.combine(&a, &b, CombineOp::Difference)?;

    println!(
        "{} minus {}:",
        first.message(0)?.parameter.unwrap_or_default(),
        second.message(0)?.parameter.unwrap_or_default(),
    );
    println!(
        "  from {:.1} to {:.1} over {} grid cells",
        difference.stats.min.unwrap_or(f64::NAN),
        difference.stats.max.unwrap_or(f64::NAN),
        difference.stats.valid_count,
    );
    Ok(())
}
