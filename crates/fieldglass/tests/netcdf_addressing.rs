//! NetCDF through the umbrella, and the addressing split that gets it there
//! (#662).
//!
//! The decision this implements: a container is addressed either by a message
//! index or by variables and slices, `Session::addressing` says which, and
//! *everything downstream of a decode is the same for both*. That last clause
//! is the one worth testing — if it did not hold, the split would have leaked
//! into every operation instead of stopping at the decode.

#![cfg(all(feature = "netcdf", feature = "grib2", feature = "render"))]

use fieldglass::api::{Addressing, SourceFormat, Values};
use fieldglass::{CombineOp, DecodeOptions, PaletteOptions, Session};

const NETCDF: &[u8] =
    include_bytes!("../../fieldglass-netcdf/tests/fixtures/ersst_v5_187001_cdf1.nc");
const GRIB2: &[u8] =
    include_bytes!("../../fieldglass-grib2/tests/fixtures/regular_latlon_surface.grib2");

/// The first variable with two or more axes, sliced on its trailing pair — the
/// choice a host makes when the file names no CF axes.
fn first_slice(session: &Session) -> fieldglass::api::Field {
    let vars = session.variables();
    let var = vars
        .iter()
        .find(|v| v.dims.len() >= 2)
        .expect("a renderable variable");
    let (y, x) = match (var.detected_y_dim, var.detected_x_dim) {
        (Some(y), Some(x)) => (y, x),
        _ => (var.dims.len() as u32 - 2, var.dims.len() as u32 - 1),
    };
    // One entry per dimension; the horizontal pair's entries are ignored.
    let fixed = vec![0u32; var.dims.len()];
    session
        .decode_slice(var.index, y, x, &fixed, &DecodeOptions::default())
        .expect("the slice decodes")
}

#[test]
fn a_netcdf_file_opens_and_says_how_it_is_addressed() {
    let session = Session::open(NETCDF.to_vec()).expect("NetCDF opens through the umbrella");
    assert_eq!(session.format(), SourceFormat::NetCdf);
    assert_eq!(session.addressing(), Addressing::Variables);

    assert!(!session.variables().is_empty(), "it offers variables");
    assert!(!session.dimensions().is_empty(), "and shared dimensions");
}

#[test]
fn a_grib_file_still_says_messages() {
    let session = Session::open(GRIB2.to_vec()).expect("GRIB2 opens");
    assert_eq!(session.addressing(), Addressing::Messages);
    assert!(session.count() > 0);
    // The other half is empty rather than an error: a host that lists variables
    // of a message container gets nothing, which is the truth.
    assert!(session.variables().is_empty());
    assert!(session.dimensions().is_empty());
}

/// The heart of the decision: the addressing split stops at the decode.
#[test]
fn a_sliced_field_goes_through_the_same_operations_a_message_does() {
    let session = Session::open(NETCDF.to_vec()).expect("NetCDF opens");
    let field = first_slice(&session);
    assert!(field.ni > 0 && field.nj > 0);
    assert_eq!(
        field.values.len(),
        (field.ni as usize) * (field.nj as usize),
        "the values fill the raster the field declares"
    );

    // Each of these takes a `&Field` and knows nothing about how it was
    // addressed. If any needed a message index, the split would have leaked.
    let palette = session
        .palette(&field, &PaletteOptions::default())
        .expect("palette");
    let raster = session
        .render(&field, &PaletteOptions::default(), false)
        .expect("render");
    assert_eq!(
        raster.rgba.len(),
        (raster.width as usize) * (raster.height as usize) * 4
    );
    // The palette really was built from *this* field's range, not a default:
    // a slice that reached the painter with no stats would colour flat.
    assert!(palette.t1 >= palette.t0, "the palette spans a real range");

    let isolines = session.contours(&field, &[]).expect("contours");
    let combined = session
        .combine(&field, &field, CombineOp::Difference)
        .expect("a field combines with itself");
    assert_eq!(
        combined.stats.valid_count, field.stats.valid_count,
        "a difference with itself keeps every cell that was present"
    );
    // `x - x` is zero everywhere it is defined, which is the cheapest proof the
    // combine ran on these values rather than returning its input.
    assert_eq!(combined.stats.min, combined.stats.max);
    let _ = isolines;
}

/// A call from the other addressing mode is refused by name, not by index.
#[test]
fn the_wrong_addressing_mode_says_which_call_to_make() {
    let session = Session::open(NETCDF.to_vec()).expect("NetCDF opens");
    assert_eq!(session.count(), 0, "an array dataset holds no messages");

    let err = session.message(0).expect_err("messages are the other mode");
    assert_eq!(err.code(), "wrong_addressing");
    assert!(
        err.message().contains("decode_slice") || err.message().contains("variables"),
        "the refusal names the way forward, got {err}"
    );

    let err = session
        .decode(0, &DecodeOptions::default())
        .expect_err("decode is the other mode");
    assert_eq!(err.code(), "wrong_addressing");
    assert!(err.message().contains("decode_slice"));

    // And the mirror: a slice asked of a message container.
    let grib = Session::open(GRIB2.to_vec()).expect("GRIB2 opens");
    let err = grib
        .decode_slice(0, 0, 1, &[0, 0], &DecodeOptions::default())
        .expect_err("slices are the other mode");
    assert_eq!(err.code(), "wrong_addressing");
    assert!(err.message().contains("decode"));
}

/// The unstated axes are never guessed.
#[test]
fn a_wrong_number_of_slice_indices_is_refused_rather_than_defaulted() {
    let session = Session::open(NETCDF.to_vec()).expect("NetCDF opens");
    let vars = session.variables();
    let var = vars
        .iter()
        .find(|v| v.dims.len() > 2)
        .expect("a variable with a non-horizontal axis to fix");
    let (y, x) = (var.dims.len() as u32 - 2, var.dims.len() as u32 - 1);

    // Too few: silently defaulting the unstated axes to zero is how a host
    // renders the first time step and labels it the last.
    let err = session
        .decode_slice(var.index, y, x, &[], &DecodeOptions::default())
        .expect_err("an unstated axis must not be guessed");
    assert_eq!(err.code(), "invalid_option");

    // Too many, likewise.
    let too_many = vec![0u32; var.dims.len() + 1];
    let err = session
        .decode_slice(var.index, y, x, &too_many, &DecodeOptions::default())
        .expect_err("more indices than axes is a caller error");
    assert_eq!(err.code(), "invalid_option");
}

#[test]
fn a_variable_index_outside_the_list_is_refused() {
    let session = Session::open(NETCDF.to_vec()).expect("NetCDF opens");
    let count = session.variables().len() as u32;
    let err = session
        .decode_slice(count, 0, 1, &[0, 0], &DecodeOptions::default())
        .expect_err("past the end");
    assert_eq!(err.code(), "no_such_message");
}

/// A packed variable arrives in physical units — **once**.
///
/// The other tests in this file assert shapes, addressing and error codes, and
/// every one of them passed while `decode_slice` applied the CF mask-and-scale
/// twice: `decode_plane` is the whole chain already, and the caller unpacked
/// its output again. `(raw·s + o)·s + o` is finite and plausible for every
/// input, so nothing but a value oracle can see it. On this fixture it turned
/// 250 K into 265.625 K; on a GOES or ERA5 archive, whose `scale_factor` is
/// around 0.01, it is wrong by about two orders of magnitude.
///
/// `cf_packed_data.nc` is 640 bytes and deliberately awkward: `temp` is `int16`
/// with `scale_factor` 0.0625, `add_offset` 250, `_FillValue` -9999 and a
/// `valid_range` of `[0, 10000]`, so three of its twelve elements are masked
/// for two different reasons. The expected values come from **netCDF4-python**
/// reading the same file with `set_auto_maskandscale(True)`, not from this
/// project's decoder.
#[test]
fn a_packed_variable_is_unpacked_exactly_once() {
    const PACKED: &[u8] =
        include_bytes!("../../fieldglass-netcdf/tests/fixtures/cf_packed_data.nc");

    // netCDF4-python, flattened row-major; `None` where it masks an element.
    // Raw `int16` behind these: -50, 0, 2500, 10000, 15000, -9999, 5000, 9999,
    // 1, 7500, 250, 10000 — so -50 and 15000 fall outside `valid_range` and
    // -9999 is the fill value.
    const ORACLE: [Option<f64>; 12] = [
        None,
        Some(250.0),
        Some(406.25),
        Some(875.0),
        None,
        None,
        Some(562.5),
        Some(874.9375),
        Some(250.0625),
        Some(718.75),
        Some(265.625),
        Some(875.0),
    ];

    let session = Session::open(PACKED.to_vec()).expect("the fixture opens");
    let vars = session.variables();
    // `lat` and `lon` have one dimension each: axes, not fields.
    assert_eq!(vars.len(), 1, "only `temp` is renderable");
    assert_eq!(vars[0].name, "temp");

    let field = session
        .decode_slice(vars[0].index, 0, 1, &[0, 0], &DecodeOptions::default())
        .expect("the slice decodes");

    assert_eq!((field.ni, field.nj), (4, 3));

    // The values and the presence plane are separate: an absent cell still
    // occupies its slot, so the mask is what has to agree there and the number
    // beside it is not meaningful.
    let values: Vec<f64> = match &field.values {
        Values::F32(v) => v.iter().map(|x| f64::from(*x)).collect(),
        Values::F64(v) => v.clone(),
        // `Values` is `#[non_exhaustive]`: a width added later should fail this
        // test loudly rather than be silently skipped by a `_ => vec![]`.
        other => panic!("unhandled value width: {other:?}"),
    };
    assert_eq!(values.len(), 12);
    assert_eq!(field.mask.len(), 12);

    for (i, want) in ORACLE.iter().enumerate() {
        match want {
            None => assert_eq!(field.mask[i], 0, "element {i} should be masked"),
            Some(want) => {
                assert_eq!(field.mask[i], 1, "element {i} should be present");
                assert!(
                    (values[i] - want).abs() <= 1e-9,
                    "element {i}: decoded {}, netCDF4-python reads {want}",
                    values[i]
                );
            }
        }
    }

    // Stated separately from the comparison above, because it is the property
    // that fails first and most legibly when the unpacking runs twice: the
    // stored codes span -9999..15000 and the physical values 250..875.
    let present: Vec<f64> = values
        .iter()
        .zip(&field.mask)
        .filter(|(_, m)| **m == 1)
        .map(|(v, _)| *v)
        .collect();
    assert!(
        present.iter().all(|v| (250.0..=875.0).contains(v)),
        "values are outside the physical range the attributes imply: {present:?}"
    );
}
