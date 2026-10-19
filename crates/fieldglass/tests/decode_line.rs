//! A line through an array (#172): its values along one axis, every other axis
//! held at a cell.
//!
//! Two oracles, because they catch different mistakes.
//!
//! **netCDF4-python**, on the one committed NetCDF fixture with a third axis:
//! `temperature(time=2, lat=3, lon=4)` holds `t·12 + j·4 + i`, so every value
//! states its own position and a line read one index off is a different number.
//!
//! **The slices the line crosses.** A line is exactly where a stack of slices
//! intersects one column, so for every axis of every multi-axis variable in the
//! corpus — NetCDF and Zarr alike — each point of the line must equal the cell a
//! `decode_slice` reports at that position. That holds the line to the path the
//! map on screen already takes, for axes and fixtures no hand-picked sample
//! would reach.

use fieldglass::{DecodeOptions, Dtype, Error, Line, Session, Values};

const DIMSCALE: &str = "../fieldglass-netcdf/tests/fixtures/netcdf4_dimscale.nc";

fn open(path: &str) -> Session {
    Session::open(std::fs::read(path).unwrap_or_else(|e| panic!("{path}: {e}")))
        .unwrap_or_else(|e| panic!("{path} opens: {e}"))
}

fn opts() -> DecodeOptions {
    DecodeOptions::new(Dtype::Auto)
}

/// A line's values, absent points as `None`.
fn points(line: &Line) -> Vec<Option<f64>> {
    let dense: Vec<f64> = match &line.values {
        Values::F64(v) => v.clone(),
        Values::F32(v) => v.iter().map(|&x| f64::from(x)).collect(),
        other => panic!("a values width this test does not read: {other:?}"),
    };
    line.mask
        .iter()
        .zip(dense)
        .map(|(&m, v)| (m == 1).then_some(v))
        .collect()
}

fn variable(session: &Session, name: &str) -> u32 {
    session
        .variables()
        .iter()
        .position(|v| v.name == name)
        .unwrap_or_else(|| panic!("no variable {name}")) as u32
}

#[test]
fn a_line_along_time_matches_netcdf4_python() {
    let session = open(DIMSCALE);
    let t = variable(&session, "temperature");
    // `ds['temperature'][:, 1, 2]` → [6.0, 18.0]; `ds['time'][:]` → [0.0, 6.0].
    let line = session
        .decode_line(t, 0, &[0, 1, 2], &opts())
        .expect("the line");
    assert_eq!(points(&line), vec![Some(6.0), Some(18.0)]);
    assert_eq!(line.dimension, "time");
    assert_eq!(line.variable.as_deref(), Some("temperature"));
    assert_eq!(line.units.as_deref(), Some("K"));
    assert_eq!(line.coordinates, Some(vec![0.0, 6.0]));
    assert_eq!(
        line.coordinate_units.as_deref(),
        Some("hours since 2020-01-01 00:00:00")
    );
    assert_eq!(
        (line.stats.min, line.stats.max, line.stats.valid_count),
        (Some(6.0), Some(18.0), 2)
    );
}

#[test]
fn a_line_along_a_horizontal_axis_matches_netcdf4_python() {
    let session = open(DIMSCALE);
    let t = variable(&session, "temperature");
    // `ds['temperature'][1, 2, :]` → [20.0, 21.0, 22.0, 23.0].
    let line = session
        .decode_line(t, 2, &[1, 2, 0], &opts())
        .expect("the line");
    assert_eq!(
        points(&line),
        vec![Some(20.0), Some(21.0), Some(22.0), Some(23.0)]
    );
    assert_eq!(line.dimension, "lon");
    assert!(
        line.coordinates.is_some(),
        "lon has a coordinate array of its own name"
    );
}

/// The entry for the axis being read along is ignored — the same vector a host
/// holds for the slice on screen can be passed as it is.
#[test]
fn the_along_axis_index_is_ignored() {
    let session = open(DIMSCALE);
    let t = variable(&session, "temperature");
    let a = session
        .decode_line(t, 0, &[0, 1, 2], &opts())
        .expect("line");
    let b = session
        .decode_line(t, 0, &[1, 1, 2], &opts())
        .expect("line");
    assert_eq!(points(&a), points(&b));
}

/// A line through a combined map (#1208): along an axis of the map, a row of
/// `A − B`; along a held axis, the comparison stepped along it with `b` kept
/// as far from `a` as the two positions say, so the point at `a`'s own entry
/// is the map's value and a step with no `b` is absent.
#[test]
fn a_combined_line_is_the_combined_map_along_the_axis() {
    use fieldglass::{CombineOp, LineRequest};
    let session = open(DIMSCALE);
    let t = variable(&session, "temperature");
    // `t·12 + j·4 + i`: time 1 less time 0 is 12 everywhere, and a cell less
    // the next one along lon is −1.
    let row = session
        .decode_line_combined(
            &LineRequest::new(t, 2, vec![1, 2, 0]),
            &LineRequest::new(t, 2, vec![0, 2, 0]),
            CombineOp::Difference,
            &opts(),
        )
        .expect("the row");
    assert_eq!(points(&row), vec![Some(12.0); 4]);
    assert_eq!(
        (row.variable.as_deref(), row.dimension.as_str()),
        (Some("temperature"), "lon"),
        "labelled as a's"
    );
    assert_eq!(
        (row.stats.min, row.stats.max, row.stats.valid_count),
        (Some(12.0), Some(12.0), 4)
    );

    let along_time = session
        .decode_line_combined(
            &LineRequest::new(t, 0, vec![1, 1, 2]),
            &LineRequest::new(t, 0, vec![0, 1, 2]),
            CombineOp::Difference,
            &opts(),
        )
        .expect("the line along time");
    assert_eq!(points(&along_time), vec![None, Some(12.0)]);

    // The same entry along the axis is each step against itself.
    let sum = session
        .decode_line_combined(
            &LineRequest::new(t, 0, vec![0, 1, 2]),
            &LineRequest::new(t, 0, vec![0, 1, 3]),
            CombineOp::Sum,
            &opts(),
        )
        .expect("the sum along time");
    // `[6, 18] + [7, 19]`.
    assert_eq!(points(&sum), vec![Some(13.0), Some(37.0)]);
}

/// Each point of a combined line is the combine of the two slices' cells, on
/// every multi-axis variable of the fixture, along every axis.
#[test]
fn a_combined_line_is_the_combine_of_the_slices_it_crosses() {
    use fieldglass::{CombineOp, LineRequest};
    let session = open(DIMSCALE);
    let t = variable(&session, "temperature");
    let cell = |time: u32, j: u32, i: u32| -> Option<f64> {
        let field = session
            .decode_slice(t, 1, 2, &[time, 0, 0], &DecodeOptions::new(Dtype::F64))
            .expect("slice");
        let k = (j as usize) * (field.ni as usize) + i as usize;
        (field.mask[k] == 1).then(|| field.values.to_f64()[k])
    };
    for (along, a, b) in [
        (0u32, [1u32, 2, 3], [0u32, 0, 1]),
        (1, [0, 1, 3], [1, 0, 3]),
        (2, [1, 0, 2], [0, 2, 0]),
    ] {
        let line = session
            .decode_line_combined(
                &LineRequest::new(t, along, a.to_vec()),
                &LineRequest::new(t, along, b.to_vec()),
                CombineOp::Ratio,
                &opts(),
            )
            .expect("the line");
        let shift = i64::from(b[along as usize]) - i64::from(a[along as usize]);
        let got = points(&line);
        for (k, got) in got.iter().enumerate() {
            let at = |p: [u32; 3], k: i64| -> Option<[u32; 3]> {
                let mut p = p;
                p[along as usize] = u32::try_from(k).ok()?;
                Some(p)
            };
            let k = k as i64;
            let want_a = at(a, k).and_then(|p| cell(p[0], p[1], p[2]));
            let want_b = at(b, k + shift)
                .filter(|p| p[along as usize] < [2, 3, 4][along as usize])
                .and_then(|p| cell(p[0], p[1], p[2]));
            let want = fieldglass::combine_cell(want_a, want_b, CombineOp::Ratio);
            match (got, want) {
                (Some(g), Some(w)) => assert!(
                    (g - w).abs() <= 1e-6 * w.abs().max(1.0),
                    "along {along}, point {k}: {g} against {w}"
                ),
                (g, w) => assert_eq!(*g, w, "along {along}, point {k}"),
            }
        }
    }
}

/// What a combined line refuses: two axes, a position off the axis, a message
/// stream.
#[test]
fn a_combined_line_refuses_what_it_cannot_read() {
    use fieldglass::{CombineOp, LineRequest};
    let session = open(DIMSCALE);
    let t = variable(&session, "temperature");
    let refused = |a: LineRequest, b: LineRequest| {
        session.decode_line_combined(&a, &b, CombineOp::Difference, &opts())
    };
    match refused(
        LineRequest::new(t, 0, vec![0, 0, 0]),
        LineRequest::new(t, 2, vec![0, 0, 0]),
    ) {
        Err(Error::InvalidOption { detail, .. }) => {
            assert!(detail.contains("same axis"), "{detail}");
        }
        other => panic!("two axes: {other:?}"),
    }
    match refused(
        LineRequest::new(t, 0, vec![0, 0, 0]),
        LineRequest::new(t, 0, vec![5, 0, 0]),
    ) {
        Err(Error::InvalidOption { detail, .. }) => {
            assert!(detail.contains("past the end"), "{detail}");
        }
        other => panic!("off the axis: {other:?}"),
    }
    let grib = open("../fieldglass-grib2/tests/fixtures/regular_latlon_surface.grib2");
    assert!(matches!(
        grib.decode_line_combined(
            &LineRequest::new(0, 0, vec![0, 0]),
            &LineRequest::new(0, 0, vec![0, 0]),
            CombineOp::Difference,
            &opts(),
        ),
        Err(Error::WrongAddressing { .. })
    ));
}

/// Every point of every line equals the cell `decode_slice` reports there.
#[test]
fn every_line_is_the_intersection_of_the_slices_it_crosses() {
    let mut sessions: Vec<(String, Session)> = Vec::new();
    for entry in std::fs::read_dir("../fieldglass-netcdf/tests/fixtures").expect("netcdf fixtures")
    {
        let path = entry.expect("entry").path();
        if !matches!(path.extension().and_then(|e| e.to_str()), Some("nc" | "h5")) {
            continue;
        }
        if let Ok(bytes) = std::fs::read(&path)
            && let Ok(session) = Session::open(bytes)
        {
            sessions.push((path.display().to_string(), session));
        }
    }
    for store in ["cf_v2", "cf_v3", "v2_nested", "v3_nested", "v3_sharded"] {
        let dir = format!("../fieldglass-zarr/tests/fixtures/stores/{store}");
        if let Ok(session) = Session::open_store(load(&dir)) {
            sessions.push((dir, session));
        }
    }

    let (mut lines, mut points_checked) = (0usize, 0usize);
    for (label, session) in &sessions {
        for (index, var) in session.variables().iter().enumerate() {
            let (Some(y), Some(x)) = (var.detected_y_dim, var.detected_x_dim) else {
                continue;
            };
            let rank = var.dims.len();
            if rank < 3 {
                continue;
            }
            // Hold every axis at its middle, and read along each axis in turn.
            let held: Vec<u32> = var.dims.iter().map(|d| (d.length / 2) as u32).collect();
            for along in 0..rank {
                let line = session
                    .decode_line(index as u32, along as u32, &held, &opts())
                    .unwrap_or_else(|e| panic!("{label} {} along {along}: {e}", var.name));
                let got = points(&line);
                assert_eq!(got.len() as u64, var.dims[along].length);
                lines += 1;
                // The first, middle and last point of each line, not every one.
                // Each point costs a `decode_slice`, which on NetCDF still decodes
                // the whole variable — every point was 3,150 decodes and 107 s.
                // The breadth is the point of this test, and the ends and middle
                // are where an off-by-one or a reversed axis shows; the
                // netCDF4-python tests above cover every point of a line whose
                // values encode their own position.
                let len = got.len();
                let mut sampled: Vec<usize> = vec![0, len / 2, len.saturating_sub(1)];
                sampled.dedup();
                for k in sampled {
                    let point = &got[k];
                    let mut at = held.clone();
                    at[along] = k as u32;
                    let slice = session
                        .decode_slice(index as u32, y, x, &at, &opts())
                        .unwrap_or_else(|e| panic!("{label} {}: {e}", var.name));
                    let cell = (at[y as usize] * slice.ni + at[x as usize]) as usize;
                    let want = match &slice.values {
                        Values::F64(v) => (slice.mask[cell] == 1).then(|| v[cell]),
                        Values::F32(v) => (slice.mask[cell] == 1).then(|| f64::from(v[cell])),
                        other => panic!("{other:?}"),
                    };
                    assert_eq!(
                        *point, want,
                        "{label} {} along axis {along}, point {k}: the line says {point:?}, \
                         the slice through it says {want:?}",
                        var.name
                    );
                    points_checked += 1;
                }
            }
        }
    }
    assert!(
        lines > 0,
        "the corpus holds a variable with a third axis to read a line along"
    );
    eprintln!("{lines} lines, {points_checked} points, each equal to its slice");
}

#[test]
fn the_refusals_name_what_was_wrong() {
    let session = open(DIMSCALE);
    let t = variable(&session, "temperature");
    for (along, indices, needle) in [
        (3, vec![0, 0, 0], "along_dim 3"),
        (0, vec![0, 0], "needs 3 entries"),
        (0, vec![0, 9, 0], "past the end"),
    ] {
        match session.decode_line(t, along, &indices, &opts()) {
            Err(Error::InvalidOption { detail, .. }) => {
                assert!(detail.contains(needle), "{needle:?} not in {detail:?}");
            }
            other => panic!("along {along}, {indices:?}: expected a refusal, got {other:?}"),
        }
    }
    assert!(matches!(
        session.decode_line(99, 0, &[0, 0, 0], &opts()),
        Err(Error::NoSuchMessage { .. })
    ));

    // A message stream has no axes to read along.
    let grib = open("../fieldglass-grib2/tests/fixtures/regular_latlon_surface.grib2");
    assert!(matches!(
        grib.decode_line(0, 0, &[0, 0], &opts()),
        Err(Error::WrongAddressing { .. })
    ));
}

/// A fixture store directory as the objects a host would hand over.
fn load(dir: &str) -> fieldglass_core::bytes::MemoryObjects {
    fn walk(root: &std::path::Path, dir: &std::path::Path, out: &mut Vec<(String, Vec<u8>)>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                let key = path
                    .strip_prefix(root)
                    .expect("under the root")
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/");
                out.push((key, std::fs::read(&path).expect("read")));
            }
        }
    }
    let root = std::path::Path::new(dir);
    let mut entries = Vec::new();
    walk(root, root, &mut entries);
    fieldglass_core::bytes::MemoryObjects::from_iter(entries)
}
