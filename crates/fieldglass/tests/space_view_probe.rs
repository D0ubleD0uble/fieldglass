//! A space view's source-projection probe places its cell (#1676).
//!
//! A click on a geostationary grid drawn in its own projection read
//! `279.08 K · grid 12,12`, with no position, while the same cell drawn on a
//! map read `38.48°N, 74.99°W`. The probe's source view asked only the
//! families the point operations take, and a space view is not one of them
//! yet (#979), although its forward map exists. A pixel whose line of sight
//! misses the Earth has no position at all, and the probe now says so.
//!
//! The oracle is PROJ 9.4.0, independent of the Rust projector:
//!
//! ```text
//! echo "1002.007 3786611.6" | cs2cs -f %.9f \
//!   +proj=geos +h=35786023 +lon_0=-75 +sweep=x +a=6378137 +b=6356752.31414 \
//!   +to +proj=longlat +a=6378137 +b=6356752.31414
//! -74.988007277   38.479027669
//! ```
//!
//! The easting and northing are cell (12, 12)'s scan angles, as the fixture's
//! oracle records them (`2.7999973099213094e-05`, `0.10581199662556173` rad),
//! times `perspective_point_height`. A scan angle of 0.16 rad, past the limb
//! at about 0.151, gives `inf inf`: PROJ places nothing there either.

use fieldglass::render::{PixelProbe, Source, probe_pixel};
use fieldglass::{DecodeOptions, Field, GridGeometry, RenderOptions, Session};

const GOES: &[u8] = include_bytes!("../../fieldglass-netcdf/tests/fixtures/goes16_abi_cmip.nc");
const ORACLE: &str =
    include_str!("../../fieldglass-netcdf/tests/fixtures/goes16_abi_cmip.nc.oracle.json");

/// Cell (12, 12) of `goes16_abi_cmip.nc` by PROJ 9.4.0 (module docs).
const PROJ_LAT: f64 = 38.479_027_669;
const PROJ_LON: f64 = -74.988_007_277;

/// The `CMI` plane of the committed GOES-16 Mesoscale window.
fn cmi() -> Field {
    let session = Session::open(GOES.to_vec()).expect("opens");
    let var = session
        .variables()
        .iter()
        .find(|v| v.name == "CMI")
        .expect("CMI")
        .clone();
    let (y, x) = (
        var.detected_y_dim.expect("y"),
        var.detected_x_dim.expect("x"),
    );
    session
        .decode_slice(var.index, y, x, &[0, 0], &DecodeOptions::default())
        .expect("decodes")
}

fn values(field: &Field) -> Vec<Option<f64>> {
    (0..field.mask.len())
        .map(|k| (field.mask[k] == 1).then(|| field.values.get(k)).flatten())
        .collect()
}

/// The source-view probe of cell `(i, j)` of `source`, asked by the pixel the
/// source view paints it at.
fn probe_cell(source: &Source<'_>, values: &[Option<f64>], i: u32, j: u32) -> PixelProbe {
    let options = RenderOptions::default();
    assert_eq!(options.projection, "source");
    let py = if source.scan.flips_source_rows(options.flip_y) {
        source.nj - 1 - j
    } else {
        j
    };
    let probe = probe_pixel(source, values, &options, i, py)
        .expect("the probe runs")
        .expect("the pixel is on the raster");
    assert_eq!(
        (probe.grid_i, probe.grid_j),
        (Some(i as i32), Some(j as i32)),
        "the probe names the cell asked for"
    );
    probe
}

#[test]
fn the_source_view_probe_places_a_space_view_cell_where_proj_does() {
    let field = cmi();
    assert_eq!(field.georef.kind, "space_view");
    let values = values(&field);
    let probe = probe_cell(&field.source(), &values, 12, 12);
    let (lat, lon) = (
        probe.lat.expect("a cell on the Earth has a latitude"),
        probe.lon.expect("and a longitude"),
    );
    // A metre at the ground is about 1e-5 degrees; the fixture's scan angles
    // are float32-scaled int16, which both sides read the same.
    assert!(
        (lat - PROJ_LAT).abs() < 1e-6 && (lon - PROJ_LON).abs() < 1e-6,
        "({lat}, {lon}) against PROJ's ({PROJ_LAT}, {PROJ_LON})"
    );
    assert!(!probe.off_earth, "a cell on the Earth is not off it");
    assert!(probe.value.is_some(), "the value is still read");
    // The decimals a host writes the position with come from the same map:
    // a 2 km Mesoscale cell is about 0.02 to 0.03 degrees.
    let step = probe.cell_step.expect("the cell's spacing");
    assert!((0.01..0.05).contains(&step), "cell step {step}");

    // On a map the same grid's cell has the same spacing, so the two readouts
    // of one cell are written alike.
    let map = RenderOptions::new("equirectangular", "nearest");
    let raster = fieldglass::render::project(&field.source(), &values, &map).expect("projects");
    let (w, h) = (raster.width, raster.height);
    let on_map = (0..h)
        .step_by(12)
        .flat_map(|py| (0..w).step_by(12).map(move |px| (px, py)))
        .find_map(|(px, py)| {
            probe_pixel(&field.source(), &values, &map, px, py)
                .expect("the map probe runs")
                .filter(|p| (p.grid_i, p.grid_j) == (Some(12), Some(12)))
        })
        .expect("some map pixel shows cell (12, 12)");
    assert_eq!(on_map.cell_step, Some(step), "{on_map:?}");
    assert!(!on_map.off_earth);
}

/// Every sampled cell of the fixture's own oracle, an independent NumPy
/// transcription of the GOES-R PUG fixed-grid algorithm (NOTICE.md), reads at
/// its position. The samples are off the diagonal as well as on it, so a
/// forward map that swapped `i` and `j` cannot pass, as (12, 12) alone let it.
#[test]
fn every_oracle_cell_of_the_space_view_reads_at_its_position() {
    let oracle: serde_json::Value = serde_json::from_str(ORACLE).expect("oracle JSON");
    let samples = oracle["geolocation_samples"].as_array().expect("samples");
    assert_eq!(samples.len(), 9, "the oracle's 3 x 3 sampling");
    let field = cmi();
    let values = values(&field);
    let source = field.source();
    let mut off_diagonal = 0;
    for s in samples {
        let at = |k: &str| s[k].as_f64().unwrap_or_else(|| panic!("{k} in {s}"));
        let (i, j) = (at("i") as u32, at("j") as u32);
        off_diagonal += usize::from(i != j);
        let probe = probe_cell(&source, &values, i, j);
        let (lat, lon) = (probe.lat.expect("lat"), probe.lon.expect("lon"));
        assert!(
            (lat - at("lat")).abs() < 1e-6 && (lon - at("lon")).abs() < 1e-6,
            "cell ({i}, {j}) read ({lat}, {lon}), the oracle says ({}, {})",
            at("lat"),
            at("lon")
        );
        assert!(!probe.off_earth);
    }
    assert!(off_diagonal >= 6, "the samples leave the diagonal");
}

/// The position the probe gives every cell resolves back to that cell through
/// the grid's own inverse, the direction the map view asks in.
#[test]
fn every_space_view_cell_round_trips_through_the_inverse() {
    let field = cmi();
    let values = values(&field);
    let source = field.source();
    let geometry = &field.georef.geometry;
    for j in 0..field.nj {
        for i in 0..field.ni {
            let probe = probe_cell(&source, &values, i, j);
            let (lat, lon) = (probe.lat.expect("lat"), probe.lon.expect("lon"));
            let index = geometry
                .inverse(lat, lon)
                .unwrap_or_else(|| panic!("({lat}, {lon}) of cell ({i}, {j}) is on the grid"));
            assert!(
                (index.i - f64::from(i)).abs() < 1e-6 && (index.j - f64::from(j)).abs() < 1e-6,
                "cell ({i}, {j}) at ({lat}, {lon}) inverts to ({}, {})",
                index.i,
                index.j
            );
        }
    }
}

#[test]
fn a_space_view_pixel_past_the_limb_is_off_the_earth() {
    let field = cmi();
    let values = values(&field);
    // The fixture's own satellite and Earth, its scan widened to a 0.32 rad
    // square: wider than the disc (about 0.30 rad across), so the corners look
    // past the limb and the middle sees the Earth.
    let GridGeometry::Geostationary(mut p) = field.georef.geometry.clone() else {
        panic!("a space view: {:?}", field.georef.geometry);
    };
    let n = field.ni;
    assert_eq!((n, field.nj), (24, 24));
    p.x0 = -0.16;
    p.dx_rad = 0.32 / f64::from(n - 1);
    p.y0 = 0.16;
    p.dy_rad = -0.32 / f64::from(n - 1);
    let disc = GridGeometry::Geostationary(p);
    let source = Source {
        geometry: Ok(&disc),
        ..field.source()
    };

    for (i, j) in [(0, 0), (n - 1, 0), (0, n - 1), (n - 1, n - 1)] {
        let probe = probe_cell(&source, &values, i, j);
        assert_eq!(
            (probe.lat, probe.lon),
            (None, None),
            "corner ({i}, {j}) looks past the limb, where PROJ gives inf"
        );
        assert!(
            probe.off_earth,
            "corner ({i}, {j}) says it is off the Earth"
        );
        assert_eq!(probe.cell_step, None, "and has no spacing");
    }
    let middle = probe_cell(&source, &values, n / 2, n / 2);
    assert!(
        middle.lat.is_some() && !middle.off_earth,
        "the middle of the disc is on the Earth: {middle:?}"
    );
}

/// The flag is about this pixel's line of sight, not about grids the probe
/// cannot place: a raster nothing places reports no position and is not said
/// to be off the Earth.
#[test]
fn an_unplaced_raster_is_not_called_off_the_earth() {
    let field = cmi();
    let values = values(&field);
    let source = Source {
        geometry: Err(fieldglass::Error::Unsupported {
            detail: "no grid".into(),
        }),
        ..field.source()
    };
    let probe = probe_cell(&source, &values, 12, 12);
    assert_eq!((probe.lat, probe.lon), (None, None));
    assert!(!probe.off_earth);

    // Nor is a space view whose Earth has no shape: its line of sight meets
    // nothing anywhere, which is a grid that places nothing (#610), not a
    // pixel past the limb.
    let GridGeometry::Geostationary(mut p) = field.georef.geometry.clone() else {
        panic!("a space view");
    };
    p.r_eq = 0.0;
    p.r_pol = 0.0;
    let shapeless = GridGeometry::Geostationary(p);
    let source = Source {
        geometry: Ok(&shapeless),
        ..field.source()
    };
    let probe = probe_cell(&source, &values, 12, 12);
    assert_eq!((probe.lat, probe.lon), (None, None));
    assert!(!probe.off_earth, "a shapeless Earth is not off the Earth");
}
