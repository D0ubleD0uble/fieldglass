#![forbid(unsafe_code)]

pub mod reader;

pub use reader::Grib2Reader;
pub use reader::open;
