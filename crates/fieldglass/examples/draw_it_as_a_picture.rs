//! Draw a field as a picture.
//!
//! Reads a forecast field, colors it, and saves it as an image file you can
//! open in any image viewer.
//!
//!     cargo run -p fieldglass --example draw_it_as_a_picture
//!
//! The picture is saved as `field.bmp` in the folder you ran the command from.
//! To draw your own file, change the constants below.

use std::{error::Error, fs::File, io::Write};

use fieldglass::Session;

// The file, which field in it, and where to save the picture.
const FILE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/tests/fixtures/ecmwf_ifs_0p25_msl.grib2"
);
const FIELD: u32 = 0;
const PICTURE: &str = "field.bmp";

fn main() -> Result<(), Box<dyn Error>> {
    let session = Session::open(std::fs::read(FILE)?)?;
    let field = session.decode(FIELD, &Default::default())?;

    // Color every cell, north up, with the default color map.
    let picture = session.render(&field, &Default::default(), false)?;
    save_bmp(PICTURE, picture.width, picture.height, &picture.rgba)?;
    println!(
        "Saved a {} × {} picture to {PICTURE}",
        picture.width, picture.height
    );
    Ok(())
}

/// Save red, green, blue and alpha pixels as a BMP image, the simplest image
/// file most viewers open. You don't need to follow this part to use the crate.
fn save_bmp(path: &str, width: u32, height: u32, rgba: &[u8]) -> std::io::Result<()> {
    let row = (width * 3).div_ceil(4) * 4; // each row padded to 4 bytes
    let size = 54 + row * height;
    let mut out = Vec::with_capacity(size as usize);
    out.extend_from_slice(b"BM");
    for value in [size, 0, 54, 40] {
        out.extend_from_slice(&value.to_le_bytes());
    }
    out.extend_from_slice(&(width as i32).to_le_bytes());
    out.extend_from_slice(&(-(height as i32)).to_le_bytes()); // rows top to bottom
    out.extend_from_slice(&[1, 0, 24, 0]);
    out.extend_from_slice(&[0; 24]);
    for y in 0..height as usize {
        let start = out.len();
        for x in 0..width as usize {
            let p = &rgba[(y * width as usize + x) * 4..][..4];
            out.extend_from_slice(&[p[2], p[1], p[0]]); // BMP stores blue first
        }
        out.resize(start + row as usize, 0);
    }
    File::create(path)?.write_all(&out)
}
