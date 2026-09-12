//! A field's mean over longitude, row by row, against latitude (#240).
//!
//! **The oracle is eccodes 2.34.1**, the pinned version: `grib_get_data -m -9999
//! -L "%.9f %.9f"`, grouped by latitude and averaged, for one fixture from each
//! family #240 names — regular lat/lon, Gaussian, and reduced Gaussian. Every
//! row's latitude and mean is held to it, not a sample, and the constants were
//! generated from that output rather than typed.
//!
//! The reduced fixture is the one that proves something beyond arithmetic. Its
//! values arrive widened to the widest row, repeating each short row's points
//! across unequal numbers of columns, so a mean over the widened cells weights
//! each point by how many times it was copied. The test checks that such a
//! naive mean really does differ on this file — otherwise the reduced handling
//! would be tested by a fixture on which it makes no difference.

use fieldglass::{DecodeOptions, Dtype, Error, Line, Session, Values};

/// `regular_latlon_surface.grib2`: 31 rows, eccodes 2.34.1.
const REGULAR_LATITUDES: [f64; 31] = [
    60.0, 58.0, 56.0, 54.0, 52.0, 50.0, 48.0, 46.0, 44.0, 42.0, 40.0, 38.0, 36.0, 34.0, 32.0, 30.0,
    28.0, 26.0, 24.0, 22.0, 20.0, 18.0, 16.0, 14.0, 12.0, 10.0, 8.0, 6.0, 4.0, 2.0, 0.0,
];
const REGULAR_MEANS: [Option<f64>; 31] = [
    Some(275.48492431812497),
    Some(277.571960449375),
    Some(278.556335449375),
    Some(279.243286131875),
    Some(280.216186523125),
    Some(280.517761230625),
    Some(281.402893065625),
    Some(281.09216308500004),
    Some(283.13891601375),
    Some(284.37896728625),
    Some(286.11889648375),
    Some(287.53936767625),
    Some(288.59484863375),
    Some(289.425537109375),
    Some(290.44500732625),
    Some(291.54675292875),
    Some(292.187683105),
    Some(291.969604491875),
    Some(291.626159668125),
    Some(293.6060791025),
    Some(296.308532715),
    Some(298.370605468125),
    Some(300.5827636725),
    Some(303.885314941875),
    Some(306.517028809375),
    Some(308.1945190425),
    Some(308.170715330625),
    Some(306.091247559375),
    Some(303.703918456875),
    Some(301.312194825625),
    Some(301.342529296875),
];

/// `regular_gaussian_f32.grib2`: 64 rows, eccodes 2.34.1.
const GAUSSIAN_LATITUDES: [f64; 64] = [
    87.863798839,
    85.096526988,
    82.312912948,
    79.525606573,
    76.73689968,
    73.947515154,
    71.157752012,
    68.367756108,
    65.577607011,
    62.787351799,
    59.997020108,
    57.206631528,
    54.416199526,
    51.625733675,
    48.835240966,
    46.044726631,
    43.254194665,
    40.463648178,
    37.673089629,
    34.882520994,
    32.091943882,
    29.301359622,
    26.510769325,
    23.720173934,
    20.929574254,
    18.13897099,
    15.348364759,
    12.557756115,
    9.767145559,
    6.976533554,
    4.185920533,
    1.395306911,
    -1.395306911,
    -4.185920533,
    -6.976533554,
    -9.767145559,
    -12.557756115,
    -15.348364759,
    -18.13897099,
    -20.929574254,
    -23.720173934,
    -26.510769325,
    -29.301359622,
    -32.091943882,
    -34.882520994,
    -37.673089629,
    -40.463648178,
    -43.254194665,
    -46.044726631,
    -48.835240966,
    -51.625733675,
    -54.416199526,
    -57.206631528,
    -59.997020108,
    -62.787351799,
    -65.577607011,
    -68.367756108,
    -71.157752012,
    -73.947515154,
    -76.73689968,
    -79.525606573,
    -82.312912948,
    -85.096526988,
    -87.863798839,
];
const GAUSSIAN_MEANS: [Option<f64>; 64] = [
    Some(1.0),
    Some(1.0),
    Some(1.0),
    Some(1.0),
    Some(1.0),
    Some(1.0),
    Some(1.0),
    Some(1.0),
    Some(1.0),
    Some(1.0),
    Some(1.0),
    Some(1.0),
    Some(1.0),
    Some(1.0),
    Some(1.0),
    Some(1.0),
    Some(1.0),
    Some(1.0),
    Some(1.0),
    Some(1.0),
    Some(1.0),
    Some(1.0),
    Some(1.0),
    Some(1.0),
    Some(1.0),
    Some(1.0),
    Some(1.0),
    Some(1.0),
    Some(1.0),
    Some(1.0),
    Some(1.0),
    Some(1.0),
    Some(1.0),
    Some(1.0),
    Some(1.0),
    Some(1.0),
    Some(1.0),
    Some(1.0),
    Some(1.0),
    Some(1.0),
    Some(1.0),
    Some(1.0),
    Some(1.0),
    Some(1.0),
    Some(1.0),
    Some(1.0),
    Some(1.0),
    Some(1.0),
    Some(1.0),
    Some(1.0),
    Some(1.0),
    Some(1.0),
    Some(1.0),
    Some(1.0),
    Some(1.0),
    Some(1.0),
    Some(1.0),
    Some(1.0),
    Some(1.0),
    Some(1.0),
    Some(1.0),
    Some(1.0),
    Some(1.0),
    Some(1.0),
];

/// `reduced_gg_n32_smooth.grib1`: 64 rows, eccodes 2.34.1.
const REDUCED_LATITUDES: [f64; 64] = [
    87.863798839,
    85.096526988,
    82.312912948,
    79.525606573,
    76.73689968,
    73.947515154,
    71.157752012,
    68.367756108,
    65.577607011,
    62.787351799,
    59.997020108,
    57.206631528,
    54.416199526,
    51.625733675,
    48.835240966,
    46.044726631,
    43.254194665,
    40.463648178,
    37.673089629,
    34.882520994,
    32.091943882,
    29.301359622,
    26.510769325,
    23.720173934,
    20.929574254,
    18.13897099,
    15.348364759,
    12.557756115,
    9.767145559,
    6.976533554,
    4.185920533,
    1.395306911,
    -1.395306911,
    -4.185920533,
    -6.976533554,
    -9.767145559,
    -12.557756115,
    -15.348364759,
    -18.13897099,
    -20.929574254,
    -23.720173934,
    -26.510769325,
    -29.301359622,
    -32.091943882,
    -34.882520994,
    -37.673089629,
    -40.463648178,
    -43.254194665,
    -46.044726631,
    -48.835240966,
    -51.625733675,
    -54.416199526,
    -57.206631528,
    -59.997020108,
    -62.787351799,
    -65.577607011,
    -68.367756108,
    -71.157752012,
    -73.947515154,
    -76.73689968,
    -79.525606573,
    -82.312912948,
    -85.096526988,
    -87.863798839,
];
const REDUCED_MEANS: [Option<f64>; 64] = [
    Some(248.055618288),
    Some(248.2921995951852),
    Some(248.71555752222224),
    Some(249.3220733625),
    Some(250.1053795697778),
    Some(251.05854797359999),
    Some(252.172154744),
    Some(253.436050415),
    Some(254.83806186305557),
    Some(256.3647328696),
    Some(258.001785277),
    Some(259.7336347791111),
    Some(261.54398634222224),
    Some(263.41550191208336),
    Some(265.3304229736),
    Some(267.2707152188889),
    Some(269.2180531825926),
    Some(271.1537139895),
    Some(273.05955708883334),
    Some(274.9173858643333),
    Some(276.709625244375),
    Some(278.41937255765623),
    Some(280.030258178125),
    Some(281.52717590359373),
    Some(282.8958129890625),
    Some(284.12306213375),
    Some(285.19763183624997),
    Some(286.1091156010937),
    Some(286.848724365),
    Some(287.4099273684375),
    Some(287.78683471749997),
    Some(287.97615051234374),
    Some(287.97615051234374),
    Some(287.78683471749997),
    Some(287.4099273684375),
    Some(286.848724365),
    Some(286.1091156010937),
    Some(285.19763183624997),
    Some(284.12306213375),
    Some(282.8958129890625),
    Some(281.52717590359373),
    Some(280.030258178125),
    Some(278.41937255765623),
    Some(276.709625244375),
    Some(274.9173858643333),
    Some(273.05955708883334),
    Some(271.1537139895),
    Some(269.2180531825926),
    Some(267.2707152188889),
    Some(265.3304229736),
    Some(263.41550191208336),
    Some(261.54398634222224),
    Some(259.7336347791111),
    Some(258.001785277),
    Some(256.3647328696),
    Some(254.83806186305557),
    Some(253.436050415),
    Some(252.172154744),
    Some(251.05854797359999),
    Some(250.1053795697778),
    Some(249.3220733625),
    Some(248.71555752222224),
    Some(248.2921995951852),
    Some(248.055618288),
];
/// Decode message 0 and take its zonal mean, through the path a host uses.
fn zonal(path: &str) -> (Line, fieldglass::Field, Vec<Option<f64>>) {
    let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let session = Session::open(bytes).expect("opens");
    let field = session
        .decode(0, &DecodeOptions::new(Dtype::Auto))
        .expect("decodes");
    let values = values_of(&field.values, &field.mask);
    let line = session
        .zonal_mean(&field.source(), &values, &field.parameter, &field.units)
        .unwrap_or_else(|e| panic!("{path}: {e}"));
    (line, field, values)
}

fn values_of(values: &Values, mask: &[u8]) -> Vec<Option<f64>> {
    let dense: Vec<f64> = match values {
        Values::F64(v) => v.clone(),
        Values::F32(v) => v.iter().map(|&x| f64::from(x)).collect(),
        other => panic!("{other:?}"),
    };
    mask.iter()
        .zip(dense)
        .map(|(&m, v)| (m == 1).then_some(v))
        .collect()
}

/// Relative agreement with eccodes' mean.
///
/// `1e-8`, which every fixture meets. It was `1e-4` first, and that was too
/// loose to mean anything for the reduced grid: the largest difference a mean
/// over the *widened* cells makes on that file is `6.3e-3` in a 280 K field,
/// under `1e-4`'s allowance of `0.028`. A test that could not tell the reduced
/// handling from its absence was passing for the wrong reason, and the proof in
/// `a_reduced_grid_averages_only_the_points_each_row_stores` is what found it.
const TOLERANCE: f64 = 1e-8;

fn agrees(got: f64, want: f64) -> bool {
    (got - want).abs() <= TOLERANCE * want.abs().max(1.0)
}

fn holds_to_eccodes(path: &str, latitudes: &[f64], means: &[Option<f64>]) {
    let (line, _, _) = zonal(path);
    assert_eq!(line.dimension, "latitude");
    assert_eq!(line.coordinate_units.as_deref(), Some("degrees_north"));
    let lats = line
        .coordinates
        .as_ref()
        .expect("a zonal mean states its latitudes");
    assert_eq!(lats.len(), latitudes.len(), "{path}: one point per row");
    let got = values_of(&line.values, &line.mask);
    for (j, (&want_lat, &want)) in latitudes.iter().zip(means).enumerate() {
        assert!(
            (lats[j] - want_lat).abs() < 1e-6,
            "{path} row {j}: latitude {} where eccodes says {want_lat}",
            lats[j]
        );
        match (got[j], want) {
            (Some(g), Some(w)) => assert!(
                agrees(g, w),
                "{path} row {j}: mean {g} where eccodes says {w}"
            ),
            (g, w) => assert_eq!(g, w, "{path} row {j}"),
        }
    }
}

#[test]
fn a_regular_grid_averages_each_row_as_eccodes_does() {
    holds_to_eccodes(
        "../fieldglass-grib2/tests/fixtures/regular_latlon_surface.grib2",
        &REGULAR_LATITUDES,
        &REGULAR_MEANS,
    );
}

#[test]
fn a_gaussian_grid_places_each_row_at_its_gaussian_latitude() {
    holds_to_eccodes(
        "../fieldglass-grib2/tests/fixtures/regular_gaussian_f32.grib2",
        &GAUSSIAN_LATITUDES,
        &GAUSSIAN_MEANS,
    );
}

#[test]
fn a_reduced_grid_averages_only_the_points_each_row_stores() {
    holds_to_eccodes(
        "../fieldglass-grib1/tests/fixtures/reduced_gg_n32_smooth.grib1",
        &REDUCED_LATITUDES,
        &REDUCED_MEANS,
    );

    // And the handling is load-bearing on this file, at this tolerance: a mean
    // over the widened cells — what ignoring `points_per_row` computes — fails
    // the same comparison on at least one row. Without this, a fixture on which
    // the reduced handling made no measurable difference would pass the oracle
    // above whether or not the handling existed.
    let (_, field, values) =
        zonal("../fieldglass-grib1/tests/fixtures/reduced_gg_n32_smooth.grib1");
    let ni = field.ni as usize;
    let caught = (0..field.nj as usize)
        .filter(|&j| {
            let row: Vec<f64> = values[j * ni..(j + 1) * ni]
                .iter()
                .flatten()
                .copied()
                .collect();
            let naive = row.iter().sum::<f64>() / row.len() as f64;
            REDUCED_MEANS[j].is_some_and(|want| !agrees(naive, want))
        })
        .count();
    assert!(
        caught > 0,
        "a mean over the widened cells must fail the eccodes comparison on some row, or \
         this fixture cannot tell the reduced handling from its absence"
    );
}

/// The two shapes #240's acceptance names that no fixture holds: a uniform
/// field is a constant line, and a row with no present value is a gap.
#[test]
fn a_uniform_field_is_constant_and_an_empty_row_is_a_gap() {
    let geometry = fieldglass_core::GridGeometry::LatLon(fieldglass_core::LatLonParams {
        ni: 4,
        nj: 3,
        lat_first: 10.0,
        lon_first: 0.0,
        lat_last: -10.0,
        lon_last: 270.0,
    });
    let source = fieldglass::Source {
        geometry: Ok(&geometry),
        ni: 4,
        nj: 3,
        scan: fieldglass::Scan::north_down(),
        family: "latlon",
        points_per_row: None,
    };
    let uniform = vec![Some(7.0); 12];
    let line = fieldglass::render::zonal_mean(&source, &uniform, "t", "K").expect("mean");
    assert_eq!(values_of(&line.values, &line.mask), vec![Some(7.0); 3]);
    assert_eq!(line.coordinates, Some(vec![10.0, 0.0, -10.0]));

    // The middle row entirely absent; a partly-absent row averages what is there.
    let holes = vec![
        Some(1.0),
        Some(3.0),
        None,
        Some(5.0),
        None,
        None,
        None,
        None,
        Some(2.0),
        Some(2.0),
        Some(2.0),
        Some(2.0),
    ];
    let line = fieldglass::render::zonal_mean(&source, &holes, "t", "K").expect("mean");
    assert_eq!(
        values_of(&line.values, &line.mask),
        vec![Some(3.0), None, Some(2.0)]
    );
}

#[test]
fn a_grid_whose_rows_are_not_latitude_circles_is_refused() {
    let geometry =
        fieldglass_core::GridGeometry::RotatedLatLon(fieldglass_core::RotatedLatLonParams {
            ni: 2,
            nj: 2,
            lat_first: 0.0,
            lon_first: 0.0,
            lat_last: 1.0,
            lon_last: 1.0,
            south_pole_lat: -40.0,
            south_pole_lon: 10.0,
            angle_of_rotation: 0.0,
        });
    let source = fieldglass::Source {
        geometry: Ok(&geometry),
        ni: 2,
        nj: 2,
        scan: fieldglass::Scan::north_down(),
        family: "rotated_ll",
        points_per_row: None,
    };
    match fieldglass::render::zonal_mean(&source, &[Some(1.0); 4], "t", "K") {
        Err(Error::Unsupported { detail }) => assert!(detail.contains("rotated_ll"), "{detail}"),
        other => panic!("a rotated grid must be refused, got {other:?}"),
    }
}
