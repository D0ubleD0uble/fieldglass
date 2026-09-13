//! [`Session`] — open bytes, list messages, decode a field, and operate on it.
//!
//! Everything here takes `&[u8]` in and returns owned plain data. No host type
//! appears, and no operation touches the filesystem or the network: ADR-0005
//! puts fetching in the host, so a session is handed the bytes it works on.
//!
//! **The session holds no decode cache.** The napi handles do, because Node's
//! heap is reclaimed; wasm linear memory never shrinks, and an animation holds
//! many fields at once, so a field is handed to the caller and passed back by
//! reference to every operation that consumes one.

// Split by feature: a braced `use` list takes no `#[cfg]` on its members, so
// the items behind `core`'s optional surfaces need their own statement (#552).
#[cfg(feature = "zarr")]
use fieldglass_core::bytes::ObjectSource;
#[cfg(any(feature = "grib1", feature = "grib2"))]
use fieldglass_core::bytes::{ByteSource, read_up_to};
#[cfg(any(feature = "grib1", feature = "grib2"))]
use fieldglass_core::units::normalize_units;
#[cfg(feature = "render")]
use std::borrow::Cow;
use std::sync::Arc;
#[cfg(any(feature = "netcdf", feature = "zarr"))]
use std::{collections::HashMap, sync::Mutex};

use fieldglass_core::cf::SlicePlacement;
use fieldglass_core::{Format as CoreFormat, GridGeometry, detect_from_bytes};
#[cfg(feature = "render")]
use fieldglass_core::{
    LonLatBox, Resampling, SourceGrid, TargetRaster,
    colormap::{Colormap, Palette, ScaleMode, default_colormap},
    warp,
};
#[cfg(any(feature = "netcdf", feature = "zarr"))]
use fieldglass_core::{
    array::{ArraySource, AttributeValue, CfUnpacking, ElementType, attribute},
    cf::{RenderableArray, curvilinear_pair, renderable_arrays, slice_placement},
};
#[cfg(feature = "analysis")]
use fieldglass_core::{contour_segments, contour_segments_global, nice_levels};

#[cfg(feature = "analysis")]
use crate::api::Isoline;
#[cfg(feature = "render")]
use crate::api::Warped;
use crate::api::{
    Addressing, DimensionInfo, Dtype, Field, Georef, LeftOutArray, Line, MessageInfo, Probe, Scan,
    SourceFormat, Stats, Values, VariableInfo,
};
#[cfg(feature = "analysis")]
use crate::combine::CombineOp;
use crate::error::Error;

/// How a decode should be shaped.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schema", schemars(rename_all = "camelCase"))]
#[non_exhaustive]
pub struct DecodeOptions {
    /// Element type to decode into. [`Dtype::Auto`] keeps the source's own
    /// width and is the only setting that never loses precision.
    #[serde(default)]
    pub dtype: Dtype,
}

impl DecodeOptions {
    /// The one field this type has.
    ///
    /// A constructor rather than a struct literal because the type is
    /// `#[non_exhaustive]`: a caller outside this crate cannot write one, and
    /// `Default::default()` followed by a field assignment is the pattern
    /// `clippy::field_reassign_with_default` exists to discourage. Every option
    /// type on this surface has one for that reason (#573).
    #[must_use]
    pub fn new(dtype: Dtype) -> Self {
        Self { dtype }
    }
}

/// How a field should be resampled onto a geographic box.
#[cfg(feature = "render")]
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schema", schemars(rename_all = "camelCase"))]
#[non_exhaustive]
pub struct WarpOptions {
    /// Bilinear when true, nearest otherwise. A grid that is a list of cell
    /// centres downgrades to nearest whatever this says.
    #[serde(default = "yes")]
    pub bilinear: bool,
    /// Output window `[lat_min, lat_max, lon_min, lon_max]`. `None` uses the
    /// source grid's own extent.
    #[serde(default)]
    pub bounds: Option<[f64; 4]>,
    /// Output raster columns. `None` keeps the source grid's `ni` (#465).
    #[serde(default)]
    pub width: Option<u32>,
    /// Output raster rows — the other half of [`width`](Self::width). `None`
    /// keeps the source grid's `nj`.
    ///
    /// Together with [`bounds`](Self::bounds) this is the whole of what a map
    /// view asks for: *this window at W × H pixels*. Send both or neither; one
    /// alone is an [`Error::InvalidOption`], for the reason
    /// [`crate::RenderOptions::height`] gives. Zero on either axis, and a raster
    /// past this target's allocation ceiling, are refused rather than
    /// allocated.
    #[serde(default)]
    pub height: Option<u32>,
}

#[cfg(feature = "render")]
fn yes() -> bool {
    true
}

#[cfg(feature = "render")]
impl Default for WarpOptions {
    fn default() -> Self {
        Self {
            bilinear: true,
            bounds: None,
            width: None,
            height: None,
        }
    }
}

#[cfg(feature = "render")]
impl WarpOptions {
    /// The resampling this warp wants, with the source grid's own extent as the
    /// window. Assign [`bounds`](Self::bounds) afterwards for a manual one.
    ///
    /// A constructor for the reason [`DecodeOptions::new`] gives.
    #[must_use]
    pub fn new(bilinear: bool) -> Self {
        Self {
            bilinear,
            bounds: None,
            width: None,
            height: None,
        }
    }
}

/// Colour, decided once in Rust and exported as data.
#[cfg(feature = "render")]
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schema", schemars(rename_all = "camelCase"))]
#[non_exhaustive]
pub struct PaletteOptions {
    /// A colormap name `core` knows. Unknown names are an error rather than a
    /// silent fallback: a host that misspells one should hear about it.
    #[serde(default)]
    pub colormap: Option<String>,
    /// A colormap given as its 768-byte lookup table instead of by name, as
    /// [`RenderOptions::colormap_table`](crate::RenderOptions::colormap_table)
    /// describes, and refused on the same terms.
    #[serde(default)]
    pub colormap_table: Option<Vec<u8>>,
    /// Walk the colormap high-to-low. Applied after `colormap` is resolved,
    /// so a reversed unknown name is still an error.
    #[serde(default)]
    pub reversed: bool,
    /// Low end of the display range. `None` takes the field's own minimum.
    #[serde(default)]
    pub min: Option<f64>,
    /// High end of the display range. `None` takes the field's own maximum.
    #[serde(default)]
    pub max: Option<f64>,
    /// `"linear"` (default) or `"log10"`.
    #[serde(default)]
    pub scale: Option<String>,
}

#[cfg(feature = "render")]
impl PaletteOptions {
    /// The two fields that pick the ramp and the transform; the display range
    /// and the reversal are assigned afterwards.
    ///
    /// Both are `Option` because `None` is a real answer for each — the default
    /// colormap, and the linear scale — so this is not "the required fields" so
    /// much as "the ones a caller almost always states". A constructor for the
    /// reason [`DecodeOptions::new`] gives.
    #[must_use]
    pub fn new(colormap: Option<&str>, scale: Option<&str>) -> Self {
        Self {
            colormap: colormap.map(str::to_string),
            colormap_table: None,
            reversed: false,
            min: None,
            max: None,
            scale: scale.map(str::to_string),
        }
    }
}

/// A painted raster: RGBA bytes plus the dimensions they cover.
#[cfg(feature = "render")]
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schema", schemars(rename_all = "camelCase"))]
#[non_exhaustive]
pub struct Raster {
    /// Non-premultiplied RGBA, `width * height * 4` bytes, row-major from the
    /// top-left. Ready for `putImageData` or an `RGBA8` texture upload.
    pub rgba: Vec<u8>,
    /// Raster columns.
    pub width: u32,
    /// Raster rows.
    pub height: u32,
}

/// An open file.
#[derive(Debug)]
pub struct Session {
    reader: Reader,
}

/// One variant per format feature (#552). `lib.rs` refuses a build with none of
/// them, so this enum always has at least one variant and every `match` on it
/// below stays exhaustive with its arms gated the same way.
/// The bytes a message-addressed reader reads, type-erased.
///
/// One instantiation of each GRIB reader in this crate rather than one per source
/// type (#709). `Session::open` boxes the buffer it was handed and
/// `Session::open_source` boxes whatever the host brought, so both arrive here as
/// the same type and the readers are monomorphised once — which is what keeps a
/// source-taking constructor from costing the browser bundle a second copy of
/// every decode path. The cost is a virtual call per `read`, against a decode
/// that does far more work per slab than that.
#[cfg(any(feature = "grib1", feature = "grib2"))]
type Bytes = Box<dyn ByteSource>;

enum Reader {
    #[cfg(feature = "grib1")]
    Grib1(Box<fieldglass_grib1::Grib1Reader<Bytes>>),
    #[cfg(feature = "grib2")]
    Grib2(Box<fieldglass_grib2::Grib2Reader<Bytes>>),
    /// A container of named arrays — a NetCDF file or a Zarr store — held as
    /// the [`ArraySource`] its reader presents (#704). One arm for every such
    /// container: the variable list, the slice and its placement are core's CF
    /// rules over that seam, so a second container needs no second copy of them.
    #[cfg(any(feature = "netcdf", feature = "zarr"))]
    Arrays(Box<Arrays>),
}

/// Written rather than derived: the bytes behind a message reader are a
/// `dyn ByteSource` the host brought, and a trait object cannot describe itself.
/// Requiring `Debug` of every host source to satisfy a derive would be the tail
/// wagging the dog, and the variant plus the message count is what a reader of
/// this actually wants.
impl std::fmt::Debug for Reader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            #[cfg(feature = "grib1")]
            Self::Grib1(r) => f
                .debug_struct("Grib1")
                .field("messages", &r.messages.len())
                .finish(),
            #[cfg(feature = "grib2")]
            Self::Grib2(r) => f
                .debug_struct("Grib2")
                .field("messages", &r.messages.len())
                .finish(),
            #[cfg(any(feature = "netcdf", feature = "zarr"))]
            Self::Arrays(a) => f.debug_struct("Arrays").field("format", &a.format).finish(),
        }
    }
}

/// A container of named arrays, and which one it is.
#[cfg(any(feature = "netcdf", feature = "zarr"))]
struct Arrays {
    /// The container's structure and values. For NetCDF the dataset view is
    /// resolved once, on open: for a NetCDF-4 backing it walks the whole object
    /// model, and every variable and slice question is asked of it afterwards.
    source: Box<dyn ArraySource>,
    /// What [`Session::format`] reports.
    format: SourceFormat,
    /// Slice placements already derived, keyed by what determines them
    /// (ADR-0011).
    ///
    /// A placement is a pure function of the container's metadata and its
    /// coordinate arrays, both fixed for the session's lifetime, so serving a
    /// second identical question from here is unobservable except in time. It
    /// is memoised rather than left to each host because *every* host needs
    /// it and only one had it: `fieldglass-napi` cached the curvilinear index
    /// for NetCDF, did not for Zarr — `ZarrHandle` re-derived it on every
    /// repaint — and the browser host cached neither.
    placements: Mutex<HashMap<PlacementKey, Arc<SlicePlacement>>>,
}

/// What a slice placement is determined by, and so what the memo is keyed on.
///
/// Two shapes because the expensive placement is not per-slice. Keeping one
/// key per *slice* would rebuild a whole-mesh index once per field, which is
/// the cost this memo exists to remove.
#[cfg(any(feature = "netcdf", feature = "zarr"))]
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum PlacementKey {
    /// A slice placed by a 2-D coordinate pair, keyed on that pair.
    ///
    /// One file's fields share one mesh: every RTOFS ice field sits on the
    /// same tripolar grid, so keying on the field would pay an `O(n log n)`
    /// build and the tree's whole footprint again for an identical answer.
    /// The names are group-qualified, so two groups each holding a `lat` and
    /// a `lon` stay distinct.
    ///
    /// Used **only** when the pair spans exactly the axes asked for. A
    /// cross-section through a curvilinear array — time against X, say — is
    /// not placed by the pair at all, and must not be handed the pair's
    /// answer.
    Coordinates { lat: String, lon: String },
    /// Every other placement, keyed on the array and the two axes.
    ///
    /// These are the cheap families — 1-D lat/lon, a WRF or CF projected
    /// domain, or nothing placed it — whose values are a handful of
    /// parameters. Memoised all the same, because deriving them re-reads the
    /// coordinate arrays, which on a chunked backing is a chunk read and a
    /// decompress per repaint.
    Axes { array: String, y: usize, x: usize },
}

#[cfg(any(feature = "netcdf", feature = "zarr"))]
impl std::fmt::Debug for Arrays {
    // The source is a trait object with no `Debug` of its own to lean on, and
    // printing a whole dataset would not help anyone reading a session.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Arrays")
            .field("format", &self.format)
            .field("arrays", &self.source.group().arrays_qualified().len())
            // How much the memo is holding, which is the one part of a session
            // that grows with use. A poisoned lock prints nothing rather than
            // panicking inside a formatter.
            .field("placements", &self.placements.lock().map(|p| p.len()).ok())
            .finish()
    }
}

#[cfg(any(feature = "netcdf", feature = "zarr"))]
impl Arrays {
    /// A container of arrays with an empty memo, which is how every arm that
    /// opens one starts.
    fn new(source: Box<dyn ArraySource>, format: SourceFormat) -> Self {
        Self {
            source,
            format,
            placements: Mutex::new(HashMap::new()),
        }
    }

    /// Where a slice sits on the Earth, derived once per thing that determines
    /// it (ADR-0011).
    ///
    /// The single call to [`slice_placement`] in this crate, so
    /// [`Session::decode_slice`] and [`Session::place_slice`] cannot disagree
    /// about a slice's geometry — they already shared the derivation, and now
    /// share the memo of it too.
    ///
    /// A poisoned lock is treated as a miss and the placement rebuilt. A memo
    /// can always recompute, so there is nothing here worth panicking over:
    /// the failure is slower answers, not wrong ones.
    fn placement(
        &self,
        var: &RenderableArray,
        y: usize,
        x: usize,
    ) -> Result<Arc<SlicePlacement>, fieldglass_core::FieldglassError> {
        let key = self.placement_key(var, y, x);
        if let Some(hit) = self
            .placements
            .lock()
            .ok()
            .and_then(|p| p.get(&key).cloned())
        {
            return Ok(hit);
        }
        let built = Arc::new(slice_placement(self.source.as_ref(), &var.name, y, x)?);
        if let Ok(mut p) = self.placements.lock() {
            p.insert(key, Arc::clone(&built));
        }
        Ok(built)
    }

    /// What this slice's placement is determined by — see [`PlacementKey`].
    ///
    /// Metadata only: [`curvilinear_pair`] reads the `coordinates` attribute
    /// and never a coordinate's values, so deriving the key costs nothing the
    /// memo is there to avoid.
    fn placement_key(&self, var: &RenderableArray, y: usize, x: usize) -> PlacementKey {
        // The guard on sharing across fields: the pair may only answer for the
        // axes it spans. Written as the pair's axis *names*, in order, because
        // that is exactly the test `slice_placement` applies before it reaches
        // for the pair — so the key cannot disagree with the placement about
        // whether this slice is placed by the pair at all. Restating it
        // positionally would: an array declaring one dimension name twice
        // resolves both positions to the first, and the key would then say "not
        // the pair" for a slice the placement does place by it.
        if let (Some(y_axis), Some(x_axis)) = (var.dims.get(y), var.dims.get(x))
            && let Some(pair) = curvilinear_pair(self.source.group(), &var.name)
            && pair.y_dim == y_axis.name
            && pair.x_dim == x_axis.name
        {
            return PlacementKey::Coordinates {
                lat: pair.lat,
                lon: pair.lon,
            };
        }
        PlacementKey::Axes {
            array: var.name.clone(),
            y,
            x,
        }
    }
}

/// What counts as a value, decided once: mask the absent and the non-finite
/// cells, range the rest, and pack them at the requested width.
///
/// Shared by every decode that hands back values — a field (message or slice)
/// and a line (#172) — so a line through a field cannot disagree with the field
/// about which of its cells are masked.
pub(crate) fn pack_values(
    raw: &[Option<f64>],
    options: &DecodeOptions,
) -> (Values, Vec<u8>, Stats) {
    let mut values = Vec::with_capacity(raw.len());
    let mut mask = Vec::with_capacity(raw.len());
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    let mut valid_count = 0u32;
    for cell in raw {
        match cell {
            // A non-finite decoded value is not a value: it cannot be ranged,
            // coloured, or interpolated, so it joins the masked cells rather
            // than poisoning the min / max.
            Some(v) if v.is_finite() => {
                values.push(*v);
                mask.push(1);
                min = min.min(*v);
                max = max.max(*v);
                valid_count += 1;
            }
            _ => {
                values.push(0.0);
                mask.push(0);
            }
        }
    }
    let stats = Stats {
        min: (valid_count > 0).then_some(min),
        max: (valid_count > 0).then_some(max),
        valid_count,
    };
    (
        Values::build(values, &mask, options.dtype.clone()),
        mask,
        stats,
    )
}

/// The half of a decode that is the same whichever way the container was
/// addressed: mask the absent cells, range the present ones, and pack.
///
/// Shared by [`Session::decode`] and [`Session::decode_slice`] rather than
/// written twice (#662). A second copy would be a second place for "what counts
/// as a value" to be decided, and the two would drift the first time one of
/// them learned something.
#[allow(clippy::too_many_arguments)]
fn build_field(
    raw: &[Option<f64>],
    ni: u32,
    nj: u32,
    geometry: &GridGeometry,
    scan: Scan,
    declared: &str,
    parameter: String,
    units: String,
    options: &DecodeOptions,
) -> Field {
    let (values, mask, stats) = pack_values(raw, options);
    Field {
        values,
        mask,
        ni,
        nj,
        georef: Georef::from_declared(geometry, scan, declared),
        stats,
        parameter,
        units,
    }
}

/// Everything [`detect_from_bytes`] looks at: `"GRIB"` plus the edition octet,
/// `CDF\x01`, or the HDF5 signature. Opening from a source reads this much and
/// not the file (#709).
#[cfg(any(feature = "grib1", feature = "grib2"))]
const DETECT_PREFIX: usize = 8;

/// One slice's placement: the grid, its storage order, and its raster shape.
///
/// What [`Session::place_slice`] hands back, and what
/// `source()` turns into the projection pipeline's own input — named rather than
/// linked, because that method is behind `render`/`analysis` and this type is
/// not. Owned
/// rather than borrowed because the geometry is *derived* — a 2-D coordinate pair
/// becomes a cell-centre lookup, a CF `grid_mapping` becomes a projection — so
/// there is nothing inside the container to borrow it from.
///
/// Not a wire type: it carries a [`GridGeometry`], which is the engine's own
/// shape rather than anything a host serialises. A host reports placement to its
/// UI from [`Field::georef`](crate::api::Field), and paints with this.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct PlacedSlice {
    /// The memoised placement, shared rather than copied.
    ///
    /// Every accessor below borrows out of this. Holding the `Arc` is what
    /// makes placing a curvilinear slice twice actually cheap: the cached
    /// value is a [`GridGeometry::Lookup`] over every cell — 28 bytes of
    /// index per cell, ~400 MB on a global ocean mesh — so returning it *by
    /// value* would pay an `O(n)` copy on each call and give back most of what
    /// the memo saves (ADR-0011).
    ///
    /// Private, and the type is `#[non_exhaustive]`, so this is a
    /// representation change rather than an API one.
    placement: Arc<SlicePlacement>,
    ni: u32,
    nj: u32,
}

impl PlacedSlice {
    /// Where the cells are.
    pub fn geometry(&self) -> &GridGeometry {
        &self.placement.geometry
    }

    /// The storage order, defaulted to north-down where the container states
    /// none — see [`fieldglass_core::cf::SlicePlacement::scan`] for why that is
    /// a different answer from "the container said north-down".
    pub fn scan(&self) -> Scan {
        self.placement.scan.unwrap_or_else(Scan::north_down)
    }

    /// Raster columns.
    pub fn ni(&self) -> u32 {
        self.ni
    }

    /// Raster rows.
    pub fn nj(&self) -> u32 {
        self.nj
    }

    /// What to call the family in a picker caption or a refusal.
    pub fn family(&self) -> &str {
        self.placement.geometry.label()
    }

    /// This placement as the projection pipeline's input.
    ///
    /// The one call that makes a host able to paint a container it has never
    /// heard of: hand it to `Session::project`, `probe_pixel`,
    /// `overlay_polylines` or `field_csv` beside the values
    /// [`Session::decode_slice`] returned.
    ///
    /// Those four are named rather than linked because they do not share a gate
    /// — three are behind `render` and `field_csv` behind `analysis` — and a doc
    /// link only resolves in a build where both ends are compiled.
    #[cfg(any(feature = "render", feature = "analysis"))]
    pub fn source(&self) -> crate::render::Source<'_> {
        crate::render::Source {
            geometry: Ok(&self.placement.geometry),
            ni: self.ni,
            nj: self.nj,
            scan: self.scan(),
            family: self.family(),
            points_per_row: None,
        }
    }
}

/// The refusal a source-taking constructor gives a container that is not a
/// message stream, naming the call to make instead.
///
/// One place, so `open_source` and `open_message_at` cannot word it differently,
/// and so each answer says *why* rather than only "no": NetCDF because its
/// reader holds a buffer rather than reading through the seam, Zarr because it is
/// keyed rather than ranged.
#[cfg(any(feature = "grib1", feature = "grib2"))]
fn not_a_message_stream(format: CoreFormat, called: &str) -> Error {
    let detail = match format {
        CoreFormat::NetCdf => format!(
            "`{called}` opens a message stream, and these bytes are NetCDF, whose reader holds \
             its file as a buffer rather than reading through a source; call `Session::open` \
             with the bytes instead"
        ),
        CoreFormat::Unknown => format!(
            "`{called}` found no container this build knows in the bytes at the offset it was \
             given"
        ),
        other => format!(
            "`{called}` opens a message stream, and these bytes are {other:?}; a keyed store \
             opens with `Session::open_store`"
        ),
    };
    Error::UnsupportedFormat { detail }
}

/// The refusal a message call gets from a variable dataset, and vice versa.
///
/// One place so the two halves cannot word it differently, and so the `detail`
/// always names the call to make instead — an error that only says "no" costs
/// the caller a trip to the docs.
///
/// `mode` is how *this* session is addressed, which is the wire value
/// `expected` carries; `called` belongs to the other mode. Both are derived
/// from it rather than written at the call site, because until #679 the
/// message-stream direction reported a GRIB session as addressed by variables
/// and said `decode_slice` addressed messages.
fn wrong_addressing(mode: Addressing, called: &str, instead: &str) -> Error {
    let (this, other) = match mode {
        Addressing::Messages => ("messages", "variables"),
        Addressing::Variables => ("variables", "messages"),
    };
    Error::WrongAddressing {
        expected: this.to_string(),
        detail: format!("`{called}` addresses {other}; call `{instead}` instead"),
    }
}

/// The element type's name on the wire.
///
/// NetCDF's own type names (`short`, `float`, `double`, `ubyte`, …), which is
/// what a NetCDF variable reported before #704 and so what a host already shows.
/// A Zarr array of the same type reports the same name; a width NetCDF has no
/// name for (a Zarr `float16`) is spelled from its kind and width.
#[cfg(any(feature = "netcdf", feature = "zarr"))]
fn dtype_name(element_type: &ElementType) -> String {
    match element_type {
        ElementType::Int(8) => "byte".to_string(),
        ElementType::Int(16) => "short".to_string(),
        ElementType::Int(32) => "int".to_string(),
        ElementType::Int(64) => "int64".to_string(),
        ElementType::Uint(8) => "ubyte".to_string(),
        ElementType::Uint(16) => "ushort".to_string(),
        ElementType::Uint(32) => "uint".to_string(),
        ElementType::Uint(64) => "uint64".to_string(),
        ElementType::Float(32) => "float".to_string(),
        ElementType::Float(64) => "double".to_string(),
        ElementType::Int(bits) => format!("int{bits}"),
        ElementType::Uint(bits) => format!("uint{bits}"),
        ElementType::Float(bits) => format!("float{bits}"),
        ElementType::Text => "char".to_string(),
        ElementType::Other(name) => name.clone(),
        other => format!("{other:?}").to_lowercase(),
    }
}

/// One line through the array named `array` in `source` — the implementation
/// behind [`Session::decode_line`], for a host that holds an [`ArraySource`]
/// rather than a `Session` (#172).
///
/// Keyed by the array's **name** rather than a position in a variable list,
/// because the two kinds of caller number variables differently: `Session`
/// counts positions in [`Session::variables`], while a host that built its own
/// listing — `fieldglass-napi`'s NetCDF handle numbers by the file's decode
/// index — has only the name in common with it. One implementation keyed on the
/// thing both have is what stops a line from having two definitions.
///
/// See [`Session::decode_line`] for the meaning of every argument and the
/// errors, with one difference: an array this source does not list as renderable
/// is [`Error::NoSuchMessage`] with `index` 0, since there is no position to
/// report.
#[cfg(any(feature = "netcdf", feature = "zarr"))]
pub fn line_through(
    source: &dyn ArraySource,
    array: &str,
    along_dim: u32,
    indices: &[u32],
    options: &DecodeOptions,
) -> Result<Line, Error> {
    let vars = renderable_arrays(source.group());
    let var = vars
        .iter()
        .find(|v| v.name == array)
        .ok_or(Error::NoSuchMessage {
            index: 0,
            count: u32::try_from(vars.len()).unwrap_or(u32::MAX),
        })?;
    let rank = var.dims.len();
    let along = along_dim as usize;
    if along >= rank {
        return Err(Error::InvalidOption {
            detail: format!(
                "`{}` has {rank} dimensions, so along_dim {along} is outside them",
                var.name
            ),
        });
    }
    if indices.len() != rank {
        return Err(Error::InvalidOption {
            detail: format!(
                "`{}` has {rank} dimensions, so `indices` needs {rank} entries \
                 (the one being read along is ignored); {} were given",
                var.name,
                indices.len()
            ),
        });
    }
    let mut region = Vec::with_capacity(rank);
    for (d, dim) in var.dims.iter().enumerate() {
        if d == along {
            region.push(0..dim.length);
            continue;
        }
        let at = u64::from(indices[d]);
        if at >= dim.length {
            return Err(Error::InvalidOption {
                detail: format!(
                    "index {at} is past the end of `{}`'s dimension {d} \
                     (`{}`, length {})",
                    var.name, dim.name, dim.length
                ),
            });
        }
        region.push(at..at + 1);
    }
    // A region with every axis but one a single index comes back as
    // exactly the points on the line, in index order.
    let raw = source.read_region(&var.name, &region)?;
    let attributes = source
        .array(&var.name)
        .map(|d| d.attributes.as_slice())
        .unwrap_or_default();
    let physical = CfUnpacking::from_attributes(attributes).apply(&raw);
    let (values, mask, stats) = pack_values(&physical, options);
    let dimension = var.dims[along].name.clone();
    let coordinates = axis_coordinates(source, &dimension, var.dims[along].length);
    Ok(Line {
        values,
        mask,
        stats,
        variable: var.name.clone(),
        units: array_units(source, &var.name),
        coordinate_units: coordinates
            .as_ref()
            .map(|_| array_units(source, &dimension))
            .filter(|u| !u.is_empty()),
        coordinates,
        dimension,
    })
}

/// The coordinate values of an axis, from the 1-D array CF names after it
/// (#172).
///
/// `None` unless that array exists, spans exactly this axis, and has a value at
/// every position — a coordinate with a hole has no position to plot its point
/// at, and a host is better served falling back to indices than handed a gap
/// to invent across.
#[cfg(any(feature = "netcdf", feature = "zarr"))]
fn axis_coordinates(source: &dyn ArraySource, dimension: &str, length: u64) -> Option<Vec<f64>> {
    let array = source.array(dimension)?;
    if array.dimensions.len() != 1 || array.dimensions[0] != dimension {
        return None;
    }
    // A one-axis region, the whole axis. Spelled through `from_ref` because
    // `&[0..length]` reads to clippy as a mistyped `[0; length]`.
    let whole = 0..length;
    let raw = source
        .read_region(dimension, std::slice::from_ref(&whole))
        .ok()?;
    let physical = CfUnpacking::from_attributes(&array.attributes).apply(&raw);
    physical.into_iter().collect()
}

/// An array's `units`, as the container spells them, or empty.
#[cfg(any(feature = "netcdf", feature = "zarr"))]
fn array_units(source: &dyn ArraySource, array: &str) -> String {
    source
        .array(array)
        .and_then(|d| attribute(&d.attributes, "units"))
        .and_then(AttributeValue::text)
        .map(str::to_string)
        .unwrap_or_default()
}

/// One 2-D plane of an array, raw, as `nj` rows (along `y`) of `ni` values
/// (along `x`).
///
/// Read as one region — the full extent of the two horizontal axes, one index
/// of every other — so a container that fetches by chunk fetches only the
/// chunks the plane covers. The region comes back in the array's declared axis
/// order, so an array whose `x` axis precedes its `y` is transposed into rows.
/// A held index past its axis is refused in the words the NetCDF plane
/// extraction always used.
#[cfg(any(feature = "netcdf", feature = "zarr"))]
fn read_plane(
    source: &dyn ArraySource,
    var: &fieldglass_core::cf::RenderableArray,
    y: usize,
    x: usize,
    fixed: &[usize],
) -> Result<Vec<Option<f64>>, Error> {
    let mut region = Vec::with_capacity(var.dims.len());
    for (d, dim) in var.dims.iter().enumerate() {
        if d == y || d == x {
            region.push(0..dim.length);
            continue;
        }
        let at = fixed[d] as u64;
        if at >= dim.length {
            return Err(fieldglass_core::FieldglassError::Parse(format!(
                "slice index {} out of range for dimension {d} (length {})",
                fixed[d], dim.length
            ))
            .into());
        }
        region.push(at..at + 1);
    }
    let raw = source.read_region(&var.name, &region)?;
    if y < x {
        return Ok(raw);
    }
    // Stored with `x` outer and `y` inner: `raw[i·nj + j]` is the cell at row
    // `j`, column `i`.
    let (ni, nj) = (var.dims[x].length as usize, var.dims[y].length as usize);
    let mut rows = Vec::with_capacity(raw.len());
    for j in 0..nj {
        for i in 0..ni {
            rows.push(raw.get(i * nj + j).copied().flatten());
        }
    }
    Ok(rows)
}

impl Session {
    /// Open a container from its bytes.
    ///
    /// The format is detected from the bytes, never from a name: ADR-0005 hands
    /// the host's fetched buffer straight in, and a range-fetched GRIB message
    /// has no filename to guess from.
    pub fn open(bytes: Vec<u8>) -> Result<Self, Error> {
        let reader = match detect_from_bytes(&bytes) {
            #[cfg(feature = "grib1")]
            CoreFormat::Grib1 => Reader::Grib1(Box::new(
                fieldglass_grib1::Grib1Reader::from_source(Box::new(bytes) as Bytes)?,
            )),
            #[cfg(not(feature = "grib1"))]
            CoreFormat::Grib1 => {
                return Err(Error::UnsupportedFormat {
                    detail: "GRIB1; this build was compiled without the `grib1` feature"
                        .to_string(),
                });
            }
            #[cfg(feature = "grib2")]
            CoreFormat::Grib2 => Reader::Grib2(Box::new(
                fieldglass_grib2::Grib2Reader::from_source(Box::new(bytes) as Bytes)?,
            )),
            #[cfg(not(feature = "grib2"))]
            CoreFormat::Grib2 => {
                return Err(Error::UnsupportedFormat {
                    detail: "GRIB2; this build was compiled without the `grib2` feature"
                        .to_string(),
                });
            }
            #[cfg(feature = "netcdf")]
            CoreFormat::NetCdf => {
                let reader = fieldglass_netcdf::NetcdfReader::from_bytes(bytes)?;
                // The view is resolved here, so a variable list costs one walk
                // per file rather than one per question.
                Reader::Arrays(Box::new(Arrays::new(
                    Box::new(fieldglass_netcdf::NetcdfArrays::open(reader)?),
                    SourceFormat::NetCdf,
                )))
            }
            #[cfg(not(feature = "netcdf"))]
            CoreFormat::NetCdf => {
                return Err(Error::UnsupportedFormat {
                    detail: "NetCDF; this build was compiled without the `netcdf` feature"
                        .to_string(),
                });
            }
            CoreFormat::Unknown => {
                return Err(Error::UnsupportedFormat {
                    detail: "the bytes match no container this build knows".to_string(),
                });
            }
        };
        Ok(Self { reader })
    }

    /// The reader a detected format asks for, over a source.
    ///
    /// Shared by [`Session::open_source`] and, through it, the feature-gating
    /// refusals — so a build without an edition says the same thing here as
    /// [`Session::open`] says.
    #[cfg(any(feature = "grib1", feature = "grib2"))]
    fn from_detected(format: CoreFormat, source: Bytes) -> Result<Self, Error> {
        let reader = match format {
            #[cfg(feature = "grib1")]
            CoreFormat::Grib1 => Reader::Grib1(Box::new(
                fieldglass_grib1::Grib1Reader::from_source(source)?,
            )),
            #[cfg(not(feature = "grib1"))]
            CoreFormat::Grib1 => {
                return Err(Error::UnsupportedFormat {
                    detail: "GRIB1; this build was compiled without the `grib1` feature"
                        .to_string(),
                });
            }
            #[cfg(feature = "grib2")]
            CoreFormat::Grib2 => Reader::Grib2(Box::new(
                fieldglass_grib2::Grib2Reader::from_source(source)?,
            )),
            #[cfg(not(feature = "grib2"))]
            CoreFormat::Grib2 => {
                return Err(Error::UnsupportedFormat {
                    detail: "GRIB2; this build was compiled without the `grib2` feature"
                        .to_string(),
                });
            }
            other => return Err(not_a_message_stream(other, "open_source")),
        };
        Ok(Self { reader })
    }

    /// Open a message stream from a source the host reads through, rather than
    /// from a buffer it has all of (#709).
    ///
    /// [`Session::open`] takes the whole file. That is the right shape when the
    /// host has it — and the wrong one for the work ADR-0005 exists for: a file
    /// too large to hold (#114), an archive reached over HTTP range requests
    /// (#247), an object in a bucket (#252). Those hosts have a
    /// [`ByteSource`], and until now reaching a reader with one meant going
    /// around this crate to `Grib1Reader::from_source` — the host divergence
    /// [ADR-0006] exists to prevent.
    ///
    /// The format is detected from the first eight bytes, which is all
    /// [`detect_from_bytes`] reads, so opening costs one small read and not the
    /// file.
    ///
    /// # Only message streams
    ///
    /// A GRIB file, of either edition. **Not NetCDF**: `NetcdfReader` holds its
    /// bytes as a buffer rather than reading through the seam, so there is
    /// nothing for this to hand it — the error says so and names
    /// [`Session::open`]. A Zarr store is keyed rather than ranged and has
    /// `Session::open_store` — named rather than linked, because that one is
    /// behind the `zarr` feature and this is not. Both refusals name the call to
    /// make instead rather than only saying no.
    ///
    /// # Errors
    ///
    /// A source whose leading bytes match no container this build opens, a
    /// container that is not a message stream, and any failure of the source or
    /// of the scan.
    ///
    /// [ADR-0006]: https://github.com/D0ubleD0uble/fieldglass/blob/master/docs/decisions/0006-one-umbrella-crate-and-host-bindings-over-it.md
    #[cfg(any(feature = "grib1", feature = "grib2"))]
    pub fn open_source<S: ByteSource + 'static>(source: S) -> Result<Self, Error> {
        let source: Bytes = Box::new(source);
        // Eight bytes is everything `detect_from_bytes` looks at: "GRIB" plus
        // the edition octet, `CDF\x01`, or the HDF5 signature. `read_up_to`
        // rather than a fixed read, so a source shorter than that says "no
        // container" instead of "short read".
        let head = read_up_to(&source, 0, DETECT_PREFIX)?;
        let format = detect_from_bytes(&head);
        drop(head);
        Self::from_detected(format, source)
    }

    /// Open the single message at `offset` in `source`, without scanning for it.
    ///
    /// What a host does with a sidecar index (#685): the index says where a
    /// message begins, so there is no reason to walk the file to find it — and
    /// over a transport that walk is the whole point of having the index. The
    /// session holds exactly that one message, so its
    /// [`count`](Session::count) is 1 and the message is index 0 whatever its
    /// place in the file.
    ///
    /// The source need hold nothing but that message's range. It still has to
    /// report the whole file's [`size`](ByteSource::size), because the message's
    /// own length is read from its indicator section.
    ///
    /// # Errors
    ///
    /// Bytes at `offset` that are not the start of a GRIB message of an edition
    /// this build decodes, and any failure of the source.
    #[cfg(any(feature = "grib1", feature = "grib2"))]
    pub fn open_message_at<S: ByteSource + 'static>(source: S, offset: u64) -> Result<Self, Error> {
        let source: Bytes = Box::new(source);
        // Detected *at the offset*, not at the start of the file: a sidecar
        // index points into an archive whose first bytes may be another
        // message, another edition, or not a message at all.
        let head = read_up_to(&source, offset, DETECT_PREFIX)?;
        let format = detect_from_bytes(&head);
        drop(head);
        let reader = match format {
            #[cfg(feature = "grib1")]
            CoreFormat::Grib1 => Reader::Grib1(Box::new(
                fieldglass_grib1::Grib1Reader::from_message_at(source, offset)?,
            )),
            #[cfg(feature = "grib2")]
            CoreFormat::Grib2 => Reader::Grib2(Box::new(
                fieldglass_grib2::Grib2Reader::from_message_at(source, offset)?,
            )),
            other => return Err(not_a_message_stream(other, "open_message_at")),
        };
        Ok(Self { reader })
    }

    /// Open a Zarr store from the objects a host holds for it (#704).
    ///
    /// A store is many objects rather than one run of bytes, so it cannot come
    /// through [`Session::open`]: the host fills an [`ObjectSource`] — a map of
    /// the files it read from a directory, or the objects it fetched from a
    /// bucket — and hands that over, which is ADR-0005's split for a keyed
    /// store. Both editions, consolidated or not. The session is addressed by
    /// variables, like NetCDF's, and answers through the same rules.
    ///
    /// # Errors
    ///
    /// When the objects are not a Zarr store or a root document does not parse.
    /// An array that fails on its own is left out of [`Session::variables`]
    /// rather than failing the store.
    #[cfg(feature = "zarr")]
    pub fn open_store<O: ObjectSource + 'static>(objects: O) -> Result<Self, Error> {
        let store = fieldglass_zarr::ZarrStore::open(objects)?;
        Ok(Self {
            reader: Reader::Arrays(Box::new(Arrays::new(Box::new(store), SourceFormat::Zarr))),
        })
    }

    /// Which container the bytes turned out to be. Detected at
    /// [`Session::open`], never re-sniffed.
    pub fn format(&self) -> SourceFormat {
        match &self.reader {
            #[cfg(feature = "grib1")]
            Reader::Grib1(_) => SourceFormat::Grib1,
            #[cfg(feature = "grib2")]
            Reader::Grib2(_) => SourceFormat::Grib2,
            #[cfg(any(feature = "netcdf", feature = "zarr"))]
            Reader::Arrays(a) => a.format.clone(),
        }
    }

    /// How this container is addressed — messages, or variables and slices.
    ///
    /// Asked once on open. It says which half of this type applies, and a host
    /// that ignores it meets [`Error::WrongAddressing`] from the first call.
    pub fn addressing(&self) -> Addressing {
        match self.reader {
            #[cfg(feature = "grib1")]
            Reader::Grib1(_) => Addressing::Messages,
            #[cfg(feature = "grib2")]
            Reader::Grib2(_) => Addressing::Messages,
            #[cfg(any(feature = "netcdf", feature = "zarr"))]
            Reader::Arrays(_) => Addressing::Variables,
        }
    }

    /// How many messages the container holds. Message indices run
    /// `0..count()`; anything outside is [`Error::NoSuchMessage`].
    pub fn count(&self) -> u32 {
        let n = match &self.reader {
            #[cfg(feature = "grib1")]
            Reader::Grib1(r) => r.message_count(),
            #[cfg(feature = "grib2")]
            Reader::Grib2(r) => r.message_count(),
            // Not an error and not a lie: an array dataset holds no messages.
            // `message` and `decode` say so properly; this is the count of the
            // thing being counted.
            #[cfg(any(feature = "netcdf", feature = "zarr"))]
            Reader::Arrays(_) => 0,
        };
        // A file with more messages than a `u32` counts does not exist; the
        // saturating cast is here so the index type and the count type agree
        // rather than because the clamp is reachable.
        u32::try_from(n).unwrap_or(u32::MAX)
    }

    /// Message `i`'s points per row, when its grid is reduced (#244).
    #[cfg(any(feature = "grib1", feature = "grib2"))]
    fn message_points_per_row(&self, i: usize) -> Option<Vec<u32>> {
        match &self.reader {
            #[cfg(feature = "grib1")]
            Reader::Grib1(r) => r.messages[i]
                .gds
                .as_ref()
                .and_then(|g| g.points_per_row())
                .map(<[u32]>::to_vec),
            #[cfg(feature = "grib2")]
            Reader::Grib2(r) => r.messages[i].gds.points_per_row().map(<[u32]>::to_vec),
            #[cfg(any(feature = "netcdf", feature = "zarr"))]
            Reader::Arrays(_) => None,
        }
    }

    // Only a message container range-checks an index; a build with no GRIB
    // decoder never reaches one.
    #[cfg(any(feature = "grib1", feature = "grib2"))]
    fn check_index(&self, index: u32) -> Result<usize, Error> {
        let count = self.count();
        if index >= count {
            return Err(Error::NoSuchMessage { index, count });
        }
        Ok(index as usize)
    }

    /// One message's metadata. Built on demand — a thousand-message file costs
    /// nothing to open.
    pub fn message(&self, index: u32) -> Result<MessageInfo, Error> {
        // Asked before the index is range-checked: against a variable dataset
        // `count()` is zero, so the range check would answer "index 0 is
        // outside the 0 available" — which tells a caller its index was wrong
        // when its whole question was.
        #[cfg(any(feature = "netcdf", feature = "zarr"))]
        if matches!(self.reader, Reader::Arrays(_)) {
            return Err(wrong_addressing(
                Addressing::Variables,
                "message",
                "variables",
            ));
        }
        #[cfg(any(feature = "grib1", feature = "grib2"))]
        {
            let i = self.check_index(index)?;
            Ok(match &self.reader {
                #[cfg(feature = "grib1")]
                Reader::Grib1(r) => grib1_message(r, i),
                #[cfg(feature = "grib2")]
                Reader::Grib2(r) => grib2_message(r, i),
                #[cfg(any(feature = "netcdf", feature = "zarr"))]
                Reader::Arrays(_) => {
                    return Err(wrong_addressing(
                        Addressing::Variables,
                        "message",
                        "variables",
                    ));
                }
            })
        }
        // A build with no GRIB decoder has no message path at all. Answering
        // rather than panicking: the guard above already returned for the only
        // reader such a build can hold, so this is unreachable in fact and
        // total in type.
        #[cfg(not(any(feature = "grib1", feature = "grib2")))]
        {
            let _ = index;
            Err(wrong_addressing(
                Addressing::Variables,
                "message",
                "variables",
            ))
        }
    }

    /// Decode one message into a field: values, mask, geometry, and the range
    /// a palette is built from.
    ///
    /// **A family with no raster of its own is synthesised first.** Spectral
    /// coefficients and HEALPix pixels are not values on a grid, so the reader
    /// puts them on one — a global lat/lon grid at the
    /// [`fieldglass_core::global_grid`] convention — and what comes back is an
    /// ordinary [`crate::Georef`] of kind `"latlon"`. Everything downstream
    /// (warp, palette, render, probe, contours, combine) therefore needs no
    /// special case, which is `docs/architecture/planned/03-composition.md`'s
    /// rule for these families and what the napi host has done since 0.4.0
    /// (#580). [`Session::message`] keeps reporting the *native* shape —
    /// `size_label` is `"T63"`, not `720×361` — so the message list still
    /// describes the file rather than describing Fieldglass.
    pub fn decode(&self, index: u32, options: &DecodeOptions) -> Result<Field, Error> {
        // Before the range check, for the reason `message` explains.
        #[cfg(any(feature = "netcdf", feature = "zarr"))]
        if matches!(self.reader, Reader::Arrays(_)) {
            return Err(wrong_addressing(
                Addressing::Variables,
                "decode",
                "decode_slice",
            ));
        }
        #[cfg(any(feature = "grib1", feature = "grib2"))]
        {
            let i = self.check_index(index)?;
            // Asked before anything else, and the same question of both readers:
            // which families need synthesising, and onto what grid, is the format
            // crate's answer, not one this crate re-derives (#546, #580).
            let synthesised = match &self.reader {
                #[cfg(feature = "grib1")]
                Reader::Grib1(r) => r.synthesize_message_global(i)?,
                #[cfg(feature = "grib2")]
                Reader::Grib2(r) => r.synthesize_message_global(i)?,
                #[cfg(any(feature = "netcdf", feature = "zarr"))]
                Reader::Arrays(_) => {
                    return Err(wrong_addressing(
                        Addressing::Variables,
                        "decode",
                        "decode_slice",
                    ));
                }
            };
            let was_synthesised = synthesised.is_some();
            let (parameter, units) = match &self.reader {
                #[cfg(feature = "grib1")]
                Reader::Grib1(r) => {
                    let (_, parameter, units) = grib1_parameter(&r.messages[i]);
                    (parameter, units)
                }
                #[cfg(feature = "grib2")]
                Reader::Grib2(r) => {
                    let (_, parameter, units) = grib2_parameter(&r.messages[i]);
                    (parameter, units)
                }
                #[cfg(any(feature = "netcdf", feature = "zarr"))]
                Reader::Arrays(_) => {
                    return Err(wrong_addressing(
                        Addressing::Variables,
                        "decode",
                        "decode_slice",
                    ));
                }
            };
            // `declared` is the family name the message states, which survives the
            // conversion only if it is carried (#645): both decoders widen a
            // reduced grid onto its regular sibling's raster, so the geometry no
            // longer knows it was a `reduced_gg`. A **synthesised** grid is the
            // case where the geometry is the honest answer — the values really are
            // on the lat/lon raster the transform filled, and nothing of the
            // declared family survives it — so that arm reads the geometry's own
            // label rather than the message's.
            let (raw, geometry, scan, declared) = match synthesised {
                Some((grid, values)) => {
                    let geometry = GridGeometry::LatLon(grid.into());
                    let declared = geometry.label().to_string();
                    (
                        values,
                        geometry,
                        // A synthesised grid runs west-to-east from 0° and
                        // north-down from the pole whatever the message it came
                        // from scanned like: nothing of the source layout survives
                        // an inverse transform or a HEALPix resample. The napi host
                        // says the same thing at its own seam.
                        Scan::north_down(),
                        declared,
                    )
                }
                None => match &self.reader {
                    #[cfg(feature = "grib1")]
                    Reader::Grib1(r) => {
                        let msg = &r.messages[i];
                        let gds = msg.gds.as_ref().ok_or_else(|| Error::Unsupported {
                            detail: "the message carries no grid description".to_string(),
                        })?;
                        let geometry = GridGeometry::from(gds);
                        (
                            r.decode_message_raster(i)?,
                            geometry,
                            grib1_scan(msg),
                            gds.grid_type_name().to_string(),
                        )
                    }
                    #[cfg(feature = "grib2")]
                    Reader::Grib2(r) => {
                        let msg = &r.messages[i];
                        let geometry = GridGeometry::from(&msg.gds);
                        (
                            r.decode_message_raster(i)?,
                            geometry,
                            grib2_scan(msg),
                            msg.gds.template_name(),
                        )
                    }
                    #[cfg(any(feature = "netcdf", feature = "zarr"))]
                    Reader::Arrays(_) => {
                        return Err(wrong_addressing(
                            Addressing::Variables,
                            "decode",
                            "decode_slice",
                        ));
                    }
                },
            };

            let (ni, nj) = geometry.dims().ok_or_else(|| Error::Unsupported {
                detail: format!("a {} field has no raster to decode onto", geometry.label()),
            })?;
            let expected = (ni as usize).saturating_mul(nj as usize);
            if raw.len() != expected {
                return Err(Error::Decode {
                    detail: format!(
                        "decoded {} values for a {ni}×{nj} grid, which needs {expected}",
                        raw.len()
                    ),
                });
            }
            let mut field = build_field(
                &raw, ni, nj, &geometry, scan, &declared, parameter, units, options,
            );
            // A reduced grid's values arrive widened to its widest row, and the
            // field has to say how many of each row's cells are the file's own
            // (#244). A synthesised grid is not reduced whatever the message
            // declared, so it states none.
            if !was_synthesised {
                field.georef.points_per_row = self.message_points_per_row(i);
            }
            Ok(field)
        }
        // As in `message`: with no GRIB decoder compiled there is no
        // message path, and the guard above has already answered.
        #[cfg(not(any(feature = "grib1", feature = "grib2")))]
        {
            let _ = (index, options);
            Err(wrong_addressing(
                Addressing::Variables,
                "decode",
                "decode_slice",
            ))
        }
    }

    /// The dataset's shared dimensions, in the order the file declares them.
    ///
    /// Shared is the point: two variables naming the same dimension are on the
    /// same axis, so a host offers one time slider for a file rather than one
    /// per variable.
    ///
    /// Empty for a message container — see [`Self::addressing`].
    pub fn dimensions(&self) -> Vec<DimensionInfo> {
        match &self.reader {
            #[cfg(feature = "grib1")]
            Reader::Grib1(_) => Vec::new(),
            #[cfg(feature = "grib2")]
            Reader::Grib2(_) => Vec::new(),
            #[cfg(any(feature = "netcdf", feature = "zarr"))]
            Reader::Arrays(a) => a
                .source
                .group()
                .dimensions_qualified()
                .into_iter()
                .map(|(name, d)| DimensionInfo {
                    name,
                    length: d.length,
                })
                .collect(),
        }
    }

    /// The arrays this container holds and this build will not read, each with
    /// why.
    ///
    /// One answer for every container (#709). A NetCDF file and a Zarr store used
    /// to state this in two different shapes — a named struct behind one reader
    /// and a tuple of two strings behind the other — so a host reading both had
    /// to know which was which. Names are spelled as [`Session::variables`]
    /// spells a readable one, so a caller can match the two lists.
    ///
    /// Empty for a message stream, which has no arrays to leave out, and for a
    /// container that reads everything it can describe. **Never an error**: one
    /// unreadable array does not fail a container.
    pub fn left_out(&self) -> Vec<LeftOutArray> {
        match &self.reader {
            #[cfg(feature = "grib1")]
            Reader::Grib1(_) => Vec::new(),
            #[cfg(feature = "grib2")]
            Reader::Grib2(_) => Vec::new(),
            #[cfg(any(feature = "netcdf", feature = "zarr"))]
            Reader::Arrays(a) => a
                .source
                .left_out()
                .into_iter()
                .map(|l| LeftOutArray {
                    name: l.name,
                    reason: l.reason,
                })
                .collect(),
        }
    }

    /// The variables a caller can decode a slice of.
    ///
    /// Renderable ones only — a variable of fewer than two dimensions has no
    /// raster to put on a map, and a coordinate variable is an axis rather than
    /// a field. `index` is the handle [`Self::decode_slice`] takes.
    ///
    /// Empty for a message container — see [`Self::addressing`].
    pub fn variables(&self) -> Vec<VariableInfo> {
        match &self.reader {
            #[cfg(feature = "grib1")]
            Reader::Grib1(_) => Vec::new(),
            #[cfg(feature = "grib2")]
            Reader::Grib2(_) => Vec::new(),
            #[cfg(any(feature = "netcdf", feature = "zarr"))]
            Reader::Arrays(a) => renderable_arrays(a.source.group())
                .into_iter()
                .enumerate()
                .map(|(i, v)| VariableInfo {
                    // Position in *this* list, not anything the reader numbers
                    // by: a host should never have to know that a reader counts
                    // every array while this offers only the renderable ones.
                    index: u32::try_from(i).unwrap_or(u32::MAX),
                    dims: v
                        .dims
                        .iter()
                        .map(|d| DimensionInfo {
                            name: d.name.clone(),
                            length: d.length,
                        })
                        .collect(),
                    dtype: dtype_name(&v.element_type),
                    units: array_units(a.source.as_ref(), &v.name),
                    detected_y_dim: v.detected_y_dim.and_then(|d| u32::try_from(d).ok()),
                    detected_x_dim: v.detected_x_dim.and_then(|d| u32::try_from(d).ok()),
                    name: v.name,
                })
                .collect(),
        }
    }

    /// Decode one 2-D slice of a variable into the same [`Field`]
    /// [`Self::decode`] returns.
    ///
    /// `y_dim` and `x_dim` index into the variable's own `dims`, and
    /// `slice_indices` holds **one entry per dimension** in that same declared
    /// order — so `slice_indices[d]` is always the position on `dims[d]`, and
    /// the two horizontal entries are ignored rather than absent. A caller
    /// never has to map a reduced list back onto the file's own axis numbering,
    /// which is why the length is the rank and not the rank minus two.
    ///
    /// A wrong length is [`Error::InvalidOption`] rather than a guess: silently
    /// defaulting the unstated axes to zero is how a host renders the first
    /// time step and labels it the last.
    ///
    /// The returned field is not special: `render`, `probe`, `contours`,
    /// `combine`, `warp` and `palette` take it exactly as they take a decoded
    /// message. That is the whole reason the addressing split stops here.
    // Every parameter is read by the arrays arm alone, so a GRIB-only build
    // sees a signature it cannot use. Kept in the signature regardless: the API
    // a host compiles against must not change shape with the feature set, or a
    // build without NetCDF would not be the same crate.
    #[cfg_attr(
        not(any(feature = "netcdf", feature = "zarr")),
        allow(unused_variables)
    )]
    pub fn decode_slice(
        &self,
        variable: u32,
        y_dim: u32,
        x_dim: u32,
        slice_indices: &[u32],
        options: &DecodeOptions,
    ) -> Result<Field, Error> {
        match &self.reader {
            #[cfg(feature = "grib1")]
            Reader::Grib1(_) => Err(wrong_addressing(
                Addressing::Messages,
                "decode_slice",
                "decode",
            )),
            #[cfg(feature = "grib2")]
            Reader::Grib2(_) => Err(wrong_addressing(
                Addressing::Messages,
                "decode_slice",
                "decode",
            )),
            #[cfg(any(feature = "netcdf", feature = "zarr"))]
            Reader::Arrays(a) => {
                let source = a.source.as_ref();
                let vars = renderable_arrays(source.group());
                let var = vars.get(variable as usize).ok_or(Error::NoSuchMessage {
                    index: variable,
                    count: u32::try_from(vars.len()).unwrap_or(u32::MAX),
                })?;
                let (y, x) = (y_dim as usize, x_dim as usize);
                let fixed: Vec<usize> = slice_indices.iter().map(|&i| i as usize).collect();
                if fixed.len() != var.dims.len() {
                    return Err(Error::InvalidOption {
                        detail: format!(
                            "`{}` has {} dimensions, so `slice_indices` needs {} entries \
                             (the two horizontal ones are ignored); {} were given",
                            var.name,
                            var.dims.len(),
                            var.dims.len(),
                            fixed.len()
                        ),
                    });
                }
                if y == x || y >= var.dims.len() || x >= var.dims.len() {
                    return Err(Error::InvalidOption {
                        detail: format!(
                            "`{}` has {} dimensions; y_dim {y} and x_dim {x} must be \
                             different and within them",
                            var.name,
                            var.dims.len()
                        ),
                    });
                }
                let plane = read_plane(source, var, y, x, &fixed)?;
                // The CF mask-and-scale, from the array's own attributes, so a
                // packed `int16` arrives in physical units the way a GRIB field
                // does. Once: a second `scale_factor` / `add_offset` pass
                // computes `(raw·s + o)·s + o`, which for the committed CF
                // fixture turns 250 K into 265.625 K. Every number is finite and
                // plausible, so nothing downstream can tell.
                let attributes = source
                    .array(&var.name)
                    .map(|d| d.attributes.as_slice())
                    .unwrap_or_default();
                let values = CfUnpacking::from_attributes(attributes).apply(&plane);
                let placement = a.placement(var, y, x)?;
                let ni = u32::try_from(var.dims[x].length).unwrap_or(u32::MAX);
                let nj = u32::try_from(var.dims[y].length).unwrap_or(u32::MAX);
                let units = array_units(source, &var.name);
                let declared = placement.geometry.label().to_string();
                Ok(build_field(
                    &values,
                    ni,
                    nj,
                    &placement.geometry,
                    placement.scan.unwrap_or_else(Scan::north_down),
                    &declared,
                    var.name.clone(),
                    units,
                    options,
                ))
            }
        }
    }

    /// One line through a variable: its values along `along_dim`, with every
    /// other axis held at `indices` (#172).
    ///
    /// The profile or time series a viewer plots when a user clicks a cell. The
    /// click has already been resolved to a cell by a probe; this reads the
    /// variable through that cell along the axis the user picked, with the
    /// axis's coordinate values to label it against.
    ///
    /// `indices` names a position on **every** axis, the one being read along
    /// included — its entry is ignored, the way `decode_slice` ignores the two
    /// horizontal ones — so a host passes the same vector it already holds for
    /// the slice on screen, with the clicked cell written into its two
    /// horizontal positions.
    ///
    /// **One region read**, of exactly the points on the line. On a Zarr store
    /// that fetches only the chunks the line crosses. A NetCDF variable is still
    /// decoded whole underneath — `NetcdfArrays::read_region` does not yet read a
    /// sub-region — which is one decode for one click rather than per frame, but
    /// is worth knowing before calling this in a loop.
    ///
    /// # Errors
    ///
    /// [`Error::WrongAddressing`] for a message stream, which has no axes to read
    /// along; [`Error::NoSuchMessage`] for a variable index outside
    /// [`variables`](Self::variables); [`Error::InvalidOption`] for an axis
    /// outside the variable's rank, an `indices` of the wrong length, or an index
    /// past the end of its axis; and a decode failure from the container.
    pub fn decode_line(
        &self,
        variable: u32,
        along_dim: u32,
        indices: &[u32],
        options: &DecodeOptions,
    ) -> Result<Line, Error> {
        #[cfg(not(any(feature = "netcdf", feature = "zarr")))]
        let _ = (variable, along_dim, indices, options);
        match &self.reader {
            #[cfg(feature = "grib1")]
            Reader::Grib1(_) => Err(wrong_addressing(
                Addressing::Messages,
                "decode_line",
                "decode",
            )),
            #[cfg(feature = "grib2")]
            Reader::Grib2(_) => Err(wrong_addressing(
                Addressing::Messages,
                "decode_line",
                "decode",
            )),
            #[cfg(any(feature = "netcdf", feature = "zarr"))]
            Reader::Arrays(a) => {
                let source = a.source.as_ref();
                let vars = renderable_arrays(source.group());
                let var = vars.get(variable as usize).ok_or(Error::NoSuchMessage {
                    index: variable,
                    count: u32::try_from(vars.len()).unwrap_or(u32::MAX),
                })?;
                line_through(source, &var.name, along_dim, indices, options)
            }
        }
    }

    /// Where one slice of a variable sits on the Earth, for a host that paints
    /// it itself.
    ///
    /// **This is for placing a slice you are not decoding.** A host that has
    /// decoded one already has the answer:
    /// [`Field::georef`](crate::api::Field::georef) carries the whole
    /// [`GridGeometry`] along with the scan and the family, so
    /// `Source { geometry: Ok(&field.georef.geometry), ni: field.ni, nj: field.nj,
    /// scan: field.georef.scan, family: &field.georef.label }` projects without
    /// any of this — and `place_slice.rs` checks the two give pixel-identical
    /// rasters, so neither is a second opinion.
    ///
    /// What this buys is *not decoding*. A picker drawing a caption, or deciding
    /// whether a variable can be drawn at all, wants where the cells are and not
    /// the values in them — and decoding a variable is the expensive half. On a
    /// NetCDF file that is a whole-variable read; on a Zarr store it is every
    /// chunk the slice covers.
    ///
    /// `PlacedSlice::source()` then hands the projection pipeline its own input,
    /// so a host never assembles one by hand (#659). Named rather than linked:
    /// that method is behind `render`/`analysis`, and this one is not.
    ///
    /// # Errors
    ///
    /// [`Error::WrongAddressing`] for a message stream, which has variables to
    /// slice; [`Error::NoSuchMessage`] for a variable index outside
    /// [`variables`](Self::variables); [`Error::InvalidOption`] when the two
    /// axes are the same or outside the variable's rank; and a decode failure
    /// when the container cannot say where the cells are.
    pub fn place_slice(&self, variable: u32, y_dim: u32, x_dim: u32) -> Result<PlacedSlice, Error> {
        // With no array container compiled there is no slice to place, and every
        // arm below refuses — so the arguments are genuinely unused in that build.
        // The same shape `decode` uses for the mirror case.
        #[cfg(not(any(feature = "netcdf", feature = "zarr")))]
        let _ = (variable, y_dim, x_dim);
        match &self.reader {
            #[cfg(feature = "grib1")]
            Reader::Grib1(_) => Err(wrong_addressing(
                Addressing::Messages,
                "place_slice",
                "message",
            )),
            #[cfg(feature = "grib2")]
            Reader::Grib2(_) => Err(wrong_addressing(
                Addressing::Messages,
                "place_slice",
                "message",
            )),
            #[cfg(any(feature = "netcdf", feature = "zarr"))]
            Reader::Arrays(a) => {
                let source = a.source.as_ref();
                let vars = renderable_arrays(source.group());
                let var = vars.get(variable as usize).ok_or(Error::NoSuchMessage {
                    index: variable,
                    count: u32::try_from(vars.len()).unwrap_or(u32::MAX),
                })?;
                let (y, x) = (y_dim as usize, x_dim as usize);
                if y == x || y >= var.dims.len() || x >= var.dims.len() {
                    return Err(Error::InvalidOption {
                        detail: format!(
                            "`{}` has {} dimensions; y_dim {y} and x_dim {x} must be \
                             different and within them",
                            var.name,
                            var.dims.len()
                        ),
                    });
                }
                // The same call `decode_slice` makes, so the geometry a host
                // paints with cannot disagree with the one the field reports —
                // and the same memo, so the second ask is free (ADR-0011).
                Ok(PlacedSlice {
                    placement: a.placement(var, y, x)?,
                    ni: u32::try_from(var.dims[x].length).unwrap_or(u32::MAX),
                    nj: u32::try_from(var.dims[y].length).unwrap_or(u32::MAX),
                })
            }
        }
    }

    /// Where a message's values will land, without decoding them.
    ///
    /// The message-side mirror of [`place_slice`](Self::place_slice), and for
    /// the same reason: **what this buys is not decoding.** An overlay, a
    /// caption, or a decision about whether a message can be drawn wants the
    /// resolved geometry, and decoding is the expensive half — for a spectral
    /// message it is an inverse spherical-harmonic transform, which is seconds
    /// rather than milliseconds.
    ///
    /// **Resolved, not declared**, which is the difference from
    /// [`message`](Self::message)'s [`MessageInfo::grid`]. A family that carries
    /// no raster of its own — spectral coefficients, HEALPix pixels — declares a
    /// grid nothing can place a point on, and its values land on the synthesised
    /// global lat/lon raster instead. This reports the raster: the same
    /// [`Georef`] [`decode`](Self::decode) puts on the field, so a host that
    /// paints an overlay before decoding cannot disagree with the field it later
    /// draws. `place_message_agrees_with_decode.rs` holds the two to that over
    /// the whole fixture corpus — geometry, family, dimensions, scan and bounds.
    ///
    /// **A grid can be real where a single 2-D field is not.** A message whose
    /// values are not one scalar per grid point — the GRIB1 true
    /// `matrixOfValues` form, GRIB2 bi-Fourier coefficients — has a genuine
    /// grid, so this succeeds, while `decode` refuses and names the call that
    /// reads them. That is not the two disagreeing: this answers about the
    /// grid, `decode` about a field of scalars. A host deciding whether it can
    /// *draw* a message therefore needs `decode`'s answer and not only this
    /// one. The corpus test pins that set by packing family so a new member has
    /// to be acknowledged.
    ///
    /// # Errors
    ///
    /// [`Error::WrongAddressing`] for a container of arrays, which has slices to
    /// place rather than messages; [`Error::NoSuchMessage`] for an index outside
    /// [`count`](Self::count); and [`Error::Unsupported`] for a GRIB1 message
    /// that carries no grid description, which is the same refusal `decode`
    /// gives it.
    pub fn place_message(&self, index: u32) -> Result<Georef, Error> {
        // Before the range check, for the reason `message` explains.
        #[cfg(any(feature = "netcdf", feature = "zarr"))]
        if matches!(self.reader, Reader::Arrays(_)) {
            return Err(wrong_addressing(
                Addressing::Variables,
                "place_message",
                "place_slice",
            ));
        }
        #[cfg(any(feature = "grib1", feature = "grib2"))]
        {
            let i = self.check_index(index)?;
            // The grid a synthesised family lands on, asked of the format crate
            // as metadata — this reads the GDS, where `decode`'s
            // `synthesize_message_global` runs the transform. The two answer the
            // same question and the corpus test holds them to it.
            let synthesis = match &self.reader {
                #[cfg(feature = "grib1")]
                Reader::Grib1(r) => r.synthesis_grid(i),
                #[cfg(feature = "grib2")]
                Reader::Grib2(r) => r.synthesis_grid(i),
                #[cfg(any(feature = "netcdf", feature = "zarr"))]
                Reader::Arrays(_) => {
                    return Err(wrong_addressing(
                        Addressing::Variables,
                        "place_message",
                        "place_slice",
                    ));
                }
            };
            if let Some(grid) = synthesis {
                // The same three the synthesised arm of `decode` builds: nothing
                // of the source layout survives an inverse transform, so the
                // scan is north-down and the family is the geometry's own.
                let geometry = GridGeometry::LatLon(grid.into());
                let declared = geometry.label().to_string();
                // A synthesised raster is built here rather than stated by the
                // file, so its corners are the geometry's — there is no
                // container value to prefer.
                return Ok(Georef::from_declared(
                    &geometry,
                    Scan::north_down(),
                    &declared,
                ));
            }
            match &self.reader {
                #[cfg(feature = "grib1")]
                Reader::Grib1(r) => {
                    let msg = &r.messages[i];
                    let gds = msg.gds.as_ref().ok_or_else(|| Error::Unsupported {
                        detail: "the message carries no grid description".to_string(),
                    })?;
                    // `raster_bounds`, not `bounds`: this is where the values
                    // land, and a reduced grid's values land on the widened
                    // raster.
                    Ok(Georef::from_declared_corners(
                        &GridGeometry::from(gds),
                        grib1_scan(msg),
                        gds.grid_type_name(),
                        gds.raster_bounds(),
                    )
                    .with_points_per_row(gds.points_per_row()))
                }
                #[cfg(feature = "grib2")]
                Reader::Grib2(r) => {
                    let msg = &r.messages[i];
                    Ok(Georef::from_declared_corners(
                        &GridGeometry::from(&msg.gds),
                        grib2_scan(msg),
                        &msg.gds.template_name(),
                        msg.gds.raster_bounds(),
                    )
                    .with_points_per_row(msg.gds.points_per_row()))
                }
                #[cfg(any(feature = "netcdf", feature = "zarr"))]
                Reader::Arrays(_) => Err(wrong_addressing(
                    Addressing::Variables,
                    "place_message",
                    "place_slice",
                )),
            }
        }
        // No GRIB decoder, so no message to place — the mirror of `message`.
        #[cfg(not(any(feature = "grib1", feature = "grib2")))]
        {
            let _ = index;
            Err(wrong_addressing(
                Addressing::Variables,
                "place_message",
                "place_slice",
            ))
        }
    }

    /// Resample a field onto a geographic box, without painting it.
    ///
    /// This is the render pipeline split at the paint step: a GPU host wants
    /// the resampled *values*, so restyling never re-decodes. The output raster
    /// is [`WarpOptions::width`] × [`WarpOptions::height`] when the caller names
    /// one (#465), and the source `ni × nj` otherwise.
    #[cfg(feature = "render")]
    pub fn warp(&self, field: &Field, options: &WarpOptions) -> Result<Warped, Error> {
        warp_field(field, options)
    }

    /// The colour decision, as data (ADR-0006 decision 3). The CPU painter
    /// reads the same value, so it is the oracle a GPU path is checked against
    /// rather than a second colour implementation.
    #[cfg(feature = "render")]
    pub fn palette(&self, field: &Field, options: &PaletteOptions) -> Result<Palette, Error> {
        build_palette(field, options)
    }

    /// Paint a field to RGBA on the CPU. The fallback path: a GPU host colours
    /// from [`Session::palette`] instead.
    ///
    /// **`flip_y` composes with the message's own scan order, it does not
    /// replace it.** Grid point `(i, j)` paints at pixel `(i, j)`, so a field
    /// stored south-to-north (`jScansPositively`) arrives upside down on a
    /// canvas whose first row is the top; `false` therefore means *north up*,
    /// not *rows as stored*, and `true` asks for the other one. This is the same
    /// question [`Scan::flips_source_rows`] answers for
    /// [`crate::render::probe_pixel`] and [`crate::render::overlay_polylines`],
    /// asked here so all three agree about which row a pixel is (#573). A host
    /// that composed the flag itself before calling this would flip twice;
    /// hand the user's request straight through instead.
    #[cfg(feature = "render")]
    pub fn render(
        &self,
        field: &Field,
        options: &PaletteOptions,
        flip_y: bool,
    ) -> Result<Raster, Error> {
        let palette = build_palette(field, options)?;
        let values = field.values.to_f64();
        let flip = field.georef.scan.flips_source_rows(flip_y);
        let rgba = palette.paint(&values, Some(&field.mask), field.ni, field.nj, flip);
        // `paint` answers an empty buffer for a raster whose byte count this
        // target cannot address — `usize` is 32 bits on wasm32, the host this
        // exists for. Say so, rather than handing back dimensions with no
        // pixels behind them for a host to read off the end of.
        let expected = (field.ni as usize)
            .checked_mul(field.nj as usize)
            .and_then(|px| px.checked_mul(4));
        if Some(rgba.len()) != expected {
            return Err(Error::Unsupported {
                detail: format!(
                    "a {}×{} RGBA raster does not fit this target's address space",
                    field.ni, field.nj
                ),
            });
        }
        Ok(Raster {
            rgba,
            width: field.ni,
            height: field.nj,
        })
    }

    /// Sample one geographic point out of a field.
    pub fn probe(&self, field: &Field, lat: f64, lon: f64) -> Option<Probe> {
        // An empty raster has no cell to report, and `f64::clamp` *panics* when
        // its bounds cross — which `0.0 ..= ni - 1.0` does at `ni == 0`. A
        // malformed file reaching here is exactly the input a fuzzer supplies.
        if field.ni == 0 || field.nj == 0 {
            return None;
        }
        let index = field.georef.geometry.inverse(lat, lon)?;
        let i = index.i.round().clamp(0.0, f64::from(field.ni) - 1.0) as usize;
        let j = index.j.round().clamp(0.0, f64::from(field.nj) - 1.0) as usize;
        let flat = j * field.ni as usize + i;
        let present = field.mask.get(flat).copied().unwrap_or(0) == 1;
        Some(Probe {
            lat,
            lon,
            i: index.i,
            j: index.j,
            value: present.then(|| field.values.get(flat)).flatten(),
        })
    }

    /// Combine two aligned fields element by element — the difference map and
    /// its siblings (#239, #579).
    ///
    /// The result is a [`Field`] like any other, on **A's** placement, so
    /// `warp`, `palette`, `render`, [`probe`](Self::probe) and
    /// [`contours`](Self::contours) all apply to it with no special case (the
    /// first three are named in code spans because they are behind the `render`
    /// feature, and a link to a compiled-out item is a rustdoc error). Its
    /// `parameter` and `units` are A's verbatim: the caption `A − B` is the
    /// host's to compose, and a units algebra here would have to answer what
    /// `A / B` of two different parameters is measured in.
    ///
    /// This is the one operation that takes two fields. It stays on `Session`
    /// rather than moving to `Field` because ADR-0006 decision 2 makes the API
    /// types plain data a binding is generated from — a method on one would be
    /// a smart object every host had to mirror — and because `warp` and the
    /// rest already take a field this session need not have produced. Arity is
    /// the only difference.
    ///
    /// # Errors
    ///
    /// [`Error::Unsupported`] when the two do not align cell for cell, naming
    /// the property that differs. See [`crate::combine::aligned`].
    #[cfg(feature = "analysis")]
    pub fn combine(&self, a: &Field, b: &Field, op: CombineOp) -> Result<Field, Error> {
        crate::combine::combine_api_fields(a, b, op)
    }

    /// Isolines through a field, in fractional grid coordinates.
    ///
    /// `levels` empty asks for a nice set spanning the field's own range.
    #[cfg(feature = "analysis")]
    pub fn contours(&self, field: &Field, levels: &[f64]) -> Result<Vec<Isoline>, Error> {
        let chosen: Vec<f64> = if levels.is_empty() {
            match (field.stats.min, field.stats.max) {
                (Some(min), Some(max)) => nice_levels(min, max, 10),
                _ => Vec::new(),
            }
        } else {
            levels.to_vec()
        };
        if chosen.is_empty() {
            return Ok(Vec::new());
        }
        let cells: Vec<Option<f64>> = optional_values(field);
        let ni = field.ni as usize;
        let nj = field.nj as usize;
        // `periodic_x` is the wrong question here, and asking it was the same
        // host divergence #571 set out to end: the tracer may only unwrap the
        // seam where geographic longitude advances uniformly eastward with `i`,
        // which a rotated grid's rows — small circles — do not. Unwrapping one
        // of those eastward sweeps most of the way round the globe and draws a
        // rim-to-rim streak in place of closing a one-cell gap.
        let raw = if field.georef.geometry.contour_seam_wraps() {
            contour_segments_global(&cells, ni, nj, &chosen)
        } else {
            contour_segments(&cells, ni, nj, &chosen)
        };
        Ok(raw
            .into_iter()
            .map(|level| Isoline {
                value: level.level,
                segments: level
                    .segments
                    .into_iter()
                    .map(|[a, b]| [a.0, a.1, b.0, b.1])
                    .collect(),
            })
            .collect())
    }
}

// ---------------------------------------------------------------------------
// Operations, as free functions so a caller with only a `Field` can run them
// ---------------------------------------------------------------------------

/// The field as `core`'s own `Option`-per-cell shape, which the contour and
/// warp kernels consume. One allocation, at the boundary, rather than a branch
/// per element inside them.
#[cfg(feature = "analysis")]
fn optional_values(field: &Field) -> Vec<Option<f64>> {
    (0..field.mask.len())
        .map(|k| (field.mask[k] == 1).then(|| field.values.get(k)).flatten())
        .collect()
}

#[cfg(feature = "render")]
fn warp_field(field: &Field, options: &WarpOptions) -> Result<Warped, Error> {
    let geometry = &field.georef.geometry;
    // Refused before the window is resolved, so a bad size is reported as a bad
    // size rather than being masked by a grid that also states no extent.
    let size = crate::render::resolve_output_size(options.width, options.height)?;
    let window = match options.bounds {
        // The host hands the window over positionally, which is the one place
        // the order is not the type's statement; read it back through the
        // type so it is stated once rather than at the destructure below.
        Some(b) => LonLatBox::from_array(b),
        // `lonlat_bbox` reports where the data is, which is the right answer
        // to a different question; `render_window` is the window question, and
        // states once — in `core`, for both hosts — that a periodic grid's
        // window runs the full turn rather than stopping at its last declared
        // column (#571).
        None => geometry.render_window().ok_or_else(|| Error::Unsupported {
            detail: format!("a {} grid states no extent to warp onto", geometry.label()),
        })?,
    };
    let LonLatBox {
        lat_min,
        lat_max,
        lon_min,
        lon_max,
    } = window;
    if !(lat_min.is_finite() && lat_max.is_finite() && lon_min.is_finite() && lon_max.is_finite())
        || lat_max <= lat_min
        || lon_max <= lon_min
    {
        return Err(Error::InvalidOption {
            detail: format!("the warp window {window:?} encloses no area"),
        });
    }

    // Sampled in place rather than through `optional_values`: that shape is
    // 16 bytes a cell, so a 3.7-million-point NBM field would cost 60 MB of
    // linear memory on top of the field it already holds, for no gain here.
    let ni = field.ni as usize;
    let sample = |i: usize, j: usize| -> Option<f64> {
        let k = j * ni + i;
        if field.mask.get(k).copied().unwrap_or(0) != 1 {
            return None;
        }
        field.values.get(k)
    };
    let inverse = geometry.inverse_at();
    let source = SourceGrid {
        ni: field.ni,
        nj: field.nj,
        sample: &sample,
        inverse_at: &inverse,
        periodic_i: field.georef.periodic_x,
        resampling: geometry.resampling(),
    };
    // The caller's raster when they named one (#465); otherwise the source
    // grid's own shape, which is what this warp has always produced. There is
    // no display floor here the way there is for the render targets: this call
    // returns values, not pixels, and upsampling values a caller did not ask
    // for would cost linear memory the browser host never gets back.
    let (width, height) = size.unwrap_or((field.ni, field.nj));
    let target = TargetRaster {
        width,
        height,
        lat_max,
        lat_min,
        lon_min,
        lon_max,
        lon_periodic: field.georef.periodic_x,
    };
    let method = if options.bilinear {
        Resampling::Bilinear
    } else {
        Resampling::Nearest
    };
    let out = warp(&source, &target, method);
    Ok(Warped {
        // f32 by design: a warped raster is a display product, and the host
        // uploads it as a texture. The unwarped `Field` keeps the source width.
        values: out.values.into_iter().map(|v| v as f32).collect(),
        mask: out.mask,
        width: out.width,
        height: out.height,
        bounds: window.to_array(),
    })
}

#[cfg(feature = "render")]
fn build_palette(field: &Field, options: &PaletteOptions) -> Result<Palette, Error> {
    let colormap = match (
        options.colormap_table.as_deref(),
        options.colormap.as_deref(),
    ) {
        (Some(table), name) => Cow::Owned(crate::render::colormap_from_table(table, name)?),
        (None, Some(name)) => {
            Cow::Borrowed(Colormap::by_name(name).ok_or_else(|| Error::InvalidOption {
                detail: format!("no colormap named {name:?}"),
            })?)
        }
        (None, None) => Cow::Borrowed(default_colormap()),
    };
    let scale = match options.scale.as_deref() {
        None | Some("linear") => ScaleMode::Linear,
        Some("log10") => ScaleMode::Log10,
        Some(other) => {
            return Err(Error::InvalidOption {
                detail: format!("no scale named {other:?}; expected \"linear\" or \"log10\""),
            });
        }
    };
    let min = options
        .min
        .or(field.stats.min)
        .ok_or_else(|| Error::InvalidOption {
            detail: "the field has no present values, so it has no range to colour".to_string(),
        })?;
    let max = options.max.or(field.stats.max).unwrap_or(min);
    // A logarithm needs a positive domain. `transformed_domain` would answer
    // `-inf` (for 0) or `NaN`, and every cell would then paint the low end of
    // the ramp — a picture that looks like data and is not. `core` documents
    // that its callers reject this; this is that rejection.
    // Written as "not (finite and positive)" rather than `min <= 0.0`: a `NaN`
    // minimum fails every comparison, so the simpler form would let it through
    // and produce exactly the domain this guard exists to refuse.
    if matches!(scale, ScaleMode::Log10) && !(min.is_finite() && min > 0.0) {
        return Err(Error::InvalidOption {
            detail: format!(
                "a log10 scale needs a finite, positive minimum; the range starts at {min}"
            ),
        });
    }
    Ok(Palette::build(&colormap, options.reversed, min, max, scale))
}

// ---------------------------------------------------------------------------
// Per-format metadata
// ---------------------------------------------------------------------------

#[cfg(feature = "grib1")]
fn grib1_scan(msg: &fieldglass_grib1::Grib1Message) -> Scan {
    match &msg.gds {
        Some(gds) => scan_of_grib1(gds).unwrap_or_else(Scan::north_down),
        None => Scan::north_down(),
    }
}

#[cfg(feature = "grib1")]
fn scan_of_grib1(gds: &fieldglass_grib1::GridDescription) -> Option<Scan> {
    gds.scanning_mode()
        .map(|m| Scan::new(m.i_negative, m.j_positive, m.j_consecutive))
}

#[cfg(feature = "grib2")]
fn grib2_scan(msg: &fieldglass_grib2::Grib2Message) -> Scan {
    match msg.gds.scanning_mode() {
        Some(sm) => Scan::new(sm & 0x80 != 0, sm & 0x40 != 0, sm & 0x20 != 0),
        None => Scan::north_down(),
    }
}

/// `(abbreviation, name, units)` for one GRIB1 message.
///
/// Split out of [`grib1_message`] so [`Session::decode`] does not build a whole
/// `MessageInfo` for the two strings it needs: that would build the `Georef`
/// too, and a projected family's `lonlat_bbox` walks its perimeter 512 times
/// per edge.
#[cfg(feature = "grib1")]
fn grib1_parameter(msg: &fieldglass_grib1::Grib1Message) -> (String, String, String) {
    match fieldglass_grib1::tables::lookup_parameter(
        msg.pds.parameter_id,
        msg.pds.table_version,
        msg.pds.originating_centre,
    ) {
        // Units are normalised at the display seam, the same way the napi host
        // does it: the ECMWF local tables are generated from eccodes'
        // Fortran-style exponents and ON388 chains solidi, so the raw strings
        // disagree about the same unit.
        Some(param) => (
            param.abbreviation.to_string(),
            param.name.to_string(),
            normalize_units(param.units).into_owned(),
        ),
        // The format crate owns the fallback rendering, so this seam and the
        // napi one cannot disagree about it (#633).
        None => (
            String::new(),
            fieldglass_grib1::tables::unresolved_parameter(
                msg.pds.originating_centre,
                msg.pds.table_version,
                msg.pds.parameter_id,
            ),
            String::new(),
        ),
    }
}

#[cfg(feature = "grib1")]
fn grib1_message(reader: &fieldglass_grib1::Grib1Reader<Bytes>, index: usize) -> MessageInfo {
    let msg = &reader.messages[index];
    let grid = msg.gds.as_ref().map(|gds| {
        // `bounds`, not `raster_bounds`: this is what the message *declares*,
        // and a reduced grid declares the corner of the grid it really is.
        Georef::from_declared_corners(
            &GridGeometry::from(gds),
            grib1_scan(msg),
            gds.grid_type_name(),
            gds.bounds(),
        )
        .with_points_per_row(gds.points_per_row())
    });
    let (abbreviation, parameter, units) = grib1_parameter(msg);
    MessageInfo {
        // Round-trips the `u32` handle `Session::message` was given and
        // `check_index` widened, so it cannot be a narrowing in practice.
        index: index as u32,
        offset_bytes: msg.byte_offset,
        parameter,
        abbreviation,
        units,
        level: fieldglass_grib1::level_value_str(&msg.pds),
        level_type: fieldglass_grib1::level_type_str(&msg.pds),
        reference_time: Some(fieldglass_grib1::reference_time(&msg.pds)),
        forecast: fieldglass_grib1::forecast_display(&msg.pds),
        packing: reader.packing_label(index).unwrap_or("unknown").to_string(),
        size_label: msg.gds.as_ref().and_then(|g| g.size_label()),
        grid,
        forecast_hours: fieldglass_grib1::forecast_hours(&msg.pds),
        // Time range 10 spends `P1` as the high octet of a two-octet value, so
        // reporting it as a lead time there would be reporting half a number.
        p1_octet: (msg.pds.time_range != 10).then_some(i32::from(msg.pds.p1)),
        originating_centre: fieldglass_grib1::tables_cct::lookup_centre(msg.pds.originating_centre)
            .map(str::to_string)
            .unwrap_or_else(|| format!("Centre {}", msg.pds.originating_centre)),
        sub_centre: fieldglass_core::cct_tables::lookup_sub_centre(
            msg.pds.originating_centre.into(),
            msg.pds.sub_centre.into(),
        )
        .map(str::to_string),
        edition: Some(1),
        // GRIB1 has no discipline, production status or data type: these are
        // §1 fields that arrived with edition 2. `None` is the answer, not a
        // gap — see the field docs on `MessageInfo`.
        discipline: None,
        total_length_bytes: Some(u64::from(msg.is.total_length)),
        production_status: None,
        data_type: None,
    }
}

/// `(abbreviation, name, units)` for one GRIB2 message. Split out for the
/// reason [`grib1_parameter`] is.
#[cfg(feature = "grib2")]
fn grib2_parameter(msg: &fieldglass_grib2::Grib2Message) -> (String, String, String) {
    let discipline = msg.is.discipline;
    // A template with no horizontal product common carries no category and no
    // number, so there is nothing to name and nothing to report as unresolved
    // either — every field stays empty. Only a message that *has* the codes and
    // finds no table for them gets the fallback (#633).
    let Some(common) = msg.pds.common() else {
        return (String::new(), String::new(), String::new());
    };
    match fieldglass_grib2::lookup_parameter(
        msg.ids.originator(),
        discipline,
        common.parameter_category,
        common.parameter_number,
    ) {
        Some((abbr, long, units)) => (
            abbr.to_string(),
            long.to_string(),
            normalize_units(units).into_owned(),
        ),
        // The format crate owns the fallback rendering, so this seam and the
        // napi one cannot disagree about it (#633).
        None => (
            String::new(),
            fieldglass_grib2::unresolved_parameter(
                discipline,
                common.parameter_category,
                common.parameter_number,
            ),
            String::new(),
        ),
    }
}

#[cfg(feature = "grib2")]
fn grib2_message(reader: &fieldglass_grib2::Grib2Reader<Bytes>, index: usize) -> MessageInfo {
    let msg = &reader.messages[index];
    let common = msg.pds.common();
    let (abbreviation, parameter, units) = grib2_parameter(msg);
    // The format crate owns every display rule (#545); a template with no
    // horizontal product common has none of the three fields to render.
    let (level, level_type) = match common {
        Some(c) => (
            fieldglass_grib2::level_value_str(c),
            fieldglass_grib2::level_type_str(c),
        ),
        None => ("—".to_string(), "—".to_string()),
    };
    MessageInfo {
        // Round-trips the `u32` handle `Session::message` was given and
        // `check_index` widened, so it cannot be a narrowing in practice.
        index: index as u32,
        offset_bytes: msg.byte_offset,
        parameter,
        abbreviation,
        units,
        level,
        level_type,
        reference_time: Some(msg.ids.reference_time_iso8601()),
        forecast: common
            .map(fieldglass_grib2::forecast_display)
            .unwrap_or_else(|| "—".to_string()),
        packing: msg.drs.template_name(),
        grid: Some(
            Georef::from_declared_corners(
                &GridGeometry::from(&msg.gds),
                grib2_scan(msg),
                &msg.gds.template_name(),
                msg.gds.bounds(),
            )
            .with_points_per_row(msg.gds.points_per_row()),
        ),
        size_label: msg.gds.size_label(),
        forecast_hours: common.and_then(fieldglass_grib2::forecast_hours),
        // A GRIB1 octet, and edition 2 does not have it.
        p1_octet: None,
        originating_centre: fieldglass_grib2::tables_cct::lookup_centre(msg.ids.centre)
            .map(str::to_string)
            .unwrap_or_else(|| format!("Centre {}", msg.ids.centre)),
        sub_centre: fieldglass_core::cct_tables::lookup_sub_centre(
            msg.ids.centre,
            msg.ids.sub_centre,
        )
        .map(str::to_string),
        edition: Some(i32::from(msg.is.edition)),
        discipline: Some(fieldglass_grib2::lookup_discipline(msg.is.discipline).to_string()),
        total_length_bytes: Some(msg.is.total_length),
        production_status: Some(
            fieldglass_grib2::lookup_production_status(msg.ids.production_status).to_string(),
        ),
        data_type: Some(fieldglass_grib2::lookup_data_type(msg.ids.data_type).to_string()),
    }
}

#[cfg(all(test, feature = "grib2", feature = "render", feature = "analysis"))]
mod tests {
    use super::*;
    use fieldglass_core::{LatLonParams, RotatedLatLonParams, projection::GridGeometry};

    /// A message can declare a zero-width grid, and `decode` accepts it: zero
    /// values match a zero-cell raster. `probe` must answer `None` for such a
    /// field.
    ///
    /// The guard it exercises is belt-and-braces, and honestly so: every
    /// geometry's own inverse already refuses a grid with no extent, so the
    /// clamp below it is not reached today. It is there because the clamp's
    /// bounds are `0.0 ..= ni - 1.0`, which *cross* at `ni == 0`, and
    /// `f64::clamp` panics rather than saturating when its bounds cross —
    /// see the assertion at the end. A future geometry whose inverse is more
    /// permissive would turn that into an aborted Worker.
    #[test]
    fn probing_an_empty_raster_answers_rather_than_panicking() {
        let geometry = GridGeometry::LatLon(LatLonParams {
            ni: 0,
            nj: 0,
            lat_first: 90.0,
            lon_first: 0.0,
            lat_last: -90.0,
            lon_last: 359.0,
        });
        let field = Field {
            values: Values::F64(Vec::new()),
            mask: Vec::new(),
            ni: 0,
            nj: 0,
            georef: Georef::from_geometry(&geometry, Scan::north_down()),
            stats: Stats {
                min: None,
                max: None,
                valid_count: 0,
            },
            parameter: String::new(),
            units: String::new(),
        };
        // The session is irrelevant to `probe`; it reads only the field.
        assert!(grib2_session().probe(&field, 0.0, 0.0).is_none());

        // The hazard the guard exists for, stated rather than assumed.
        assert!(
            std::panic::catch_unwind(|| 0.0_f64.clamp(0.0, -1.0)).is_err(),
            "f64::clamp is expected to panic on crossed bounds; if it ever \
             saturates instead, the guard above is redundant"
        );
    }

    /// A session over an arbitrary fixture, for the operations that read only
    /// the `Field` handed to them and never the reader behind it.
    fn grib2_session() -> Session {
        Session {
            reader: Reader::Grib2(Box::new(
                fieldglass_grib2::Grib2Reader::from_source(Box::new(
                    std::fs::read("../fieldglass-grib2/tests/fixtures/gfs_c255_latlon.grib2")
                        .expect("fixture"),
                ) as Bytes)
                .expect("parse"),
            )),
        }
    }

    /// A field on a grid periodic in its *rotated* frame, for the two questions
    /// this crate used to answer with `periodic_x` and now asks the geometry.
    fn rotated_periodic_field(values: Vec<f64>) -> Field {
        let geometry = GridGeometry::RotatedLatLon(RotatedLatLonParams {
            ni: 16,
            nj: 8,
            lat_first: 40.0,
            lon_first: 0.0,
            lat_last: -40.0,
            // A full turn once the 22.5° step is counted.
            lon_last: 337.5,
            south_pole_lat: -30.0,
            south_pole_lon: 10.0,
            angle_of_rotation: 0.0,
        });
        let mask = vec![1u8; values.len()];
        Field {
            values: Values::F64(values),
            mask,
            ni: 16,
            nj: 8,
            georef: Georef::from_geometry(&geometry, Scan::north_down()),
            stats: Stats {
                min: Some(0.0),
                max: Some(15.0),
                valid_count: 128,
            },
            parameter: String::new(),
            units: String::new(),
        }
    }

    /// The two answers this crate composed for itself, now the geometry's — and
    /// rotated lat/lon is the family where the two questions come apart.
    ///
    /// A rotated grid's columns close on themselves, so `periodic_x` is `true`
    /// and the warp may wrap a column index. Neither of the questions below
    /// follows from that, because both are about *geographic* longitude, which
    /// along a rotated row is neither uniform nor monotonic:
    ///
    /// * the default warp window used to be widened a full turn on the strength
    ///   of `periodic_x`, which frames a rotated polar cap 23° wide as the whole
    ///   globe;
    /// * the contour tracer used to be told to unwrap the seam on the same
    ///   strength, which sweeps most of the way round the globe and draws a
    ///   rim-to-rim streak in place of closing a one-cell gap.
    ///
    /// `fieldglass-napi` never did either, which is the divergence #571 closed.
    #[test]
    fn a_periodic_rotated_grid_neither_widens_its_window_nor_unwraps_its_seam() {
        let field = rotated_periodic_field((0..128).map(|k| f64::from(k % 16)).collect());
        assert!(
            field.georef.periodic_x,
            "the columns really do close on themselves"
        );

        let warped = warp_field(
            &field,
            &WarpOptions {
                bounds: None,
                bilinear: false,
                width: None,
                height: None,
            },
        )
        .expect("a periodic rotated grid warps");
        let extent = field
            .georef
            .geometry
            .lonlat_bbox()
            .expect("the walk places it");
        let window = LonLatBox::from_array(warped.bounds);
        assert!(
            (window.lon_max - window.lon_min - (extent.lon_max - extent.lon_min)).abs() < 1e-9,
            "the window must be the walked extent, not a full turn: got {}°, \
             extent {}°",
            window.lon_max - window.lon_min,
            extent.lon_max - extent.lon_min
        );

        // The tracer takes the bounded march, asked through `Session::contours`
        // itself rather than through a copy of its rule. A ramp across the
        // columns crosses every level once per row, and the global march adds
        // the seam cell between column 15 and column 0 — where the ramp falls 15
        // back to 0 and so crosses every level a second time — so the two are
        // distinguishable by segment count alone.
        let session = grib2_session();
        let levels = [4.5, 9.5];
        let bounded: usize = session
            .contours(&field, &levels)
            .expect("contours")
            .iter()
            .map(|l| l.segments.len())
            .sum();
        let cells: Vec<Option<f64>> = optional_values(&field);
        let unwrapped: usize = fieldglass_core::contour_segments_global(&cells, 16, 8, &levels)
            .iter()
            .map(|l| l.segments.len())
            .sum();
        assert!(
            bounded < unwrapped,
            "the bounded march must draw fewer segments than the unwrapped one, \
             or this test cannot tell them apart: {bounded} vs {unwrapped}"
        );
    }

    /// The swath fixture: four variables, all placed by one 2-D coordinate
    /// pair. Reached from a unit test rather than `tests/` because what is
    /// being asserted is the memo's *keying*, which is private — an
    /// integration test can only see that the answers agree, not that they
    /// came from one entry.
    #[cfg(feature = "netcdf")]
    const SWATH: &[u8] = include_bytes!("../../fieldglass-netcdf/tests/fixtures/mirs_swath_n21.nc");

    /// A real tripolar ocean mesh, whose fields carry a third axis — so a
    /// cross-section through one is a slice its 2-D coordinates do not span.
    #[cfg(feature = "netcdf")]
    const TRIPOLAR: &[u8] =
        include_bytes!("../../fieldglass-netcdf/tests/fixtures/rtofs_tripolar_arctic.nc");

    /// The memo a session is holding, panicking if the session is not one that
    /// holds arrays.
    #[cfg(feature = "netcdf")]
    fn memo(session: &Session) -> Vec<PlacementKey> {
        let Reader::Arrays(a) = &session.reader else {
            panic!("this fixture is a container of arrays");
        };
        let mut keys: Vec<_> = a
            .placements
            .lock()
            .expect("not poisoned")
            .keys()
            .cloned()
            .collect();
        keys.sort_by_key(|k| format!("{k:?}"));
        keys
    }

    /// Every field on one mesh shares one cached index (ADR-0011).
    ///
    /// This is the property the whole memo turns on, and the reason the key is
    /// the coordinate pair rather than the field: the four swath variables are
    /// on the same lat/lon pair, so placing all four must leave **one** entry.
    /// Keyed per field it would leave four, and a global ocean grid would pay
    /// its `O(n log n)` build and its whole footprint once per variable.
    #[cfg(feature = "netcdf")]
    #[test]
    fn one_coordinate_pair_serves_every_field_on_it() {
        let session = Session::open(SWATH.to_vec()).expect("the swath opens");
        let vars = session.variables();
        assert_eq!(vars.len(), 4, "the fixture's four swath variables");

        for (i, v) in vars.iter().enumerate() {
            let (Some(y), Some(x)) = (v.detected_y_dim, v.detected_x_dim) else {
                panic!("{}: a swath variable has detected axes", v.name);
            };
            let placed = session
                .place_slice(i as u32, y, x)
                .unwrap_or_else(|e| panic!("{}: {e}", v.name));
            assert_eq!(placed.family(), "lookup", "{} is curvilinear", v.name);
        }

        let keys = memo(&session);
        assert_eq!(keys.len(), 1, "four fields, one mesh, one entry: {keys:?}");
        let PlacementKey::Coordinates { lat, lon } = &keys[0] else {
            panic!("a curvilinear slice is keyed on its coordinate pair: {keys:?}");
        };
        assert!(
            lat.contains("Latitude") && lon.contains("Longitude"),
            "keyed on the pair that built it, qualified: {lat:?} {lon:?}"
        );
    }

    /// The guard on sharing: a cross-section is not placed by the pair, so it
    /// must not be served the pair's answer.
    ///
    /// The failure this rules out is the one coordinate-keying invites. Asking
    /// for a *different* axis pair on a curvilinear array — here the swath's
    /// scan-line axis against its channel axis — is a slice the 2-D
    /// coordinates do not span. If the key ignored the axes, the second call
    /// would hit the first call's entry and report a lookup geometry for a
    /// slice that has none, placing the raster on the wrong cells entirely.
    #[cfg(feature = "netcdf")]
    #[test]
    fn a_slice_the_pair_does_not_span_is_keyed_and_placed_on_its_own() {
        let session = Session::open(TRIPOLAR.to_vec()).expect("the mesh opens");
        let vars = session.variables();
        let (index, var) = vars
            .iter()
            .enumerate()
            .find(|(_, v)| v.dims.len() > 2 && v.detected_y_dim.is_some())
            .map(|(i, v)| (i as u32, v))
            .expect("a variable with a third axis to cut against");
        let (y, x) = (
            var.detected_y_dim.expect("a detected Y"),
            var.detected_x_dim.expect("a detected X"),
        );
        let other = (0..var.dims.len() as u32)
            .find(|d| *d != y && *d != x)
            .expect("a third axis");

        let image = session.place_slice(index, y, x).expect("the image slice");
        assert_eq!(image.family(), "lookup");

        // The same array, cut the other way: the third axis against X.
        let cross = session
            .place_slice(index, other, x)
            .expect("the cross-section places");
        assert_ne!(
            cross.family(),
            "lookup",
            "the coordinate pair does not span these axes, so it cannot place them"
        );

        let keys = memo(&session);
        assert_eq!(
            keys.len(),
            2,
            "two distinct questions, two entries: {keys:?}"
        );
        assert!(
            keys.iter()
                .any(|k| matches!(k, PlacementKey::Coordinates { .. })),
            "the image slice keyed on its pair: {keys:?}"
        );
        assert!(
            keys.iter().any(|k| matches!(
                k,
                PlacementKey::Axes { array, y: ky, x: kx }
                    if array == &var.name
                        && *ky == other as usize
                        && *kx == x as usize
            )),
            "the cross-section keyed on its own array and axes: {keys:?}"
        );
    }

    /// A second ask returns the same placement, not merely an equal one.
    ///
    /// `Arc::ptr_eq` on what the two calls borrow is the only assertion that
    /// distinguishes "memoised" from "rebuilt and happened to agree", and it
    /// does it without timing anything.
    #[cfg(feature = "netcdf")]
    #[test]
    fn the_second_ask_borrows_the_first_answer() {
        let session = Session::open(SWATH.to_vec()).expect("the swath opens");
        let v = &session.variables()[0];
        let (y, x) = (
            v.detected_y_dim.expect("a detected Y"),
            v.detected_x_dim.expect("a detected X"),
        );
        let first = session.place_slice(0, y, x).expect("places");
        let second = session.place_slice(0, y, x).expect("places again");
        assert!(
            Arc::ptr_eq(&first.placement, &second.placement),
            "the second call served the cached placement"
        );
        assert_eq!(memo(&session).len(), 1);
    }
}

/// The refusal a build gets for a container it can recognise but not decode.
///
/// One module per format feature, each compiled only when that feature is
/// *off*, so between them they cover every partial build the
/// `cargo-clippy-umbrella-features` hook constructs — and they run rather than
/// only compile, because that hook's sibling runs `cargo test --lib` over the
/// same sets. Under default features neither module exists, which is why the
/// assertion lives here and not in `tests/`: no integration test can observe a
/// dispatch arm the build it runs in compiled out (#552).
#[cfg(all(test, not(feature = "grib1")))]
mod grib1_compiled_out {
    use super::*;

    #[test]
    fn a_grib1_message_is_refused_as_grib1_and_not_as_unknown_bytes() {
        // A minimal GRIB1 message: "GRIB", a 3-byte total length, edition 1.
        // Detection reads the magic and the edition byte and nothing else, so
        // this is enough to reach the dispatch arm under test.
        let mut bytes = b"GRIB".to_vec();
        bytes.extend_from_slice(&[0, 0, 8, 1]);
        match Session::open(bytes) {
            Err(Error::UnsupportedFormat { detail }) => {
                assert!(
                    detail.contains("GRIB1") && detail.contains("grib1"),
                    "the refusal must name the container and the feature that \
                     would decode it, not just decline: {detail}"
                );
            }
            other => panic!("expected an UnsupportedFormat naming GRIB1, got {other:?}"),
        }
    }
}

/// The `grib2` half of [`grib1_compiled_out`].
#[cfg(all(test, not(feature = "grib2")))]
mod grib2_compiled_out {
    use super::*;

    #[test]
    fn a_grib2_message_is_refused_as_grib2_and_not_as_unknown_bytes() {
        // GRIB2 Section 0: "GRIB", two reserved bytes, discipline at offset
        // 6, edition at offset 7 — which is the byte `detect_from_bytes`
        // reads to tell the editions apart.
        let mut bytes = b"GRIB".to_vec();
        bytes.extend_from_slice(&[0, 0, 0, 2]);
        match Session::open(bytes) {
            Err(Error::UnsupportedFormat { detail }) => {
                assert!(
                    detail.contains("GRIB2") && detail.contains("grib2"),
                    "the refusal must name the container and the feature that \
                     would decode it, not just decline: {detail}"
                );
            }
            other => panic!("expected an UnsupportedFormat naming GRIB2, got {other:?}"),
        }
    }
}
