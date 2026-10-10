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

use fieldglass_core::{CornerPair, GridGeometry, LonLatBox, PlaneUnits};

/// Scan order of the decoded values: the order a field's `values` are stored
/// in, which for a column-major message is not the order the message stored
/// (#792). See [`Georef::scan`].
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
        /// NetCDF, either classic (CDF-1/2/5) or NetCDF-4 over HDF5.
        ///
        /// Spelled `netcdf` on the wire, not the `snake_case` derive's
        /// `net_cdf`: the other two variants are `grib1` and `grib2`, and a
        /// host comparing strings should not have to remember that one of the
        /// three grew an underscore from how the Rust identifier is
        /// capitalised. Pinned rather than left to the rename rule because this
        /// is the vocabulary the JSON schema publishes; #662 added the variant
        /// and it has not shipped in a release, so this is the last moment
        /// changing it is free.
        #[serde(rename = "netcdf")]
        NetCdf,
        /// A Zarr store, v2 or v3, opened from its objects with
        /// `Session::open_store` (#704, behind the `zarr` feature).
        Zarr,
    }

    /// How a container is addressed: what a caller asks for to get a field.
    ///
    /// The two shapes are not a detail of the format, they are what the format
    /// *is*. GRIB is a stream of self-describing messages, so a message index
    /// is the whole address. NetCDF and Zarr hold named variables of two or
    /// more dimensions, so a field is a variable plus which axes are the
    /// horizontal pair plus where the caller is standing on the others — four
    /// things, and no ordering of them is more natural than another.
    ///
    /// A host asks this once, on open, and then knows which half of
    /// [`crate::Session`] applies. Everything downstream of a decode —
    /// `render`, `probe`, `contours`, `combine`, `warp`, `palette` — takes a
    /// [`Field`] and is the same for both.
    #[serde(rename_all = "snake_case")]
    pub enum Addressing {
        /// A flat list of messages: [`crate::Session::count`],
        /// [`crate::Session::message`], [`crate::Session::decode`].
        Messages,
        /// Named variables over shared dimensions:
        /// [`crate::Session::variables`], [`crate::Session::dimensions`],
        /// [`crate::Session::decode_slice`].
        Variables,
    }

    /// One dimension of an array dataset, as the file names it.
    ///
    /// Shared: two variables that name the same dimension are on the same axis,
    /// which is what lets a host offer one time slider for a whole file rather
    /// than one per variable.
    #[serde(rename_all = "camelCase")]
    pub struct DimensionInfo {
        /// The dimension's name, path-qualified for a nested group.
        pub name: String,
        /// How many points it has.
        pub length: u64,
    }

    /// One array a container holds and this build will not read, and why.
    ///
    /// Beside [`crate::Session::variables`] rather than inside it: a host shows
    /// these so a user can see *why* a name is absent from the list, instead of
    /// wondering whether the file has it. Not an error — a container reads every
    /// array it can and leaves the rest here (#709).
    #[serde(rename_all = "camelCase")]
    pub struct LeftOutArray {
        /// The array's name, spelled as a readable one would be, so a host can
        /// match it against the list it did get.
        pub name: String,
        /// Why, as the reader phrased it.
        pub reason: String,
    }

    /// One renderable variable of an array dataset.
    #[serde(rename_all = "camelCase")]
    pub struct VariableInfo {
        /// Position in [`crate::Session::variables`], and the handle
        /// [`crate::Session::decode_slice`] takes.
        pub index: u32,
        /// The variable's name, path-qualified for a nested group.
        pub name: String,
        /// Its axes in declared (C) order — the order `y_dim` and `x_dim`
        /// index into, and the order `slice_indices` is given in.
        pub dims: Vec<DimensionInfo>,
        /// The element type as the file declares it, named.
        pub dtype: String,
        /// Units from the variable's own attributes, or `None` when it states
        /// none (#775).
        pub units: Option<String>,
        /// Which axis the file's own conventions say is latitude, when they say
        /// so. `None` for a WRF or satellite file, whose horizontal axes are
        /// projected and carry no CF axis attribute — the caller picks, which is
        /// why this is a hint rather than the answer.
        pub detected_y_dim: Option<u32>,
        /// The longitude half of [`Self::detected_y_dim`], under the same rule.
        pub detected_x_dim: Option<u32>,
        /// Which axis is time: the one whose coordinate the file's conventions
        /// mark as time (`axis = "T"`, `standard_name = "time"`, or units of the
        /// form `hours since …`), or one named `time`. Never an image axis. What
        /// a host animates along (#170).
        pub detected_time_dim: Option<u32>,
    }

    /// One axis of a variable, with the coordinate values along it (#171).
    ///
    /// What a host labels a cross-section's axes from: a plot of any two
    /// dimensions needs the numbers down its side and along its foot, and those
    /// are the 1-D coordinate array CF names after the dimension. `coordinates`
    /// is `None` when the container holds no such array — an axis is then its
    /// own index — and `units` is empty when it states none.
    #[serde(rename_all = "camelCase")]
    pub struct AxisValues {
        /// The dimension's name, as the container spells it.
        pub dimension: String,
        /// How many points it has, whether or not it has coordinates.
        pub length: u64,
        /// The coordinate value at each index, in index order.
        pub coordinates: Option<Vec<f64>>,
        /// The coordinate array's own `units`, or `None` when it states none or
        /// the axis has no coordinate array (#775). For a time axis this is the
        /// CF form, `hours since 2020-01-01`.
        pub units: Option<String>,
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

    /// Units an [`Affine`] origin and spacing are expressed in.
    #[serde(rename_all = "snake_case")]
    pub enum AxisUnits {
        /// Degrees **in the plane [`Georef::proj4`] names**, which for
        /// `latlon` and `gaussian` is geographic — `x0`/`dx` are longitudes,
        /// `y0`/`dy` latitudes.
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

    /// Whether a grid can be placed on the Earth, and why not when it cannot
    /// (#776).
    ///
    /// The reason [`Georef::corners`] alone does not give: a `null` corner pair
    /// can mean there is nothing to place, or that there is a raster and the
    /// projection cannot place it, and a host acts differently on the two. The
    /// first has nothing to draw. The second still draws in its own grid
    /// coordinates and only the map is missing.
    ///
    /// A sibling of the value rather than a wrapper around it (#574's decision):
    /// the corners stay a plain `[f64; 4] | null`, and this says which absence
    /// a `null` is.
    #[derive(Copy, Eq)]
    #[serde(rename_all = "snake_case")]
    pub enum Placement {
        /// The grid has a raster and its projection places at least part of it:
        /// [`Georef::bounds_lonlat`] is present.
        ///
        /// Not a promise that every cell has a position. A full-disc satellite
        /// image is placed, and its corner pixels look past the Earth, so its
        /// [`Georef::corners`] is `null` while the disc itself is on the map.
        Placed,
        /// There is genuinely nothing to place: the family carries no grid
        /// points of its own (spectral coefficients, HEALPix pixels, bi-Fourier
        /// coefficients), the grid declares zero columns or rows, or the
        /// message states no grid at all (GRIB1 predefined grid 255).
        ///
        /// Not "this build cannot read the grid": a template this build does not
        /// model is [`Unsupported`](Self::Unsupported), and a raster with no
        /// usable projection is [`Unplaceable`](Self::Unplaceable). Those two
        /// have grid points; this one has none.
        ///
        /// On a [`MessageInfo`] this is about where the *values* land, so a
        /// spectral or HEALPix message is [`Placed`](Self::Placed) there: its
        /// values are synthesised onto a global lat/lon raster, even though the
        /// grid it declares is `no_raster`.
        NoRaster,
        /// There is a raster, and nothing places any of it on the Earth: a
        /// polar stereographic grid stating a zero grid step, a Lambert cone
        /// whose standard parallels are both on the equator, a first point the
        /// forward map sends to infinity, a §3.90 camera that sees no Earth, a
        /// GRIB2 rotation or scale factor that is not a finite number (#823), or
        /// a NetCDF, HDF5 or Zarr slice with no coordinates to place it by. The
        /// grid still renders in its own grid coordinates; it has no position on
        /// a map.
        Unplaceable,
        /// The message declares a grid in a template this build does not model
        /// (a GRIB2 §3 template such as 3.4 or 3.101, a GRIB1 data
        /// representation type such as 90), so the grid has points and this
        /// build knows neither their shape nor where they are.
        ///
        /// Kept apart from [`NoRaster`](Self::NoRaster) because the reasons
        /// differ and so does what fixes them: nothing makes a spectral field a
        /// raster, and support for the template would make this one placeable.
        /// Nothing decodes it today, so a host offers no render.
        Unsupported,
        /// A GRIB1 message with no grid description, identified only by a
        /// predefined grid number (WMO ON388 Table B) this build does not have
        /// in its catalogue. The message names a grid; this build cannot say
        /// which.
        ///
        /// Only [`MessageInfo::placement`] reports this. A [`Georef`] exists
        /// only once a grid is known, so its placement is never this one.
        PredefinedUnresolved,
    }

    /// Where a grid's raster sits in the plane [`Georef::proj4`] names: the
    /// first scanned point's cell centre, the step from it along each axis,
    /// and the units all four are measured in.
    ///
    /// One object rather than five sibling fields so that they cannot disagree
    /// (#870). A grid with no plane has no affine at all, and so no units; it
    /// does not report "degrees" beside a missing origin.
    #[serde(rename_all = "camelCase")]
    #[cfg_attr(feature = "schema", schemars(rename_all = "camelCase"))]
    pub struct Affine {
        /// Plane coordinate of the first grid point's cell centre, along the
        /// column axis.
        pub x0: f64,
        /// Plane coordinate of the first grid point's cell centre, along the
        /// row axis.
        pub y0: f64,
        /// Signed step between columns. `None` for an axis with no constant
        /// step, such as a single column.
        pub dx: Option<f64>,
        /// Signed step between rows. Negative for the usual north-to-south
        /// scan, so `y0 + j * dy` walks the rows as stored. `None` for a
        /// Gaussian grid, whose rows are not uniformly spaced: inventing a mean
        /// step would misplace every row but the middle.
        pub dy: Option<f64>,
        /// What `x0` / `y0` / `dx` / `dy` are measured in.
        pub units: AxisUnits,
    }

    /// Where a decoded field sits on the Earth, flattened to what a host reads.
    ///
    /// A browser map library needs two things and this carries both: a CRS it
    /// can name ([`proj4`](Self::proj4)) and an [`affine`](Self::affine)
    /// placing the raster in that CRS. Everything is `Option` because a family
    /// that cannot state it says so rather than guessing, and a grid this build
    /// does not model has none of it.
    #[serde(rename_all = "camelCase")]
    #[cfg_attr(feature = "schema", schemars(rename_all = "camelCase"))]
    pub struct Georef {
        /// The grid itself, as `core` models it.
        ///
        /// The fields below are a flattened *view* of this, which is what a
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
        /// The family name the **file** uses for its own grid — the decoder's
        /// own grid-type string, and what a grid-type column should show.
        ///
        /// Usually [`kind`](Self::kind), and deliberately not always. It is
        /// more specific for a family this build does not place points on
        /// (`spherical_harmonic`, `healpix`, `unsupported(3.99)`, all of kind
        /// `unsupported`) and for the reduced families, whose rows are widened
        /// onto their regular sibling's raster before anything reads them:
        /// a `reduced_gg` message is `kind` `gaussian` and `label`
        /// `reduced_gaussian`, which is what eccodes calls it and what the
        /// other host shows (#645). A slice placed by its 2-D coordinate
        /// arrays is `kind` `lookup` and `label` `curvilinear`, the name a
        /// NetCDF or Zarr user knows it by, in both hosts (#808).
        ///
        /// A field's own georef reports the grid its *values* are on, so a
        /// synthesised raster is `latlon` in both — see
        /// [`MessageInfo::grid`](crate::api::MessageInfo::grid) for the other
        /// half of that split.
        pub label: String,
        /// Grid columns (west-to-east point count of one row). `None` for a
        /// grid with no raster shape — spherical-harmonic or bi-Fourier
        /// coefficients, HEALPix pixels, a template this build does not read
        /// — where it used to be `0` (#775). `Some(0)` is a grid that declares
        /// zero columns.
        pub ni: Option<u32>,
        /// Grid rows, under the same rule as [`ni`](Self::ni).
        pub nj: Option<u32>,
        /// `[lat_min, lat_max, lon_min, lon_max]` in degrees. `lon_min` may
        /// fall below -180 (or `lon_max` above 180) to describe a window
        /// spanning the antimeridian; do not normalise it into range without
        /// collapsing the span.
        pub bounds_lonlat: Option<[f64; 4]>,
        /// The first and last scanned grid points, as
        /// `[lat_first, lon_first, lat_last, lon_last]` (#726).
        ///
        /// The corner pair a message list shows, and **not** a bounding box:
        /// these are two diagonally opposite grid points in scan order, so
        /// `lat_last` may be less or greater than `lat_first`.
        /// [`bounds_lonlat`](Self::bounds_lonlat) is the extent.
        ///
        /// An array for the reason `bounds_lonlat` is one, and in the order the
        /// name states. `None` when the family has no raster, or when the
        /// projection cannot place its far corner — reported absent rather than
        /// clamped, so a host shows nothing instead of a plausible fiction.
        /// [`placement`](Self::placement) says which of the two it is.
        ///
        /// Where the container states its corners they are reported as stated
        /// (see [`Georef::from_declared_corners`]), so a grid can carry a
        /// corner pair and still be [`Placement::Unplaceable`]: the file wrote
        /// the numbers down, and the projection it describes cannot use them.
        pub corners: Option<[f64; 4]>,
        /// Whether this grid can be placed on the Earth, and why not — the
        /// reason a `null` [`corners`](Self::corners) or
        /// [`bounds_lonlat`](Self::bounds_lonlat) is `null` (#776).
        ///
        /// Computed from [`geometry`](Self::geometry), the same geometry that
        /// fills `bounds_lonlat`, and never from the corners a container
        /// reports. Never [`Placement::PredefinedUnresolved`].
        pub placement: Placement,
        /// Whether the grid can be reprojected: a host may offer map targets
        /// other than the grid's own coordinates, and the warp will place it.
        ///
        /// `core`'s `GridGeometry::reprojectable`, asked with
        /// [`scan`](Self::scan). **Its own field because it is not derivable
        /// from [`placement`](Self::placement) and [`kind`](Self::kind)**
        /// (#776). A placed grid can still decline: a lat/lon, Gaussian,
        /// Mercator or rotated grid scanned east to west, because their inverse
        /// maps assume columns run west to east, and a planar grid whose cell is
        /// larger than the Earth radius the message declares. Deriving it in a
        /// host would mean a family list and the scan rule in every host.
        pub reprojectable: bool,
        /// Points per row for a **reduced** grid — `PL`, the number of values
        /// each row really holds — and `None` for every other family (#244).
        ///
        /// A reduced grid's [`geometry`](Self::geometry) is its regular
        /// sibling's, widened to the widest row, which is what lets it reproject
        /// like any other grid. The file's own points are fewer: an N32 reduced
        /// Gaussian grid holds 6,114 values that the widened raster spreads over
        /// 8,192 cells by repeating each short row's values. Anything that
        /// reports individual points rather than painting the raster needs this
        /// to report the file's.
        pub points_per_row: Option<Vec<u32>>,
        /// A PROJ string for the grid's own plane, for a map library that
        /// takes one. `None` for a family this build does not name a CRS for.
        pub proj4: Option<String>,
        /// Where the raster sits in the [`proj4`](Self::proj4) plane. `None`
        /// for a family with no plane — a list of cell centres, spectral
        /// coefficients, HEALPix, a template this build does not model — and
        /// for a grid whose plane states no extent to place it in.
        pub affine: Option<Affine>,
        /// The grid closes on itself in the column axis: one column step past
        /// the last column lands back on the first. A renderer wraps rather
        /// than clamping there, or the seam meridian draws as a hole.
        pub periodic_x: bool,
        /// The order `values` are in, so a consumer can walk them without
        /// re-deriving it.
        ///
        /// This is the decoded raster's order, which is not always the order
        /// the message stored: a GRIB message stored column-major is
        /// transposed while decoding, so its `jConsecutive` is `false` here
        /// even though its scanning-mode octet sets the bit (#792). The two
        /// direction flags say which way the rows and columns run, which no
        /// reader changes while decoding.
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

    /// That a spectral field's map shows fewer wavenumbers than the file holds
    /// (#637): the truncation the message declares, and the one its values
    /// were synthesised at.
    ///
    /// **A smoothed field is never shown silently.** A spectral message's
    /// values are synthesised onto the 0.5° global grid, which carries
    /// wavenumbers up to T359; a message declaring more is band-limited to
    /// that, which is the correct picture at that resolution and a different,
    /// smoother field from the one the file holds. This is the fact a host
    /// shows beside it — "shown at T359 of T7999" — carried as data so that
    /// every host has it rather than only the one that wrote the caption.
    ///
    /// A value read out of such a field is the smoothed one; the file's own
    /// value at a point is [`crate::Session::probe_message`].
    #[serde(rename_all = "camelCase")]
    #[cfg_attr(feature = "schema", schemars(rename_all = "camelCase"))]
    pub struct SpectralTruncation {
        /// The truncation `T` the message declares — what its coefficients
        /// hold.
        pub declared: u32,
        /// The truncation the values were synthesised at, always below
        /// `declared`.
        pub truncated_to: u32,
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

    /// One line through an array: its values along one axis, every other axis
    /// held at an index (#172).
    ///
    /// A vertical profile or a time series at a grid point — the plot a viewer
    /// draws beside a map when a user clicks a cell. The values are what
    /// [`crate::Session::decode_slice`] would report at that cell on each slice
    /// along the axis, and a test holds the two to that.
    #[serde(rename_all = "camelCase")]
    #[cfg_attr(feature = "schema", schemars(rename_all = "camelCase"))]
    pub struct Line {
        /// The values along the axis, in index order. Read `mask` before a
        /// value: an absent point still occupies its slot.
        pub values: Values,
        /// One byte per point: `1` present, `0` absent. Same length as `values`.
        pub mask: Vec<u8>,
        /// Range and count over the present points.
        pub stats: Stats,
        /// The array's name, or the parameter's for a zonal mean. `None` for the
        /// zonal mean of a GRIB2 message whose product template carries no
        /// parameter codes (#775).
        pub variable: Option<String>,
        /// The array's units, as its attributes state them, or `None` when they
        /// state none (#775).
        pub units: Option<String>,
        /// The axis the line runs along, named as the array names it.
        pub dimension: String,
        /// The axis's coordinate values, in index order — the times or levels to
        /// label the line against.
        ///
        /// `None` when the axis has no 1-D coordinate array of its own name, or
        /// when one of its values is absent: a coordinate with a hole has no
        /// honest position to plot that point at, so the host falls back to
        /// indices rather than being handed a gap it would have to invent across.
        pub coordinates: Option<Vec<f64>>,
        /// The coordinate array's units, when there are coordinates.
        pub coordinate_units: Option<String>,
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
        /// Set when the values are a spectral field band-limited to what the
        /// grid can carry, below the truncation the message declares (#637);
        /// `None` for every other field, including a spectral one the grid
        /// carries in full. A host shows it beside the field.
        pub truncation: Option<SpectralTruncation>,
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
        /// carries them all: the discipline is a Code Table 0.0 *name*, and
        /// only `Discipline <n>` for a discipline no table defines. This is
        /// the string every host shows — the umbrella, the
        /// wasm binding and the napi binding all render it from the format
        /// crate's own `unresolved_parameter` (#633).
        ///
        /// `None` only when the message has no parameter codes to name at all,
        /// which is a GRIB2 product template carrying no horizontal product
        /// common (#775). [`units`](Self::units) is `None` in both cases — an
        /// unresolved parameter has a name to show but no unit to state.
        pub parameter: Option<String>,
        /// The parameter's units as its table states them. `None` when the
        /// parameter did not resolve, when the table states no units for it
        /// (a dimensionless quantity, in most tables), or when the array's
        /// attributes state none (#775).
        pub units: Option<String>,
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
        /// naming the codes no table in this build resolved. `None` for a GRIB2
        /// product template that carries no parameter codes.
        pub parameter: Option<String>,
        /// The table's short name for the parameter, e.g. `"2t"`. `None` when
        /// the parameter did not resolve, or the table that resolved it gives
        /// no short name (#775).
        pub abbreviation: Option<String>,
        /// Units as the parameter's table states them, under the same rule as
        /// [`Field::units`].
        pub units: Option<String>,
        /// The level, rendered — `"500 hPa"`, `"2 m above ground"`. `None`
        /// when the message states no level value: a GRIB1 surface or
        /// whole-column type, a GRIB2 surface coded missing, or a GRIB2
        /// product template that carries no surface (#775). A GRIB2 surface
        /// with no scaled value is named here instead (`"Ground or water
        /// surface"`), because Code Table 4.5's entry is the description.
        pub level: Option<String>,
        /// The level's surface type on its own, for grouping messages that
        /// share a surface at different values. `None` for a GRIB2 product
        /// template that carries no surface.
        pub level_type: Option<String>,
        /// Reference (analysis) time as RFC 3339. Every GRIB message states
        /// one, in its §1 (GRIB2) or PDS (GRIB1).
        pub reference_time: String,
        /// Forecast time relative to `reference_time`, rendered — `"+6h"`, or
        /// `"+30 Minute"` for a unit the edition does not convert to hours.
        /// `None` for a GRIB2 product template that carries no forecast time.
        pub forecast: Option<String>,
        /// Which packing the data section uses, named — what decodes it, and
        /// the first thing to look at when a decode is wrong. `None` when the
        /// GRIB1 data section's header could not be read (#775).
        pub packing: Option<String>,
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
        /// `None` only for a GRIB1 message that carries no §2 at all and whose
        /// `pds.grid_number` is not a predefined grid this build's catalogue
        /// resolves — either 255, "no predefined grid", or a number outside the
        /// catalogue. [`placement`](Self::placement) tells the two apart.
        pub grid: Option<Georef>,
        /// Whether this message's **values** can be placed on the Earth, and
        /// why not (#776) — the answer a message list reads to decide whether
        /// it can draw the message at all.
        ///
        /// Where the values land rather than what the file declares, which is
        /// the difference from `grid.placement`. The two differ only for the
        /// families whose values are synthesised onto a raster: a spectral or
        /// HEALPix message declares a `no_raster` grid, and its values land on
        /// a global lat/lon one, so this is [`Placement::Placed`]. Always the
        /// [`Georef::placement`] [`crate::Session::place_message`] reports when
        /// that call succeeds; `place_message_agrees_with_decode.rs` holds the
        /// two together over the corpus.
        ///
        /// With no `grid`, this is [`Placement::PredefinedUnresolved`] when the
        /// message names a predefined grid this build cannot resolve, and
        /// [`Placement::NoRaster`] when it names none (grid number 255).
        ///
        /// Placed is not decodable: a GRIB1 matrix-of-values or GRIB2
        /// bi-Fourier message is refused by [`crate::Session::decode`]
        /// whatever this says, and the refusal names the call that reads it.
        pub placement: Placement,
        /// Whether the grid this message's values land on can be reprojected —
        /// the answer a projection picker reads, and
        /// [`Georef::reprojectable`] of the same grid
        /// [`placement`](Self::placement) describes. `false` when there is no
        /// grid.
        ///
        /// Like `placement`, it is about the values: a spectral message's
        /// declared grid does not reproject and its synthesised lat/lon field
        /// does, so this is `true`.
        pub reprojectable: bool,
        /// How the file names its own grid where `Ni × Nj` is not how it is
        /// described — `N32`, `O1280`, `T639`.
        pub size_label: Option<String>,
        /// What [`crate::Session::decode`] will set as
        /// [`Field::truncation`] for this message: the map of a spectral
        /// message declaring more wavenumbers than the synthesis grid carries
        /// is band-limited, and says so (#637). Read from the declaration, so a
        /// message list can show it before anything is decoded. `None` for
        /// everything else.
        pub truncation: Option<SpectralTruncation>,

        // The identification a message list shows beside the parameter. These
        // are file facts rather than derived ones, and every one of them needed
        // a WMO or CCT table lookup that a host was doing for itself (#726) —
        // which the conventions put in the Rust tables, not at the binding
        // layer. Resolved here so each host reads a string rather than a code.
        /// Forecast lead time in whole hours, `None` for a template that states
        /// none.
        ///
        /// [`forecast`](Self::forecast) is the same fact rendered for display
        /// ("+6 h", an averaging interval, "analysis"); this is the number, for
        /// a host that sorts or animates by it.
        pub forecast_hours: Option<i32>,
        /// The originating centre, named from the CCT common code table, or
        /// `Centre <n>` when the table has no entry.
        pub originating_centre: String,
        /// The sub-centre, named from WMO Common Code Table C-12 under the
        /// originating centre, or `Sub-centre <n>` when the table has no entry.
        /// `None` only for code 0, which GRIB uses to mean there is no
        /// sub-centre — the common case.
        pub sub_centre: Option<String>,
        /// The edition, and the identification only that edition carries
        /// (#773). A field one edition does not have is not on the other's
        /// variant, so a host never reads "not applicable" as "absent".
        pub identification: Identification,
        /// The length the message declares for itself, in bytes.
        ///
        /// Distinct from [`offset_bytes`](Self::offset_bytes), which says where
        /// it starts. Together they are the range a host would re-fetch.
        pub total_length_bytes: u64,
        /// Whether this message's `u`/`v` components are resolved along the
        /// grid's own axes rather than east and north (GRIB1 GDS octet 17 bit 5,
        /// GRIB2 §3 Flag Table 3.3 bit 5).
        ///
        /// What a vector plot must know before it draws an arrow (#241): a
        /// grid-relative pair drawn as east/north points wrong by the grid's
        /// convergence angle, which over a continental Lambert domain is tens of
        /// degrees. HRRR and NAM set it. `None` for a message whose family
        /// states no resolution flags, and for a container that is not GRIB.
        pub uv_relative_to_grid: Option<bool>,
    }

    /// The identification fields that belong to one GRIB edition, tagged with
    /// it: `{"edition": "grib1", ...}` or `{"edition": "grib2", ...}` on the
    /// wire (#773).
    ///
    /// A field exists only on the edition that has it. GRIB1 has no
    /// discipline, production status or data type (they are §0 and §1 fields
    /// edition 2 introduced), and GRIB2 has no one-octet `P1`.
    #[serde(tag = "edition", rename_all = "snake_case", rename_all_fields = "camelCase")]
    pub enum Identification {
        /// WMO FM 92 GRIB edition 1.
        Grib1 {
            /// The PDS `P1` octet, when the time-range indicator is one that
            /// makes it a lead time rather than the second half of an
            /// interval. `None` for time range 10, where `P1` is a two-octet
            /// value and not this field.
            p1_octet: Option<i32>,
        },
        /// WMO FM 92 GRIB edition 2.
        Grib2 {
            /// The discipline (§0 octet 7), named from Code Table 0.0, or
            /// `Discipline <n>` for a code the table does not name.
            discipline: String,
            /// The production status (§1 octet 20), named from Code Table 1.3,
            /// or `Production status <n>` for a code the table does not name.
            production_status: String,
            /// The data type — analysis, forecast, reanalysis — (§1 octet 21),
            /// named from Code Table 1.4, or `Data type <n>` for a code the
            /// table does not name.
            data_type: String,
        },
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

    /// A spectral message's full-detail value at a probed cell (#637): the sum
    /// over every wavenumber the file holds, where the map shows the field
    /// band-limited to what its grid carries.
    ///
    /// Carried beside the displayed value rather than instead of it, so a
    /// readout can show both — "325.6 K at T359 (shown) · 318.4 K at T7999
    /// (full detail)" — and neither disagrees with the colour under the cursor
    /// without saying why.
    #[serde(rename_all = "camelCase")]
    #[cfg_attr(feature = "schema", schemars(rename_all = "camelCase"))]
    pub struct FullDetail {
        /// The full sum at the cell's node.
        pub value: f64,
        /// The truncation `value` carries (`declared`) and the one the
        /// displayed value carries (`truncated_to`).
        pub truncation: SpectralTruncation,
    }

    /// One point probed out of a message by index
    /// ([`crate::Session::probe_message`]): the value the decoded field shows,
    /// and for a band-limited spectral message the file's full-detail value at
    /// the same cell.
    #[serde(rename_all = "camelCase")]
    #[cfg_attr(feature = "schema", schemars(rename_all = "camelCase"))]
    pub struct MessageProbe {
        /// Latitude asked for, echoed back.
        pub lat: f64,
        /// Longitude asked for, echoed back.
        pub lon: f64,
        /// Fractional column the point landed on.
        pub i: f64,
        /// Fractional row the point landed on.
        pub j: f64,
        /// The decoded field's value at the cell — what the map shows there.
        /// `None` when the cell is masked.
        pub value: Option<f64>,
        /// Set only when the decoded field is band-limited
        /// ([`Field::truncation`] is set): the full sum at the same cell.
        pub full_detail: Option<FullDetail>,
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

/// The core label, as the wire type every host binds (#637).
impl From<fieldglass_core::sht::SpectralTruncation> for SpectralTruncation {
    fn from(t: fieldglass_core::sht::SpectralTruncation) -> Self {
        Self {
            declared: t.declared,
            truncated_to: t.truncated_to,
        }
    }
}

impl Placement {
    /// Every value, in declaration order: the vocabulary a host that declares
    /// the field by hand (the napi binding's TypeScript union) is held to.
    pub const ALL: [Self; 5] = [
        Self::Placed,
        Self::NoRaster,
        Self::Unplaceable,
        Self::Unsupported,
        Self::PredefinedUnresolved,
    ];

    /// Whether `geom` can be placed, and why not — what
    /// [`Georef::placement`] reports for it.
    ///
    /// For a host that holds a geometry without a [`Georef`] around it. Where
    /// there is one, read its field: this walks the grid's perimeter, which
    /// building the `Georef` has already done.
    pub fn of(geom: &GridGeometry) -> Self {
        Self::from_extent(geom.dims(), geom.lonlat_bbox().is_some())
    }

    /// Whether a raster of `ni` × `nj` cells on `geom` can be placed, and why
    /// not: the [`Georef::placement`] that [`Georef::from_slice`] reports for
    /// the same arguments.
    ///
    /// For a host answering a picker about a slice it is not decoding, which
    /// wants this and not a whole [`Georef`]: building one copies the geometry,
    /// and a lookup grid's geometry is every cell centre it has (#574).
    pub fn of_raster(geom: &GridGeometry, ni: u32, nj: u32) -> Self {
        match geom.dims() {
            None => Self::raster_without_geometry(ni, nj),
            dims => Self::from_extent(dims, geom.lonlat_bbox().is_some()),
        }
    }

    /// A raster whose geometry has no grid points: one with cells still
    /// renders in grid coordinates, so it is [`Unplaceable`](Self::Unplaceable)
    /// rather than [`NoRaster`](Self::NoRaster) (#776). The one statement of
    /// that rule, for [`of_raster`](Self::of_raster) and
    /// [`Georef::from_slice`].
    fn raster_without_geometry(ni: u32, nj: u32) -> Self {
        if ni > 0 && nj > 0 {
            Self::Unplaceable
        } else {
            Self::NoRaster
        }
    }

    /// The rule itself, over the two answers it reads — for a caller that
    /// already holds both and should not walk the grid a second time to ask
    /// [`of`](Self::of).
    ///
    /// `placed` is whether `GridGeometry::lonlat_bbox` answered, which is the
    /// projection-can-place-something question: it is `None` exactly when no
    /// part of the grid's perimeter lands on the Earth, and it keeps a full disc
    /// whose perimeter is all limb (the geostationary fallback) placed. The far
    /// corner is deliberately *not* the test: that disc's corners are in space.
    pub fn from_extent(dims: Option<(u32, u32)>, placed: bool) -> Self {
        match dims {
            // A zero-width grid is declared, and it is no raster: nothing to
            // paint, and a corner at index `ni - 1` does not exist.
            // `None` is a geometry with no grid points. Where the container
            // knows better — an unmodelled template, a slice with no
            // coordinates — the caller says so instead of asking this.
            None | Some((0, _) | (_, 0)) => Self::NoRaster,
            Some(_) if placed => Self::Placed,
            Some(_) => Self::Unplaceable,
        }
    }

    /// The wire spelling, for a host that maps this into a DTO of its own
    /// rather than serialising it: `"placed"`, `"no_raster"`, `"unplaceable"`,
    /// `"unsupported"` or `"predefined_unresolved"`. A test holds it to the
    /// serde tag.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Placed => "placed",
            Self::NoRaster => "no_raster",
            Self::Unplaceable => "unplaceable",
            Self::Unsupported => "unsupported",
            Self::PredefinedUnresolved => "predefined_unresolved",
        }
    }
}

impl Georef {
    /// Flatten a [`GridGeometry`] and the scan order of its decoded values
    /// (what [`Georef::scan`] reports, not the flags the message stored; #792)
    /// into the scalar form a host consumes.
    ///
    /// The projected families report their origin and spacing in the
    /// projection plane, which is what [`GridGeometry::proj4`] describes — the
    /// grid origin is applied on top of the CRS, not baked into it, so a host
    /// placing the raster needs both halves and this carries them together.
    ///
    /// [`label`](Georef::label) comes from the geometry, which is right for a
    /// grid that has no declaring message behind it — a synthesised raster, a
    /// combined field. Where there *is* one, use
    /// [`from_declared`](Self::from_declared) so the file's own name for its
    /// grid survives the conversion.
    pub fn from_geometry(geom: &GridGeometry, scan: Scan) -> Self {
        Self::from_declared(geom, scan, geom.label())
    }

    /// This placement with the points per row a **reduced** grid stores (#244).
    ///
    /// Separate from the constructors because only the GRIB reduced families
    /// have one, and every other caller would otherwise pass `None` to say so.
    /// See [`points_per_row`](Self::points_per_row) for what it is for.
    #[must_use]
    pub fn with_points_per_row(mut self, points_per_row: Option<&[u32]>) -> Self {
        self.points_per_row = points_per_row.map(<[u32]>::to_vec);
        self
    }

    /// As [`from_declared`](Self::from_declared), but with the corner pair the
    /// **container** reports rather than one recomputed from the geometry.
    ///
    /// Use this wherever the container states its corners, which is every GRIB
    /// message with a §2 or §3. Recomputing them instead is measurably not the
    /// same answer, and a message table shows the file's:
    ///
    /// - A projected family's far corner comes back in a different longitude
    ///   normalisation — a CMC polar-stereographic grid states
    ///   `-31.886937598141174`, and projecting the last grid point through
    ///   `core` gives `328.1130624018588`, the same meridian written the other
    ///   way.
    /// - A global lat/lon grid's far corner accumulates past the antimeridian
    ///   rather than wrapping: `539.75` for a grid whose file says `179.75`.
    ///   Continuous longitude is what the warp wants and not what a caption
    ///   does.
    /// - A **reduced** grid states the corner of the grid it really is, where
    ///   the geometry has already been widened onto its regular sibling:
    ///   `357.188` against a computed `357.1875`.
    ///
    /// [`corners`](Self::corners) therefore means "the corner pair this
    /// container reports", and falls back to the geometry only for a container
    /// that reports none.
    ///
    /// A geometry with no grid points is reported [`Placement::NoRaster`], as
    /// [`from_declared`](Self::from_declared) reports it; `Session`, which
    /// reads the container, says `unsupported` or `unplaceable` where the
    /// container knows which.
    pub fn from_declared_corners(
        geom: &GridGeometry,
        scan: Scan,
        declared: &str,
        corners: Option<CornerPair>,
    ) -> Self {
        Self::from_container(geom, scan, declared, corners, Placement::NoRaster, None)
    }

    /// The placement of a raster whose shape the **caller** states: a slice of
    /// an array, `ni` columns by `nj` rows, on `geom`.
    ///
    /// Where a geometry places nothing — a NetCDF, HDF5 or Zarr slice with no
    /// coordinate arrays, which the resolvers answer with a source-only
    /// geometry — the slice still has cells and still renders in grid
    /// coordinates, so it is [`Placement::Unplaceable`]; only a slice with no
    /// cells at all is [`Placement::NoRaster`] (#776).
    ///
    /// One rule for every caller: [`Session::decode_slice`] puts this on the
    /// field it returns, and a host that places a slice itself, without
    /// decoding it, reports the same answer to its picker (#574).
    ///
    /// [`Session::decode_slice`]: crate::Session::decode_slice
    pub fn from_slice(geom: &GridGeometry, scan: Scan, declared: &str, ni: u32, nj: u32) -> Self {
        Self::from_container(
            geom,
            scan,
            declared,
            None,
            Placement::raster_without_geometry(ni, nj),
            Some((ni, nj)),
        )
    }

    /// [`from_declared_corners`](Self::from_declared_corners) with the
    /// placement to report when the geometry has no grid points, which only
    /// the container can say: [`Placement::Unsupported`] for a template this
    /// build does not model, [`Placement::Unplaceable`] for a raster the
    /// container declares and nothing places, [`Placement::NoRaster`] when
    /// there really are no points.
    ///
    /// `raster` is the columns and rows the container states, read only when
    /// the geometry has none of its own: a raster nothing places still has
    /// that shape, and a host sizes everything it draws from these two fields
    /// (#823).
    pub(crate) fn from_container(
        geom: &GridGeometry,
        scan: Scan,
        declared: &str,
        corners: Option<CornerPair>,
        without_geometry: Placement,
        raster: Option<(u32, u32)>,
    ) -> Self {
        let computed = Self::build(geom, scan, declared, without_geometry, raster);
        Self {
            // `.or`, not a plain assignment: naming the field in a struct
            // update replaces what `from_declared` computed, so a container
            // that reports no corners would lose them entirely rather than
            // fall back. §3.12 transverse Mercator is exactly that container —
            // `bounds()` reports `None` for it by design — and it is how this
            // bug was found.
            corners: corners
                .map(|c| [c.lat_first, c.lon_first, c.lat_last, c.lon_last])
                .or(computed.corners),
            ..computed
        }
    }

    /// As [`from_geometry`](Self::from_geometry), but with the family name the
    /// **message** declares rather than the one the geometry reports.
    ///
    /// The two differ for the reduced families. Both decoders widen a reduced
    /// grid's rows onto a regular raster before anything places a point on it,
    /// so the geometry that arrives here is the regular sibling and
    /// [`GridGeometry::kind`] — which is the serde tag, and must stay the
    /// variant's own name — answers `"gaussian"` for a `reduced_gg`. The file
    /// still says `reduced_gaussian`, that is what eccodes prints and what the
    /// extension's grid-type column shows, and losing it here is what made the
    /// two hosts disagree in public (#645).
    ///
    /// So `declared` is the decoder's own string —
    /// `fieldglass_grib1::gds::GridDescription::grid_type_name`,
    /// `fieldglass_grib2::gds::GridDefinitionSection::template_name` — read
    /// rather than re-derived, the way `raster_bounds` is (#543).
    ///
    /// A geometry with no grid points is reported [`Placement::NoRaster`]; see
    /// [`from_declared_corners`](Self::from_declared_corners).
    pub fn from_declared(geom: &GridGeometry, scan: Scan, declared: &str) -> Self {
        Self::build(geom, scan, declared, Placement::NoRaster, None)
    }

    /// The one constructor body. `without_geometry` is what a geometry with no
    /// grid points reports, and `raster` the shape it has all the same; see
    /// [`from_container`](Self::from_container).
    fn build(
        geom: &GridGeometry,
        scan: Scan,
        declared: &str,
        without_geometry: Placement,
        raster: Option<(u32, u32)>,
    ) -> Self {
        debug_assert!(
            without_geometry != Placement::Placed,
            "a grid with no geometry cannot be placed"
        );
        // No shape is `None`, not a zero a host would have to know means
        // "none" (#775).
        let (ni, nj) = geom
            .dims()
            .or(raster)
            .map_or((None, None), |(i, j)| (Some(i), Some(j)));
        // One question, asked of `core`: a family that has a plane reports its
        // origin and step in that plane's own units, and one that has none (a
        // list of cell centres, an unmodelled grid) reports nothing, units
        // included, rather than a plausible-looking zero (#870). A rotated
        // lat/lon grid has a plane — its own rotated frame, measured in
        // degrees — so it reports one.
        let affine = geom.plane_affine().map(|a| Affine {
            x0: a.x0,
            y0: a.y0,
            dx: a.dx,
            dy: a.dy,
            units: match a.units {
                PlaneUnits::Metres => AxisUnits::Metres,
                PlaneUnits::Degrees => AxisUnits::Degrees,
            },
        });
        // Walked once and read twice: the extent is what `placement` asks
        // about, and the perimeter walk is the expensive half of this call.
        let bbox = geom.lonlat_bbox();
        Self {
            geometry: geom.clone(),
            kind: geom.kind().to_string(),
            label: declared.to_string(),
            ni,
            nj,
            bounds_lonlat: bbox.map(LonLatBox::to_array),
            corners: geom
                .corner_pair()
                .map(|c| [c.lat_first, c.lon_first, c.lat_last, c.lon_last]),
            placement: match geom.dims() {
                None => without_geometry,
                dims => Placement::from_extent(dims, bbox.is_some()),
            },
            reprojectable: geom.reprojectable(scan),
            points_per_row: None,
            proj4: geom.proj4(),
            affine,
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
        // Stated rather than left to the JSON Schema default, because it is
        // the one object on the surface a declaration generator should type
        // loosely (#574): the per-family fields are `core`'s, not the host's.
        "additionalProperties": true,
        "description": "The grid as the engine models it, tagged by `kind`. A host hands it back unread; the per-family fields beside `kind` are not part of the host contract."
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
        "additionalProperties": false,
        "description": "The scan order of the decoded values, which for a column-major message is not the order the message stored."
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use fieldglass_core::{
        GeostationaryParams, LambertParams, LambertProjector, LatLonParams, PlanarGridProjector,
        RotatedLatLonParams,
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
        let a = g.affine.expect("a Lambert grid has a plane");
        assert!(matches!(a.units, AxisUnits::Metres));
        let proj = LambertProjector::new(p);
        for (i, j) in [(0u32, 0u32), (7, 3), (99, 79)] {
            let (lat, lon) = geom.forward(i, j).expect("grid point");
            let (x, y) = proj.forward_xy(lat, lon);
            let want_x = a.x0 + f64::from(i) * a.dx.unwrap();
            let want_y = a.y0 + f64::from(j) * a.dy.unwrap();
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
        let a = g.affine.expect("a rotated grid has a plane");
        assert!(matches!(a.units, AxisUnits::Degrees));
        assert!(
            g.proj4
                .as_deref()
                .is_some_and(|s| s.starts_with("+proj=ob_tran ")),
            "{:?}",
            g.proj4
        );
        assert_eq!((a.x0, a.y0), (-18.0, -20.0));
        assert_eq!((a.dx, a.dy), (Some(1.0), Some(1.0)));
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
        let a = g.affine.expect("a Gaussian grid has a plane");
        assert!(a.dx.is_some());
        assert_eq!(a.dy, None);
        assert!(matches!(a.units, AxisUnits::Degrees));
    }

    /// A grid with no plane states no units for one (#870). Before, it
    /// reported `"degrees"` beside a null origin, which read as if the units
    /// described something. On the wire the whole affine is a present `null`,
    /// so a strict `=== null` guard sees it.
    #[test]
    fn a_grid_with_no_plane_states_no_affine_and_no_units() {
        let geom = GridGeometry::Unsupported {
            label: "spherical harmonics".to_string(),
            declared: None,
        };
        let g = Georef::from_geometry(&geom, scan());
        assert_eq!(g.affine, None);
        let wire = serde_json::to_value(&g).expect("serialises");
        assert_eq!(wire.get("affine"), Some(&serde_json::Value::Null));
        for gone in ["axisUnits", "x0", "y0", "dx", "dy"] {
            assert!(wire.get(gone).is_none(), "{gone} is not a Georef key");
        }
    }

    /// The wire spelling a napi host maps through [`Placement::as_str`] is the
    /// serde tag every other host reads, for every variant.
    #[test]
    fn placement_as_str_is_the_serde_tag() {
        for p in Placement::ALL {
            // Exhaustive, so a variant added without joining `ALL` fails to
            // compile here rather than going missing from a host's union.
            match p {
                Placement::Placed
                | Placement::NoRaster
                | Placement::Unplaceable
                | Placement::Unsupported
                | Placement::PredefinedUnresolved => {}
            }
            assert_eq!(
                serde_json::to_value(p).expect("serialises"),
                serde_json::Value::String(p.as_str().to_string()),
            );
        }
    }

    /// A GOES-East-like full disc: the raster's corner pixels look past the
    /// Earth, so there is no corner pair to report, and the disc is still on
    /// the map. That is the case the rule keys on the extent rather than on the
    /// far corner for: a `null` corner is not by itself "unplaceable".
    #[test]
    fn a_full_disc_is_placed_although_its_corners_are_space() {
        let p = GeostationaryParams {
            ni: 21,
            nj: 21,
            h_metres: 42_164_160.0,
            r_eq: 6_378_137.0,
            r_pol: 6_356_752.314_14,
            sub_lon_deg: -75.0,
            sweep_x: true,
            x0: -0.151844,
            dx_rad: 0.0151844,
            y0: 0.151844,
            dy_rad: -0.0151844,
        };
        let g = Georef::from_geometry(&GridGeometry::Geostationary(p), scan());
        assert_eq!(g.corners, None, "the corner pixels are off the disc");
        assert!(g.bounds_lonlat.is_some());
        assert_eq!(g.placement, Placement::Placed);
        assert!(g.reprojectable);

        // The same grid seen from inside the Earth has no line of sight to it,
        // so nothing of it can be placed: a raster, and no position for it.
        let blind = GridGeometry::Geostationary(GeostationaryParams {
            h_metres: p.r_eq / 2.0,
            ..p
        });
        let g = Georef::from_geometry(&blind, scan());
        assert_eq!(g.placement, Placement::Unplaceable);
        assert_eq!(Placement::of(&blind), Placement::Unplaceable);
        assert!(!g.reprojectable);
    }

    /// A family with no grid points, and a grid declaring none, are both
    /// "nothing to place" rather than "cannot place".
    #[test]
    fn no_grid_points_is_no_raster() {
        let unmodelled = GridGeometry::Unsupported {
            label: "spherical_harmonic".to_string(),
            declared: None,
        };
        assert_eq!(
            Georef::from_geometry(&unmodelled, scan()).placement,
            Placement::NoRaster
        );
        let empty = GridGeometry::LatLon(LatLonParams {
            ni: 0,
            nj: 181,
            lat_first: 90.0,
            lon_first: 0.0,
            lat_last: -90.0,
            lon_last: 359.0,
        });
        assert_eq!(Placement::of(&empty), Placement::NoRaster);
    }

    /// A placed grid that still declines to reproject: the reason
    /// `reprojectable` is a field of its own rather than derived from
    /// `placement` and `kind` (#776).
    #[test]
    fn a_placed_grid_scanned_east_to_west_does_not_reproject() {
        let geom = GridGeometry::LatLon(LatLonParams {
            ni: 360,
            nj: 181,
            lat_first: 90.0,
            lon_first: 359.0,
            lat_last: -90.0,
            lon_last: 0.0,
        });
        let east_to_west = Scan::new(true, false, false);
        let g = Georef::from_geometry(&geom, east_to_west);
        assert_eq!(g.placement, Placement::Placed);
        assert!(!g.reprojectable);
        assert!(Georef::from_geometry(&geom, scan()).reprojectable);
    }
}
