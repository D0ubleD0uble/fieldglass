//! End-to-end checks over the **committed** GRIB fixtures.
//!
//! Deliberately not `samples/`: that directory is git-ignored (see
//! `samples/README.md`), so a suite keyed on it would pass vacuously in CI and
//! in a fresh clone. The oracle-checked fixtures under `crates/*/tests/fixtures`
//! are in the tree, so a regression here is a red build rather than a silence.
//!
//! Two things are proved:
//!
//! 1. A decode produces exactly the raster its geometry describes, and every
//!    operation that consumes a field agrees with that raster.
//! 2. **The CPU painter is the GPU path's oracle.** The shader's arithmetic
//!    (`fieldglass::shader_index` over `shader_values`) selects the same lookup
//!    entry `Palette::index` does, to within one entry at a bin edge. The
//!    browser smoke page checks the real GLSL against this same rule; this
//!    checks the rule itself, on real data, with no GPU.

use fieldglass::{
    DecodeOptions, Dtype, Palette, PaletteOptions, ScaleMode, Session, colormaps, shader_index,
    shader_mask, shader_values,
};

/// Committed fixtures covering all four modelled grid families and a spread of
/// §5 packings. Paths are relative to this crate's directory.
const FIXTURES: &[(&str, &str)] = &[
    (
        "latlon / simple",
        "../fieldglass-grib2/tests/fixtures/gfs_c255_latlon.grib2",
    ),
    (
        "lambert / complex+spd",
        "../fieldglass-grib2/tests/fixtures/hrrr_complex_spd_lambert.grib2",
    ),
    (
        "lambert / jpeg2000",
        "../fieldglass-grib2/tests/fixtures/rap_jpeg2000_lambert.grib2",
    ),
    (
        // GRIB1, and the polar stereographic grid with real spacings — the
        // GRIB2 `polar_stereographic_surface.grib2` fixture declares Dx = Dy = 0
        // and is covered separately by `a_degenerate_grid_is_declined`.
        "grib1 polar stereographic",
        "../fieldglass-grib1/tests/fixtures/cmc_wind_300_2010052400_p012.grib",
    ),
    (
        "grib1 reduced gaussian",
        "../fieldglass-grib1/tests/fixtures/reduced_gg_n32.grib1",
    ),
    (
        "reduced gaussian",
        "../fieldglass-grib2/tests/fixtures/reduced_gaussian_pressure_level.grib2",
    ),
    (
        "regular gaussian",
        "../fieldglass-grib2/tests/fixtures/regular_gaussian_f32.grib2",
    ),
];

fn open(path: &str) -> Session {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    Session::open(bytes).unwrap_or_else(|e| panic!("{path}: {e}"))
}

#[test]
fn a_decode_fills_the_raster_its_geometry_describes() {
    for (label, path) in FIXTURES {
        let session = open(path);
        assert!(session.count() > 0, "{label}: no messages");
        let field = session
            .decode(0, &DecodeOptions::default())
            .unwrap_or_else(|e| panic!("{label}: {e}"));

        let cells = (field.ni as usize) * (field.nj as usize);
        assert_eq!(field.values.len(), cells, "{label}: value count");
        assert_eq!(field.mask.len(), cells, "{label}: mask length");
        assert_eq!(
            (field.georef.ni, field.georef.nj),
            (field.ni, field.nj),
            "{label}: the georef and the field disagree about the raster"
        );
        assert!(
            field.stats.valid_count > 0,
            "{label}: every cell decoded as absent, which is not a field"
        );
        assert_eq!(
            field.stats.valid_count as usize,
            field.mask.iter().filter(|&&m| m == 1).count(),
            "{label}: valid_count does not match the mask"
        );
        // Absent cells must not colour the range.
        let (min, max) = (field.stats.min.expect("min"), field.stats.max.expect("max"));
        for k in 0..cells {
            if field.mask[k] == 1 {
                let v = field.values.get(k).expect("present value");
                assert!(v.is_finite() && v >= min && v <= max, "{label}: cell {k}");
            }
        }
        assert!(
            field.georef.proj4.is_some(),
            "{label}: a modelled family must name its CRS"
        );
    }
}

/// The grid places its own first point, through the field a host actually got.
#[test]
fn probe_at_the_grids_own_corner_lands_on_cell_zero() {
    for (label, path) in FIXTURES {
        let session = open(path);
        let field = session.decode(0, &DecodeOptions::default()).expect(label);
        let (lat, lon) = field
            .georef
            .geometry
            .forward(0, 0)
            .unwrap_or_else(|| panic!("{label}: no first point"));
        let probe = session
            .probe(&field, lat, lon)
            .unwrap_or_else(|| panic!("{label}: the grid refused its own corner"));
        assert!(probe.i.abs() < 1e-6, "{label}: i = {}", probe.i);
        assert!(probe.j.abs() < 1e-6, "{label}: j = {}", probe.j);
    }
}

/// `Auto` never loses a digit. Whatever width it picked, converting back to
/// `f64` must reproduce the `f64` decode exactly for every present cell.
#[test]
fn the_automatic_dtype_is_lossless() {
    for (label, path) in FIXTURES {
        let session = open(path);
        let auto = session.decode(0, &DecodeOptions::default()).expect(label);
        // `#[non_exhaustive]` on the API types means an options struct is
        // built from its default and adjusted, never with a struct literal;
        // that is what lets a field be added without breaking a host.
        let mut wide_options = DecodeOptions::default();
        wide_options.dtype = Dtype::F64;
        let wide = session.decode(0, &wide_options).expect(label);
        for k in 0..auto.mask.len() {
            if auto.mask[k] == 1 {
                assert_eq!(
                    auto.values.get(k),
                    wide.values.get(k),
                    "{label}: cell {k} changed under the automatic dtype ({:?})",
                    auto.values.dtype()
                );
            }
        }
    }
}

/// A warp with no window asked for covers the source's own extent and produces
/// a raster the same size, with something in it.
#[test]
fn a_warp_covers_the_sources_own_extent() {
    for (label, path) in FIXTURES {
        let session = open(path);
        let field = session.decode(0, &DecodeOptions::default()).expect(label);
        // Nearest, so the comparison is against the field's own present count
        // rather than against bilinear's eroded edge: a bitmapped field (the
        // GFS fixture is 27% present) would otherwise make any threshold here
        // a statement about the bitmap, not about the warp.
        let mut options = fieldglass::WarpOptions::default();
        options.bilinear = false;
        let out = session
            .warp(&field, &options)
            .unwrap_or_else(|e| panic!("{label}: {e}"));
        assert_eq!((out.width, out.height), (field.ni, field.nj), "{label}");
        assert_eq!(out.values.len(), out.mask.len(), "{label}");
        let present = out.mask.iter().filter(|&&m| m == 1).count();
        let valid = field.stats.valid_count as usize;
        // A geographic grid's own box *is* the grid, so a nearest warp at the
        // same size is close to the identity. A projected grid's box is the
        // enclosing lat/lon rectangle of a conic or polar domain, whose corners
        // are legitimately outside it, so it keeps less — but never a minority.
        let floor = match field.georef.kind.as_str() {
            "latlon" | "gaussian" => 0.95,
            _ => 0.60,
        };
        let kept = present as f64 / valid as f64;
        assert!(
            kept >= floor,
            "{label}: a nearest warp onto the grid's own box kept {kept:.3} of the \
             field's {valid} present cells, below the {floor} floor for a \
             {} grid",
            field.georef.kind
        );
    }
}

/// **The acceptance rule.** For every fixture and a spread of palette options,
/// the shader's `f32` arithmetic over the rebased field selects the same lookup
/// entry the CPU painter does, never more than one entry away, and it agrees
/// exactly about which cells have no colour at all.
#[test]
fn the_shader_arithmetic_matches_the_cpu_painter() {
    let mut reversed = PaletteOptions::default();
    reversed.colormap = Some(colormaps()[3].name().to_string());
    reversed.reversed = true;

    // A log ramp over a wide positive domain: the case where `t0` is a
    // logarithm and the rebasing has to happen before the narrowing.
    let mut log10 = PaletteOptions::default();
    log10.scale = Some("log10".to_string());
    log10.min = Some(1.0);
    log10.max = Some(1.0e5);

    let cases: Vec<(&str, PaletteOptions)> = vec![
        ("default linear", PaletteOptions::default()),
        ("reversed, named colormap", reversed),
        ("log10", log10),
    ];

    let mut compared = 0usize;
    let mut off_by_one = 0usize;
    for (label, path) in FIXTURES {
        let session = open(path);
        let field = session.decode(0, &DecodeOptions::default()).expect(label);
        for (case, options) in &cases {
            let palette = session
                .palette(&field, options)
                .unwrap_or_else(|e| panic!("{label} / {case}: {e}"));
            let d = shader_values(&field, &palette);
            let gpu_mask = shader_mask(&field, &palette);
            let span = (palette.t1 - palette.t0) as f32;

            for k in 0..field.mask.len() {
                let value = field.values.get(k).expect("cell");
                let cpu = (field.mask[k] == 1).then(|| palette.index(value)).flatten();
                // Which cells have no colour must agree exactly: an
                // off-by-one there is a hole in the picture, not a shade.
                assert_eq!(
                    gpu_mask[k] == 1,
                    cpu.is_some(),
                    "{label} / {case}: cell {k} disagrees about being paintable"
                );
                let Some(cpu) = cpu else { continue };
                let gpu = shader_index(d[k], span);
                compared += 1;
                let delta = i32::from(gpu) - i32::from(cpu);
                assert!(
                    delta.abs() <= 1,
                    "{label} / {case}: cell {k} value {value} → GPU {gpu}, CPU {cpu}"
                );
                if delta != 0 {
                    off_by_one += 1;
                }
            }
        }
    }
    // Printed so the margin is visible in a CI log rather than only the
    // pass/fail: a rate creeping toward the ceiling is the early warning.
    println!("shader vs painter: {off_by_one} of {compared} cells differ by one entry");
    assert!(compared > 100_000, "only {compared} cells compared");
    // A bin edge is where the two roundings can land on opposite sides. If a
    // meaningful fraction differs, the arithmetic has drifted rather than the
    // rounding.
    let ratio = off_by_one as f64 / compared as f64;
    assert!(
        ratio < 1e-3,
        "{off_by_one} of {compared} cells ({ratio:.2e}) differ; \
         bin-edge rounding alone should be far rarer"
    );
}

/// A palette is data, and data has to survive the trip to a host. The workspace
/// enables serde_json's `float_roundtrip`, so this is bit-exact.
#[test]
fn a_palette_survives_the_json_trip_a_host_makes() {
    let palette = Palette::build(&colormaps()[0], true, 0.5, 1250.0, ScaleMode::Log10);
    let json = serde_json::to_string(&palette).expect("serialise");
    let back: Palette = serde_json::from_str(&json).expect("deserialise");
    assert_eq!(palette, back);
}

/// An index past the end is a named error, not a panic and not an empty field.
#[test]
fn an_out_of_range_message_is_a_named_error() {
    let session = open(FIXTURES[0].1);
    let count = session.count();
    let err = session
        .decode(count, &DecodeOptions::default())
        .expect_err("should refuse");
    assert_eq!(err.code(), "no_such_message");
    assert_eq!(
        session.message(count).expect_err("refuse").code(),
        err.code()
    );
}

/// A grid whose GDS declares zero spacing places every point at the same spot.
/// It must decline rather than answer, or a probe anywhere returns cell (0, 0)
/// and a warp draws a stripe.
#[test]
fn a_degenerate_grid_is_declined() {
    let session = open("../fieldglass-grib2/tests/fixtures/polar_stereographic_surface.grib2");
    let field = session
        .decode(0, &DecodeOptions::default())
        .expect("the values are still decodable");
    assert_eq!(field.georef.kind, "polar_stereo");
    assert_eq!(field.georef.dx, Some(0.0), "the file really does say zero");
    assert!(
        session.probe(&field, 60.0, 0.0).is_none(),
        "a grid with no spacing must place nothing"
    );
    let err = session
        .warp(&field, &Default::default())
        .expect_err("and must refuse to warp");
    assert_eq!(err.code(), "invalid_option");
}

/// Bytes that are not a container this build knows are refused by code, so a
/// host can tell "wrong file" from "broken file".
#[test]
fn unknown_bytes_are_refused_by_code() {
    let Err(err) = Session::open(vec![0u8; 64]) else {
        panic!("64 zero bytes are not a container and must be refused");
    };
    assert_eq!(err.code(), "unsupported_format");
}
