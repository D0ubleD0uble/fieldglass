//! Which scan angle is `x` and which is `y`, when a CF geostationary grid
//! does not say it the way the GOES fixture does (#1415).
//!
//! #1387's gate places a geostationary slice only when the X dimension's
//! coordinate is the `x` scan angle and the Y dimension's is `y`. It reads a
//! projection `standard_name` first (`projection_x_coordinate`, or the
//! `projection_x_angular_coordinate` CF Appendix F gives a scan angle), then
//! the `axis` attribute, then the names. Every committed geostationary file
//! states all three and they agree, so a gate reading only one of them, or
//! letting an unidentifiable pair through, passed the suite.
//!
//! Each case here is `goes_geostationary.nc` read through the real NetCDF
//! reader, with its coordinate metadata restated before
//! [`slice_placement`] sees it. The values still come from the file.

use std::ops::Range;

use fieldglass_core::FieldglassError;
use fieldglass_core::array::{ArraySource, Attribute, Group};
use fieldglass_core::cf::{SOURCE_ONLY, slice_placement};
use fieldglass_core::projection::GridGeometry;
use fieldglass_netcdf::{NetcdfArrays, NetcdfReader};

const GOES: &[u8] = include_bytes!("fixtures/goes_geostationary.nc");

/// `Rad` is `(y, x)`: the upright pick puts dimension 0 down the rows.
const Y_DIM: usize = 0;
const X_DIM: usize = 1;

/// How the two coordinates describe themselves in one variant.
struct Restated {
    /// What the `x` and `y` dimensions (and their coordinate arrays) are
    /// called.
    names: [&'static str; 2],
    /// The `standard_name` of each, or `None` to drop it.
    standard_names: [Option<&'static str>; 2],
    /// Whether each keeps its `axis = "X"` / `"Y"`.
    axis: [bool; 2],
}

/// The fixture with its coordinates restated, values read from the file.
struct Variant {
    arrays: NetcdfArrays,
    group: Group,
    /// `(new, original)` for each renamed coordinate array.
    renamed: Vec<(String, String)>,
}

impl Variant {
    fn new(restated: &Restated) -> Self {
        let reader = NetcdfReader::from_bytes(GOES.to_vec()).expect("fixture opens");
        let arrays = NetcdfArrays::open(reader).expect("a view");
        let mut group = arrays.group().clone();
        let mut renamed = Vec::new();
        for (i, original) in ["x", "y"].into_iter().enumerate() {
            let name = restated.names[i];
            for dim in &mut group.dimensions {
                if dim.name == original {
                    dim.name = name.to_string();
                }
            }
            for array in &mut group.arrays {
                for dim in &mut array.dimensions {
                    if dim == original {
                        *dim = name.to_string();
                    }
                }
            }
            let coord = group
                .arrays
                .iter_mut()
                .find(|a| a.name == original)
                .expect("the fixture's coordinate array");
            coord.name = name.to_string();
            coord
                .attributes
                .retain(|a| a.name != "standard_name" && (restated.axis[i] || a.name != "axis"));
            if let Some(standard_name) = restated.standard_names[i] {
                coord
                    .attributes
                    .push(Attribute::text("standard_name", standard_name));
            }
            renamed.push((name.to_string(), original.to_string()));
        }
        Self {
            arrays,
            group,
            renamed,
        }
    }

    fn place(&self, y_dim: usize, x_dim: usize) -> GridGeometry {
        slice_placement(self, "Rad", y_dim, x_dim)
            .expect("the slice resolves")
            .geometry
    }
}

impl ArraySource for Variant {
    fn group(&self) -> &Group {
        &self.group
    }

    fn read_region(
        &self,
        array: &str,
        region: &[Range<u64>],
    ) -> Result<Vec<Option<f64>>, FieldglassError> {
        let original = self
            .renamed
            .iter()
            .find(|(new, _)| new == array)
            .map_or(array, |(_, original)| original.as_str());
        self.arrays.read_region(original, region)
    }
}

/// The fixture as committed, upright: what every placed variant must match.
fn committed() -> GridGeometry {
    let reader = NetcdfReader::from_bytes(GOES.to_vec()).expect("fixture opens");
    let arrays = NetcdfArrays::open(reader).expect("a view");
    let geometry = slice_placement(&arrays, "Rad", Y_DIM, X_DIM)
        .expect("the slice resolves")
        .geometry;
    assert!(
        matches!(geometry, GridGeometry::Geostationary(_)),
        "the premise: the committed fixture is placed, got {geometry:?}"
    );
    geometry
}

fn assert_source_only(geometry: &GridGeometry, case: &str) {
    assert!(
        matches!(geometry, GridGeometry::Unsupported { label, .. } if label == SOURCE_ONLY),
        "{case}: expected source-only, placed as {geometry:?}"
    );
}

/// A projection `standard_name` alone identifies the axes: no `axis`, and
/// names the name heuristic does not know.
#[test]
fn a_projection_standard_name_alone_places_the_grid() {
    let want = committed();
    for [x, y] in [
        [
            "projection_x_angular_coordinate",
            "projection_y_angular_coordinate",
        ],
        ["projection_x_coordinate", "projection_y_coordinate"],
    ] {
        let variant = Variant::new(&Restated {
            names: ["columns", "rows"],
            standard_names: [Some(x), Some(y)],
            axis: [false, false],
        });
        assert_eq!(variant.place(Y_DIM, X_DIM), want, "standard_name {x}");
    }
}

/// The same coordinates picked the other way round are still the #1387 swap.
#[test]
fn a_transposed_pick_identified_by_standard_name_is_source_only() {
    let variant = Variant::new(&Restated {
        names: ["columns", "rows"],
        standard_names: [
            Some("projection_x_angular_coordinate"),
            Some("projection_y_angular_coordinate"),
        ],
        axis: [false, false],
    });
    assert_source_only(&variant.place(X_DIM, Y_DIM), "transposed angular pair");
}

/// Nothing says which way round the pair is, so it is not placed in either
/// order (the module doc of `cf::placement`).
#[test]
fn an_unidentifiable_pair_is_source_only() {
    let variant = Variant::new(&Restated {
        names: ["columns", "rows"],
        standard_names: [None, None],
        axis: [false, false],
    });
    assert_source_only(&variant.place(Y_DIM, X_DIM), "columns/rows upright");
    assert_source_only(&variant.place(X_DIM, Y_DIM), "columns/rows transposed");
}

/// Each half of the gate on its own: one coordinate identified in place and
/// the other not identifiable is still not a pair the formula can trust.
#[test]
fn a_half_identified_pair_is_source_only() {
    for (case, restated) in [
        (
            "only y identified",
            Restated {
                names: ["columns", "y"],
                standard_names: [None, Some("projection_y_angular_coordinate")],
                axis: [false, true],
            },
        ),
        (
            "only x identified",
            Restated {
                names: ["x", "rows"],
                standard_names: [Some("projection_x_angular_coordinate"), None],
                axis: [true, false],
            },
        ),
    ] {
        assert_source_only(&Variant::new(&restated).place(Y_DIM, X_DIM), case);
    }
}

/// Without a projection `standard_name`, `axis` alone and the names alone
/// each still identify the pair.
#[test]
fn axis_alone_or_names_alone_place_the_grid() {
    let want = committed();
    let by_axis = Variant::new(&Restated {
        names: ["columns", "rows"],
        standard_names: [None, None],
        axis: [true, true],
    });
    assert_eq!(by_axis.place(Y_DIM, X_DIM), want, "axis alone");
    assert_source_only(&by_axis.place(X_DIM, Y_DIM), "axis alone, transposed");

    let by_name = Variant::new(&Restated {
        names: ["x", "y"],
        standard_names: [None, None],
        axis: [false, false],
    });
    assert_eq!(by_name.place(Y_DIM, X_DIM), want, "names alone");
    assert_source_only(&by_name.place(X_DIM, Y_DIM), "names alone, transposed");
}
