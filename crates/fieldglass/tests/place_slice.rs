//! `Session::place_slice` — the geometry a host paints with (#659).
//!
//! `decode_slice` already resolves a slice's placement and folds it into the
//! `Field`'s `georef`, which is the *wire* form: enough for a caption, not enough
//! to project a raster. A host that runs the projection pipeline needs the grid
//! itself, and before this the napi binding could reach that pipeline only through
//! its own sixty-five-field metadata object — which meant a new container could
//! not be rendered until it learned to fill one in.
//!
//! So what is checked here is **agreement**. Two ways of asking where a slice
//! sits could differ, and a host painting with one while captioning from the other
//! would put a correct label on a wrong picture.

use fieldglass::{DecodeOptions, Session};

/// Both containers, since the whole point is that a host needs to know neither.
fn sessions() -> Vec<(&'static str, Session)> {
    let mut out = Vec::new();
    out.push((
        "netcdf",
        Session::open(
            std::fs::read("../fieldglass-zarr/tests/fixtures/cf_twin.nc").expect("the twin"),
        )
        .expect("opens"),
    ));
    for store in ["cf_v2", "cf_v3"] {
        let dir = format!("../fieldglass-zarr/tests/fixtures/stores/{store}");
        let mut objects = Vec::new();
        fn walk(root: &std::path::Path, at: &std::path::Path, out: &mut Vec<(String, Vec<u8>)>) {
            for e in std::fs::read_dir(at).expect("readable").flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(root, &p, out);
                } else {
                    let key = p
                        .strip_prefix(root)
                        .expect("under the root")
                        .components()
                        .map(|c| c.as_os_str().to_string_lossy().into_owned())
                        .collect::<Vec<_>>()
                        .join("/");
                    out.push((key, std::fs::read(&p).expect("readable")));
                }
            }
        }
        let root = std::path::Path::new(&dir);
        walk(root, root, &mut objects);
        out.push((
            if store == "cf_v2" {
                "zarr v2"
            } else {
                "zarr v3"
            },
            Session::open_store(fieldglass::MemoryObjects::from_iter(objects)).expect("opens"),
        ));
    }
    out
}

/// The placement a host paints with is the placement the field reports.
#[test]
fn the_placement_agrees_with_the_decoded_field() {
    let mut checked = 0usize;
    for (name, session) in sessions() {
        for var in session.variables() {
            let (Some(y), Some(x)) = (var.detected_y_dim, var.detected_x_dim) else {
                continue;
            };
            let fixed = vec![0u32; var.dims.len()];
            let field = session
                .decode_slice(var.index, y, x, &fixed, &DecodeOptions::default())
                .unwrap_or_else(|e| panic!("{name}/{}: {e:?}", var.name));
            let placed = session
                .place_slice(var.index, y, x)
                .unwrap_or_else(|e| panic!("{name}/{}: {e:?}", var.name));

            // The raster shape, which is what a projection sizes from.
            assert_eq!(
                (placed.ni(), placed.nj()),
                (field.ni, field.nj),
                "{name}/{}: raster shape disagrees",
                var.name
            );
            // And the family, which is what a refusal quotes back at the user.
            assert_eq!(
                placed.family(),
                field.georef.label,
                "{name}/{}: family disagrees",
                var.name
            );
            checked += 1;
        }
    }
    assert!(checked >= 3, "only {checked} slices compared");
}

/// The placement drives the projection pipeline, and the picture is the one the
/// values describe.
///
/// This is the claim `#659` needs: a host with a `Session` and nothing else can
/// paint. No `MessageMeta`, no per-container metadata object, no knowledge of
/// which reader answered.
#[test]
fn a_host_can_project_from_the_placement_alone() {
    for (name, session) in sessions() {
        let var = session
            .variables()
            .into_iter()
            .find(|v| v.detected_y_dim.is_some() && v.detected_x_dim.is_some())
            .unwrap_or_else(|| panic!("{name} has a placeable variable"));
        let (y, x) = (var.detected_y_dim.unwrap(), var.detected_x_dim.unwrap());
        let fixed = vec![0u32; var.dims.len()];

        let field = session
            .decode_slice(var.index, y, x, &fixed, &DecodeOptions::default())
            .expect("decodes");
        let placed = session.place_slice(var.index, y, x).expect("places");

        let values: Vec<Option<f64>> = field
            .mask
            .iter()
            .zip(match &field.values {
                fieldglass::Values::F64(v) => v.clone(),
                fieldglass::Values::F32(v) => v.iter().map(|x| f64::from(*x)).collect(),
                other => panic!("a width this test does not handle: {other:?}"),
            })
            .map(|(&present, value)| (present == 1).then_some(value))
            .collect();

        let options = fieldglass::render::RenderOptions::new("equirectangular", "nearest");
        let projected = session
            .project(&placed.source(), &values, &options)
            .unwrap_or_else(|e| panic!("{name}: projecting from the placement: {e:?}"));

        assert!(projected.width > 0 && projected.height > 0, "{name}");
        assert_eq!(
            projected.values.len() as u32,
            projected.width * projected.height,
            "{name}: the raster is its stated size"
        );
        assert!(
            projected.mask.contains(&1),
            "{name}: the projected raster holds values, not a hole"
        );
    }
}

/// The same projection, built from the decoded field and nothing else.
///
/// This is the finding that matters for #659: `decode_slice`'s `Field` already
/// carries the whole `GridGeometry` on its `georef`, so a host needs **no new
/// umbrella API** to paint — not `place_slice`, and certainly not napi's
/// sixty-five-field metadata object. `place_slice` earns its place only by
/// placing a slice *without decoding it*, which a picker caption wants; it is not
/// the route to a raster.
#[test]
fn a_field_alone_is_enough_to_project() {
    for (name, session) in sessions() {
        let var = session
            .variables()
            .into_iter()
            .find(|v| v.detected_y_dim.is_some() && v.detected_x_dim.is_some())
            .expect("a placeable variable");
        let (y, x) = (var.detected_y_dim.unwrap(), var.detected_x_dim.unwrap());
        let field = session
            .decode_slice(
                var.index,
                y,
                x,
                &vec![0; var.dims.len()],
                &DecodeOptions::default(),
            )
            .expect("decodes");

        // Built from the field, with nothing else in hand.
        let source = fieldglass::render::Source {
            geometry: Ok(&field.georef.geometry),
            ni: field.ni,
            nj: field.nj,
            scan: field.georef.scan,
            family: &field.georef.label,
        };
        let values: Vec<Option<f64>> = field
            .mask
            .iter()
            .zip(match &field.values {
                fieldglass::Values::F64(v) => v.clone(),
                fieldglass::Values::F32(v) => v.iter().map(|x| f64::from(*x)).collect(),
                other => panic!("a width this test does not handle: {other:?}"),
            })
            .map(|(&present, value)| (present == 1).then_some(value))
            .collect();

        let options = fieldglass::render::RenderOptions::new("equirectangular", "nearest");
        let projected = session
            .project(&source, &values, &options)
            .unwrap_or_else(|e| panic!("{name}: {e:?}"));
        assert!(projected.mask.contains(&1), "{name}");

        // And it is the *same* raster the placement route gives, or the two
        // would be two answers to one question.
        let placed = session.place_slice(var.index, y, x).expect("places");
        let other = session
            .project(&placed.source(), &values, &options)
            .expect("projects");
        assert_eq!(
            (projected.width, projected.height),
            (other.width, other.height),
            "{name}: the two routes disagree on the raster"
        );
        assert_eq!(projected.values, other.values, "{name}: different pixels");
    }
}

/// A message stream has no slices, and says which call to make instead.
#[test]
fn a_message_stream_refuses_and_names_the_call() {
    let session = Session::open(
        std::fs::read("../fieldglass-grib2/tests/fixtures/regular_latlon_surface.grib2")
            .expect("a GRIB2 fixture"),
    )
    .expect("opens");

    let err = session
        .place_slice(0, 0, 1)
        .expect_err("no variables to slice");
    assert_eq!(err.code(), "wrong_addressing", "{}", err.message());
    assert!(
        err.message().contains("message"),
        "the refusal must name the call to make: {}",
        err.message()
    );
}

/// The two axes have to be different and within the variable.
#[test]
fn the_axes_are_checked_before_anything_is_placed() {
    let (_, session) = sessions().into_iter().next().expect("a session");
    let var = session
        .variables()
        .into_iter()
        .find(|v| v.detected_y_dim.is_some())
        .expect("a placeable variable");

    for (y, x) in [(0, 0), (0, 99)] {
        let err = session
            .place_slice(var.index, y, x)
            .expect_err("refused before placing");
        assert_eq!(err.code(), "invalid_option", "{}", err.message());
    }
    // And an index past the variable list is the other refusal.
    let err = session
        .place_slice(9_999, 0, 1)
        .expect_err("no such variable");
    assert_eq!(err.code(), "no_such_message", "{}", err.message());
}
