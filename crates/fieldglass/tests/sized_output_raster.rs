//! The caller's own output raster (#465): *this window at W × H pixels*.
//!
//! The acceptance criterion the issue states is geometric rather than numeric —
//! "a 512 × 512 CONUS window from HRRR and from GFS produces pixel-aligned
//! rasters" — so the oracle here is **the two sources against each other**. A
//! Lambert grid and a lat/lon grid share nothing but the window and the size,
//! so if pixel `(px, py)` names the same geographic point in both, the raster
//! is the caller's and not either grid's. Nothing in this file re-derives the
//! pixel-to-lat/lon map, which would only check the implementation against a
//! copy of itself.
//!
//! Every render option is read the same way by four operations — the render,
//! the overlay projection, the contour projection and the pixel probe — so each
//! is exercised at a named size. A size that reached only the render would leave
//! coastlines and a probe reading a different map, which is the failure this
//! option is most likely to introduce.
//!
//! Committed fixtures only, for the reason `decode_and_colour.rs` gives:
//! `samples/` is git-ignored and a suite keyed on it would pass vacuously.

use fieldglass::{DecodeOptions, RenderOptions, Session};

const GFS: &str = "../fieldglass-grib2/tests/fixtures/gfs_c255_latlon.grib2";
const HRRR: &str = "../fieldglass-grib2/tests/fixtures/hrrr_complex_spd_lambert.grib2";

/// The CONUS window the issue names, and a size that is neither grid's own.
const CONUS: (f64, f64, f64, f64) = (24.0, 50.0, -125.0, -66.0);
const SIZE: (u32, u32) = (512, 512);

/// A decoded field plus the borrowed pieces every display call takes.
struct Subject {
    field: fieldglass::Field,
    cells: Vec<Option<f64>>,
}

fn subject(path: &str) -> Subject {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let session = Session::open(bytes).unwrap_or_else(|e| panic!("{path}: {e}"));
    let field = session
        .decode(0, &DecodeOptions::default())
        .unwrap_or_else(|e| panic!("{path}: {e}"));
    let cells: Vec<Option<f64>> = (0..field.mask.len())
        .map(|k| (field.mask[k] == 1).then(|| field.values.get(k)).flatten())
        .collect();
    Subject { field, cells }
}

impl Subject {
    fn source(&self) -> fieldglass::Source<'_> {
        fieldglass::Source {
            geometry: Ok(&self.field.georef.geometry),
            ni: self.field.ni,
            nj: self.field.nj,
            scan: self.field.georef.scan,
            family: &self.field.georef.kind,
        }
    }
}

/// The CONUS window, optionally at a named size.
fn conus(size: Option<(u32, u32)>) -> RenderOptions {
    let mut o = RenderOptions::new("equirectangular", "bilinear");
    let (lat_min, lat_max, lon_min, lon_max) = CONUS;
    o.bounds_lat_min = Some(lat_min);
    o.bounds_lat_max = Some(lat_max);
    o.bounds_lon_min = Some(lon_min);
    o.bounds_lon_max = Some(lon_max);
    if let Some((width, height)) = size {
        o.width = Some(width);
        o.height = Some(height);
    }
    o
}

/// A ring of geographic vertices over CONUS, as `overlay_polylines` takes them:
/// flat `[lat, lon, …]` plus the vertex count.
fn conus_ring() -> (Vec<f64>, Vec<u32>) {
    let latlon = vec![
        25.0, -124.0, 49.0, -124.0, 49.0, -67.0, 25.0, -67.0, 40.0, -100.0, 30.0, -90.0,
    ];
    let rings = vec![(latlon.len() / 2) as u32];
    (latlon, rings)
}

/// The acceptance criterion: a 512 × 512 CONUS window from HRRR and from GFS
/// produces pixel-aligned rasters.
///
/// "Pixel-aligned" is asserted as the two sources agreeing about what each pixel
/// *is*: the probe reports the geographic point under a pixel, so equal points
/// across a Lambert grid and a lat/lon grid means one shared raster. The values
/// under those points are of course different — two different fields — and this
/// says nothing about them.
#[test]
fn a_sized_window_puts_hrrr_and_gfs_on_the_same_raster() {
    let (gfs, hrrr) = (subject(GFS), subject(HRRR));
    let options = conus(Some(SIZE));

    let a = fieldglass::render::project(&gfs.source(), &gfs.cells, &options).expect("GFS projects");
    let b =
        fieldglass::render::project(&hrrr.source(), &hrrr.cells, &options).expect("HRRR projects");

    assert_eq!((a.width, a.height), SIZE, "GFS raster");
    assert_eq!((b.width, b.height), SIZE, "HRRR raster");
    // Neither grid's own shape, so a build that ignored the option and kept
    // `ni × nj` cannot pass by coincidence.
    assert_ne!((gfs.field.ni, gfs.field.nj), SIZE);
    assert_ne!((hrrr.field.ni, hrrr.field.nj), SIZE);
    assert_eq!(a.bounds, b.bounds, "both echo the window they were given");

    // Corners, edges and an interior point. Pixel 511 is the far edge, which is
    // where an off-by-one in the pixel step shows up first.
    let probes = [
        (0, 0),
        (511, 0),
        (0, 511),
        (511, 511),
        (256, 256),
        (1, 510),
        (37, 400),
    ];
    for (px, py) in probes {
        let want = fieldglass::render::probe_pixel(&gfs.source(), &gfs.cells, &options, px, py)
            .expect("GFS probes")
            .expect("the pixel is on the raster");
        let got = fieldglass::render::probe_pixel(&hrrr.source(), &hrrr.cells, &options, px, py)
            .expect("HRRR probes")
            .expect("the pixel is on the raster");
        assert_eq!(
            (want.lat, want.lon),
            (got.lat, got.lon),
            "pixel ({px}, {py}) is a different place in each raster"
        );
        let (lat, lon) = (want.lat.expect("a lat"), want.lon.expect("a lon"));
        let (lat_min, lat_max, lon_min, lon_max) = CONUS;
        assert!(
            (lat_min..=lat_max).contains(&lat) && (lon_min..=lon_max).contains(&lon),
            "pixel ({px}, {py}) at ({lat}, {lon}) fell outside the window asked for"
        );
    }

    // One pixel off the raster, so the size is the bound the probe uses too.
    assert_eq!(
        fieldglass::render::probe_pixel(&gfs.source(), &gfs.cells, &options, 512, 0)
            .expect("probes"),
        None,
        "a pixel past the named width is off the raster"
    );
}

/// The size is not decorative: a different size is a different map, so the test
/// above cannot be passing because both sides ignore the option identically.
#[test]
fn a_different_size_is_a_different_map() {
    let gfs = subject(GFS);
    let big = conus(Some((512, 512)));
    let small = conus(Some((256, 256)));

    assert_eq!(
        fieldglass::render::project(&gfs.source(), &gfs.cells, &small)
            .map(|p| (p.width, p.height))
            .expect("projects"),
        (256, 256)
    );

    let at = |o: &RenderOptions| {
        fieldglass::render::probe_pixel(&gfs.source(), &gfs.cells, o, 100, 100)
            .expect("probes")
            .expect("on the raster")
    };
    let (a, b) = (at(&big), at(&small));
    assert_ne!(
        (a.lat, a.lon),
        (b.lat, b.lon),
        "pixel (100, 100) cannot name the same point in a 512- and a 256-wide raster"
    );
}

/// The overlay and contour projections read the same size the render does, so
/// coastlines and isolines land on the raster the field was painted into.
#[test]
fn the_overlay_and_contour_projections_use_the_named_size() {
    let gfs = subject(GFS);
    let hrrr = subject(HRRR);
    let options = conus(Some(SIZE));
    let (latlon, rings) = conus_ring();

    let a = fieldglass::render::overlay_polylines(&gfs.source(), &options, &latlon, &rings)
        .expect("GFS projects the ring");
    let b = fieldglass::render::overlay_polylines(&hrrr.source(), &options, &latlon, &rings)
        .expect("HRRR projects the ring");
    assert!(!a.xy.is_empty(), "the CONUS ring should project to pixels");
    assert_eq!(
        a.xy, b.xy,
        "a geographic ring lands on the same pixels whichever field is under it"
    );

    // `ProjectedPolylines` reports no dimensions of its own — it is pixel runs
    // for a raster the caller already has — so "it used the named size" is
    // asserted as the pixels lying inside that raster and outside the smaller
    // one. A projection that had kept the source's 1440 × 721 shape puts the
    // ring's east edge well past 512.
    let inside = |xy: &[f64], (w, h): (u32, u32)| {
        xy.as_chunks::<2>()
            .0
            .iter()
            .all(|p| p[0] >= 0.0 && p[0] <= f64::from(w) && p[1] >= 0.0 && p[1] <= f64::from(h))
    };
    assert!(inside(&a.xy, SIZE), "the ring left a {SIZE:?} raster");

    let half = fieldglass::render::overlay_polylines(
        &gfs.source(),
        &conus(Some((256, 256))),
        &latlon,
        &rings,
    )
    .expect("projects at half the size");
    assert!(inside(&half.xy, (256, 256)), "the ring left a 256 raster");
    assert!(
        !inside(&a.xy, (256, 256)),
        "the 512 projection must not fit inside 256 as well, or the size is \
         not being read"
    );

    // The contour projection runs through the same call, so it inherits the
    // size — checked here rather than assumed, because the levels are traced in
    // grid space and only geolocated afterwards.
    //
    // Not the `inside` test: contours are traced over the *whole* field, so the
    // vertices outside the CONUS window project outside the raster on purpose
    // and clipping them is the drawing host's job. What the size has to do is
    // scale the map, so the two projections are compared against each other —
    // the same isolines, at half the pixels.
    let contours = fieldglass::render::contour_polylines(&gfs.source(), &gfs.cells, &options, None)
        .expect("GFS contours project");
    assert!(!contours.xy.is_empty(), "CONUS should carry isolines");
    let coarse = fieldglass::render::contour_polylines(
        &gfs.source(),
        &gfs.cells,
        &conus(Some((256, 256))),
        None,
    )
    .expect("contours project at half the size");
    assert_eq!(
        contours.xy.len(),
        coarse.xy.len(),
        "the raster size must not change which isolines are traced, only where \
         they land"
    );
    let extent = |xy: &[f64], axis: usize| {
        xy.as_chunks::<2>()
            .0
            .iter()
            .map(|p| p[axis].abs())
            .fold(0.0_f64, f64::max)
    };
    for (axis, name) in [(0, "x"), (1, "y")] {
        let (fine, half) = (extent(&contours.xy, axis), extent(&coarse.xy, axis));
        assert!(half > 0.0, "the {name} extent at 256 collapsed");
        let ratio = fine / half;
        assert!(
            (ratio - 2.0).abs() < 0.02,
            "halving the raster should halve the contour {name} extent, got {fine} / {half} \
             = {ratio}"
        );
    }
}

/// With the option absent nothing moves: the render is the raster this build
/// produced before #465, display floor and all.
///
/// Pinned as numbers rather than recomputed from `ni × nj`, because the rule
/// being pinned *is* the derivation — the source's shape raised until its long
/// edge reaches `MIN_REPROJECTED_LONG_EDGE` (#514). A test that re-derived it
/// would agree with a broken derivation.
#[test]
fn an_unnamed_size_renders_exactly_as_before() {
    // GFS is 144 × 73, floored to a 720 long edge. HRRR is already past it.
    for (path, want) in [(GFS, (720, 365)), (HRRR, (1799, 1059))] {
        let s = subject(path);
        let plain = fieldglass::render::project(&s.source(), &s.cells, &conus(None))
            .unwrap_or_else(|e| panic!("{path}: {e}"));
        assert_eq!(
            (plain.width, plain.height),
            want,
            "{path}: an unnamed size renders as it did before #465"
        );
        // And it is not the sized raster, which is what makes the assertions
        // above about 512 × 512 mean anything.
        assert_ne!((plain.width, plain.height), SIZE, "{path}");
    }
}

/// `Session::warp` takes the same pair, so the browser host — which has no
/// painter — can ask for a window at a size too.
#[test]
fn the_values_only_warp_takes_a_named_size() {
    let gfs = subject(GFS);
    let (lat_min, lat_max, lon_min, lon_max) = CONUS;

    let mut options = fieldglass::WarpOptions::new(true);
    options.bounds = Some([lat_min, lat_max, lon_min, lon_max]);
    let plain = Session::open(std::fs::read(GFS).expect("reads"))
        .expect("opens")
        .warp(&gfs.field, &options)
        .expect("warps");
    assert_eq!(
        (plain.width, plain.height),
        (gfs.field.ni, gfs.field.nj),
        "no size named keeps the source grid's shape"
    );

    options.width = Some(SIZE.0);
    options.height = Some(SIZE.1);
    let sized = Session::open(std::fs::read(GFS).expect("reads"))
        .expect("opens")
        .warp(&gfs.field, &options)
        .expect("warps");
    assert_eq!((sized.width, sized.height), SIZE);
    assert_eq!(
        sized.values.len(),
        (SIZE.0 as usize) * (SIZE.1 as usize),
        "the buffer is the raster it says it is"
    );
    assert_eq!(sized.mask.len(), sized.values.len());
    assert_eq!(
        sized.bounds,
        [lat_min, lat_max, lon_min, lon_max],
        "the window is still the caller's"
    );
}

/// Both halves or neither, on both option types, with the same code — a host
/// that branches on `Error::code` gets one answer whichever call it made.
#[test]
fn a_half_stated_size_is_refused_by_both_option_types() {
    let gfs = subject(GFS);

    for (width, height) in [(Some(512), None), (None, Some(512)), (Some(0), Some(512))] {
        let mut render = conus(None);
        render.width = width;
        render.height = height;
        let err = fieldglass::render::project(&gfs.source(), &gfs.cells, &render)
            .expect_err("render refuses {width:?} × {height:?}");
        assert_eq!(err.code(), "invalid_option");

        let mut warp = fieldglass::WarpOptions::new(true);
        warp.width = width;
        warp.height = height;
        let err = Session::open(std::fs::read(GFS).expect("reads"))
            .expect("opens")
            .warp(&gfs.field, &warp)
            .expect_err("warp refuses {width:?} × {height:?}");
        assert_eq!(err.code(), "invalid_option");
    }
}
