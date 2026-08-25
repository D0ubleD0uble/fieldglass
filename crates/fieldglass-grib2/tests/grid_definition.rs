//! Integration coverage for §3 GDS template parsing across the three
//! fixtures shipped with the crate.

use fieldglass_core::{
    LambertAzimuthalParams, LambertAzimuthalProjector, PlanarGridProjector,
    TransverseMercatorParams, TransverseMercatorProjector,
};
use fieldglass_grib2::{Grib2Reader, GridTemplate, lookup_earth_shape, lookup_grid_template};

const GFS_LATLON: &[u8] = include_bytes!("fixtures/gfs_c255_latlon.grib2");
const ETA_LAMBERT: &[u8] = include_bytes!("fixtures/eta_lambert_msg0.grib2");
const ECMWF_GAUSSIAN: &[u8] = include_bytes!("fixtures/reduced_gaussian_pressure_level.grib2");
const ROTATED_LATLON: &[u8] = include_bytes!("fixtures/rotated_latlon_surface.grib2");
const POLAR_STEREO: &[u8] = include_bytes!("fixtures/polar_stereographic_surface.grib2");
const TRANSVERSE_MERCATOR: &[u8] = include_bytes!("fixtures/transverse_mercator_ukv.grib2");
const LAMBERT_AZIMUTHAL: &[u8] = include_bytes!("fixtures/lambert_azimuthal_efas.grib2");

#[test]
fn gfs_latlon_decodes_template_3_0() {
    let reader = Grib2Reader::from_bytes(GFS_LATLON.to_vec()).expect("parse");
    let msg = &reader.messages[0];

    assert_eq!(msg.gds.template_number, 0);
    assert_eq!(msg.gds.num_data_points, 144 * 73);

    let t = match msg.gds.template {
        GridTemplate::LatLon(t) => t,
        other => panic!("expected LatLon, got {other:?}"),
    };
    assert_eq!(t.shape_of_earth, 6);
    assert_eq!(t.ni, 144);
    assert_eq!(t.nj, 73);
    assert!((t.la1 - 90.0).abs() < 1e-9);
    assert!((t.lo1 - 0.0).abs() < 1e-9);
    assert!((t.la2 - (-90.0)).abs() < 1e-9);
    assert!((t.lo2 - 357.5).abs() < 1e-9);
    assert_eq!(t.di, Some(2.5));
    assert_eq!(t.dj, Some(2.5));

    assert_eq!(msg.gds.dimensions(), Some((144, 73)));
    assert_eq!(msg.gds.template_name(), "latlon");
    assert_eq!(lookup_grid_template(0), "Latitude/longitude");
    assert_eq!(
        lookup_earth_shape(t.shape_of_earth),
        "Spherical (radius 6 371 229.0 m)"
    );
}

#[test]
fn eta_lambert_decodes_template_3_30() {
    let reader = Grib2Reader::from_bytes(ETA_LAMBERT.to_vec()).expect("parse");
    let msg = &reader.messages[0];

    assert_eq!(msg.gds.template_number, 30);
    assert_eq!(msg.gds.num_data_points, 93 * 65);

    let t = match msg.gds.template {
        GridTemplate::Lambert(t) => t,
        other => panic!("expected Lambert, got {other:?}"),
    };
    assert_eq!(t.shape_of_earth, 6);
    assert_eq!(t.nx, 93);
    assert_eq!(t.ny, 65);
    assert!((t.la1 - 12.190).abs() < 1e-3, "la1={}", t.la1);
    assert!((t.lo1 - 226.541).abs() < 1e-3, "lo1={}", t.lo1);
    assert!((t.lad - 25.0).abs() < 1e-9);
    assert!((t.lov - 265.0).abs() < 1e-9);
    // Eta operational 80 km tangent-Lambert grid.
    assert!((t.dx_metres - 81271.0).abs() < 1.0, "dx={}", t.dx_metres);
    assert!((t.dy_metres - 81271.0).abs() < 1.0);
    assert!((t.latin1 - 25.0).abs() < 1e-9);
    assert!((t.latin2 - 25.0).abs() < 1e-9);

    assert_eq!(msg.gds.dimensions(), Some((93, 65)));
    assert_eq!(msg.gds.template_name(), "lambert");
    assert_eq!(lookup_grid_template(30), "Lambert conformal");

    // §3.30 states only the first grid point, so `bounds()` derives the last
    // one from the projection (#472) rather than reporting `LaD`/`LoV` in its
    // place — a latitude of true scale in a column labelled "last point".
    // eccodes 2.34.1's own iterator on this fixture
    // (`grib_get_data -L "%.9f %.9f"`, last row) gives
    // (57.289403949, 310.614902750); the longitude here is the ±180 form.
    let (la1, lo1, la2, lo2) = msg.gds.bounds().expect("Lambert has bounds");
    assert!((la1 - 12.190).abs() < 1e-3 && (lo1 - 226.541).abs() < 1e-3);
    assert!(
        (la2 - 57.289_403_949).abs() < 1e-6 && (lo2 - (-49.385_097_250)).abs() < 1e-6,
        "derived last point ({la2}, {lo2}) should match eccodes' iterator"
    );
    assert!(
        (la2 - t.lad).abs() > 1.0,
        "the last point must no longer be LaD"
    );
}

#[test]
fn ecmwf_gaussian_decodes_template_3_40_reduced() {
    let reader = Grib2Reader::from_bytes(ECMWF_GAUSSIAN.to_vec()).expect("parse");
    let msg = &reader.messages[0];

    assert_eq!(msg.gds.template_number, 40);
    assert_eq!(msg.gds.num_data_points, 6114);
    // Reduced grid: optional list carries one entry per parallel.
    assert_eq!(msg.gds.optional_list_octet_size, 2);
    assert_eq!(msg.gds.optional_list_interp, 1);

    let t = match msg.gds.template {
        GridTemplate::Gaussian(t) => t,
        other => panic!("expected Gaussian, got {other:?}"),
    };
    assert!(t.is_reduced, "fixture is a reduced Gaussian");
    assert_eq!(t.ni, None, "reduced grids have no constant Ni");
    assert_eq!(t.nj, 64);
    assert_eq!(t.di, None, "reduced grids have no constant Di");
    assert_eq!(t.n_parallels, 32);
    // N32 reduced Gaussian — first/last parallel pair is symmetric ~±87.864°.
    assert!((t.la1 - 87.8638).abs() < 1e-3, "la1={}", t.la1);
    assert!((t.la2 - (-87.8638)).abs() < 1e-3);

    // No constant Ni in the template, but the section keeps the `PL` list, and
    // `dimensions` reports the raster those rows expand into (#503) — 128 wide,
    // the widest row, by the 64 rows. Bounds stay what the file declares.
    assert_eq!(
        msg.gds.points_per_row().map(<[u32]>::len),
        Some(64),
        "one width per row",
    );
    assert_eq!(msg.gds.dimensions(), Some((128, 64)));
    assert!(msg.gds.bounds().is_some());
    assert_eq!(msg.gds.template_name(), "gaussian");
    assert_eq!(lookup_grid_template(40), "Gaussian latitude/longitude");
}

#[test]
fn rotated_latlon_decodes_template_3_1() {
    let reader = Grib2Reader::from_bytes(ROTATED_LATLON.to_vec()).expect("parse");
    let msg = &reader.messages[0];

    assert_eq!(msg.gds.template_number, 1);
    assert_eq!(msg.gds.num_data_points, 16 * 31);

    let t = match msg.gds.template {
        GridTemplate::RotatedLatLon(t) => t,
        other => panic!("expected RotatedLatLon, got {other:?}"),
    };
    assert_eq!(t.shape_of_earth, 6);
    assert_eq!((t.ni, t.nj), (16, 31));
    assert!((t.la1 - 60.0).abs() < 1e-9);
    assert!((t.lo1 - 0.0).abs() < 1e-9);
    assert!((t.la2 - 0.0).abs() < 1e-9);
    assert!((t.lo2 - 30.0).abs() < 1e-9);
    assert_eq!(t.di, Some(2.0));
    assert_eq!(t.dj, Some(2.0));

    assert_eq!(msg.gds.dimensions(), Some((16, 31)));
    assert_eq!(msg.gds.template_name(), "rotated_latlon");
    assert_eq!(lookup_grid_template(1), "Rotated latitude/longitude");
    assert_eq!(
        lookup_earth_shape(t.shape_of_earth),
        "Spherical (radius 6 371 229.0 m)"
    );
}

#[test]
fn polar_stereographic_decodes_template_3_20() {
    let reader = Grib2Reader::from_bytes(POLAR_STEREO.to_vec()).expect("parse");
    let msg = &reader.messages[0];

    assert_eq!(msg.gds.template_number, 20);
    assert_eq!(msg.gds.num_data_points, 16 * 31);

    let t = match msg.gds.template {
        GridTemplate::PolarStereographic(t) => t,
        other => panic!("expected PolarStereographic, got {other:?}"),
    };
    assert_eq!(t.shape_of_earth, 6);
    assert_eq!((t.nx, t.ny), (16, 31));
    assert!((t.la1 - 60.0).abs() < 1e-9);
    assert!((t.lo1 - 0.0).abs() < 1e-9);
    // Sample template carries a north-pole projection (flag bit 1 clear).
    assert!(!t.south_pole);

    assert_eq!(msg.gds.dimensions(), Some((16, 31)));
    assert_eq!(msg.gds.template_name(), "polar_stereo");
    assert_eq!(lookup_grid_template(20), "Polar stereographic");
}

/// §3.12 — transverse Mercator, the template UKV is published on.
///
/// The values are asserted against the fixture builder's own inputs rather
/// than against eccodes' decode of them, because that is where the interesting
/// failures live: §3.12 units are 10^-2 m where every other planar template
/// uses 10^-3 m, and all but `Di`/`Dj` are signed sign-magnitude. Reading the
/// longitude of the reference point unsigned turns -2° into 2149.48°, and
/// reading `X1` unsigned turns -238 km into +21 700 km — both of which parse
/// cleanly and place the grid somewhere else entirely.
#[test]
fn ukv_decodes_template_3_12() {
    let reader = Grib2Reader::from_bytes(TRANSVERSE_MERCATOR.to_vec()).expect("parse");
    let msg = &reader.messages[0];

    assert_eq!(msg.gds.template_number, 12);
    assert_eq!(msg.gds.num_data_points, 24 * 30);
    // 14 fixed octets + a 70-byte template payload.
    assert_eq!(msg.gds.section_length, 84);

    let t = match msg.gds.template {
        GridTemplate::TransverseMercator(t) => t,
        other => panic!("expected TransverseMercator, got {other:?}"),
    };

    assert_eq!(t.ni, 24);
    assert_eq!(t.nj, 30);
    assert_eq!(t.scanning_mode, 0);

    // Airy 1830, declared as shape 3 with the axes in km — so they arrive
    // rounded to the metre, which is the encoder's limit and not ours.
    assert_eq!(t.shape_of_earth, 3);
    assert!((t.earth_major_m - 6_377_563.0).abs() < 1e-6);
    assert!((t.earth_minor_m - 6_356_257.0).abs() < 1e-6);

    // Sign-magnitude, longitude included.
    assert!((t.lat_ref - 49.0).abs() < 1e-9);
    assert!(
        (t.lon_ref - (-2.0)).abs() < 1e-9,
        "lon_ref was {} — read as unsigned it would be 2149.48",
        t.lon_ref
    );

    // The scale factor is an IEEE-32 float on the wire, so 0.9996012717 comes
    // back as its nearest f32. Asserting the exact stored value rather than the
    // intended one keeps the test honest about where the rounding happened.
    assert!((t.scale_factor - 0.999_601_244_926_452_6).abs() < 1e-15);

    // Units of 10^-2 m throughout, signed except the increments.
    assert!((t.false_easting_m - 400_000.0).abs() < 1e-6);
    assert!((t.false_northing_m - (-100_000.0)).abs() < 1e-6);
    assert!((t.di_metres - 48_000.0).abs() < 1e-6);
    assert!((t.dj_metres - 48_000.0).abs() < 1e-6);
    assert!((t.x1_metres - (-238_000.0)).abs() < 1e-6);
    assert!((t.y1_metres - 1_222_000.0).abs() < 1e-6);
    assert!((t.x2_metres - 866_000.0).abs() < 1e-6);
    assert!((t.y2_metres - (-170_000.0)).abs() < 1e-6);

    assert_eq!(msg.gds.dimensions(), Some((24, 30)));
    assert_eq!(msg.gds.scanning_mode(), Some(0));
    assert_eq!(msg.gds.template_name(), "transverse_mercator");
    assert_eq!(lookup_grid_template(12), "Transverse Mercator");
    // §3.12 carries no corner latitudes, and substituting the projection
    // parameters would put a 400 000 m false easting in a field the message
    // table prints as a longitude. Corners come from the projector instead.
    assert_eq!(msg.gds.bounds(), None);
}

/// The §3.12 fixture's data decodes to what eccodes decodes it to, and does so
/// in scan order. The values are a ramp on purpose: a constant field survives a
/// transposed or flipped raster unchanged, so it could not catch the scan-order
/// mistake a new grid template is most likely to introduce.
#[test]
fn ukv_template_3_12_values_match_eccodes() {
    let reader = Grib2Reader::from_bytes(TRANSVERSE_MERCATOR.to_vec()).expect("parse");
    let values = reader.decode_message_values(0).expect("decode");
    assert_eq!(values.len(), 24 * 30);

    // `grib_get_data` at eccodes 2.34.1, points 1, 2, 360 and 720. Quantised to
    // 16 bits by the encoder, so the tolerance is the quantisation step rather
    // than float noise.
    for (index, expected) in [
        (0usize, 273.149_993_90_f64),
        (1, 273.177_825_93),
        (359, 283.122_161_87),
        (719, 293.122_161_87),
    ] {
        let got = values[index].unwrap_or_else(|| panic!("point {index} decoded as missing"));
        assert!(
            (got - expected).abs() < 1e-4,
            "point {index}: got {got}, eccodes says {expected}"
        );
    }
    assert!(
        values.iter().all(Option::is_some),
        "the fixture carries no bitmap, so no point may decode as missing"
    );
}

/// The §3.12 grid walk lands on the last grid point the message itself
/// declares.
///
/// `X2`/`Y2` are redundant with `X1 + (Ni-1)·Di` and `Y1 ± (Nj-1)·Dj`, which
/// makes them free oracle: they are the encoder's own statement of where the
/// scan ends, so walking to a different corner means the scanning-mode sign was
/// applied the wrong way round. That is the failure a value check cannot see —
/// the field decodes perfectly and renders upside down.
#[test]
fn the_template_3_12_scan_walk_ends_where_the_message_says_it_does() {
    let reader = Grib2Reader::from_bytes(TRANSVERSE_MERCATOR.to_vec()).expect("parse");
    let t = match reader.messages[0].gds.template {
        GridTemplate::TransverseMercator(t) => t,
        other => panic!("expected TransverseMercator, got {other:?}"),
    };

    // The same sign convention `fieldglass-napi` applies: §3.12 stores the
    // increments as unsigned magnitudes and the direction in the scanning mode.
    let i_scans_negatively = t.scanning_mode & 0x80 != 0;
    let j_scans_positively = t.scanning_mode & 0x40 != 0;
    let dx = if i_scans_negatively {
        -t.di_metres
    } else {
        t.di_metres
    };
    let dy = if j_scans_positively {
        t.dj_metres
    } else {
        -t.dj_metres
    };

    let projector = TransverseMercatorProjector::new(TransverseMercatorParams {
        semi_major_m: t.earth_major_m,
        semi_minor_m: t.earth_minor_m,
        ni: t.ni,
        nj: t.nj,
        lat_ref: t.lat_ref,
        lon_ref: t.lon_ref,
        scale_factor: t.scale_factor,
        false_easting_m: t.false_easting_m,
        false_northing_m: t.false_northing_m,
        x1_metres: t.x1_metres,
        y1_metres: t.y1_metres,
        dx_metres: dx,
        dy_metres: dy,
    });
    assert!(projector.is_well_defined());

    let (x2, y2) = projector.grid_corners_xy()[3];
    assert!(
        (x2 - t.x2_metres).abs() < 1e-6 && (y2 - t.y2_metres).abs() < 1e-6,
        "walked to ({x2}, {y2}), message declares ({}, {})",
        t.x2_metres,
        t.y2_metres
    );

    // And the corner geolocations are the ones PROJ 9.4.0 reports for this
    // grid — the same oracle `fieldglass-core`'s own tests use, repeated here
    // through the parsed message so the whole chain is covered, not just the
    // projector.
    let (lat, lon) = projector.grid_point_lonlat(0, 0);
    assert!((lat - 60.374_180_125).abs() < 1e-7 && (lon - (-13.611_297_212)).abs() < 1e-7);
}

/// A §3.12 GDS truncated inside its payload must be rejected, not read past.
/// §3.12 is the longest of the planar templates at 84 octets, so a producer
/// that writes a 3.10-sized section and calls it 3.12 is the realistic way to
/// arrive here.
#[test]
fn a_truncated_template_3_12_is_rejected() {
    // Section header (length, number 3), source, point count, optional-list
    // octets, template number 12 — then a payload one byte short of the 70
    // the template needs.
    let payload_len = 69usize;
    let section_len = 14 + payload_len;
    let mut gds = Vec::with_capacity(section_len);
    gds.extend_from_slice(&(section_len as u32).to_be_bytes());
    gds.push(3);
    gds.push(0);
    gds.extend_from_slice(&720u32.to_be_bytes());
    gds.push(0);
    gds.push(0);
    gds.extend_from_slice(&12u16.to_be_bytes());
    gds.resize(section_len, 0);

    let err = fieldglass_grib2::parse_grid_definition(&gds)
        .expect_err("a 70-byte payload cannot fit in 69 bytes");
    let text = format!("{err}");
    assert!(
        text.contains("3.12") && text.contains("70"),
        "unexpected error: {text}"
    );
}

/// §3.140 — Lambert azimuthal equal-area, the template EFAS and OSI SAF use.
///
/// The trap it shares with §3.12 is that `Lo1`, `standardParallel` and
/// `centralLongitude` are all signed sign-magnitude, so a western value parses
/// as a large positive one if read unsigned. The trap it does *not* share is
/// the unit: these grid lengths really are millimetres, where §3.12's are
/// centimetres — which is why the two are parsed by different readers rather
/// than one shared helper.
#[test]
fn efas_decodes_template_3_140() {
    let reader = Grib2Reader::from_bytes(LAMBERT_AZIMUTHAL.to_vec()).expect("parse");
    let msg = &reader.messages[0];

    assert_eq!(msg.gds.template_number, 140);
    assert_eq!(msg.gds.num_data_points, 20 * 16);
    // 14 fixed octets + a 50-byte template payload.
    assert_eq!(msg.gds.section_length, 64);

    let t = match msg.gds.template {
        GridTemplate::LambertAzimuthal(t) => t,
        other => panic!("expected LambertAzimuthal, got {other:?}"),
    };

    assert_eq!(t.nx, 20);
    assert_eq!(t.ny, 16);
    assert_eq!(t.scanning_mode, 64);

    // GRS80 from the fixed shape code 4, so the axes are exact.
    assert_eq!(t.shape_of_earth, 4);
    assert!((t.earth_major_m - 6_378_137.0).abs() < 1e-6);
    assert!((t.earth_minor_m - 6_356_752.314).abs() < 1e-6);

    assert!((t.la1 - 35.0).abs() < 1e-9);
    assert!(
        (t.lo1 - (-10.0)).abs() < 1e-9,
        "lo1 was {} — read as unsigned it would be 2157.48",
        t.lo1
    );
    assert!((t.standard_parallel - 52.0).abs() < 1e-9);
    assert!((t.central_longitude - 10.0).abs() < 1e-9);
    assert!((t.dx_metres - 200_000.0).abs() < 1e-6);
    assert!((t.dy_metres - 200_000.0).abs() < 1e-6);

    assert_eq!(msg.gds.dimensions(), Some((20, 16)));
    assert_eq!(msg.gds.scanning_mode(), Some(64));
    assert_eq!(msg.gds.template_name(), "lambert_azimuthal");
    assert_eq!(lookup_grid_template(140), "Lambert azimuthal equal area");
}

/// The §3.140 grid geolocates to what eccodes' own
/// `lambert_azimuthal_equal_area` iterator reports, through the parsed message
/// rather than through hand-built parameters — so the whole chain is covered.
#[test]
fn the_template_3_140_grid_geolocates_as_eccodes_does() {
    let reader = Grib2Reader::from_bytes(LAMBERT_AZIMUTHAL.to_vec()).expect("parse");
    let t = match reader.messages[0].gds.template {
        GridTemplate::LambertAzimuthal(t) => t,
        other => panic!("expected LambertAzimuthal, got {other:?}"),
    };

    // The same sign convention `fieldglass-napi` applies.
    let dx = if t.scanning_mode & 0x80 != 0 {
        -t.dx_metres
    } else {
        t.dx_metres
    };
    let dy = if t.scanning_mode & 0x40 != 0 {
        t.dy_metres
    } else {
        -t.dy_metres
    };

    let projector = LambertAzimuthalProjector::new(LambertAzimuthalParams {
        semi_major_m: t.earth_major_m,
        semi_minor_m: t.earth_minor_m,
        ni: t.nx,
        nj: t.ny,
        lat_first: t.la1,
        lon_first: t.lo1,
        standard_parallel: t.standard_parallel,
        central_longitude: t.central_longitude,
        dx_metres: dx,
        dy_metres: dy,
    });
    assert!(projector.is_well_defined());

    // eccodes 2.34.1, read at full precision through the `latitudes` and
    // `longitudes` array keys.
    for (i, j, lat, lon) in [
        (0u32, 0u32, 34.999999991_f64, -10.000000000_f64),
        (19, 0, 34.622366847, 31.637938749),
        (0, 15, 60.108219531, -24.240260690),
        (19, 15, 59.435027618, 46.673881242),
    ] {
        let (got_lat, got_lon) = projector.grid_point_lonlat(i, j);
        assert!(
            (got_lat - lat).abs() < 1e-7 && (got_lon - lon).abs() < 1e-7,
            "({i}, {j}) gave ({got_lat}, {got_lon}), eccodes says ({lat}, {lon})"
        );
    }

    // And the section reports that same corner (#472): §3.140 used to put its
    // tangent point in the last-point slot at the crate level, while the napi
    // layer substituted the derived one — two answers to one question.
    let (la1, lo1, la2, lo2) = reader.messages[0]
        .gds
        .bounds()
        .expect("Lambert azimuthal has bounds");
    assert!((la1 - 34.999_999_991).abs() < 1e-7 && (lo1 - (-10.0)).abs() < 1e-7);
    assert!(
        (la2 - 59.435_027_618).abs() < 1e-7 && (lo2 - 46.673_881_242).abs() < 1e-7,
        "bounds' last point ({la2}, {lo2}) should be the grid's own far corner"
    );
}

/// A §3.140 GDS truncated inside its payload must be rejected, not read past.
#[test]
fn a_truncated_template_3_140_is_rejected() {
    let payload_len = 49usize;
    let section_len = 14 + payload_len;
    let mut gds = Vec::with_capacity(section_len);
    gds.extend_from_slice(&(section_len as u32).to_be_bytes());
    gds.push(3);
    gds.push(0);
    gds.extend_from_slice(&320u32.to_be_bytes());
    gds.push(0);
    gds.push(0);
    gds.extend_from_slice(&140u16.to_be_bytes());
    gds.resize(section_len, 0);

    let err = fieldglass_grib2::parse_grid_definition(&gds)
        .expect_err("a 50-byte payload cannot fit in 49 bytes");
    let text = format!("{err}");
    assert!(
        text.contains("3.140") && text.contains("50"),
        "unexpected error: {text}"
    );
}

const SPECTRAL_T63: &[u8] = include_bytes!("fixtures/spectral_simple_t63.grib2");
const BIFOURIER: &[u8] = include_bytes!("fixtures/bifourier_rectangle_keepaxes.grib2");
const HEALPIX_N2: &[u8] = include_bytes!("fixtures/healpix_n2_ring.grib2");
const HEALPIX_N4: &[u8] = include_bytes!("fixtures/healpix_n4_nested.grib2");
const OCTAHEDRAL_O32: &[u8] = include_bytes!("fixtures/octahedral_gaussian_o32.grib2");
const REGULAR_GAUSSIAN: &[u8] = include_bytes!("fixtures/regular_gaussian_f32.grib2");

/// `size_label` answers where `dimensions` does not, on every fixture the crate
/// ships (#416).
///
/// The two divide the corpus, and asserting that across it is what stops a
/// future template being added to one and forgotten in the other — which would
/// show a field as either sizeless or double-sized.
///
/// Three groups, not two. Most grids are measured: `Ni × Nj` is the answer and
/// there is no name to give. Spectral, bi-Fourier and HEALPix fields are named
/// only: they have no raster at all, and reporting one would invent it. Reduced
/// Gaussian grids are the third case — named *and* shaped. The name is what the
/// file says (`N32`), the shape is the raster this crate expands its rows into
/// (#503), and a display prefers the name. It was the grid neither answered for
/// before #500, which this test found.
#[test]
fn size_label_and_dimensions_are_complements() {
    let measured: &[(&str, &[u8])] = &[
        ("latlon", GFS_LATLON),
        ("lambert", ETA_LAMBERT),
        ("rotated", ROTATED_LATLON),
        ("polar stereo", POLAR_STEREO),
        ("transverse mercator", TRANSVERSE_MERCATOR),
        ("lambert azimuthal", LAMBERT_AZIMUTHAL),
    ];
    for (name, bytes) in measured {
        let reader = Grib2Reader::from_bytes(bytes.to_vec()).expect("parse");
        let gds = &reader.messages[0].gds;
        assert!(gds.dimensions().is_some(), "{name} has dimensions");
        assert_eq!(gds.size_label(), None, "{name} needs no size label");
    }

    let named_only: &[(&str, &[u8], &str)] = &[
        ("spectral", SPECTRAL_T63, "T63"),
        ("bi-Fourier", BIFOURIER, "N3 M4"),
        ("HEALPix Nside 2", HEALPIX_N2, "Nside 2"),
        ("HEALPix Nside 4", HEALPIX_N4, "Nside 4"),
    ];
    for (name, bytes, expected) in named_only {
        let reader = Grib2Reader::from_bytes(bytes.to_vec()).expect("parse");
        let gds = &reader.messages[0].gds;
        assert_eq!(gds.dimensions(), None, "{name} has no Ni x Nj");
        assert_eq!(
            gds.size_label().as_deref(),
            Some(*expected),
            "{name} states its own size"
        );
    }

    // Rows of differing width: no constant Ni, but a raster to expand into and
    // a name the file states. The raster is strictly larger than the field,
    // which is why the name is the one to show.
    // (label, fixture, size label, raster width) — the row count is 64 for both.
    let named_and_shaped: &[(&str, &[u8], &str, u32)] = &[
        ("classic reduced Gaussian", ECMWF_GAUSSIAN, "N32", 128),
        ("octahedral reduced Gaussian", OCTAHEDRAL_O32, "O32", 144),
    ];
    for (name, bytes, expected, width) in named_and_shaped {
        let reader = Grib2Reader::from_bytes(bytes.to_vec()).expect("parse");
        let gds = &reader.messages[0].gds;
        assert_eq!(
            gds.dimensions(),
            Some((*width, 64)),
            "{name} expands to a raster"
        );
        assert_eq!(
            gds.size_label().as_deref(),
            Some(*expected),
            "{name} states its own size"
        );
        assert!(
            (gds.num_data_points as usize) < *width as usize * 64,
            "{name}: the raster must be larger than the field, or there is \
             nothing for the name to be more honest about"
        );
    }
}

/// A regular Gaussian grid is not mistaken for a reduced one.
///
/// The arm building the name is guarded on `is_reduced`, not on the
/// classification: a regular grid reports `Ni × Nj` and must carry no name at
/// all, rather than a confident `N32` displacing a perfectly good shape.
///
/// eccodes does name this grid — it calls it `F32`, the third of the `F`/`N`/`O`
/// family — and the omission here is deliberate rather than an oversight. The
/// label exists for a grid whose size `Ni × Nj` cannot state; this one's size
/// *is* `128 × 64`, and a name would displace the more useful answer.
///
/// Worth knowing that eccodes' own `gg_sfc_grib2.tmpl` is **not** this: despite
/// the name it is `reduced_gg` with `Ni` missing, which is what an earlier
/// draft of this test asserted against and failed on.
#[test]
fn a_regular_gaussian_grid_reports_dimensions_and_no_name() {
    let reader = Grib2Reader::from_bytes(REGULAR_GAUSSIAN.to_vec()).expect("parse");
    let gds = &reader.messages[0].gds;
    let GridTemplate::Gaussian(t) = gds.template else {
        panic!("expected a Gaussian template");
    };
    assert!(!t.is_reduced, "the sample is a regular Gaussian grid");
    assert!(!t.is_octahedral, "and so cannot be octahedral");
    assert!(gds.dimensions().is_some(), "a regular grid has Ni x Nj");
    assert_eq!(gds.size_label(), None, "and needs no name of its own");
}

/// The `O`/`N` split is read off the row widths, not off `N`.
///
/// Both fixtures are `N = 32` with 64 rows: the same Gaussian number, the same
/// row count, different widths. A classifier that keyed on anything but the
/// `PL` list would call them the same thing, and this is the pair that says so.
#[test]
fn classic_and_octahedral_differ_only_in_their_row_widths() {
    let classic = Grib2Reader::from_bytes(ECMWF_GAUSSIAN.to_vec()).expect("parse");
    let octahedral = Grib2Reader::from_bytes(OCTAHEDRAL_O32.to_vec()).expect("parse");
    let (a, b) = (&classic.messages[0].gds, &octahedral.messages[0].gds);

    let (GridTemplate::Gaussian(ta), GridTemplate::Gaussian(tb)) = (a.template, b.template) else {
        panic!("expected two Gaussian templates");
    };
    assert_eq!(ta.n_parallels, tb.n_parallels, "same Gaussian number");
    assert_eq!(ta.nj, tb.nj, "same row count");
    assert!(ta.is_reduced && tb.is_reduced, "both reduced");
    assert!(
        !ta.is_octahedral,
        "the ECMWF fixture is classic (eccodes: N32)"
    );
    assert!(
        tb.is_octahedral,
        "the built fixture is octahedral (eccodes: O32)"
    );
    assert_ne!(a.size_label(), b.size_label());
}
