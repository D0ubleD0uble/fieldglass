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

use fieldglass_core::{GridGeometry, LonLatBox, PlaneUnits};

/// Scan order of the decoded raster, as the message's own flags state it.
///
/// `core`'s type, re-exported rather than restated: it is what
/// [`GridGeometry::reprojectable`] is asked alongside, so a second copy here
/// would be a host DTO that the engine then had to convert back (#571). The
/// geometry already accounts for the two direction bits — `forward(0, 0)` is
/// the declared first point whichever way the grid runs — which is why this is
/// the one thing beside it: which way *up* a host should draw the rows.
pub use fieldglass_core::Scan;

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
    ///
    /// **Not gated by the format features (#552).** It is the wire vocabulary a
    /// host compares strings against, and dropping a variant with its decoder
    /// would change the JSON schema for every consumer of the shipped
    /// declarations. A build without `grib1` simply never constructs
    /// [`SourceFormat::Grib1`], because it never opens a session over one.
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

    /// Units the [`Georef`] origin and spacing are expressed in.
    #[serde(rename_all = "snake_case")]
    pub enum AxisUnits {
        /// Degrees **in the plane [`Georef::proj4`] names**, which for
        /// `latlon`, `gaussian` and `lookup` is geographic — `x0`/`dx` are
        /// longitudes, `y0`/`dy` latitudes.
        ///
        /// `rotated_latlon` also reports degrees, and they are **not**
        /// geographic: its CRS is a PROJ `ob_tran` and its corners are
        /// rotated-frame coordinates, which is the whole reason the frame is
        /// named. A host that read them as longitude and latitude would put a
        /// COSMO domain in the Sahara. The unit is not enough on its own; the
        /// CRS beside it is what says what the degrees measure.
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
        ///
        /// A `core` type, like [`geometry`](Self::geometry), so it is described
        /// to a schema consumer the same way — but written out property by
        /// property rather than loosely, because a host reads these three and
        /// #574 generates its declarations from this schema.
        /// `the_scan_schema_names_every_field_scan_serialises` holds the two in
        /// step.
        #[cfg_attr(feature = "schema", schemars(schema_with = "scan_schema"))]
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
        /// The parameter's name, as the table that resolved it states it.
        ///
        /// # The unresolved-parameter contract
        ///
        /// When no table in this build resolves the message's parameter codes,
        /// this is `Parameter <codes>` — the numeric codes that went
        /// unresolved, slash-separated, outermost first:
        ///
        /// | Format | Rendering | Codes |
        /// |---|---|---|
        /// | GRIB2 | `Parameter 209/10/0` | discipline / category / number |
        /// | GRIB1 | `Parameter 98/128/210` | centre / table version / id |
        ///
        /// The codes rather than a bare `"Unknown"` because they are the only
        /// thing that tells a user *which* table is missing, and no other field
        /// carries them: a host reads the discipline as a Code Table 0.0
        /// *name*, which is itself unresolved for a discipline no table
        /// defines. This is the string every host shows — the umbrella, the
        /// wasm binding and the napi binding all render it from the format
        /// crate's own `unresolved_parameter` (#633).
        ///
        /// Empty only when the message has no parameter codes to name at all,
        /// which is a GRIB2 product template carrying no horizontal product
        /// common. [`units`](Self::units) is empty in both cases — an
        /// unresolved parameter has a name to show but no unit to state.
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
        /// The parameter's name, under the same contract as
        /// [`Field::parameter`]: the table's name, or `Parameter <codes>`
        /// naming the codes no table in this build resolved.
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
        /// Forecast time relative to `reference_time`, rendered — `"+6h"`, or
        /// `"+30 Minute"` for a unit the edition does not convert to hours.
        pub forecast: String,
        /// Which packing the data section uses, named — what decodes it, and
        /// the first thing to look at when a decode is wrong.
        pub packing: String,
        /// The grid the message **declares**, not the one its field is decoded
        /// onto.
        ///
        /// The two differ for the families that carry no raster of their own:
        /// a spectral message declares a `"spherical_harmonic"` grid and a
        /// HEALPix one a `"healpix"` grid, both of kind `"unsupported"` —
        /// nothing places a point on either — while
        /// [`crate::Session::decode`] hands back a field on the synthesised
        /// `"latlon"` grid (#580). This is the message list's answer, so it
        /// describes the file; [`Field::georef`] is the field's, so it
        /// describes where the values are. [`size_label`](Self::size_label)
        /// names the native shape beside it.
        ///
        /// `None` only for a GRIB1 message that carries no §2 at all: its grid
        /// is whatever `pds.grid_number` predefines, which this call does not
        /// resolve.
        pub grid: Option<Georef>,
        /// How the file names its own grid where `Ni × Nj` is not how it is
        /// described — `N32`, `O1280`, `T639`.
        pub size_label: Option<String>,
    }

    /// A resampled raster: [`crate::Session::warp`] without the paint step.
    #[cfg(feature = "render")]
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

    /// One entry of the field-combine vocabulary: what a host's Compare picker
    /// shows and what it sends back.
    ///
    /// [`crate::combine_ops`] builds the whole list from
    /// [`CombineOp::ALL`](fieldglass_core::CombineOp::ALL), so both hosts offer
    /// the same operations in the same order and an op added to the enum
    /// reaches each picker without either being edited (#342).
    #[cfg(feature = "analysis")]
    #[serde(rename_all = "camelCase")]
    #[cfg_attr(feature = "schema", schemars(rename_all = "camelCase"))]
    pub struct CombineOpInfo {
        /// The stable wire tag, which is what
        /// [`Session::combine`](crate::Session::combine)'s host wrappers parse
        /// back — `"a_minus_b"`, `"ratio"`.
        pub value: String,
        /// The menu label, e.g. `"A − B"`.
        pub label: String,
    }

    /// One level's isoline segments, in grid coordinates.
    #[cfg(feature = "analysis")]
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
        // list of cell centres, an unmodelled grid) reports nothing rather
        // than a plausible-looking zero. A rotated lat/lon grid has a plane —
        // its own rotated frame, measured in degrees — so it reports one.
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
            bounds_lonlat: geom.lonlat_bbox().map(LonLatBox::to_array),
            proj4: geom.proj4(),
            axis_units,
            x0,
            y0,
            dx,
            dy,
            periodic_x: geom.is_periodic_x(),
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

/// How [`Georef::scan`] is described to a schema consumer.
///
/// Written out rather than left loose like [`geometry_schema`]: these three
/// booleans are what a host actually reads off a `Georef`, and #574 generates
/// `native.ts` from this document. `core` cannot derive the schema itself —
/// that would follow `schemars` into every format crate — so the property list
/// is restated here and
/// `the_scan_schema_names_every_field_scan_serialises` asserts it against what
/// `Scan` really serialises to.
#[cfg(feature = "schema")]
fn scan_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({
        "type": "object",
        "required": ["iNegative", "jPositive", "jConsecutive"],
        "properties": {
            "iNegative": { "type": "boolean" },
            "jPositive": { "type": "boolean" },
            "jConsecutive": { "type": "boolean" }
        },
        "description": "fieldglass_core::Scan: the message's own scanning-mode direction flags."
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use fieldglass_core::{
        LambertParams, LambertProjector, LatLonParams, PlanarGridProjector, RotatedLatLonParams,
    };

    /// [`scan_schema`] restates `Scan`'s three properties by hand, because
    /// `core` cannot derive `JsonSchema` — the derive would follow it into every
    /// format crate. A restatement drifts, so this is what holds the two in
    /// step: the schema's property names must be exactly the keys `Scan`
    /// serialises to, and a field added, renamed or dropped in `core` fails
    /// here rather than in a `.d.ts` #574 generates from the schema.
    #[cfg(feature = "schema")]
    #[test]
    fn the_scan_schema_names_every_field_scan_serialises() {
        let serialised = serde_json::to_value(Scan::new(true, false, true))
            .expect("Scan serialises to an object");
        let mut wire: Vec<String> = serialised
            .as_object()
            .expect("an object")
            .keys()
            .cloned()
            .collect();
        wire.sort();

        let schema = scan_schema(&mut schemars::SchemaGenerator::default());
        let json = serde_json::to_value(&schema).expect("the schema is JSON");
        let mut declared: Vec<String> = json["properties"]
            .as_object()
            .expect("the schema declares properties")
            .keys()
            .cloned()
            .collect();
        declared.sort();
        assert_eq!(declared, wire, "scan_schema must name what Scan serialises");

        let mut required: Vec<String> = json["required"]
            .as_array()
            .expect("the schema lists required properties")
            .iter()
            .map(|v| v.as_str().expect("a name").to_string())
            .collect();
        required.sort();
        assert_eq!(
            required, wire,
            "every property Scan always writes must be required"
        );
    }

    fn scan() -> Scan {
        Scan::north_down()
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

    /// A rotated grid's plane is its own rotated frame, so the units a host
    /// reads are degrees and the origin is the corner the message states —
    /// *not* a geographic one. A host that read these as geographic would put
    /// a COSMO domain in the Sahara, which is why the pair is asserted here
    /// beside the CRS that measures them. `grid_geometry_proj.rs` is where the
    /// numbers are checked against PROJ.
    #[test]
    fn a_rotated_grid_reports_its_rotated_frame_in_degrees() {
        let p = RotatedLatLonParams {
            ni: 40,
            nj: 42,
            lat_first: -20.0,
            lon_first: -18.0,
            lat_last: 21.0,
            lon_last: 21.0,
            south_pole_lat: -40.0,
            south_pole_lon: 10.0,
            angle_of_rotation: 0.0,
        };
        let g = Georef::from_geometry(&GridGeometry::RotatedLatLon(p), scan());
        assert!(matches!(g.axis_units, AxisUnits::Degrees));
        assert!(
            g.proj4
                .as_deref()
                .is_some_and(|s| s.starts_with("+proj=ob_tran ")),
            "{:?}",
            g.proj4
        );
        assert_eq!((g.x0, g.y0), (Some(-18.0), Some(-20.0)));
        assert_eq!((g.dx, g.dy), (Some(1.0), Some(1.0)));
        // And it is not the geographic corner: the first point is over the
        // Atlantic off Morocco, nowhere near (-20, -18).
        let (lat, lon) = GridGeometry::RotatedLatLon(p)
            .forward(0, 0)
            .expect("placed");
        assert!((lat - 27.695_222_279).abs() < 1e-6, "{lat}");
        assert!((lon - -9.144_631_530).abs() < 1e-6, "{lon}");
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
