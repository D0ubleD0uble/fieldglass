//! A colormap sent as its lookup table paints exactly as the same colormap
//! named (#236).
//!
//! An imported colour table reaches the painter as 768 bytes, through the same
//! option types a named colormap does. The property that makes that safe is
//! that a table is not a second colour path: send the registry's own table
//! for a map and every byte of the palette and the raster is what naming it
//! gives.

use fieldglass::{DecodeOptions, PaletteOptions, Session, colormaps, parse_cpt};

const FIXTURE: &str = "../fieldglass-grib2/tests/fixtures/gfs_c255_latlon.grib2";

fn session() -> Session {
    let bytes = std::fs::read(FIXTURE).expect("fixture");
    Session::open(bytes).expect("opens")
}

fn by_table(table: Vec<u8>, reversed: bool) -> PaletteOptions {
    let mut options = PaletteOptions::default();
    options.colormap_table = Some(table);
    options.reversed = reversed;
    options
}

#[test]
fn a_registered_colormap_sent_as_its_table_paints_identically() {
    let session = session();
    let field = session
        .decode(0, &DecodeOptions::default())
        .expect("decodes");
    for colormap in colormaps() {
        for reversed in [false, true] {
            let mut named = PaletteOptions::default();
            named.colormap = Some(colormap.name().to_string());
            named.reversed = reversed;
            let table = by_table(colormap.lut(false).to_vec(), reversed);

            let label = format!("{} reversed={reversed}", colormap.name());
            assert_eq!(
                session.palette(&field, &table).expect("table palette"),
                session.palette(&field, &named).expect("named palette"),
                "{label}: palette"
            );
            assert_eq!(
                session.render(&field, &table, false).expect("table render"),
                session.render(&field, &named, false).expect("named render"),
                "{label}: raster"
            );
        }
    }
}

#[test]
fn an_imported_cpt_paints_its_own_colours() {
    let text = std::fs::read_to_string("../fieldglass-core/tests/fixtures/cpt/batlow.cpt")
        .expect("cpt fixture");
    let table = parse_cpt(&text).expect("batlow parses");
    let session = session();
    let field = session
        .decode(0, &DecodeOptions::default())
        .expect("decodes");
    let palette = session
        .palette(&field, &by_table(table.lut().to_vec(), false))
        .expect("palette");
    for i in 0..256 {
        assert_eq!(
            &palette.lut[i * 4..i * 4 + 3],
            &table.lut()[i * 3..i * 3 + 3],
            "entry {i}"
        );
        assert_eq!(palette.lut[i * 4 + 3], 255, "entry {i} is opaque");
    }
}

#[test]
fn a_table_is_refused_beside_a_name_or_at_the_wrong_length() {
    let session = session();
    let field = session
        .decode(0, &DecodeOptions::default())
        .expect("decodes");

    let mut both = by_table(vec![0; 768], false);
    both.colormap = Some("viridis".to_string());
    let err = session.palette(&field, &both).expect_err("both");
    assert_eq!(err.code(), "invalid_option");
    assert!(err.to_string().contains("send one"), "{err}");

    for len in [0, 767, 769, 1024] {
        let err = session
            .palette(&field, &by_table(vec![0; len], false))
            .expect_err("wrong length");
        assert_eq!(err.code(), "invalid_option", "{len}");
        assert!(
            err.to_string().contains(&format!("holds {len} bytes")),
            "{err}"
        );
    }
}
