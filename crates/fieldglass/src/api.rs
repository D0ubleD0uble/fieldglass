//! The plain-data types every host binds (ADR-0006 decision 2).
//!
//! The rules these follow, and the reason each one is a rule:
//!
//! * **No generics, lifetimes, or trait objects.** A host binding is generated
//!   from the shape; a lifetime has no representation on the other side of a
//!   language boundary.
//! * **Bulk data is contiguous, with a separate `u8` mask.** `Vec<Option<f64>>`
//!   is the engine's shape and costs a branch per element to cross a seam; it
//!   is also not a typed array. The mask is a byte per cell rather than `NaN`
//!   because `isnan()` is unreliable on some mobile GPUs and a `NaN` poisons
//!   linear filtering in a texture.
//! * **The element type follows the source.** [`Values`] is `f64` unless the
//!   decoded field is exactly representable in `f32`; see [`Dtype::Auto`].
//!   A host that wants an `R32F` texture regardless asks for it by name and
//!   gets the lossy conversion it chose.
//! * **Strings only for labels.** `kind`, `proj4`, `parameter`, `units`.
//! * **`#[non_exhaustive]`, serde derives, and (under the `schema` feature) a
//!   JSON Schema**, which is what a host's declarations are generated from.

use fieldglass_core::{GridGeometry, PlaneUnits, eastward_lon_span, lon_grid_is_global};

/// Convenience: every API type derives the same set. Each states its own
/// `rename_all`.
///
/// Structs are **`camelCase` on the wire**, `snake_case` in Rust: every host
/// binding this crate today is JavaScript-shaped, napi-rs renames
/// automatically, and the two hosts would otherwise disagree about the same
/// field's name. Enum *variants* stay `snake_case`, because a variant tag is a
/// wire value a host compares strings against and `"polar_stereo"` is the one
/// `core` already reports.
macro_rules! api_type {
    ($($item:item)*) => {
        $(
            #[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
            #[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
            #[non_exhaustive]
            $item
        )*
    };
}

api_type! {
    /// The container a session was opened from.
    ///
    /// Named for the source rather than `Format` so it does not collide with
    /// `core`'s detection enum, which also answers NetCDF and "unknown" — this
    /// one enumerates only what a session can actually be.
    #[serde(rename_all = "snake_case")]
    pub enum SourceFormat {
        /// WMO FM 92 GRIB edition 1.
        Grib1,
        /// WMO FM 92 GRIB edition 2.
        Grib2,
    }

    /// Which element type a caller wants back from a decode.
    #[derive(Default)]
    #[serde(rename_all = "snake_case")]
    pub enum Dtype {
        /// Whatever the source supports losslessly — the default, and the only
        /// setting that never discards precision.
        ///
        /// [`Values::F32`] comes back **only if every present value survives the
        /// round trip**, otherwise [`Values::F64`] does. That is stricter than
        /// "the packing used 24 bits or fewer", and deliberately so. A
        /// simple-packed value is `(R + X·2ᴱ)·10⁻ᴰ`: at 24 bits the *ordinals*
        /// fit an `f32` mantissa, but the values need `log2(max|v| / 2ᴱ)` bits
        /// once the reference value sits far from zero relative to the quantum,
        /// and a non-zero decimal scale factor makes the quantum a negative
        /// power of ten, which no binary float represents exactly at all.
        /// Checking the decoded numbers costs one pass and answers the question
        /// the bit count only approximates: does this field fit?
        ///
        /// Masked cells do not participate — their value is not data.
        #[default]
        Auto,
        /// Narrow to `f32` whatever the source was. The caller has decided the
        /// loss is acceptable (an `R32F` texture, say).
        F32,
        /// Widen to `f64` whatever the source was.
        F64,
    }

    /// Scan order of the decoded raster, as the message's own flags state it.
    ///
    /// The geometry already accounts for the scan — `forward(0, 0)` is the
    /// declared first point whichever way the grid runs — so this is here for
    /// the one thing geometry cannot answer: which way *up* a host should draw
    /// the rows. A `j_positive` grid scans south-to-north, so a north-up canvas
    /// flips it.
    ///
    /// [`Self::j_consecutive`] is the odd one out and is **descriptive only**:
    /// the decoders transpose such a field before it reaches a caller, so the
    /// raster this describes is row-major whatever that field says. Acting on
    /// it transposes twice.
    #[serde(rename_all = "camelCase")]
    #[cfg_attr(feature = "schema", schemars(rename_all = "camelCase"))]
    pub struct Scan {
        /// Points run east→west rather than west→east.
        pub i_negative: bool,
        /// Rows run south→north rather than north→south.
        pub j_positive: bool,
        /// The *message* stores adjacent points consecutive in `j`
        /// (column-major). Descriptive only — the decoded raster has already
        /// been transposed into rows; see the type docs.
        pub j_consecutive: bool,
    }

    /// Units the [`Georef`] origin and spacing are expressed in.
    #[serde(rename_all = "snake_case")]
    pub enum AxisUnits {
        /// Geographic families: `x0`/`dx` are longitudes, `y0`/`dy` latitudes.
        Degrees,
        /// Projected families: `x0`/`y0` are the grid origin in the projection
        /// plane described by [`Georef::proj4`], with no false easting or
        /// northing, and `dx`/`dy` are metre spacings carrying the scan sign.
        Metres,
    }

    /// Where a decoded field sits on the Earth, flattened to scalars.
    ///
    /// A browser map library needs two things and this carries both: a CRS it
    /// can name ([`proj4`](Self::proj4)) and an affine placing the raster in
    /// that CRS (`x0`, `y0`, `dx`, `dy`). Everything is `Option` because a
    /// family that cannot state it says so rather than guessing — a Gaussian
    /// grid's rows are not uniformly spaced, so its `dy` is absent, and a grid
    /// this build does not model has none of it.
    #[serde(rename_all = "camelCase")]
    #[cfg_attr(feature = "schema", schemars(rename_all = "camelCase"))]
    pub struct Georef {
        /// The grid itself, as `core` models it.
        ///
        /// The scalars below are a flattened *view* of this, which is what a
        /// host reads; this field is what the engine needs back to place a
        /// point — `warp`, `probe`, and `contours` all invert through it, and
        /// the inverse is a projection, not something four scalars reconstruct.
        /// Keeping the two together means a field carries everything an
        /// operation on it needs, which is the whole point of the host owning
        /// the memory. It serialises, so a `Field` still survives a JSON round
        /// trip whole.
        ///
        /// `core` does not derive `JsonSchema` (it is a decode-side crate and
        /// the derive would follow it everywhere), so under the `schema`
        /// feature this field is described as the tagged object it serialises
        /// to. A generated `.d.ts` types it loosely and a host does not read it.
        #[cfg_attr(feature = "schema", schemars(schema_with = "geometry_schema"))]
        pub geometry: GridGeometry,
        /// The family tag `core` reports: `latlon`, `gaussian`, `mercator`,
        /// `rotated_latlon`, `lambert`, `polar_stereo`, `transverse_mercator`,
        /// `lambert_azimuthal`, `space_view`, `lookup`, or `unsupported`.
        pub kind: String,
        /// The most specific name available — the decoder's own grid-type
        /// string for a family this build does not model.
        pub label: String,
        /// Grid columns (west-to-east point count of one row).
        pub ni: u32,
        /// Grid rows.
        pub nj: u32,
        /// `[lat_min, lat_max, lon_min, lon_max]` in degrees. `lon_min` may
        /// fall below -180 (or `lon_max` above 180) to describe a window
        /// spanning the antimeridian; do not normalise it into range without
        /// collapsing the span.
        pub bounds_lonlat: Option<[f64; 4]>,
        /// A PROJ string for the grid's own plane, for a map library that
        /// takes one. `None` for a family this build does not name a CRS for.
        pub proj4: Option<String>,
        /// What `x0` / `y0` / `dx` / `dy` are measured in.
        pub axis_units: AxisUnits,
        /// Plane coordinate of the first grid point's cell centre, along the
        /// column axis. `None` when the family has no affine.
        pub x0: Option<f64>,
        /// Plane coordinate of the first grid point's cell centre, along the
        /// row axis.
        pub y0: Option<f64>,
        /// Signed step between columns, in [`Georef::axis_units`].
        pub dx: Option<f64>,
        /// Signed step between rows. Negative for the usual north-to-south
        /// scan, so `y0 + j * dy` walks the rows as stored.
        pub dy: Option<f64>,
        /// The grid closes on itself in the column axis: one column step past
        /// the last column lands back on the first. A renderer wraps rather
        /// than clamping there, or the seam meridian draws as a hole.
        pub periodic_x: bool,
        /// The scan order the message's own flags state, so a consumer can
        /// walk `values` without re-deriving it.
        pub scan: Scan,
    }

    /// Decoded values, in whichever element type the source supports.
    #[serde(rename_all = "snake_case", tag = "dtype", content = "data")]
    pub enum Values {
        /// Single precision — what a source packed to 32 bits or fewer decodes
        /// to, and what [`Dtype::F32`] forces.
        F32(Vec<f32>),
        /// Double precision — an IEEE-64 source, or [`Dtype::F64`].
        F64(Vec<f64>),
    }

    /// Range and count of the present cells. Absent cells are excluded, so an
    /// all-masked field reports no range at all rather than `±inf`.
    #[serde(rename_all = "camelCase")]
    #[cfg_attr(feature = "schema", schemars(rename_all = "camelCase"))]
    pub struct Stats {
        /// Smallest present value. `None` when no cell is present.
        pub min: Option<f64>,
        /// Largest present value. `None` when no cell is present.
        pub max: Option<f64>,
        /// How many cells are present, i.e. the count of `1`s in the mask.
        pub valid_count: u32,
    }

    /// One decoded field: the values, where they sit, and what they are.
    ///
    /// The host owns this. The façade keeps no decode cache — linear memory
    /// never shrinks and an animation holds many fields — so a field is handed
    /// out once and passed back by reference to every operation that consumes
    /// one.
    #[serde(rename_all = "camelCase")]
    #[cfg_attr(feature = "schema", schemars(rename_all = "camelCase"))]
    pub struct Field {
        /// The cell values in scan order, `ni * nj` of them. Read `mask`
        /// before a value: an absent cell still occupies its slot.
        pub values: Values,
        /// One byte per cell: `1` present, `0` absent. Same length as `values`.
        pub mask: Vec<u8>,
        /// Grid columns.
        pub ni: u32,
        /// Grid rows.
        pub nj: u32,
        /// Where the cells sit on the Earth.
        pub georef: Georef,
        /// Range and count over the present cells.
        pub stats: Stats,
        /// The parameter's human-readable name, or `"Unknown"` when no table
        /// in this build resolves the message's parameter id.
        pub parameter: String,
        /// The parameter's units as its table states them. Empty when the
        /// parameter did not resolve, or is dimensionless.
        pub units: String,
    }

    /// One message's metadata, built on demand.
    ///
    /// Lazy by design: a thousand-message file should not serialise a thousand
    /// of these to open.
    #[serde(rename_all = "camelCase")]
    #[cfg_attr(feature = "schema", schemars(rename_all = "camelCase"))]
    pub struct MessageInfo {
        /// Position in `0..Session::count()`; the handle every other call
        /// takes.
        pub index: u32,
        /// Byte offset of the message's first byte within the container, so a
        /// host can range-fetch this message alone on a later visit.
        pub offset_bytes: u64,
        /// Human-readable parameter name, or `"Unknown"`.
        pub parameter: String,
        /// The table's short name for the parameter, e.g. `"2t"`. Empty when
        /// the parameter did not resolve.
        pub abbreviation: String,
        /// Units as the parameter's table states them.
        pub units: String,
        /// The level, rendered — `"500 hPa"`, `"2 m above ground"`.
        pub level: String,
        /// The level's surface type on its own, for grouping messages that
        /// share a surface at different values.
        pub level_type: String,
        /// Reference (analysis) time as RFC 3339. `None` when the message
        /// carries no usable date.
        pub reference_time: Option<String>,
        /// Forecast time relative to `reference_time`, rendered — `"+6 h"`.
        pub forecast: String,
        /// Which packing the data section uses, named — what decodes it, and
        /// the first thing to look at when a decode is wrong.
        pub packing: String,
        /// `None` when the message carries no grid at all (a spectral field).
        pub grid: Option<Georef>,
        /// How the file names its own grid where `Ni × Nj` is not how it is
        /// described — `N32`, `O1280`, `T639`.
        pub size_label: Option<String>,
    }

    /// A resampled raster: [`crate::Session::warp`] without the paint step.
    #[serde(rename_all = "camelCase")]
    #[cfg_attr(feature = "schema", schemars(rename_all = "camelCase"))]
    pub struct Warped {
        /// Resampled values, row-major from the north-west corner of `bounds`.
        pub values: Vec<f32>,
        /// One byte per output pixel: `1` present, `0` off-grid or masked.
        pub mask: Vec<u8>,
        /// Output columns.
        pub width: u32,
        /// Output rows.
        pub height: u32,
        /// `[lat_min, lat_max, lon_min, lon_max]` of the output window.
        pub bounds: [f64; 4],
    }

    /// One point sampled out of a field.
    #[serde(rename_all = "camelCase")]
    #[cfg_attr(feature = "schema", schemars(rename_all = "camelCase"))]
    pub struct Probe {
        /// Latitude asked for, echoed back.
        pub lat: f64,
        /// Longitude asked for, echoed back.
        pub lon: f64,
        /// Fractional column / row the point landed on.
        pub i: f64,
        /// Fractional row the point landed on.
        pub j: f64,
        /// `None` when the cell is masked.
        pub value: Option<f64>,
    }

    /// One level's isoline segments, in grid coordinates.
    #[serde(rename_all = "camelCase")]
    #[cfg_attr(feature = "schema", schemars(rename_all = "camelCase"))]
    pub struct Isoline {
        /// The level these segments trace.
        pub value: f64,
        /// `[i0, j0, i1, j1]` per segment, in fractional grid indices.
        pub segments: Vec<[f64; 4]>,
    }
}

impl Values {
    /// Number of elements.
    pub fn len(&self) -> usize {
        match self {
            Self::F32(v) => v.len(),
            Self::F64(v) => v.len(),
        }
    }

    /// Whether there are no elements at all.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Which width this holds.
    ///
    /// A host reads this rather than matching the enum: [`Values`] is
    /// `#[non_exhaustive]`, so a `match` in a downstream crate needs a wildcard
    /// arm, and a wildcard arm is exactly where a future variant would be
    /// silently mishandled.
    pub fn dtype(&self) -> Dtype {
        match self {
            Self::F32(_) => Dtype::F32,
            Self::F64(_) => Dtype::F64,
        }
    }

    /// The `f32` payload, or `None` when this is not `f32`.
    pub fn as_f32(&self) -> Option<&[f32]> {
        match self {
            Self::F32(v) => Some(v),
            _ => None,
        }
    }

    /// The `f64` payload, or `None` when this is not `f64`.
    pub fn as_f64(&self) -> Option<&[f64]> {
        match self {
            Self::F64(v) => Some(v),
            _ => None,
        }
    }

    /// Read one element as `f64`.
    pub fn get(&self, i: usize) -> Option<f64> {
        match self {
            Self::F32(v) => v.get(i).map(|&x| f64::from(x)),
            Self::F64(v) => v.get(i).copied(),
        }
    }

    /// Every element as `f64`, borrowing where it already is.
    pub fn to_f64(&self) -> std::borrow::Cow<'_, [f64]> {
        match self {
            Self::F64(v) => std::borrow::Cow::Borrowed(v),
            Self::F32(v) => std::borrow::Cow::Owned(v.iter().map(|&x| f64::from(x)).collect()),
        }
    }

    /// Narrow to `f32` **only if every present value survives the round trip**,
    /// otherwise keep `f64`.
    ///
    /// The rule and why it is stricter than a bit count are on [`Dtype::Auto`],
    /// which is where a caller meets it: this is private, and a doc a reader
    /// cannot reach is not where the explanation belongs (#582).
    fn narrow(values: Vec<f64>, mask: &[u8]) -> Self {
        let exact = values
            .iter()
            .zip(mask)
            .all(|(&v, &m)| m == 0 || f64::from(v as f32) == v);
        if exact {
            Self::F32(values.into_iter().map(|v| v as f32).collect())
        } else {
            Self::F64(values)
        }
    }

    /// Build from decoded `f64`s under a caller's [`Dtype`] request.
    pub(crate) fn build(values: Vec<f64>, mask: &[u8], dtype: Dtype) -> Self {
        match dtype {
            Dtype::Auto => Self::narrow(values, mask),
            Dtype::F32 => Self::F32(values.into_iter().map(|v| v as f32).collect()),
            Dtype::F64 => Self::F64(values),
        }
    }
}

impl Georef {
    /// Flatten a [`GridGeometry`] and the message's scan flags into the
    /// scalar form a host consumes.
    ///
    /// The projected families report their origin and spacing in the
    /// projection plane, which is what [`GridGeometry::proj4`] describes — the
    /// grid origin is applied on top of the CRS, not baked into it, so a host
    /// placing the raster needs both halves and this carries them together.
    pub fn from_geometry(geom: &GridGeometry, scan: Scan) -> Self {
        let (ni, nj) = geom.dims().unwrap_or((0, 0));
        // One question, asked of `core`: a family that has a plane reports its
        // origin and step in that plane's own units, and one that has none (a
        // list of cell centres, a rotated frame with no CRS, an unmodelled
        // grid) reports nothing rather than a plausible-looking zero.
        let affine = geom.plane_affine();
        let axis_units = match affine.map(|a| a.units) {
            Some(PlaneUnits::Metres) => AxisUnits::Metres,
            Some(PlaneUnits::Degrees) | None => AxisUnits::Degrees,
        };
        let (x0, y0) = (affine.map(|a| a.x0), affine.map(|a| a.y0));
        let (dx, dy) = (affine.and_then(|a| a.dx), affine.and_then(|a| a.dy));
        Self {
            geometry: geom.clone(),
            kind: geom.kind().to_string(),
            label: geom.label().to_string(),
            ni,
            nj,
            bounds_lonlat: geom
                .lonlat_bbox()
                .map(|(lat_min, lat_max, lon_min, lon_max)| [lat_min, lat_max, lon_min, lon_max]),
            proj4: geom.proj4(),
            axis_units,
            x0,
            y0,
            dx,
            dy,
            periodic_x: geometry_is_periodic_x(geom),
            scan,
        }
    }
}

/// How [`Georef::geometry`] is described to a schema consumer: an object
/// discriminated by `kind`, whose per-family payload is `core`'s and not part
/// of the host-facing contract.
#[cfg(feature = "schema")]
fn geometry_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({
        "type": "object",
        "required": ["kind"],
        "properties": { "kind": { "type": "string" } },
        "description": "fieldglass_core::projection::GridGeometry, serde-tagged by `kind`."
    })
}

/// Whether the grid's column axis closes on itself.
///
/// Only the geographic families can be: a projected grid's columns are
/// projection-plane metres, and no finite number of them wraps the Earth.
/// Judged from the corner longitudes and the column count, which is exactly
/// the rule the warp's `SourceGrid::periodic_i` is set from.
fn geometry_is_periodic_x(geom: &GridGeometry) -> bool {
    match geom {
        GridGeometry::LatLon(p) => {
            lon_grid_is_global(eastward_lon_span(p.lon_first, p.lon_last), p.ni)
        }
        GridGeometry::Gaussian(p) => {
            lon_grid_is_global(eastward_lon_span(p.lon_first, p.lon_last), p.ni)
        }
        // Evenly spaced in longitude like the two above, so the same corner
        // test decides it.
        GridGeometry::Mercator(p) => {
            lon_grid_is_global(eastward_lon_span(p.lon_first, p.lon_last), p.ni)
        }
        // Judged in its own rotated frame, which is the frame its corners and
        // its inverse map are both stated in. A rotated grid that closes on
        // itself there closes on itself on the sphere too.
        GridGeometry::RotatedLatLon(p) => {
            lon_grid_is_global(eastward_lon_span(p.lon_first, p.lon_last), p.ni)
        }
        // A projected grid's columns are projection-plane metres, and no finite
        // number of them wraps the Earth.
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fieldglass_core::{LambertParams, LambertProjector, LatLonParams, PlanarGridProjector};

    fn scan() -> Scan {
        Scan {
            i_negative: false,
            j_positive: false,
            j_consecutive: false,
        }
    }

    /// A field packed at a negative power of two round-trips through `f32`
    /// and narrows; the same field scaled by a power of ten does not, and
    /// stays `f64` rather than losing digits silently.
    #[test]
    fn auto_narrows_only_what_survives_the_round_trip() {
        let binary: Vec<f64> = (0..64).map(|k| 273.0 + f64::from(k) / 16.0).collect();
        let mask = vec![1u8; binary.len()];
        assert!(
            matches!(Values::build(binary, &mask, Dtype::Auto), Values::F32(_)),
            "a binary-quantised field fits f32"
        );

        let decimal: Vec<f64> = (0..64).map(|k| 273.0 + f64::from(k) / 10.0).collect();
        assert!(
            matches!(Values::build(decimal, &mask, Dtype::Auto), Values::F64(_)),
            "a decimal-scaled field does not, and must not be narrowed by default"
        );
    }

    /// A masked cell's value is not data, so it must not decide the width of
    /// the whole field.
    #[test]
    fn masked_cells_do_not_veto_narrowing() {
        let values = vec![1.0, 0.1, 2.0];
        let mask = vec![1, 0, 1];
        assert!(matches!(
            Values::build(values, &mask, Dtype::Auto),
            Values::F32(_)
        ));
    }

    #[test]
    fn an_explicit_dtype_overrides_the_source() {
        let values = vec![0.1, 0.2];
        let mask = vec![1, 1];
        assert!(matches!(
            Values::build(values.clone(), &mask, Dtype::F32),
            Values::F32(_)
        ));
        assert!(matches!(
            Values::build(values, &mask, Dtype::F64),
            Values::F64(_)
        ));
    }

    /// A global 1° grid wraps; a regional window does not.
    #[test]
    fn periodic_x_follows_the_columns_not_the_span() {
        let global = GridGeometry::LatLon(LatLonParams {
            ni: 360,
            nj: 181,
            lat_first: 90.0,
            lon_first: 0.0,
            lat_last: -90.0,
            lon_last: 359.0,
        });
        assert!(Georef::from_geometry(&global, scan()).periodic_x);

        let regional = GridGeometry::LatLon(LatLonParams {
            ni: 100,
            nj: 50,
            lat_first: 50.0,
            lon_first: -120.0,
            lat_last: 25.0,
            lon_last: -70.0,
        });
        assert!(!Georef::from_geometry(&regional, scan()).periodic_x);
    }

    /// The affine a host places the raster with must reproduce the grid's own
    /// forward map: `x0 + i·dx` inverted through the CRS is grid point `i`.
    #[test]
    fn the_lambert_affine_matches_the_grids_own_forward_map() {
        let p = LambertParams {
            earth_radius_m: 6_371_229.0,
            ni: 100,
            nj: 80,
            lat_first: 20.0,
            lon_first: -120.0,
            lad: 25.0,
            lov: -95.0,
            dx_metres: 12_000.0,
            dy_metres: 12_000.0,
            latin1: 25.0,
            latin2: 25.0,
        };
        let geom = GridGeometry::Lambert(p);
        let g = Georef::from_geometry(&geom, scan());
        assert!(matches!(g.axis_units, AxisUnits::Metres));
        let proj = LambertProjector::new(p);
        for (i, j) in [(0u32, 0u32), (7, 3), (99, 79)] {
            let (lat, lon) = geom.forward(i, j).expect("grid point");
            let (x, y) = proj.forward_xy(lat, lon);
            let want_x = g.x0.unwrap() + f64::from(i) * g.dx.unwrap();
            let want_y = g.y0.unwrap() + f64::from(j) * g.dy.unwrap();
            assert!((x - want_x).abs() < 1e-3, "x at ({i},{j}): {x} != {want_x}");
            assert!((y - want_y).abs() < 1e-3, "y at ({i},{j}): {y} != {want_y}");
        }
    }

    /// Gaussian rows are not uniformly spaced, so the affine must not claim
    /// one. A host reading `dy = Some(..)` would misplace every row but the
    /// middle.
    #[test]
    fn a_gaussian_grid_reports_no_row_spacing() {
        let geom = GridGeometry::Gaussian(fieldglass_core::GaussianParams {
            ni: 128,
            nj: 64,
            lat_first: 87.863_799,
            lon_first: 0.0,
            lat_last: -87.863_799,
            lon_last: 357.1875,
            n_parallels: 32,
        });
        let g = Georef::from_geometry(&geom, scan());
        assert!(g.dx.is_some());
        assert_eq!(g.dy, None);
    }
}
