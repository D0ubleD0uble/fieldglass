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
use fieldglass_core::bytes::ByteSource;
#[cfg(feature = "zarr")]
use fieldglass_core::bytes::ObjectSource;
#[cfg(any(feature = "grib1", feature = "grib2", feature = "netcdf"))]
use fieldglass_core::bytes::read_head;
#[cfg(any(feature = "grib1", feature = "grib2"))]
use fieldglass_core::units::normalize_table_units;
use std::sync::Arc;
#[cfg(any(feature = "netcdf", feature = "zarr"))]
use std::{collections::HashMap, sync::Mutex};

use fieldglass_core::cf::SlicePlacement;
use fieldglass_core::{Format as CoreFormat, GridGeometry, detect_from_bytes};
#[cfg(feature = "render")]
use fieldglass_core::{
    LonLatBox, Resampling, SourceGrid, TargetRaster,
    colormap::{Palette, ScaleMode},
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
use crate::api::{ColorRange, Warped};
// Only the message streams name a placement themselves; a slice's comes from
// `Georef::from_slice`.
use crate::api::{
    Addressing, DimensionInfo, DisplayField, Dtype, Field, FullDetail, Georef, LeftOutArray, Line,
    MessageInfo, MessageProbe, Probe, Scan, SourceFormat, Stats, Values, VariableInfo,
    VectorPairing,
};
#[cfg(any(feature = "grib1", feature = "grib2"))]
use crate::api::{GridSupport, Placement};
// The band-limit label only a spectral message carries (#637).
#[cfg(any(feature = "grib1", feature = "grib2"))]
use crate::api::SpectralTruncation;
// The edition-tagged identification only a GRIB message carries (#773).
#[cfg(any(feature = "grib1", feature = "grib2"))]
use crate::api::Identification;
// What makes a GRIB message the same message in another copy of its file (#959).
#[cfg(any(feature = "grib1", feature = "grib2"))]
use crate::api::MessageFingerprint;
// The axis a cross-section labels itself from: only an array container has one
// (#171), so the import is gated the way its readers are.
#[cfg(any(feature = "netcdf", feature = "zarr"))]
use crate::api::AxisValues;
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

/// How a decode to **draw** should be shaped: [`Session::decode_display`]'s
/// options (#1082).
///
/// A type of its own rather than a field on [`DecodeOptions`], because the two
/// decodes return different types: `decode` a [`Field`], whose values are the
/// file's, and `decode_display` a [`DisplayField`], whose values may not be.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schema", schemars(rename_all = "camelCase"))]
#[non_exhaustive]
pub struct DisplayDecodeOptions {
    /// How many of the finest JPEG 2000 wavelet levels to discard: `0` (the
    /// default) is the message's own field, `1` half the size on each axis, `2`
    /// a quarter. Only a GRIB2 message packed as JPEG 2000 (§5.40) honors a
    /// non-zero value, and only on a regular grid with no bitmap; every other
    /// message refuses one as [`Error::Unsupported`] rather than decoding in
    /// full and pretending.
    #[serde(default)]
    pub reduction: u8,
    /// Element type to decode into, as [`DecodeOptions::dtype`].
    #[serde(default)]
    pub dtype: Dtype,
}

impl DisplayDecodeOptions {
    /// The reduction, with the source's own element width. A constructor for
    /// the reason [`DecodeOptions::new`] gives.
    #[must_use]
    pub fn new(reduction: u8) -> Self {
        Self {
            reduction,
            dtype: Dtype::Auto,
        }
    }
}

/// Which slice of a variable a host asks for: the variable, its two image axes,
/// and a position on every axis (#1073).
///
/// What a JavaScript host's `decodeSlice` takes as one object, where
/// [`Session::decode_slice`] takes the same four as arguments. The object is
/// the JS shape because four positional numbers are four chances to pass them
/// in the wrong order, and the hosts take it identically so that code written
/// against one reads against the other.
///
/// `variable` is a position in `variables()` on every host: the list
/// [`Session::variables`] returns. `y_dim` and `x_dim` are
/// positions in that variable's axes, and `slice_indices` holds **one entry per
/// axis** in the same order, the two image axes ignored rather than absent.
///
/// **On the wire** it is a camelCase object. Every key is required and an
/// unknown one is refused: a misspelt `sliceIndex` dropped silently would
/// decode the first time step and label it the one asked for.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename_all = "camelCase"))]
#[non_exhaustive]
pub struct SliceRequest {
    /// Which variable, as the host's `variables()` numbers it.
    pub variable: u32,
    /// Which of its axes runs down the rows.
    pub y_dim: u32,
    /// Which of its axes runs across the columns.
    pub x_dim: u32,
    /// A position on every axis of the variable, in its declared order, the two
    /// image axes ignored. A JavaScript host also takes a `Uint32Array`.
    #[cfg_attr(feature = "schema", schemars(extend("x-fieldglass-typed-array" = "Uint32Array")))]
    pub slice_indices: Vec<u32>,
}

impl SliceRequest {
    /// A request, from its parts. A constructor for the reason
    /// [`DecodeOptions::new`] gives.
    #[must_use]
    pub fn new(variable: u32, y_dim: u32, x_dim: u32, slice_indices: Vec<u32>) -> Self {
        Self {
            variable,
            y_dim,
            x_dim,
            slice_indices,
        }
    }
}

/// Which line through a variable a host asks for: the variable, the axis to
/// read along, and a position on every axis (#1073).
///
/// What a JavaScript host's `decodeLine` takes as one object, where
/// [`Session::decode_line`] takes the same three as arguments, for the reason
/// [`SliceRequest`] gives. `slice_indices` holds one entry per axis, the one
/// read along included and ignored. Every key is required and an unknown one
/// is refused.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename_all = "camelCase"))]
#[non_exhaustive]
pub struct LineRequest {
    /// Which variable, as the host's `variables()` numbers it.
    pub variable: u32,
    /// The axis to read along, as a position in the variable's axes.
    pub along_dim: u32,
    /// A position on every axis of the variable, in its declared order, the
    /// one read along ignored. A JavaScript host also takes a `Uint32Array`.
    #[cfg_attr(feature = "schema", schemars(extend("x-fieldglass-typed-array" = "Uint32Array")))]
    pub slice_indices: Vec<u32>,
}

impl LineRequest {
    /// A request, from its parts. A constructor for the reason
    /// [`DecodeOptions::new`] gives.
    #[must_use]
    pub fn new(variable: u32, along_dim: u32, slice_indices: Vec<u32>) -> Self {
        Self {
            variable,
            along_dim,
            slice_indices,
        }
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
    /// centers downgrades to nearest whatever this says.
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

/// Color, decided once in Rust and exported as data.
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
    /// Low end of the display range. `None` takes the field's own minimum, by
    /// the rule [`crate::color_range()`] states.
    #[serde(default)]
    pub min: Option<f64>,
    /// High end of the display range. `None` takes the field's own maximum.
    #[serde(default)]
    pub max: Option<f64>,
    /// A value to center the ramp on, for a diverging map: the range is made
    /// symmetric about it. Validated like the two ends.
    #[serde(default)]
    pub center: Option<f64>,
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
            center: None,
            scale: scale.map(str::to_string),
        }
    }
}

/// The color range contours are spaced over when no levels are named: the
/// range fields of `PaletteOptions`, so a host sends contours the range it
/// renders with and the two agree (#1155). Every field `None` is the field's
/// own range on a linear scale.
#[cfg(feature = "analysis")]
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schema", schemars(rename_all = "camelCase"))]
#[non_exhaustive]
pub struct ContourOptions {
    /// Low end of the range. `None` takes the field's own minimum.
    #[serde(default)]
    pub min: Option<f64>,
    /// High end of the range. `None` takes the field's own maximum.
    #[serde(default)]
    pub max: Option<f64>,
    /// A value to center the range on, as `PaletteOptions::center`.
    #[serde(default)]
    pub center: Option<f64>,
    /// `"linear"` (default) or `"log10"`. The levels stay evenly spaced; the
    /// scale decides which ranges are refused, as it does for a render.
    #[serde(default)]
    pub scale: Option<String>,
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
    /// The range the cells were colored over, and how many fell outside it,
    /// counted as they were painted (#1155).
    pub range: crate::api::ColorRange,
}

/// An open file.
#[derive(Debug)]
pub struct Session {
    reader: Reader,
}

/// One variant per format feature (#552). `lib.rs` refuses a build with none of
/// them, so this enum always has at least one variant and every `match` on it
/// below stays exhaustive with its arms gated the same way.
/// The bytes a ranged reader reads, type-erased.
///
/// One instantiation of each GRIB reader and of the NetCDF reader in this crate
/// rather than one per source type (#709, #950). `Session::open` boxes the
/// buffer it was handed and `Session::open_source` boxes whatever the host
/// brought, so both arrive here as the same type and the readers are
/// monomorphised once — which is what keeps a source-taking constructor from
/// costing the browser bundle a second copy of every decode path. The cost is a
/// virtual call per `read`, against a decode that does far more work per slab
/// than that.
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
    source: Box<Source>,
    /// What [`Session::format`] reports.
    format: SourceFormat,
    /// Slice placements already derived, keyed by what determines them
    /// (ADR-0011).
    ///
    /// A placement is a pure function of the container's metadata and its
    /// coordinate arrays, both fixed for the session's lifetime, so serving a
    /// second identical question from here is unobservable except in time. It
    /// is memoized rather than left to each host because *every* host needs
    /// it and only one had it: `fieldglass-napi` cached the curvilinear index
    /// for NetCDF, did not for Zarr — its Zarr handle re-derived it on every
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
    /// parameters. Memoized all the same, because deriving them re-reads the
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
    fn new(source: Box<Source>, format: SourceFormat) -> Self {
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
///
/// Public for a host that keeps decoded values of its own: `fieldglass-napi`
/// memoizes the engine's `Vec<Option<f64>>` per message and per plane, and
/// packs from that memo for its `decode` and `decodeSlice` (#1073), so the
/// element width it reports under [`Dtype::Auto`] is this rule's and not a
/// second one. The pattern `combine_values` set (ADR-0006, #579).
pub fn pack_values(raw: &[Option<f64>], options: &DecodeOptions) -> (Values, Vec<u8>, Stats) {
    let mut values = Vec::with_capacity(raw.len());
    let mut mask = Vec::with_capacity(raw.len());
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    let mut valid_count = 0u32;
    for cell in raw {
        match cell {
            // A non-finite decoded value is not a value: it cannot be ranged,
            // colored, or interpolated, so it joins the masked cells rather
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
    parameter: Option<String>,
    units: Option<String>,
    options: &DecodeOptions,
) -> Field {
    let (values, mask, stats) = pack_values(raw, options);
    Field {
        values,
        mask,
        ni,
        nj,
        // A field with cells and no geometry to put them anywhere is a raster
        // nothing places, not "no raster" (#776) — `from_slice` states the rule
        // once, for this and for a host placing a slice without decoding it.
        georef: Georef::from_slice(geometry, scan, declared, ni, nj),
        // Set by `Session::decode` for the one family it applies to; nothing
        // else a field is built from is band-limited.
        truncation: None,
        stats,
        parameter,
        units,
    }
}

/// Enough for [`detect_from_bytes`] to tell a GRIB edition (`"GRIB"` plus the
/// edition octet) or a classic NetCDF file (`"CDF"` plus the version byte).
/// Opening from a source reads this much and not the file (#709). A NetCDF-4
/// file with a userblock has its HDF5 signature further in (#936), so
/// `Session::open_source` looks for it at the offsets it may sit at when this
/// prefix finds nothing.
#[cfg(any(feature = "grib1", feature = "grib2", feature = "netcdf"))]
const DETECT_PREFIX: usize = 8;

/// One slice's placement: the grid, its storage order, and its raster shape.
///
/// What [`Session::place_slice`] hands back, and what
/// `source()` turns into the projection pipeline's own input — named rather than
/// linked, because that method is behind `render`/`analysis` and this type is
/// not. Owned
/// rather than borrowed because the geometry is *derived* — a 2-D coordinate pair
/// becomes a cell-center lookup, a CF `grid_mapping` becomes a projection — so
/// there is nothing inside the container to borrow it from.
///
/// Not a wire type: it carries a [`GridGeometry`], which is the engine's own
/// shape rather than anything a host serializes. A host reports placement to its
/// UI from [`Field::georef`](crate::api::Field), and paints with this.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct PlacedSlice {
    /// The memoized placement, shared rather than copied.
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

/// The refusal [`Session::open_message_at`] gives a container that is not a
/// message stream, naming the call to make instead.
///
/// Each answer says *why* rather than only "no": NetCDF because it holds
/// variables rather than messages, so there is no message at an offset to
/// open, and anything else because it is no container this build knows.
#[cfg(any(feature = "grib1", feature = "grib2"))]
fn not_a_message_stream(format: CoreFormat, called: &str) -> Error {
    let detail = match format {
        CoreFormat::NetCdf => format!(
            "`{called}` opens one message, and these bytes are NetCDF, which holds variables \
             rather than messages; call `Session::open_source` with the whole file instead"
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

/// A reader's left-out or undrawable array, as the wire states it (#1129).
#[cfg(any(feature = "netcdf", feature = "zarr"))]
fn left_out_wire(l: fieldglass_core::LeftOut) -> LeftOutArray {
    use crate::api::LeftOutCause;
    use fieldglass_core::LeftOutCause as Core;
    let cause = match l.cause {
        Core::UnsupportedDtype { dtype } => LeftOutCause::UnsupportedDtype { dtype },
        Core::Text => LeftOutCause::Text,
        Core::DimensionClash { dimension } => LeftOutCause::DimensionClash { dimension },
        Core::Unsupported => LeftOutCause::Unsupported,
        Core::UnreadableMetadata => LeftOutCause::UnreadableMetadata,
        Core::TooFewDimensions { rank } => LeftOutCause::TooFewDimensions { rank },
        Core::Coordinate => LeftOutCause::Coordinate,
        // No wildcard: core's enum is exhaustive so that a new code fails to
        // build here, where its wire spelling has to be decided.
    };
    LeftOutArray {
        name: l.name,
        reason: l.reason,
        cause,
    }
}

/// The variables `source` offers to slice — the implementation behind
/// [`Session::variables`], for a host that holds an [`ArraySource`] rather
/// than a `Session` (#1073).
///
/// `index` is the position in this list, which every slice and line request
/// names, on every host: `fieldglass-napi` reads NetCDF through its own reader
/// and view, and lists and numbers its variables through this, so the two
/// hosts mean the same variable by the same number.
#[cfg(any(feature = "netcdf", feature = "zarr"))]
pub fn variables_of(source: &dyn ArraySource) -> Vec<VariableInfo> {
    renderable_arrays(source.group())
        .into_iter()
        .enumerate()
        .map(|(i, v)| {
            let (default_y, default_x) = default_image_axes(
                v.dims.len(),
                v.detected_y_dim,
                v.detected_x_dim,
                v.detected_time_dim,
            );
            VariableInfo {
                // Position in *this* list, not anything the reader numbers by: a
                // host should never have to know that a reader counts every array
                // while this offers only the renderable ones.
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
                units: array_units(source, &v.name),
                detected_y_dim: v.detected_y_dim.and_then(|d| u32::try_from(d).ok()),
                detected_x_dim: v.detected_x_dim.and_then(|d| u32::try_from(d).ok()),
                default_y_dim: u32::try_from(default_y).unwrap_or(u32::MAX),
                default_x_dim: u32::try_from(default_x).unwrap_or(u32::MAX),
                detected_time_dim: v.detected_time_dim.and_then(|d| u32::try_from(d).ok()),
                name: v.name,
            }
        })
        .collect()
}

/// The Y and X axes a variable of `rank` dimensions opens on (#1397).
///
/// The detected horizontals when the file names both. Otherwise the last two
/// dimensions, the fastest-varying ones in C order and where CF puts the
/// horizontal axes, skipping a detected time axis while two others remain, so
/// an unnamed `time × scanline × ground_pixel` swath opens on the swath. A
/// lone detected axis keeps its place and takes the last remaining dimension
/// as its partner. `rank` is at least 2: a variable with fewer is never
/// listed.
#[cfg(any(feature = "netcdf", feature = "zarr"))]
fn default_image_axes(
    rank: usize,
    y: Option<usize>,
    x: Option<usize>,
    time: Option<usize>,
) -> (usize, usize) {
    if let (Some(y), Some(x)) = (y, x)
        && y != x
    {
        return (y, x);
    }
    let all: Vec<usize> = (0..rank).collect();
    let spatial: Vec<usize> = all.iter().copied().filter(|&d| Some(d) != time).collect();
    let pool = if spatial.len() >= 2 { spatial } else { all };
    // The last pooled dimension other than `taken`.
    let last_but = |taken: usize| pool.iter().rev().copied().find(|&d| d != taken);
    match (y, x) {
        (Some(y), _) => (y, last_but(y).unwrap_or(0)),
        (None, Some(x)) => (last_but(x).unwrap_or(0), x),
        (None, None) => match pool[..] {
            [.., a, b] => (a, b),
            _ => (0, 1),
        },
    }
}

#[cfg(all(test, any(feature = "netcdf", feature = "zarr")))]
mod default_image_axes_tests {
    use super::default_image_axes;

    #[test]
    fn the_last_two_dimensions_without_a_detected_pair() {
        assert_eq!(default_image_axes(2, None, None, None), (0, 1));
        assert_eq!(default_image_axes(3, None, None, None), (1, 2));
        assert_eq!(default_image_axes(4, None, None, None), (2, 3));
    }

    #[test]
    fn a_detected_pair_wins_wherever_it_sits() {
        assert_eq!(default_image_axes(4, Some(2), Some(3), Some(0)), (2, 3));
        assert_eq!(default_image_axes(3, Some(0), Some(1), None), (0, 1));
    }

    /// Time is never an image axis while two others remain.
    #[test]
    fn a_detected_time_is_skipped() {
        // y × x × time: the image is the first two.
        assert_eq!(default_image_axes(3, None, None, Some(2)), (0, 1));
        // time × y × x: the last two, as without the hint.
        assert_eq!(default_image_axes(3, None, None, Some(0)), (1, 2));
        // A 2-D time × x has nothing else to show.
        assert_eq!(default_image_axes(2, None, None, Some(0)), (0, 1));
    }

    #[test]
    fn a_lone_detected_axis_keeps_its_place() {
        assert_eq!(default_image_axes(3, Some(2), None, None), (2, 1));
        assert_eq!(default_image_axes(3, None, Some(0), None), (2, 0));
        // A degenerate pair naming one axis twice keeps Y.
        assert_eq!(default_image_axes(3, Some(2), Some(2), None), (2, 1));
    }
}

/// The dataset's shared dimensions, in the order `source` declares them — the
/// implementation behind [`Session::dimensions`], for a host that holds an
/// [`ArraySource`] rather than a `Session` (#1205).
///
/// `fieldglass-napi` reads NetCDF through its own reader and view, so without
/// this its slider list would be a second derivation of one answer.
#[cfg(any(feature = "netcdf", feature = "zarr"))]
pub fn dimensions_of(source: &dyn ArraySource) -> Vec<DimensionInfo> {
    source
        .group()
        .dimensions_qualified()
        .into_iter()
        .map(|(name, d)| DimensionInfo {
            name,
            length: d.length,
        })
        .collect()
}

/// Whether variables `u` and `v` of `source` are the two components of one
/// vector, by each one's CF `standard_name` — the implementation behind
/// [`Session::vector_pairing`] for a variable container, for a host that holds
/// an [`ArraySource`] rather than a `Session` (#1205).
///
/// `u` and `v` are positions in [`variables_of`]'s list.
///
/// # Errors
///
/// [`Error::NoSuchMessage`] when either index is past the end of that list.
#[cfg(any(feature = "netcdf", feature = "zarr"))]
pub fn vector_pairing_of(source: &dyn ArraySource, u: u32, v: u32) -> Result<VectorPairing, Error> {
    let vars = renderable_arrays(source.group());
    let standard_name = |index: u32| {
        let var = vars.get(index as usize).ok_or(Error::NoSuchMessage {
            index,
            count: u32::try_from(vars.len()).unwrap_or(u32::MAX),
        })?;
        Ok::<_, Error>(
            source
                .array(&var.name)
                .and_then(|d| attribute(&d.attributes, "standard_name"))
                .and_then(AttributeValue::text)
                .map(|s| s.trim().to_string()),
        )
    };
    let (u, v) = (standard_name(u)?, standard_name(v)?);
    Ok(fieldglass_core::vector_pair::pairing_of_standard_names(u.as_deref(), v.as_deref()).into())
}

/// The arrays `source` holds and this build will not read — the
/// implementation behind [`Session::left_out`], for a host that holds an
/// [`ArraySource`] rather than a `Session` (#1073).
///
/// `fieldglass-napi` reads NetCDF through its own reader and view, so without
/// this its list would be a second derivation of one answer, and the two hosts
/// could disagree about which arrays a file leaves out.
#[cfg(any(feature = "netcdf", feature = "zarr"))]
pub fn left_out_of(source: &dyn ArraySource) -> Vec<LeftOutArray> {
    source.left_out().into_iter().map(left_out_wire).collect()
}

/// The arrays `source` reads that no map can be drawn from — the
/// implementation behind [`Session::undrawable`], for the host
/// [`left_out_of`] serves (#1073).
#[cfg(any(feature = "netcdf", feature = "zarr"))]
pub fn undrawable_of(source: &dyn ArraySource) -> Vec<LeftOutArray> {
    fieldglass_core::cf::undrawable_arrays(source.group())
        .into_iter()
        .map(left_out_wire)
        .collect()
}

/// The region of `var` a line through `indices` reads, or the one cell at
/// `indices` when `span` is false (a held combined line's Field B, #1220).
///
/// With `span` true the `along` axis is read whole and its entry in `indices`
/// is ignored; with `span` false that entry is a position like any other and
/// is checked against `var`'s own axis. Every refusal is
/// [`Error::InvalidOption`].
#[cfg(any(feature = "netcdf", feature = "zarr"))]
fn line_region(
    var: &RenderableArray,
    along: usize,
    indices: &[u32],
    span: bool,
) -> Result<Vec<std::ops::Range<u64>>, Error> {
    let rank = var.dims.len();
    if span && along >= rank {
        return Err(Error::invalid_option(format!(
            "`{}` has {rank} dimensions, so along_dim {along} is outside them",
            var.name
        )));
    }
    if indices.len() != rank {
        return Err(Error::invalid_option(format!(
            "`{}` has {rank} dimensions, so `indices` needs {rank} entries{}; {} were given",
            var.name,
            if span {
                " (the one being read along is ignored)"
            } else {
                ""
            },
            indices.len()
        )));
    }
    let mut region = Vec::with_capacity(rank);
    for (d, dim) in var.dims.iter().enumerate() {
        if span && d == along {
            region.push(0..dim.length);
            continue;
        }
        let at = u64::from(indices[d]);
        check_index_in_axis(&var.name, d, &dim.name, dim.length, at)?;
        region.push(at..at + 1);
    }
    Ok(region)
}

/// The one refusal for a held index at or past the end of its axis, worded the
/// same for a line and for a slice (#1301). An axis of length 0 has no index
/// to hold, so it is refused as [`empty_axis`] rather than as a stale index.
#[cfg(any(feature = "netcdf", feature = "zarr"))]
fn check_index_in_axis(
    array: &str,
    d: usize,
    dim_name: &str,
    length: u64,
    at: u64,
) -> Result<(), Error> {
    if length == 0 {
        return Err(empty_axis(array, d, dim_name));
    }
    if at >= length {
        return Err(Error::invalid_option(format!(
            "index {at} is past the end of `{array}`'s dimension {d} \
                 (`{dim_name}`, length {length})"
        )));
    }
    Ok(())
}

/// The refusal for a slice or a line across an axis of length 0 (#1425): an
/// unlimited `time` with no records, say. There are no cells, so nothing can be
/// drawn, and no index on the axis would change that. [`Error::Unsupported`],
/// since the file is valid and the caller asked for nothing wrong.
#[cfg(any(feature = "netcdf", feature = "zarr"))]
fn empty_axis(array: &str, d: usize, dim_name: &str) -> Error {
    Error::Unsupported {
        detail: format!("nothing to draw: `{dim_name}` (dimension {d} of `{array}`) has length 0"),
    }
}

/// Check a slice against its axes before its plane is read: the two drawn
/// must each hold at least one entry, and every held entry of `indices` must be
/// on its own axis, as [`Session::decode_line`] checks a line. `dims` is the
/// array's axes in declared order; the entries at `y_dim` and `x_dim` are
/// drawn, not held, so their indices are not checked.
///
/// An axis of length 0, drawn or held, is [`Error::Unsupported`] naming it: the
/// plane has no cells, and a raster with no pixels is what a host cannot draw
/// (#1425). A held index past the end of a non-empty axis is
/// [`Error::InvalidOption`], so a stale index is the caller's mistake and not a
/// file that did not parse (#1301). Run it after the length and axis-pair
/// checks and before reading the plane.
#[cfg(any(feature = "netcdf", feature = "zarr"))]
pub fn check_slice_indices(
    array: &str,
    dims: &[fieldglass_core::array::Dimension],
    y_dim: usize,
    x_dim: usize,
    indices: &[u32],
) -> Result<(), Error> {
    for (d, (dim, &at)) in dims.iter().zip(indices).enumerate() {
        if d == y_dim || d == x_dim {
            if dim.length == 0 {
                return Err(empty_axis(array, d, &dim.name));
            }
            continue;
        }
        check_index_in_axis(array, d, &dim.name, dim.length, u64::from(at))?;
    }
    Ok(())
}

/// The one cell of `array` at `indices`, in physical units: a held combined
/// line's Field B (#1220). Reads that cell alone, not the line through it.
#[cfg(all(feature = "analysis", any(feature = "netcdf", feature = "zarr")))]
fn cell_through(
    source: &dyn ArraySource,
    array: &str,
    along_dim: u32,
    indices: &[u32],
) -> Result<Option<f64>, Error> {
    let vars = renderable_arrays(source.group());
    let var = vars
        .iter()
        .find(|v| v.name == array)
        .ok_or_else(|| no_such_array(array, vars.len()))?;
    let region = line_region(var, along_dim as usize, indices, false)?;
    let raw = source.read_region(&var.name, &region)?;
    let attributes = source
        .array(&var.name)
        .map(|d| d.attributes.as_slice())
        .unwrap_or_default();
    let mut physical = CfUnpacking::from_attributes(attributes).apply_owned(raw);
    // A non-finite value is not a value, as `pack_values` treats it on a line.
    Ok(physical.pop().flatten().filter(|v| v.is_finite()))
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
/// is [`Error::InvalidOption`] naming it, as an axis it does not have is. It is
/// not [`Error::NoSuchMessage`], which is about a position, and a name has none
/// to report (#872).
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
        .ok_or_else(|| no_such_array(array, vars.len()))?;
    let along = along_dim as usize;
    let region = line_region(var, along, indices, true)?;
    // A region with every axis but one a single index comes back as
    // exactly the points on the line, in index order.
    let raw = source.read_region(&var.name, &region)?;
    let attributes = source
        .array(&var.name)
        .map(|d| d.attributes.as_slice())
        .unwrap_or_default();
    let physical = CfUnpacking::from_attributes(attributes).apply_owned(raw);
    let (values, mask, stats) = pack_values(&physical, options);
    let dimension = var.dims[along].name.clone();
    let coordinates = axis_coordinates(source, &dimension, var.dims[along].length);
    Ok(Line {
        values,
        mask,
        stats,
        variable: Some(var.name.clone()),
        units: array_units(source, &var.name),
        coordinate_units: coordinates
            .as_ref()
            .and_then(|_| array_units(source, &dimension)),
        coordinate_calendar: coordinates
            .as_ref()
            .and_then(|_| array_calendar(source, &dimension)),
        coordinate_vertical: coordinates
            .as_ref()
            .and_then(|_| array_vertical(source, &dimension)),
        coordinates,
        dimension,
    })
}

/// One line through a combined map (#1208): line `a` combined point by point
/// with line `b` under `op` — the implementation behind
/// [`Session::decode_line_combined`], keyed by name for the reason
/// [`line_through`] is.
///
/// `a` and `b` each name an array and a position on every axis of it, as
/// [`line_through`] takes them. Both lines run along `along_dim`. With `hold_b`
/// false it must be the same axis, by name and length, in each array; with
/// `hold_b` true `b` is one cell and its axes are its own (#1220).
///
/// `hold_b` says which kind of axis the line runs along, which only the host
/// knows:
///
/// * `false`, an axis of the map itself: `b` is read along it too, point for
///   point, and the line is a row or column of the combined map. A host writes
///   the clicked cell into both positions. Each entry for `along_dim` is
///   ignored, as on a single line.
/// * `true`, a held axis (a time or a level): `b` is the one value at its own
///   position, its entry for `along_dim` included, combined with every step of
///   `a`. That is what the map shows as Field A steps along the axis with
///   Field B where it is drawn, so the point at `a`'s own entry is the map's
///   value (#1208).
///
/// Both lines are read at full precision and combined by the rule
/// [`crate::combine_values`] applies to every cell, then packed under
/// `options`. The name, units, axis and coordinates are `a`'s: the caption
/// `A − B` is the host's to compose, as for [`Session::combine`].
///
/// # Errors
///
/// Whatever [`line_through`] reports for either line; [`Error::InvalidOption`]
/// for a held `b` whose entry for any axis, `along_dim` included, is past the
/// end of that axis of `b`; and [`Error::Unsupported`] when two lines that are
/// both read along an axis do not run along the same one.
#[cfg(all(feature = "analysis", any(feature = "netcdf", feature = "zarr")))]
pub fn line_through_combined(
    source: &dyn ArraySource,
    a: (&str, &[u32]),
    b: (&str, &[u32]),
    along_dim: u32,
    hold_b: bool,
    op: CombineOp,
    options: &DecodeOptions,
) -> Result<Line, Error> {
    let exact = DecodeOptions::new(Dtype::F64);
    let line_a = line_through(source, a.0, along_dim, a.1, &exact)?;
    let n = line_a.mask.len();
    let values_a = line_a.values.to_f64();
    let points_a: Vec<Option<f64>> = line_a
        .mask
        .iter()
        .zip(values_a.iter())
        .map(|(&m, &v)| (m == 1).then_some(v))
        .collect();
    // A held `b` is its one cell, read alone and checked against `b`'s own
    // axes: its along axis need not match `a`'s in name or length (#1220).
    let cells: Vec<Option<f64>> = if hold_b {
        let held = cell_through(source, b.0, along_dim, b.1)?;
        points_a
            .iter()
            .map(|&value_a| crate::combine::combine_cell(value_a, held, op))
            .collect()
    } else {
        let line_b = line_through(source, b.0, along_dim, b.1, &exact)?;
        if line_a.dimension != line_b.dimension || n != line_b.mask.len() {
            return Err(Error::Unsupported {
                detail: format!(
                    "the two lines of a combined line run along the same axis; `a` runs along \
                     `{}` ({n} points), `b` along `{}` ({} points)",
                    line_a.dimension,
                    line_b.dimension,
                    line_b.mask.len()
                ),
            });
        }
        let values_b = line_b.values.to_f64();
        points_a
            .iter()
            .zip(line_b.mask.iter().zip(values_b.iter()))
            .map(|(&value_a, (&m, &v))| {
                crate::combine::combine_cell(value_a, (m == 1).then_some(v), op)
            })
            .collect()
    };
    let (values, mask, stats) = pack_values(&cells, options);
    Ok(Line {
        values,
        mask,
        stats,
        ..line_a
    })
}

/// One axis of the array named `array` in `source` — the implementation behind
/// [`Session::axis_values`], keyed by name for the reason [`line_through`] is
/// (#171).
///
/// Reads the coordinate array and nothing else: labeling a cross-section's
/// axes must not decode the field.
///
/// # Errors
///
/// [`Error::InvalidOption`] for an array this source does not list as
/// renderable, named as [`line_through`] names it, or for a `dim` outside its
/// axes. A coordinate array that fails to read is not an error — the axis comes
/// back without coordinates, which is the same answer as having none, because
/// an axis a host cannot label is still an axis it can draw.
#[cfg(any(feature = "netcdf", feature = "zarr"))]
pub fn axis_values(source: &dyn ArraySource, array: &str, dim: u32) -> Result<AxisValues, Error> {
    let vars = renderable_arrays(source.group());
    let var = vars
        .iter()
        .find(|v| v.name == array)
        .ok_or_else(|| no_such_array(array, vars.len()))?;
    let axis = var
        .dims
        .get(dim as usize)
        .ok_or(Error::invalid_option(format!(
            "`{}` has {} dimensions, so dim {dim} is outside them",
            var.name,
            var.dims.len()
        )))?;
    let coordinates = axis_coordinates(source, &axis.name, axis.length);
    Ok(AxisValues {
        units: coordinates
            .as_ref()
            .and_then(|_| array_units(source, &axis.name)),
        calendar: coordinates
            .as_ref()
            .and_then(|_| array_calendar(source, &axis.name)),
        vertical: coordinates
            .as_ref()
            .and_then(|_| array_vertical(source, &axis.name)),
        dimension: axis.name.clone(),
        length: axis.length,
        coordinates,
    })
}

/// The refusal for an array name `line_through` and `axis_values` do not find
/// among the renderable ones. The name is the caller's own argument, so it is
/// reported back, beside how many arrays there were to choose from.
#[cfg(any(feature = "netcdf", feature = "zarr"))]
fn no_such_array(array: &str, renderable: usize) -> Error {
    Error::invalid_option(format!(
        "no renderable array is named `{array}`; the source lists {renderable} renderable arrays"
    ))
}

/// The coordinate values of an axis, from the 1-D array CF names after it
/// (#172).
///
/// `None` unless that array exists, spans exactly this axis, and has a value at
/// every position — a coordinate with a hole has no position to plot its point
/// at, and a host is better served falling back to indices than handed a gap
/// to invent across.
///
/// A value that is present but not finite (`NaN`, `±inf`) is a hole too (#574).
/// It has no position to plot at either, and it would not reach the two hosts
/// alike: the Node addon's `serde_json` writes it as `null` where the browser
/// binding passes `NaN`, so a declaration saying `number[]` would be wrong in
/// one of them. Refused here, no non-finite coordinate crosses either wire.
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
    let physical = CfUnpacking::from_attributes(&array.attributes).apply_owned(raw);
    physical
        .into_iter()
        .map(|v| v.filter(|x| x.is_finite()))
        .collect()
}

/// An array's `units`, as the container spells them, or `None` when it states
/// none (#775). An empty or all-blank `units` attribute states none either,
/// which is how [`Line::coordinate_units`] already read it.
#[cfg(any(feature = "netcdf", feature = "zarr"))]
fn array_units(source: &dyn ArraySource, array: &str) -> Option<String> {
    array_text(source, array, "units")
}

/// An array's `calendar`, as the container spells it, or `None` when it
/// states none (#1033): the reading behind [`AxisValues::calendar`] and
/// [`Line::coordinate_calendar`]. Blank states none, as for
/// [`array_units`].
#[cfg(any(feature = "netcdf", feature = "zarr"))]
fn array_calendar(source: &dyn ArraySource, array: &str) -> Option<String> {
    array_text(source, array, "calendar")
}

/// One text attribute of an array, `None` when it is absent, not text, or
/// blank.
#[cfg(any(feature = "netcdf", feature = "zarr"))]
fn array_text(source: &dyn ArraySource, array: &str, name: &str) -> Option<String> {
    source
        .array(array)
        .and_then(|d| attribute(&d.attributes, name))
        .and_then(AttributeValue::text)
        .filter(|u| !u.trim().is_empty())
        .map(str::to_string)
}

/// Whether an array is a vertical coordinate, and which way its values
/// increase, by CF §4.3 (#1043): the reading behind [`AxisValues::vertical`]
/// and [`Line::coordinate_vertical`].
#[cfg(any(feature = "netcdf", feature = "zarr"))]
fn array_vertical(source: &dyn ArraySource, array: &str) -> Option<crate::api::Vertical> {
    source
        .array(array)
        .and_then(fieldglass_core::cf::vertical_axis)
        .map(Into::into)
}

/// One 2-D plane of an array, raw, as `nj` rows (along `y`) of `ni` values
/// (along `x`).
///
/// Read as one region — the full extent of the two horizontal axes, one index
/// of every other — so a container that fetches by chunk fetches only the
/// chunks the plane covers, and a NetCDF variable reads only the plane (#939).
/// The region and the transpose of an array whose `x` axis precedes its `y` are
/// `fieldglass_core::cf::read_plane`'s, the one implementation the NetCDF
/// reader's own plane read shares.
#[cfg(any(feature = "netcdf", feature = "zarr"))]
fn read_plane(
    source: &dyn ArraySource,
    var: &fieldglass_core::cf::RenderableArray,
    y: usize,
    x: usize,
    fixed: &[usize],
) -> Result<Vec<Option<f64>>, Error> {
    let shape: Vec<u64> = var.dims.iter().map(|d| d.length).collect();
    Ok(fieldglass_core::cf::read_plane(
        &shape,
        y,
        x,
        fixed,
        |region| source.read_region(&var.name, region),
    )?)
}

impl Session {
    /// Open a container from its bytes.
    ///
    /// The format is detected from the bytes, never from a name: ADR-0005 hands
    /// the host's fetched buffer straight in, and a range-fetched GRIB message
    /// has no filename to guess from.
    pub fn open(bytes: Vec<u8>) -> Result<Self, Error> {
        Self::from_detected(detect_from_bytes(&bytes), Box::new(bytes))
    }

    /// The reader a detected format asks for, over a source.
    ///
    /// Shared by [`Session::open`] and [`Session::open_source`], so the two
    /// build the same readers and a build without an edition refuses it in the
    /// same words whichever way the file came in.
    // A build with no ranged reader (Zarr alone) refuses every format here, so
    // the source goes unread and the arm after the match is never reached.
    #[cfg_attr(
        not(any(feature = "grib1", feature = "grib2", feature = "netcdf")),
        allow(unused_variables, unreachable_code)
    )]
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
            // The view is resolved here, so a variable list costs one walk per
            // file rather than one per question.
            #[cfg(feature = "netcdf")]
            CoreFormat::NetCdf => Reader::Arrays(Box::new(Arrays::new(
                Box::new(fieldglass_netcdf::NetcdfArrays::open(
                    fieldglass_netcdf::NetcdfReader::from_source(source)?,
                )?),
                SourceFormat::NetCdf,
            ))),
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

    /// Open a file from a source the host reads through, rather than from a
    /// buffer it has all of (#709, #950).
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
    /// [`detect_from_bytes`] needs to tell a GRIB edition or a classic NetCDF
    /// file; an HDF5 signature after a userblock is looked for at the few
    /// offsets it may sit at. Opening costs those small reads and not the file.
    ///
    /// # Files, not stores
    ///
    /// A GRIB file of either edition, or a NetCDF file, classic or NetCDF-4.
    /// A NetCDF file is read by range like a GRIB one (#950). Opening reads a
    /// classic file's header, or an HDF5 file's superblock and then its
    /// metadata, because the session resolves the variable list once, at open.
    /// After that each slice reads only its own runs or chunks.
    ///
    /// Have the source answer [`ByteSource::identity`], with
    /// [`SourceIdentity::named`](fieldglass_core::bytes::SourceIdentity::named)
    /// if it has nothing better. The HDF5 reader memoizes what it walked
    /// (object headers, chunk records, recently decompressed chunks) only for
    /// a source that says which file it is. One that declines is read
    /// correctly, but every slice walks the metadata again.
    ///
    /// A Zarr store is keyed rather than ranged and
    /// has `Session::open_store` — named rather than linked, because that one
    /// is behind the `zarr` feature and this is not.
    ///
    /// # Errors
    ///
    /// A source whose leading bytes match no container this build opens, and
    /// any failure of the source, of the scan, or of the NetCDF header and
    /// metadata walk.
    ///
    /// [ADR-0006]: https://github.com/D0ubleD0uble/fieldglass/blob/master/docs/decisions/0006-one-umbrella-crate-and-host-bindings-over-it.md
    #[cfg(any(feature = "grib1", feature = "grib2", feature = "netcdf"))]
    pub fn open_source<S: ByteSource + 'static>(source: S) -> Result<Self, Error> {
        let source: Bytes = Box::new(source);
        // Eight bytes is everything `detect_from_bytes` needs for a GRIB
        // edition or a classic NetCDF version. `read_head` rather than a
        // fixed read, so a source shorter than that says "no container"
        // instead of "short read".
        let head = read_head(&source, 0, DETECT_PREFIX)?;
        let mut format = detect_from_bytes(&head);
        drop(head);
        // An HDF5 signature after a userblock (#936) is past the prefix. Its
        // offsets are a fixed handful, so this is a few small reads, not a scan.
        if format == CoreFormat::Unknown && fieldglass_core::find_hdf5_signature(&source)?.is_some()
        {
            format = CoreFormat::NetCdf;
        }
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
        let head = read_head(&source, offset, DETECT_PREFIX)?;
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
        Self::open_store_named(objects, "")
    }

    /// [`Session::open_store`] for a store whose root is itself an array.
    ///
    /// An array at the root has no name inside the store, so a host that knows
    /// what the store is called, such as the folder it was opened from, passes
    /// that as `root_name` and the array is listed, drawn and reported under it
    /// (#1468). An empty `root_name`, and any store whose root is a group, is
    /// as [`Session::open_store`].
    ///
    /// # Errors
    ///
    /// As [`Session::open_store`].
    #[cfg(feature = "zarr")]
    pub fn open_store_named<O: ObjectSource + 'static>(
        objects: O,
        root_name: &str,
    ) -> Result<Self, Error> {
        let store = fieldglass_zarr::ZarrStore::open_named(objects, root_name)?;
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

    /// The rows and columns message `i`'s grid section states, when it states
    /// them: what [`decode`](Self::decode) lays values onto when the geometry
    /// could not be built from the section (#823).
    ///
    /// The same answer [`grib2_without_geometry`] and
    /// [`grib1_without_geometry`] read to call such a grid `unplaceable`, so a
    /// message `message` reports as a raster nothing places decodes as one. A
    /// template this build does not model states none, and still refuses.
    #[cfg(any(feature = "grib1", feature = "grib2"))]
    fn declared_raster(&self, i: usize) -> Option<(u32, u32)> {
        match &self.reader {
            #[cfg(feature = "grib1")]
            Reader::Grib1(r) => r.messages[i].gds.as_ref().and_then(|g| g.dimensions()),
            #[cfg(feature = "grib2")]
            Reader::Grib2(r) => r.messages[i].gds.dimensions(),
            #[cfg(any(feature = "netcdf", feature = "zarr"))]
            Reader::Arrays(_) => None,
        }
        // As `raster_without_geometry` reads them: a side of zero is no raster.
        .filter(|&(ni, nj)| ni > 0 && nj > 0)
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

    /// Message `i`'s band-limit label, when its map is a spectral field cut
    /// down to what the synthesis grid carries (#637).
    #[cfg(any(feature = "grib1", feature = "grib2"))]
    fn message_truncation(&self, i: usize) -> Option<SpectralTruncation> {
        match &self.reader {
            #[cfg(feature = "grib1")]
            Reader::Grib1(r) => r.synthesis_truncation(i).map(Into::into),
            #[cfg(feature = "grib2")]
            Reader::Grib2(r) => r.synthesis_truncation(i).map(Into::into),
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
    /// **A family with no raster of its own is synthesized first.** Spectral
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
            // which families need synthesizing, and onto what grid, is the format
            // crate's answer, not one this crate re-derives (#546, #580).
            let synthesized = match &self.reader {
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
            let was_synthesized = synthesized.is_some();
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
            // longer knows it was a `reduced_gg`. A **synthesized** grid is the
            // case where the geometry is the honest answer — the values really are
            // on the lat/lon raster the transform filled, and nothing of the
            // declared family survives it — so that arm reads the geometry's own
            // label rather than the message's.
            let (raw, geometry, scan, declared) = match synthesized {
                Some((grid, values)) => {
                    let geometry = GridGeometry::LatLon(grid.into());
                    let declared = geometry.label().to_string();
                    (
                        values,
                        geometry,
                        // A synthesized grid runs west-to-east from 0° and
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

            // A modeled grid whose geometry could not be built — a §3.90
            // camera that sees no Earth, a rotation or scale factor that is not
            // a finite number (#823) — still states its rows and columns, and
            // its values are laid out on them in grid coordinates: the
            // `unplaceable` raster `message` reports. `from_slice` in
            // `build_field` gives such a field that placement.
            let (ni, nj) = geometry
                .dims()
                .or_else(|| {
                    (!was_synthesized)
                        .then(|| self.declared_raster(i))
                        .flatten()
                })
                .ok_or_else(|| Error::Unsupported {
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
            // (#244). A synthesized grid is not reduced whatever the message
            // declared, so it states none.
            if !was_synthesized {
                field.georef.points_per_row = self.message_points_per_row(i);
            }
            // A spectral message past what the synthesis grid carries was
            // band-limited on the way, and the field says so (#637). The
            // format crate's answer, read from the same declaration the
            // transform band-limited by, and the one `message` reports.
            field.truncation = self.message_truncation(i);
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

    /// Decode message `index` to **draw**, at the resolution `options` asks
    /// for (#1082).
    ///
    /// A zoomed-out map samples a grid far more coarsely than it is stored, and
    /// a JPEG 2000 (§5.40) message can answer coarsely without decoding the
    /// whole field: on the committed RAP fixture a reduction of 1 decodes in
    /// about a quarter of the time. The values are then a wavelet low-pass the
    /// message does not hold, on every `2^r`-th point of its grid, so what comes
    /// back is a [`DisplayField`] and not a [`Field`]: `render_display`,
    /// `warp_display` and `palette_display` take it (named, not linked: they
    /// are behind `render`, and this method is not), and nothing that measures
    /// does.
    ///
    /// ```compile_fail,E0308
    /// # use fieldglass::{DisplayDecodeOptions, Session};
    /// # fn demo(session: &Session) {
    /// let coarse = session.decode_display(0, &DisplayDecodeOptions::new(1)).unwrap();
    /// // A display field is not a `Field`, so it cannot be probed.
    /// session.probe(&coarse, 40.0, -100.0);
    /// # }
    /// ```
    ///
    /// Nor contoured, nor combined:
    ///
    /// ```compile_fail,E0308
    /// # use fieldglass::{ContourOptions, DisplayDecodeOptions, Session};
    /// # fn demo(session: &Session) {
    /// let coarse = session.decode_display(0, &DisplayDecodeOptions::new(1)).unwrap();
    /// session.contours(&coarse, &[], &ContourOptions::default());
    /// # }
    /// ```
    ///
    /// ```compile_fail,E0308
    /// # use fieldglass::{CombineOp, DisplayDecodeOptions, Session};
    /// # fn demo(session: &Session) {
    /// let coarse = session.decode_display(0, &DisplayDecodeOptions::new(1)).unwrap();
    /// session.combine(&coarse, &coarse, CombineOp::Difference);
    /// # }
    /// ```
    ///
    /// The same calls on a full decode compile, so each block above fails for
    /// the reason it says and not for a typo:
    ///
    /// ```
    /// # use fieldglass::{CombineOp, ContourOptions, DecodeOptions, Dtype, Session};
    /// # fn demo(session: &Session) {
    /// let full = session.decode(0, &DecodeOptions::new(Dtype::Auto)).unwrap();
    /// session.probe(&full, 40.0, -100.0);
    /// session.contours(&full, &[], &ContourOptions::default());
    /// session.combine(&full, &full, CombineOp::Difference);
    /// # }
    /// ```
    ///
    /// At reduction zero this is [`decode`](Self::decode)'s field, values,
    /// placement and labels alike, for every message `decode` serves: a host
    /// that picks the reduction from the zoom has one code path.
    ///
    /// # Errors
    ///
    /// Whatever [`decode`](Self::decode) raises, and above reduction zero
    /// [`Error::Unsupported`] for a message that has no coarse field to give:
    /// any GRIB1 message, a GRIB2 packing other than JPEG 2000, a bitmap, a
    /// reduced grid, alternate-row or column-major scanning, and a grid family
    /// with no derivable coarse grid (Gaussian among them), a level past the
    /// codestream's pyramid, and a codestream whose image origin is not zero.
    /// A caller falls back to a lower level. A
    /// codestream that does not decode, or uses a JPEG 2000 feature the codec
    /// does not implement, is [`Error::Decode`], as it is at reduction zero.
    /// [`Error::WrongAddressing`] for a variable container.
    pub fn decode_display(
        &self,
        index: u32,
        options: &DisplayDecodeOptions,
    ) -> Result<DisplayField, Error> {
        // Before anything else, for the reason `decode` gives, and naming this
        // call rather than `decode`.
        #[cfg(any(feature = "netcdf", feature = "zarr"))]
        if matches!(self.reader, Reader::Arrays(_)) {
            return Err(wrong_addressing(
                Addressing::Variables,
                "decode_display",
                "decode_slice",
            ));
        }
        let decode = DecodeOptions::new(options.dtype.clone());
        if options.reduction == 0 {
            let field = self.decode(index, &decode)?;
            return Ok(DisplayField {
                values: field.values,
                mask: field.mask,
                ni: field.ni,
                nj: field.nj,
                georef: field.georef,
                reduction: 0,
                truncation: field.truncation,
                parameter: field.parameter,
                units: field.units,
            });
        }
        match &self.reader {
            #[cfg(feature = "grib2")]
            Reader::Grib2(r) => {
                let i = self.check_index(index)?;
                let raster = r
                    .decode_message_raster_with(
                        i,
                        fieldglass_grib2::DecodeOptions::new(options.reduction),
                    )
                    // The reader's refusals are `UnsupportedSection`, which the
                    // general mapping files under `decode` — a corrupt file. A
                    // message that has no coarse field is a valid one asked for
                    // something it cannot give, and the caller's answer is to
                    // ask at reduction zero, which is what `unsupported` means.
                    // The reader keeps `UnsupportedSection` for exactly those
                    // refusals; a codestream that does not decode is `Parse`
                    // there, and stays `decode` here.
                    .map_err(|e| match e {
                        fieldglass_core::FieldglassError::UnsupportedSection(detail) => {
                            Error::Unsupported { detail }
                        }
                        other => Error::from(other),
                    })?;
                let msg = &r.messages[i];
                let (_, parameter, units) = grib2_parameter(msg);
                let (ni, nj) = (raster.ni(), raster.nj());
                let (values, mask, _) = pack_values(raster.display_values(), &decode);
                Ok(DisplayField {
                    values,
                    mask,
                    ni,
                    nj,
                    // The derived geometry, never the message's own: the coarse
                    // raster is every `2^r`-th point, and on the GDS it would
                    // draw at `1/2^r` of its size. A reduced grid and the two
                    // scanning orders that are undone after the decode are
                    // refused above, so the scan bits are the message's own and
                    // there are no points per row to state.
                    georef: Georef::from_slice(
                        raster.geometry(),
                        grib2_scan(msg),
                        &msg.gds.template_name(),
                        ni,
                        nj,
                    ),
                    reduction: raster.reduction(),
                    truncation: None,
                    parameter,
                    units,
                })
            }
            #[cfg(feature = "grib1")]
            Reader::Grib1(_) => {
                self.check_index(index)?;
                Err(Error::Unsupported {
                    detail: format!(
                        "resolution reduction {} was asked of a GRIB1 message, and only \
                         GRIB2 JPEG 2000 (§5.40) carries the resolution pyramid a reduced \
                         decode reads; decode it at reduction 0",
                        options.reduction
                    ),
                })
            }
            #[cfg(any(feature = "netcdf", feature = "zarr"))]
            Reader::Arrays(_) => Err(wrong_addressing(
                Addressing::Variables,
                "decode_display",
                "decode_slice",
            )),
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
            Reader::Arrays(a) => dimensions_of(a.source.as_ref()),
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
    ///
    /// Each carries a [`LeftOutCause`](crate::LeftOutCause) beside the
    /// reader's prose (#1129), so a host can say why in its own words.
    pub fn left_out(&self) -> Vec<LeftOutArray> {
        match &self.reader {
            #[cfg(feature = "grib1")]
            Reader::Grib1(_) => Vec::new(),
            #[cfg(feature = "grib2")]
            Reader::Grib2(_) => Vec::new(),
            #[cfg(any(feature = "netcdf", feature = "zarr"))]
            Reader::Arrays(a) => left_out_of(a.source.as_ref()),
        }
    }

    /// The arrays this container reads that no map can be drawn from, each
    /// with why (#1129): text, a type no decode turns into numbers, a
    /// coordinate, or fewer than two dimensions.
    ///
    /// [`Session::variables`], [`Session::left_out`] and this name every array
    /// in the container once between them, so a host that finds nothing to
    /// draw can say why for each array rather than only that there is
    /// nothing. Empty for a message stream.
    pub fn undrawable(&self) -> Vec<LeftOutArray> {
        match &self.reader {
            #[cfg(feature = "grib1")]
            Reader::Grib1(_) => Vec::new(),
            #[cfg(feature = "grib2")]
            Reader::Grib2(_) => Vec::new(),
            #[cfg(any(feature = "netcdf", feature = "zarr"))]
            Reader::Arrays(a) => undrawable_of(a.source.as_ref()),
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
            Reader::Arrays(a) => variables_of(a.source.as_ref()),
        }
    }

    /// Whether `u` and `v` are the two components of one vector, read from
    /// the parameter each names (#1049): the note a host puts beside arrows
    /// drawn from a pair that is not one.
    ///
    /// `u` and `v` index what [`Self::addressing`] says this container is
    /// addressed by: messages for GRIB, compared by the names their
    /// parameter tables give them, and the [`Self::variables`] list for
    /// NetCDF and Zarr, compared by each variable's CF `standard_name`. The
    /// rule, and every table it covers, is [`fieldglass_core::vector_pair`]'s.
    ///
    /// [`VectorPairing::Unknown`] when a field has no name to read and its
    /// partner does not settle it: a GRIB parameter no table names (the
    /// `Parameter 0/2/250` [`Self::message`] shows is a stand-in, not a
    /// name), one its table names only with a placeholder such as
    /// `Experimental product`, a message with no parameter codes, or a
    /// variable with no `standard_name`. A variable's `long_name` is not
    /// read: it is free text, and two unrelated ones can read as a pair.
    ///
    /// Not a gate. `render::vector_polylines` draws a pair that is not one,
    /// and the answer here is what a host says beside it. It is about the
    /// parameters alone: two fields at different levels or times still pair.
    ///
    /// # Errors
    ///
    /// [`Error::NoSuchMessage`] when either index is past the end of the
    /// messages or the variables.
    pub fn vector_pairing(&self, u: u32, v: u32) -> Result<VectorPairing, Error> {
        match &self.reader {
            #[cfg(feature = "grib1")]
            Reader::Grib1(_) => self.message_vector_pairing(u, v),
            #[cfg(feature = "grib2")]
            Reader::Grib2(_) => self.message_vector_pairing(u, v),
            #[cfg(any(feature = "netcdf", feature = "zarr"))]
            Reader::Arrays(a) => vector_pairing_of(a.source.as_ref(), u, v),
        }
    }

    /// [`Self::vector_pairing`] for a message container.
    #[cfg(any(feature = "grib1", feature = "grib2"))]
    fn message_vector_pairing(&self, u: u32, v: u32) -> Result<VectorPairing, Error> {
        let (u, v) = (self.table_name(u)?, self.table_name(v)?);
        Ok(fieldglass_core::vector_pair::pairing_of_parameter_names(u, v).into())
    }

    /// The name message `index`'s parameter table gives its parameter, or
    /// `None` when no table names it or it has no parameter codes. Unlike
    /// [`MessageInfo::parameter`], never the `Parameter d/c/n` placeholder,
    /// and it builds no [`MessageInfo`].
    #[cfg(any(feature = "grib1", feature = "grib2"))]
    fn table_name(&self, index: u32) -> Result<Option<&'static str>, Error> {
        let i = self.check_index(index)?;
        let name = match &self.reader {
            #[cfg(feature = "grib1")]
            Reader::Grib1(r) => grib1_entry(&r.messages[i]).map(|p| p.name),
            #[cfg(feature = "grib2")]
            Reader::Grib2(r) => {
                let msg = &r.messages[i];
                msg.pds.common().and_then(|common| {
                    fieldglass_grib2::lookup_parameter(
                        msg.ids.originator(),
                        msg.is.discipline,
                        common.parameter_category,
                        common.parameter_number,
                    )
                    .map(|(_, long, _)| long)
                })
            }
            #[cfg(any(feature = "netcdf", feature = "zarr"))]
            Reader::Arrays(_) => None,
        };
        Ok(name.filter(|n| !n.is_empty()))
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
    /// time step and labels it the last. A slice across an axis of length 0,
    /// drawn or held (an unlimited `time` with no records), has no cells and is
    /// [`Error::Unsupported`] naming that axis, rather than a field with no
    /// pixels to paint (#1425).
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
                    return Err(Error::invalid_option(format!(
                        "`{}` has {} dimensions, so `slice_indices` needs {} entries \
                             (the two horizontal ones are ignored); {} were given",
                        var.name,
                        var.dims.len(),
                        var.dims.len(),
                        fixed.len()
                    )));
                }
                if y == x || y >= var.dims.len() || x >= var.dims.len() {
                    return Err(Error::invalid_option(format!(
                        "`{}` has {} dimensions; y_dim {y} and x_dim {x} must be \
                             different and within them",
                        var.name,
                        var.dims.len()
                    )));
                }
                check_slice_indices(&var.name, &var.dims, y, x, slice_indices)?;
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
                let values = CfUnpacking::from_attributes(attributes).apply_owned(plane);
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
                    Some(var.name.clone()),
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
    /// that fetches only the chunks the line crosses, and on a NetCDF file only
    /// the line's bytes or the chunks it crosses (#939).
    ///
    /// # Errors
    ///
    /// [`Error::WrongAddressing`] for a message stream, which has no axes to read
    /// along; [`Error::NoSuchMessage`] for a variable index outside
    /// [`variables`](Self::variables); [`Error::InvalidOption`] for an axis
    /// outside the variable's rank, an `indices` of the wrong length, or an index
    /// past the end of its axis; [`Error::Unsupported`] for a held axis of
    /// length 0, which has no index to hold (#1425); and a decode failure from
    /// the container.
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

    /// One line through a combined map (#1208): line `a` combined point by
    /// point with line `b` under `op`, both along the same `along_dim`.
    ///
    /// Each request is what [`Session::decode_line`] takes. With `hold_b`
    /// false the line runs along an axis of the map and `b` is read along it
    /// too; with `hold_b` true it runs along a held axis (a time or a level)
    /// and `b` is its one cell at its own position, combined with every step
    /// of `a`. See `line_through_combined` for the rule and what the line is
    /// labeled with (named in prose: it needs `netcdf` or `zarr` as well, and
    /// this method is on every build with `analysis`).
    ///
    /// # Errors
    ///
    /// [`Error::InvalidOption`] when the two requests read along different
    /// axes, [`Error::WrongAddressing`] for a message stream, and otherwise
    /// what [`Session::decode_line`] and `line_through_combined` report.
    #[cfg(feature = "analysis")]
    pub fn decode_line_combined(
        &self,
        a: &LineRequest,
        b: &LineRequest,
        hold_b: bool,
        op: CombineOp,
        options: &DecodeOptions,
    ) -> Result<Line, Error> {
        #[cfg(not(any(feature = "netcdf", feature = "zarr")))]
        let _ = (a, b, hold_b, op, options);
        if a.along_dim != b.along_dim {
            return Err(Error::invalid_option(format!(
                "the two lines of a combined line run along the same axis; `a` reads along \
                 dimension {}, `b` along dimension {}",
                a.along_dim, b.along_dim
            )));
        }
        match &self.reader {
            #[cfg(feature = "grib1")]
            Reader::Grib1(_) => Err(wrong_addressing(
                Addressing::Messages,
                "decode_line_combined",
                "decode",
            )),
            #[cfg(feature = "grib2")]
            Reader::Grib2(_) => Err(wrong_addressing(
                Addressing::Messages,
                "decode_line_combined",
                "decode",
            )),
            #[cfg(any(feature = "netcdf", feature = "zarr"))]
            Reader::Arrays(arrays) => {
                let source = arrays.source.as_ref();
                let vars = renderable_arrays(source.group());
                let name = |variable: u32| {
                    vars.get(variable as usize).map(|v| v.name.as_str()).ok_or(
                        Error::NoSuchMessage {
                            index: variable,
                            count: u32::try_from(vars.len()).unwrap_or(u32::MAX),
                        },
                    )
                };
                line_through_combined(
                    source,
                    (name(a.variable)?, &a.slice_indices),
                    (name(b.variable)?, &b.slice_indices),
                    a.along_dim,
                    hold_b,
                    op,
                    options,
                )
            }
        }
    }

    /// The coordinate values along one axis of a variable, for labeling a plot
    /// of it (#171).
    ///
    /// `variable` is a position in [`Session::variables`] and `dim` a position
    /// in that variable's own axes. See [`axis_values`] for what is read and
    /// what an absent coordinate array means.
    ///
    /// # Errors
    ///
    /// [`Error::WrongAddressing`] for a message-addressed file, and whatever
    /// [`axis_values`] reports.
    #[cfg(any(feature = "netcdf", feature = "zarr"))]
    pub fn axis_values(&self, variable: u32, dim: u32) -> Result<AxisValues, Error> {
        match &self.reader {
            #[cfg(feature = "grib1")]
            Reader::Grib1(_) => Err(wrong_addressing(
                Addressing::Messages,
                "axis_values",
                "message",
            )),
            #[cfg(feature = "grib2")]
            Reader::Grib2(_) => Err(wrong_addressing(
                Addressing::Messages,
                "axis_values",
                "message",
            )),
            Reader::Arrays(a) => {
                let source = a.source.as_ref();
                let vars = renderable_arrays(source.group());
                let var = vars.get(variable as usize).ok_or(Error::NoSuchMessage {
                    index: variable,
                    count: u32::try_from(vars.len()).unwrap_or(u32::MAX),
                })?;
                axis_values(source, &var.name, dim)
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
    /// the values in them — and decoding a slice is the expensive half: on a
    /// NetCDF file or a Zarr store it is every chunk the slice covers.
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
                    return Err(Error::invalid_option(format!(
                        "`{}` has {} dimensions; y_dim {y} and x_dim {x} must be \
                             different and within them",
                        var.name,
                        var.dims.len()
                    )));
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
    /// grid nothing can place a point on, and its values land on the synthesized
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
            // The grid a synthesized family lands on, asked of the format crate
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
                return Ok(synthesized_georef(grid));
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
                    Ok(Georef::from_container(
                        &GridGeometry::from(gds),
                        grib1_scan(msg),
                        &gds.grid_type_name(),
                        gds.raster_bounds(),
                        grib1_without_geometry(gds),
                        gds.dimensions(),
                    )
                    .with_points_per_row(gds.points_per_row()))
                }
                #[cfg(feature = "grib2")]
                Reader::Grib2(r) => {
                    let msg = &r.messages[i];
                    Ok(Georef::from_container(
                        &GridGeometry::from(&msg.gds),
                        grib2_scan(msg),
                        &msg.gds.template_name(),
                        msg.gds.raster_bounds(),
                        grib2_without_geometry(&msg.gds),
                        msg.gds.dimensions(),
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
    /// one (#465), and the source `ni × nj` otherwise. That default needs a
    /// field holding one value per cell of its stated shape; one that does not
    /// is refused as [`Error::InvalidOption`] (#913).
    ///
    /// A grid nothing places — one whose numbers built no geometry, a slice no
    /// coordinates place, a planar grid with degenerate parameters — is
    /// refused as [`Error::Unsupported`] with the message the render path
    /// gives it, whether or not [`WarpOptions::bounds`] names a window. A
    /// window says where to look, not where the grid is (#843).
    #[cfg(feature = "render")]
    pub fn warp(&self, field: &Field, options: &WarpOptions) -> Result<Warped, Error> {
        warp_field(field, options)
    }

    /// [`warp`](Self::warp) for a field decoded to draw (#1082), on the grid it
    /// was decoded onto — the derived one, above reduction zero.
    ///
    /// The output is display values, as `warp`'s always are, so nothing here
    /// needs the full field: the window and the raster size are the caller's,
    /// and the cells are sampled from the coarse grid by its own geometry.
    ///
    /// # Errors
    ///
    /// As [`warp`](Self::warp).
    #[cfg(feature = "render")]
    pub fn warp_display(
        &self,
        field: &DisplayField,
        options: &WarpOptions,
    ) -> Result<Warped, Error> {
        warp_cells(&Cells::from(field), options)
    }

    /// The color decision, as data (ADR-0006 decision 3). The CPU painter
    /// reads the same value, so it is the oracle a GPU path is checked against
    /// rather than a second color implementation.
    ///
    /// The domain is the range [`crate::color_range()`] chooses. A field with
    /// no present cell and an automatic range has none, and is not refused
    /// (#1155): the palette spans a placeholder domain no cell reads, since
    /// every cell of such a field is masked. [`Session::render`] reports that
    /// as a `null` [`ColorRange`].
    ///
    /// # Errors
    ///
    /// [`Error::InvalidOption`] for an unknown colormap or scale, and for a
    /// range the rule refuses, with the refusal's fields on it.
    #[cfg(feature = "render")]
    pub fn palette(&self, field: &Field, options: &PaletteOptions) -> Result<Palette, Error> {
        build_palette(field.stats.min.zip(field.stats.max), options).map(|(palette, _)| palette)
    }

    /// [`palette`](Self::palette) for a field decoded to draw (#1082), with its
    /// automatic range taken from **the full field's** statistics, which the
    /// caller supplies, never from the display field's own values.
    ///
    /// A wavelet low-pass trims some of a field's extremes and overshoots
    /// others, so a coarse field's own range is not the file's, and two viewers
    /// at different reductions would get different colors for the same
    /// message. `full` is therefore the
    /// [`Stats`] of the same message decoded with [`decode`](Self::decode), held
    /// from an earlier full decode (the rule #880 settled: the automatic range
    /// is the whole decoded field's). A display cell outside it takes the end's
    /// color, as any out-of-range cell does.
    ///
    /// `full` may be `None` when [`PaletteOptions::min`] and
    /// [`PaletteOptions::max`] are both given, since the range needs no data
    /// then.
    ///
    /// # Errors
    ///
    /// As [`palette`](Self::palette), and [`Error::InvalidOption`] when an end
    /// is left to the data and `full` is `None`, or states no range while the
    /// display field has present cells, or is not a range (an end missing, not
    /// finite, or the minimum above the maximum).
    #[cfg(feature = "render")]
    pub fn palette_display(
        &self,
        field: &DisplayField,
        full: Option<&Stats>,
        options: &PaletteOptions,
    ) -> Result<Palette, Error> {
        let extremes =
            crate::full_field_extremes(full, field.mask.contains(&1), options.min, options.max)?;
        build_palette(extremes, options).map(|(palette, _)| palette)
    }

    /// The range [`palette`](Self::palette) spans for the same `options`, with
    /// the cells of `field` counted against it (#1156).
    ///
    /// It is the [`Raster::range`] [`render`](Self::render) reports for the
    /// same field and options, counts included, without painting: a GPU host
    /// colors from the palette and labels its legend from this. The counts
    /// cost one pass over the field.
    ///
    /// # Errors
    ///
    /// As [`palette`](Self::palette), with the same `RangeRefusal`.
    #[cfg(feature = "render")]
    pub fn palette_range(
        &self,
        field: &Field,
        options: &PaletteOptions,
    ) -> Result<ColorRange, Error> {
        self.palette_with_range(field, options)
            .map(|(_, range)| range)
    }

    /// [`palette_range`](Self::palette_range) for a field decoded to draw: the
    /// range [`palette_display`](Self::palette_display) spans, from the full
    /// field's `full` statistics, with the display field's cells counted. It
    /// is the range [`render_display`](Self::render_display) reports.
    ///
    /// # Errors
    ///
    /// As [`palette_display`](Self::palette_display).
    #[cfg(feature = "render")]
    pub fn palette_display_range(
        &self,
        field: &DisplayField,
        full: Option<&Stats>,
        options: &PaletteOptions,
    ) -> Result<ColorRange, Error> {
        self.palette_display_with_range(field, full, options)
            .map(|(_, range)| range)
    }

    /// [`palette`](Self::palette) and [`palette_range`](Self::palette_range) in
    /// one call: the palette and the range it spans, with the cells of
    /// `field` counted against it (#1196).
    ///
    /// The palette is built once. Calling the two separately builds it twice
    /// (the colormap resolved, a `colormap_table` text parsed, the lookup
    /// table filled), since the count needs one too. This is what a browser
    /// host binds, so it is the call the perf harness measures. The pair equals
    /// what the two separate calls return.
    ///
    /// # Errors
    ///
    /// As [`palette_range`](Self::palette_range): the refusals of
    /// [`palette`](Self::palette), and a field whose values are not one per
    /// cell.
    #[cfg(feature = "render")]
    pub fn palette_with_range(
        &self,
        field: &Field,
        options: &PaletteOptions,
    ) -> Result<(Palette, ColorRange), Error> {
        count_range(
            &Cells::from(field),
            field.stats.min.zip(field.stats.max),
            options,
        )
    }

    /// [`palette_with_range`](Self::palette_with_range) for a field decoded to
    /// draw: [`palette_display`](Self::palette_display) and
    /// [`palette_display_range`](Self::palette_display_range) from one build.
    ///
    /// # Errors
    ///
    /// As [`palette_display`](Self::palette_display), and a field whose values
    /// are not one per cell.
    #[cfg(feature = "render")]
    pub fn palette_display_with_range(
        &self,
        field: &DisplayField,
        full: Option<&Stats>,
        options: &PaletteOptions,
    ) -> Result<(Palette, ColorRange), Error> {
        let extremes =
            crate::full_field_extremes(full, field.mask.contains(&1), options.min, options.max)?;
        count_range(&Cells::from(field), extremes, options)
    }

    /// Paint a field to RGBA on the CPU. The fallback path: a GPU host colors
    /// from [`Session::palette`] instead. The raster is the field's `ni × nj`,
    /// so a field not holding one value per cell is refused (#913), and so is
    /// one with no cells, as [`Error::Unsupported`]: a raster with no pixels is
    /// one a browser's `ImageData` throws on (#1425).
    ///
    /// **`flip_y` composes with the field's own row order, it does not replace
    /// it.** Grid point `(i, j)` paints at pixel `(i, j)`, so a field
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
        paint_cells(
            &Cells::from(field),
            field.stats.min.zip(field.stats.max),
            options,
            flip_y,
        )
    }

    /// [`render`](Self::render) for a field decoded to draw (#1082): its own
    /// `ni × nj` raster, colored over the range
    /// [`palette_display`](Self::palette_display) chooses from the full field's
    /// `full` statistics, so a coarse map and the full one color a value alike.
    /// [`Raster::range`] counts the display field's cells.
    ///
    /// # Errors
    ///
    /// As [`render`](Self::render), and as
    /// [`palette_display`](Self::palette_display) for `full`.
    #[cfg(feature = "render")]
    pub fn render_display(
        &self,
        field: &DisplayField,
        full: Option<&Stats>,
        options: &PaletteOptions,
        flip_y: bool,
    ) -> Result<Raster, Error> {
        let extremes =
            crate::full_field_extremes(full, field.mask.contains(&1), options.min, options.max)?;
        paint_cells(&Cells::from(field), extremes, options, flip_y)
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
        // In `u64`: `Field` is public, so a caller can hand back one whose
        // `ni·nj` passes a 32-bit `usize` (#902). An index `usize` cannot
        // hold is past the end of any slice.
        let flat =
            usize::try_from((j as u64) * u64::from(field.ni) + i as u64).unwrap_or(usize::MAX);
        let present = field.mask.get(flat).copied().unwrap_or(0) == 1;
        Some(Probe {
            lat,
            lon,
            i: index.i,
            j: index.j,
            value: present.then(|| field.values.get(flat)).flatten(),
        })
    }

    /// Probe message `index` at one geographic point: the value the decoded
    /// field shows there, and — for a spectral message whose map is
    /// band-limited — the file's full-detail value at the same cell (#637).
    ///
    /// The first is [`probe`](Self::probe) of the decoded field, so it matches
    /// the color under the cursor. The second is
    /// [`probe_full_detail`](Self::probe_full_detail), set only when
    /// [`Field::truncation`](crate::Field::truncation) is: the map then shows
    /// the field band-limited to what its grid carries, and this is the full
    /// sum over every wavenumber the message holds at the node the first value
    /// was read at. For every other message the answer is the ordinary probe
    /// with no second value.
    ///
    /// Decodes the message, so it costs a decode per call; a host that already
    /// holds the field should call [`probe`](Self::probe) and
    /// [`probe_full_detail`](Self::probe_full_detail) itself.
    ///
    /// # Errors
    ///
    /// [`Error::WrongAddressing`] for a variable container, whose question is a
    /// slice; [`Error::NoSuchMessage`] past the end; and whatever decoding the
    /// message raises.
    pub fn probe_message(
        &self,
        index: u32,
        lat: f64,
        lon: f64,
    ) -> Result<Option<MessageProbe>, Error> {
        // Asked first, so a variable container is refused by name rather than
        // by `decode`'s own refusal, which names a different call.
        let full_detail = self.probe_full_detail(index, lat, lon)?;
        let field = self.decode(index, &DecodeOptions::new(Dtype::Auto))?;
        Ok(self.probe(&field, lat, lon).map(|p| MessageProbe {
            lat: p.lat,
            lon: p.lon,
            i: p.i,
            j: p.j,
            value: p.value,
            full_detail,
        }))
    }

    /// The full-detail value of message `index` at the cell a probe at
    /// `(lat, lon)` lands on, or `None` when the message's decoded field
    /// already holds the file's values there (#637).
    ///
    /// Only a spectral message whose map is band-limited
    /// ([`Field::truncation`](crate::Field::truncation) is set) answers: its
    /// decoded field is the smoothed one, so a value read off it is not the
    /// file's. This evaluates the full spherical-harmonic sum over every
    /// wavenumber the message holds, at the synthesis-grid node
    /// [`probe`](Self::probe) would read — the cell's own point, not the raw
    /// click, which is the rule every other field's probe follows — so the two
    /// values describe the same cell and differ only in the detail they carry.
    /// `None` too when the point is off the grid.
    ///
    /// Decodes the coefficients each call, then `(T+1)(T+2)/2` terms: about a
    /// fifth of a second at T7999 on top of the decode.
    ///
    /// # Errors
    ///
    /// [`Error::WrongAddressing`] for a variable container; [`Error::NoSuchMessage`]
    /// past the end; and whatever decoding the coefficients raises.
    pub fn probe_full_detail(
        &self,
        index: u32,
        lat: f64,
        lon: f64,
    ) -> Result<Option<FullDetail>, Error> {
        #[cfg(any(feature = "netcdf", feature = "zarr"))]
        if matches!(self.reader, Reader::Arrays(_)) {
            return Err(wrong_addressing(
                Addressing::Variables,
                "probe_full_detail",
                "decode_slice",
            ));
        }
        #[cfg(any(feature = "grib1", feature = "grib2"))]
        {
            let i = self.check_index(index)?;
            let grid = match &self.reader {
                #[cfg(feature = "grib1")]
                Reader::Grib1(r) => r.synthesis_grid(i),
                #[cfg(feature = "grib2")]
                Reader::Grib2(r) => r.synthesis_grid(i),
                #[cfg(any(feature = "netcdf", feature = "zarr"))]
                Reader::Arrays(_) => None,
            };
            let (Some(grid), Some(truncation)) = (grid, self.message_truncation(i)) else {
                // The decoded field holds the file's values already.
                return Ok(None);
            };
            // The cell `probe` would pick on the decoded field, by the same
            // rule, on the same geometry `decode` gave it.
            let geometry = GridGeometry::LatLon(grid.into());
            let Some(at) = geometry.inverse(lat, lon) else {
                return Ok(None);
            };
            let (lats, lons) = grid.axes();
            // `f64::clamp` panics when its bounds cross, which an empty axis
            // would make them do; the synthesis grid is never empty, but this
            // is the same guard `probe` keeps.
            if lats.is_empty() || lons.is_empty() {
                return Ok(None);
            }
            let col = at.i.round().clamp(0.0, (lons.len() - 1) as f64) as usize;
            let row = at.j.round().clamp(0.0, (lats.len() - 1) as f64) as usize;
            let value = match &self.reader {
                #[cfg(feature = "grib1")]
                Reader::Grib1(r) => r.evaluate_spectral_point(i, lats[row], lons[col])?,
                #[cfg(feature = "grib2")]
                Reader::Grib2(r) => r.evaluate_spectral_point(i, lats[row], lons[col])?,
                #[cfg(any(feature = "netcdf", feature = "zarr"))]
                Reader::Arrays(_) => return Ok(None),
            };
            Ok(Some(FullDetail { value, truncation }))
        }
        #[cfg(not(any(feature = "grib1", feature = "grib2")))]
        {
            let _ = (index, lat, lon);
            Err(wrong_addressing(
                Addressing::Variables,
                "probe_full_detail",
                "decode_slice",
            ))
        }
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
    /// The vertices are grid indices, not pixels: index `(i, j)` is the middle
    /// of the cell `render` paints as pixel `(i, j)`, so a host drawing them on
    /// that picture adds half a pixel (`(i + 0.5, j + 0.5)`) and mirrors the row
    /// (`y = nj - j - 0.5`) when `render` flipped the raster. [`Isoline`] has
    /// the detail. The Node addon's `projectContours` does both for its caller.
    ///
    /// `levels` empty asks for [`crate::AUTO_CONTOUR_LEVELS`] nice levels
    /// spanning the color range `options` chooses: the same range
    /// `Session::render` colors over for the same `min`, `max`, `center`
    /// and `scale`, so a manual range spaces the levels in this host as it
    /// does in the Node addon's contours (#1155).
    ///
    /// # Errors
    ///
    /// [`Error::InvalidOption`] for a range the render would refuse, with the
    /// same `RangeRefusal`, whether or not `levels` is empty.
    #[cfg(feature = "analysis")]
    pub fn contours(
        &self,
        field: &Field,
        levels: &[f64],
        options: &ContourOptions,
    ) -> Result<Vec<Isoline>, Error> {
        let range = crate::color_range(
            field.stats.min.zip(field.stats.max),
            options.min,
            options.max,
            options.center,
            crate::color_range::scale_is_log10(options.scale.as_deref())?,
        )?;
        let chosen: Vec<f64> = if levels.is_empty() {
            match range.used() {
                Some((min, max)) => nice_levels(min, max, crate::AUTO_CONTOUR_LEVELS),
                None => Vec::new(),
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

/// Refuse a field whose values are not one per point of its stated
/// `ni × nj` grid (#913), before an output of that shape is allocated.
/// `Field`'s members are public, so a caller can hand back one stating
/// 70,000 × 70,000 with four values, and a raster that size is gigabytes of
/// nothing.
#[cfg(feature = "render")]
fn require_field_shape(field: &Cells<'_>) -> Result<(), Error> {
    let cells = usize::try_from(u64::from(field.ni) * u64::from(field.nj)).ok();
    if cells == Some(field.values.len()) {
        return Ok(());
    }
    Err(Error::invalid_option(format!(
        "a {}×{} grid has {} cells, and {} values were given",
        field.ni,
        field.nj,
        cells.map_or_else(|| "too many".to_string(), |n| n.to_string()),
        field.values.len()
    )))
}

/// What the drawing calls read of a field: its values, mask, shape and
/// placement, borrowed from a [`Field`] or a [`DisplayField`] (#1082).
///
/// So `render` / `render_display` and `warp` / `warp_display` are one
/// implementation each, and the two field types differ only in where a color
/// range comes from, which is the one thing they must differ in. Private: the
/// public surface keeps the two types apart, and this is how the drawing half
/// treats them alike without letting a display field reach anything that
/// measures.
#[cfg(feature = "render")]
struct Cells<'a> {
    values: &'a Values,
    mask: &'a [u8],
    ni: u32,
    nj: u32,
    georef: &'a Georef,
}

#[cfg(feature = "render")]
impl<'a> From<&'a Field> for Cells<'a> {
    fn from(f: &'a Field) -> Self {
        Self {
            values: &f.values,
            mask: &f.mask,
            ni: f.ni,
            nj: f.nj,
            georef: &f.georef,
        }
    }
}

#[cfg(feature = "render")]
impl<'a> From<&'a DisplayField> for Cells<'a> {
    fn from(f: &'a DisplayField) -> Self {
        Self {
            values: &f.values,
            mask: &f.mask,
            ni: f.ni,
            nj: f.nj,
            georef: &f.georef,
        }
    }
}

#[cfg(feature = "render")]
fn warp_field(field: &Field, options: &WarpOptions) -> Result<Warped, Error> {
    warp_cells(&Cells::from(field), options)
}

#[cfg(feature = "render")]
fn warp_cells(field: &Cells<'_>, options: &WarpOptions) -> Result<Warped, Error> {
    let geometry = &field.georef.geometry;
    // Refused before the window is resolved, so a bad size is reported as a bad
    // size rather than being masked by a grid that also states no extent.
    let size = crate::render::resolve_output_size(options.width, options.height)?;
    // The render path's own gate, before the window, so a grid nothing places
    // is refused the same way with bounds or without. With bounds it used to
    // warp through an inverse map that answers `None` everywhere and hand back
    // a raster with every cell masked, saying nothing (#843). The family is
    // the geometry's label, which is what `Source::refused_as` quotes for the
    // one arm that reads it.
    crate::render::require_reprojectable(geometry, geometry.label())?;
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
        return Err(Error::invalid_option(format!(
            "the warp window {window:?} encloses no area"
        )));
    }

    // Sampled in place rather than through `optional_values`: that shape is
    // 16 bytes a cell, so a 3.7-million-point NBM field would cost 60 MB of
    // linear memory on top of the field it already holds, for no gain here.
    // Indexed in `u64`, as `render`'s warp is (#897, #902): a caller can hand
    // back a `Field` whose `ni·nj` passes a 32-bit `usize`.
    let ni = u64::from(field.ni);
    let sample = |i: usize, j: usize| -> Option<f64> {
        let k = usize::try_from((j as u64) * ni + i as u64).ok()?;
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
    // That default is the field's stated shape, so it must be the shape of
    // the values the field holds (#913). A named raster samples by index and
    // needs no such check. The default also gets the allocation check a named
    // raster gets: a field of 32-bit values can hold more cells than a raster
    // of `f64` fits in on wasm32.
    if size.is_none() {
        require_field_shape(field)?;
        if !crate::render::raster_is_allocatable(field.ni, field.nj, isize::MAX as u64) {
            return Err(Error::invalid_option(format!(
                "a {}×{} field warps to a raster larger than this target can allocate; \
                     name a smaller width and height",
                field.ni, field.nj
            )));
        }
    }
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

// How many times this thread has run `build_palette`, for the tests that hold
// a combined call to one build (#1196).
#[cfg(all(test, feature = "render"))]
thread_local! {
    static PALETTE_BUILDS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// The palette for a field whose present-cell range is `extremes`, under
/// `options`, and the range it was built over.
///
/// The range is [`crate::color_range()`] over those extremes (a field's own
/// statistics, or for a display field the full field's), so this host and the
/// addon choose it by one rule (#1155). A field with no present cell and an
/// automatic range has no range: the palette is built over a placeholder
/// domain no cell reads, every cell paints transparent, and the range reports
/// `null` ends rather than an error (#880 item 1).
#[cfg(feature = "render")]
fn build_palette(
    extremes: Option<(f64, f64)>,
    options: &PaletteOptions,
) -> Result<(Palette, ColorRange), Error> {
    #[cfg(test)]
    PALETTE_BUILDS.with(|n| n.set(n.get() + 1));
    let colormap = crate::render::resolve_colormap(
        options.colormap_table.as_deref(),
        options.colormap.as_deref(),
    )?;
    let scale = crate::render::parse_scale(options.scale.as_deref())?;
    let range = crate::color_range(
        extremes,
        options.min,
        options.max,
        options.center,
        scale == ScaleMode::Log10,
    )?;
    let (min, max) = range.paint_domain();
    Ok((
        Palette::build(&colormap, options.reversed, min, max, scale),
        range,
    ))
}

/// Paint `field`'s own `ni × nj` raster over the range `extremes` gives:
/// [`Session::render`] and [`Session::render_display`].
#[cfg(feature = "render")]
fn paint_cells(
    field: &Cells<'_>,
    extremes: Option<(f64, f64)>,
    options: &PaletteOptions,
    flip_y: bool,
) -> Result<Raster, Error> {
    // The raster is the field's stated shape, so that has to be the shape
    // of the values it holds (#913).
    require_field_shape(field)?;
    // Nor is a raster with no pixels handed back: a browser's `ImageData`
    // throws on a zero side (#1425). A slice across an empty axis is refused
    // before this, by name; this is for a field that came any other way.
    if field.ni == 0 || field.nj == 0 {
        return Err(Error::Unsupported {
            detail: format!(
                "nothing to draw: the field is {} columns by {} rows",
                field.ni, field.nj
            ),
        });
    }
    let (palette, range) = build_palette(extremes, options)?;
    let values = field.values.to_f64();
    let flip = field.georef.scan.flips_source_rows(flip_y);
    let (rgba, counts) = palette.paint_counted(&values, Some(field.mask), field.ni, field.nj, flip);
    // `paint` answers an empty buffer for a raster whose byte count this
    // target cannot address (`usize` is 32 bits on wasm32, the host this
    // exists for). Say so, rather than handing back dimensions with no
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
        range: range.counted(&counts, u64::from(field.ni) * u64::from(field.nj)),
    })
}

/// The palette and the range [`paint_cells`] reports for the same cells and
/// options, counted without painting: [`Session::palette_with_range`],
/// [`Session::palette_range`] and their display twins (#1156, #1196). The
/// palette is built here once, for the count and for the caller.
///
/// Refuses a field whose values are not one per cell, as `paint_cells` does,
/// so the two never disagree about what `total` counts.
#[cfg(feature = "render")]
fn count_range(
    field: &Cells<'_>,
    extremes: Option<(f64, f64)>,
    options: &PaletteOptions,
) -> Result<(Palette, ColorRange), Error> {
    require_field_shape(field)?;
    let (palette, range) = build_palette(extremes, options)?;
    // Widened as it goes: a palette is the cheap call, and a copy of the field
    // to count it would be the largest allocation it makes.
    let mask = Some(field.mask);
    let counts = match field.values {
        Values::F32(v) => palette.count(v.iter().map(|&x| f64::from(x)), mask),
        Values::F64(v) => palette.count(v.iter().copied(), mask),
    };
    let range = range.counted(&counts, u64::from(field.ni) * u64::from(field.nj));
    Ok((palette, range))
}

// ---------------------------------------------------------------------------
// Per-format metadata
// ---------------------------------------------------------------------------

#[cfg(feature = "grib1")]
fn grib1_scan(msg: &fieldglass_grib1::Grib1Message) -> Scan {
    match msg.gds.as_ref().and_then(|gds| gds.scanning_mode()) {
        Some(m) => decoded_scan(m.i_negative, m.j_positive),
        None => Scan::north_down(),
    }
}

#[cfg(feature = "grib2")]
fn grib2_scan(msg: &fieldglass_grib2::Grib2Message) -> Scan {
    match msg.gds.scanning_mode() {
        Some(sm) => decoded_scan(sm & 0x80 != 0, sm & 0x40 != 0),
        None => Scan::north_down(),
    }
}

/// The scan of the raster a GRIB reader's `decode_message_raster` returns,
/// from the two direction bits the message states.
///
/// `j_consecutive` is always `false` because that raster is always row-major,
/// whatever bit 3 (`0x20`) says. On a regular grid the reader transposes a
/// column-major message (`fieldglass_core::transpose_j_consecutive`); on a
/// quasi-regular grid it ignores the bit, as eccodes 2.34.1 does, and walks
/// rows; and a message with no rows and columns has no raster to order. Both
/// editions go through here so they cannot disagree.
///
/// Reporting the stored bit instead told a consumer to transpose values that
/// were already row-major, and made `combine::aligned` refuse a column-major
/// message against a row-major one on the same grid (#792). The two direction
/// bits are kept: no reader reorders the raster to undo them, so they still
/// describe `values`.
#[cfg(any(feature = "grib1", feature = "grib2"))]
fn decoded_scan(i_negative: bool, j_positive: bool) -> Scan {
    Scan::new(i_negative, j_positive, false)
}

/// `(abbreviation, name, units)` for one message, each `None` where the
/// message or its table states none (#775).
#[cfg(any(feature = "grib1", feature = "grib2"))]
type ParameterNames = (Option<String>, Option<String>, Option<String>);

/// A resolved parameter's table entry, as [`ParameterNames`].
///
/// The tables encode "no short name" and "no units" as an empty string, which
/// is how a dimensionless quantity or an entry with no short name is written in
/// them. That is the table's encoding, not a display placeholder, so it is read
/// once here rather than handed to hosts. Units are normalized first, so a
/// spelling the normalizer empties is `None` too.
#[cfg(any(feature = "grib1", feature = "grib2"))]
fn stated(abbreviation: &str, name: &str, units: &str) -> ParameterNames {
    let some = |s: &str| (!s.is_empty()).then(|| s.to_string());
    let units = normalize_table_units(units);
    (
        some(abbreviation),
        some(name),
        (!units.is_empty()).then(|| units.into_owned()),
    )
}

/// `(abbreviation, name, units)` for one GRIB1 message.
///
/// Split out of [`grib1_message`] so [`Session::decode`] does not build a whole
/// `MessageInfo` for the two strings it needs: that would build the `Georef`
/// too, and a projected family's `lonlat_bbox` walks its perimeter 512 times
/// per edge.
#[cfg(feature = "grib1")]
fn grib1_parameter(msg: &fieldglass_grib1::Grib1Message) -> ParameterNames {
    match grib1_entry(msg) {
        // Units are normalized at the display seam, the same way the napi host
        // does it: the ECMWF local tables are generated from eccodes'
        // Fortran-style exponents and ON388 chains solidi, so the raw strings
        // disagree about the same unit.
        Some(param) => stated(param.abbreviation, param.name, param.units),
        // The format crate owns the fallback rendering, so this seam and the
        // napi one cannot disagree about it (#633).
        None => (
            None,
            Some(fieldglass_grib1::tables::unresolved_parameter(
                msg.pds.originating_centre,
                msg.pds.table_version,
                msg.pds.parameter_id,
            )),
            None,
        ),
    }
}

/// One GRIB1 message's parameter table entry, `None` when no table names it.
#[cfg(feature = "grib1")]
fn grib1_entry(
    msg: &fieldglass_grib1::Grib1Message,
) -> Option<fieldglass_grib1::tables::ParameterEntry> {
    fieldglass_grib1::tables::lookup_parameter(
        msg.pds.parameter_id,
        msg.pds.table_version,
        msg.pds.originating_centre,
        msg.pds.sub_centre,
    )
}

/// Where a synthesized family's values land, as
/// [`Session::place_message`] reports it and [`MessageInfo::placement`] reads
/// it.
///
/// The same three the synthesized arm of `decode` builds: nothing of the source
/// layout survives an inverse transform, so the scan is north-down and the
/// family is the geometry's own. A synthesized raster is built here rather than
/// stated by the file, so its corners are the geometry's — there is no
/// container value to prefer.
#[cfg(any(feature = "grib1", feature = "grib2"))]
fn synthesized_georef(grid: fieldglass_core::GlobalGrid) -> Georef {
    let geometry = GridGeometry::LatLon(grid.into());
    let declared = geometry.label().to_string();
    Georef::from_declared(&geometry, Scan::north_down(), &declared)
}

/// `(placement, reprojectable, supports)` of the grid a message's values land
/// on — the [`MessageInfo`] answers a message list reads (#776, #945).
///
/// The synthesis grid when the family has one, the declared grid otherwise, and
/// `without_grid` with no grid at all. The declared grid is the right answer for
/// every message that is not synthesized: [`Session::place_message`] builds the
/// same geometry from the same section and differs only in which corner pair it
/// reports, which neither answer reads.
#[cfg(any(feature = "grib1", feature = "grib2"))]
fn landing(
    synthesis: Option<fieldglass_core::GlobalGrid>,
    declared: Option<&Georef>,
    without_grid: Placement,
) -> (Placement, bool, GridSupport) {
    if let Some(grid) = synthesis {
        let placed = synthesized_georef(grid);
        return (placed.placement, placed.reprojectable, placed.supports);
    }
    declared.map_or((without_grid, false, GridSupport::default()), |g| {
        (g.placement, g.reprojectable, g.supports)
    })
}

/// What a GRIB1 grid the geometry has no points for is (#776).
///
/// A data representation type this parser does not model still declares a grid
/// (`unsupported`); one it models with no raster (spherical harmonics) has
/// nothing to place; one it models with a raster the geometry cannot build has
/// a raster nothing places. The format crate's own answers, read rather than
/// re-derived: `GridDescription::Unsupported` is its "not modeled", and
/// `dimensions` its "has rows and columns".
#[cfg(feature = "grib1")]
fn grib1_without_geometry(gds: &fieldglass_grib1::gds::GridDescription) -> Placement {
    if matches!(
        gds,
        fieldglass_grib1::gds::GridDescription::Unsupported { .. }
    ) {
        return Placement::Unsupported;
    }
    raster_without_geometry(gds.dimensions())
}

/// The GRIB2 half of [`grib1_without_geometry`]. A §3 template this build does
/// not model is `unsupported` when the section states grid points at all
/// (octets 7–10, present in every template); a modeled one with a raster the
/// geometry cannot build — a §3.90 camera that sees no Earth — is
/// `unplaceable`; spectral, bi-Fourier and HEALPix have no rows and columns.
#[cfg(feature = "grib2")]
fn grib2_without_geometry(gds: &fieldglass_grib2::gds::GridDefinitionSection) -> Placement {
    if matches!(
        gds.template,
        fieldglass_grib2::gds::GridTemplate::Unsupported(_)
    ) {
        return if gds.num_data_points > 0 {
            Placement::Unsupported
        } else {
            Placement::NoRaster
        };
    }
    raster_without_geometry(gds.dimensions())
}

/// A modeled template whose geometry has no points: a raster nothing places
/// when the section states rows and columns, and nothing to place otherwise.
#[cfg(any(feature = "grib1", feature = "grib2"))]
fn raster_without_geometry(dims: Option<(u32, u32)>) -> Placement {
    match dims {
        Some((ni, nj)) if ni > 0 && nj > 0 => Placement::Unplaceable,
        _ => Placement::NoRaster,
    }
}

/// A sub-centre's name from WMO Common Code Table C-12, keyed on the pair.
///
/// `None` only for code 0, which GRIB writes to mean "no sub-centre". Any other
/// code the table does not assign under this center keeps its number, as
/// `"Sub-centre <n>"`, so "none" and "one we cannot name" never read alike
/// (#774). C-12 defines no missing-value code for the field, so there is no
/// `"Missing"` here: 255 (GRIB1) and 65535 (GRIB2) are codes like any other.
#[cfg(any(feature = "grib1", feature = "grib2"))]
fn sub_centre_name(centre: u16, sub_centre: u16) -> Option<String> {
    (sub_centre != 0).then(|| {
        fieldglass_core::center_tables::lookup_sub_centre(centre, sub_centre)
            .map_or_else(|| format!("Sub-centre {sub_centre}"), str::to_string)
    })
}

/// A GRIB2 code table's name for a code, or `"<what> <code>"` where the table
/// has none — unassigned, or in a range WMO delegates to centres — so the code
/// survives into the string (#774), as `originating_centre`'s `"Centre <n>"`
/// does. `"Missing"` is the table's own name for its missing code.
#[cfg(feature = "grib2")]
fn named(name: Option<&'static str>, what: &str, code: u8) -> String {
    name.map_or_else(|| format!("{what} {code}"), str::to_string)
}

#[cfg(feature = "grib1")]
fn grib1_message(reader: &fieldglass_grib1::Grib1Reader<Bytes>, index: usize) -> MessageInfo {
    let msg = &reader.messages[index];
    let grid = msg.gds.as_ref().map(|gds| {
        // `bounds`, not `raster_bounds`: this is what the message *declares*,
        // and a reduced grid declares the corner of the grid it really is.
        Georef::from_container(
            &GridGeometry::from(gds),
            grib1_scan(msg),
            &gds.grid_type_name(),
            gds.bounds(),
            grib1_without_geometry(gds),
            gds.dimensions(),
        )
        .with_points_per_row(gds.points_per_row())
    });
    let (abbreviation, parameter, units) = grib1_parameter(msg);
    // No grid description and no catalog entry: the message either names a
    // predefined grid this build does not have (ON388 Table B), or names none —
    // 255 is Table B's "no predefined grid".
    let without_grid = if msg.pds.grid_number == 255 {
        Placement::NoRaster
    } else {
        Placement::PredefinedUnresolved
    };
    let (placement, reprojectable, supports) =
        landing(reader.synthesis_grid(index), grid.as_ref(), without_grid);
    MessageInfo {
        // Round-trips the `u32` handle `Session::message` was given and
        // `check_index` widened, so it cannot be a narrowing in practice.
        index: index as u32,
        offset_bytes: msg.byte_offset,
        parameter,
        abbreviation,
        units,
        level: fieldglass_grib1::level_value_str(&msg.pds),
        level_type: Some(fieldglass_grib1::level_type_str(&msg.pds)),
        reference_time: fieldglass_grib1::reference_time(&msg.pds),
        forecast: Some(fieldglass_grib1::forecast_display(&msg.pds)),
        // `None` when the data section's header could not be read (#775).
        packing: reader.packing_label(index).map(str::to_string),
        size_label: msg.gds.as_ref().and_then(|g| g.size_label()),
        truncation: reader.synthesis_truncation(index).map(Into::into),
        // A centre's PDS extension is kept as octets, not read (#994).
        ensemble_member: None,
        grid,
        placement,
        reprojectable,
        supports,
        forecast_hours: fieldglass_grib1::forecast_hours(&msg.pds),
        uv_relative_to_grid: msg.gds.as_ref().and_then(|g| g.uv_relative_to_grid()),
        originating_centre: fieldglass_grib1::tables_cct::lookup_centre(msg.pds.originating_centre)
            .map(str::to_string)
            .unwrap_or_else(|| format!("Centre {}", msg.pds.originating_centre)),
        sub_centre: sub_centre_name(msg.pds.originating_centre.into(), msg.pds.sub_centre.into()),
        // GRIB1 has no discipline, production status or data type: these are
        // §0 and §1 fields that arrived with edition 2, so its variant does not
        // carry them (#773).
        identification: Identification::Grib1 {
            // Time range 10 spends `P1` as the high octet of a two-octet value,
            // so reporting it as a lead time there would be reporting half a
            // number.
            p1_octet: (msg.pds.time_range != 10).then_some(i32::from(msg.pds.p1)),
        },
        fingerprint: grib1_fingerprint(reader, index),
        total_length_bytes: u64::from(msg.is.total_length),
    }
}

/// The GRIB1 half of [`MessageFingerprint`] (#959): the PDS and GDS codes as
/// written, and the centre's PDS extension as octets (#994).
#[cfg(feature = "grib1")]
fn grib1_fingerprint(
    reader: &fieldglass_grib1::Grib1Reader<Bytes>,
    index: usize,
) -> MessageFingerprint {
    use std::fmt::Write as _;
    let msg = &reader.messages[index];
    let pds = &msg.pds;
    // The reader holds the whole file in memory and the scan bounded the PDS
    // to the message, so this read cannot fail. Should it, the extension is
    // left out, as the packing label is, rather than failing the listing; that
    // reads as a 28-octet PDS, so two such messages could match.
    let extension = reader.pds_extension(index).ok();
    // FM 92 Code Table 2: table versions from 128, and parameters from 128 in
    // any version, are the originating centre's own, so the same code means
    // another parameter at another centre.
    let local = pds.table_version >= 128 || pds.parameter_id >= 128;
    let dimensions = msg.gds.as_ref().and_then(|gds| match gds {
        fieldglass_grib1::GridDescription::SphericalHarmonic(sh) => {
            Some([u32::from(sh.j), u32::from(sh.k)])
        }
        _ => gds.dimensions().map(|(ni, nj)| [ni, nj]),
    });
    MessageFingerprint::Grib1 {
        table_version: pds.table_version,
        parameter: pds.parameter_id,
        local_centre: local.then_some(pds.originating_centre),
        level_type: pds.level_type,
        level_octets: [pds.level_value_1, pds.level_value_2],
        reference_time: fieldglass_grib1::reference_time(pds),
        time_unit: pds.time_unit,
        p1: pds.p1,
        p2: pds.p2,
        time_range: pds.time_range,
        extension_octets: extension.filter(|octets| !octets.is_empty()).map(|octets| {
            octets
                .iter()
                .fold(String::with_capacity(2 * octets.len()), |mut hex, b| {
                    // Writing to a `String` cannot fail.
                    let _ = write!(hex, "{b:02x}");
                    hex
                })
        }),
        grid_type: msg
            .gds
            .as_ref()
            .map(fieldglass_grib1::GridDescription::grid_type),
        grid_number: msg.gds.is_none().then_some(pds.grid_number),
        dimensions,
    }
}

/// The GRIB2 half of [`MessageFingerprint`] (#959): the §0, §1, §3 and §4 codes
/// as written.
#[cfg(feature = "grib2")]
fn grib2_fingerprint(msg: &fieldglass_grib2::Grib2Message) -> MessageFingerprint {
    let common = msg.pds.common();
    let surface = |s: &fieldglass_grib2::pds::FixedSurface| crate::api::SurfaceCodes {
        surface_type: s.surface_type,
        scale_factor: s.scale_factor,
        scaled_value: s.scaled_value,
    };
    let stats = msg.pds.stats();
    // The ensemble block (4.1) and the derived-forecast block (4.2, 4.12) are
    // read into named fields below, and their counts are not part of the
    // identity, so neither reaches `product_octets` (#959). A template the
    // reader could not parse keeps every octet, whatever its number. A
    // chemistry or aerosol template keeps its constituent type and size and
    // wavelength ranges there as written, which tell two of its fields apart;
    // its ensemble block is the named fields, as 4.1's is (#1130).
    let unread = match &msg.pds.template {
        fieldglass_grib2::ProductTemplate::SharedLayout(t) if matches!(t.number, 1 | 2 | 12) => {
            &[][..]
        }
        fieldglass_grib2::ProductTemplate::Constituent(t) => &t.qualifier[..],
        _ => msg.pds.unread_octets(),
    };
    let ensemble = msg.pds.ensemble_member();
    // Code Tables 4.1 and 4.2 reserve 192-254 for local use.
    let local = common.is_some_and(|c| {
        (192..=254).contains(&c.parameter_category) || (192..=254).contains(&c.parameter_number)
    });
    MessageFingerprint::Grib2 {
        discipline: msg.is.discipline,
        product_template: msg.pds.template_number,
        product_octets: (!unread.is_empty())
            .then(|| unread.iter().map(|b| format!("{b:02x}")).collect()),
        ensemble_type: ensemble.map(|(kind, _)| kind),
        ensemble_member: ensemble.map(|(_, member)| member),
        derived_forecast: msg.pds.derived_forecast(),
        local_centre: local.then_some(msg.ids.centre),
        parameter_category: common.map(|c| c.parameter_category),
        parameter_number: common.map(|c| c.parameter_number),
        first_surface: common.map(|c| surface(&c.first_surface)),
        second_surface: common.map(|c| surface(&c.second_surface)),
        reference_time: msg.ids.reference_time_iso8601(),
        forecast_time_unit: common.map(|c| c.forecast_time_unit),
        forecast_time: common.map(|c| c.forecast_time),
        range_end: stats.map(fieldglass_grib2::pds::StatisticalProcessing::end_time_iso8601),
        statistics: stats
            .map(|st| {
                st.specs
                    .iter()
                    .map(|spec| crate::api::StatisticalStep {
                        process: spec.stat_process,
                        length_unit: spec.stat_length_unit,
                        length: spec.stat_length,
                    })
                    .collect()
            })
            .unwrap_or_default(),
        grid_template: msg.gds.template_number,
        dimensions: msg.gds.dimensions().map(|(ni, nj)| [ni, nj]),
        data_points: msg.gds.num_data_points,
    }
}

/// `(abbreviation, name, units)` for one GRIB2 message. Split out for the
/// reason [`grib1_parameter`] is.
#[cfg(feature = "grib2")]
fn grib2_parameter(msg: &fieldglass_grib2::Grib2Message) -> ParameterNames {
    let discipline = msg.is.discipline;
    // A template with no horizontal product common carries no category and no
    // number, so there is nothing to name and nothing to report as unresolved
    // either — every field is `None` (#775). Only a message that *has* the
    // codes and finds no table for them gets the fallback (#633).
    let Some(common) = msg.pds.common() else {
        return (None, None, None);
    };
    match fieldglass_grib2::lookup_parameter(
        msg.ids.originator(),
        discipline,
        common.parameter_category,
        common.parameter_number,
    ) {
        Some((abbr, long, units)) => stated(abbr, long, units),
        // The format crate owns the fallback rendering, so this seam and the
        // napi one cannot disagree about it (#633).
        None => (
            None,
            Some(fieldglass_grib2::unresolved_parameter(
                discipline,
                common.parameter_category,
                common.parameter_number,
            )),
            None,
        ),
    }
}

#[cfg(feature = "grib2")]
fn grib2_message(reader: &fieldglass_grib2::Grib2Reader<Bytes>, index: usize) -> MessageInfo {
    let msg = &reader.messages[index];
    let common = msg.pds.common();
    let (abbreviation, parameter, units) = grib2_parameter(msg);
    // The format crate owns every display rule (#545); a template with no
    // horizontal product common has none of the three fields to render, and
    // says so with `None` rather than a placeholder of the library's choosing
    // (#775).
    let (level, level_type) = match common {
        Some(c) => (
            fieldglass_grib2::level_value_str(c),
            Some(fieldglass_grib2::level_type_str(c)),
        ),
        None => (None, None),
    };
    let grid = Georef::from_container(
        &GridGeometry::from(&msg.gds),
        grib2_scan(msg),
        &msg.gds.template_name(),
        msg.gds.bounds(),
        grib2_without_geometry(&msg.gds),
        msg.gds.dimensions(),
    )
    .with_points_per_row(msg.gds.points_per_row());
    // Every GRIB2 message carries a §3, so the no-grid answer is unreachable;
    // `NoRaster` is what it would be.
    let (placement, reprojectable, supports) = landing(
        reader.synthesis_grid(index),
        Some(&grid),
        Placement::NoRaster,
    );
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
        reference_time: msg.ids.reference_time_iso8601(),
        forecast: common.map(fieldglass_grib2::forecast_display),
        packing: Some(msg.drs.template_name()),
        grid: Some(grid),
        placement,
        reprojectable,
        supports,
        size_label: msg.gds.size_label(),
        truncation: reader.synthesis_truncation(index).map(Into::into),
        // What the identity's `ensemble_member` is read from, so the two agree.
        ensemble_member: msg.pds.ensemble_member().map(|(_, member)| member),
        forecast_hours: common.and_then(fieldglass_grib2::forecast_hours),
        uv_relative_to_grid: msg.gds.uv_relative_to_grid(),
        originating_centre: fieldglass_grib2::tables_cct::lookup_centre(msg.ids.centre)
            .map(str::to_string)
            .unwrap_or_else(|| format!("Centre {}", msg.ids.centre)),
        sub_centre: sub_centre_name(msg.ids.centre, msg.ids.sub_centre),
        // The reader only yields messages whose §0 octet 8 is 2.
        identification: Identification::Grib2 {
            discipline: named(
                fieldglass_grib2::lookup_discipline(msg.is.discipline),
                "Discipline",
                msg.is.discipline,
            ),
            production_status: named(
                fieldglass_grib2::lookup_production_status(msg.ids.production_status),
                "Production status",
                msg.ids.production_status,
            ),
            data_type: named(
                fieldglass_grib2::lookup_data_type(msg.ids.data_type),
                "Data type",
                msg.ids.data_type,
            ),
        },
        fingerprint: grib2_fingerprint(msg),
        total_length_bytes: msg.is.total_length,
    }
}

#[cfg(all(test, feature = "grib2", feature = "render", feature = "analysis"))]
mod tests {
    use super::*;
    use fieldglass_core::{LatLonParams, RotatedLatLonParams, projection::GridGeometry};

    /// `palette_with_range` and its display twin build the palette once each,
    /// where calling the separate methods builds it twice (#1196).
    #[cfg(all(feature = "render", feature = "grib2"))]
    #[test]
    fn a_palette_with_its_range_builds_the_palette_once() {
        fn builds(f: impl FnOnce()) -> u32 {
            let before = PALETTE_BUILDS.with(std::cell::Cell::get);
            f();
            PALETTE_BUILDS.with(std::cell::Cell::get) - before
        }
        // The palette methods read only the field, not the reader behind the
        // session, so a small field built in memory is enough.
        if crate::test_fixtures::outside_workspace() {
            return;
        }
        let geometry = GridGeometry::LatLon(LatLonParams {
            ni: 2,
            nj: 2,
            lat_first: 90.0,
            lon_first: 0.0,
            lat_last: -90.0,
            lon_last: 180.0,
        });
        let georef = Georef::from_geometry(&geometry, Scan::north_down());
        let field = Field {
            values: Values::F64(vec![1.0, 2.0, 3.0, 4.0]),
            mask: vec![1; 4],
            ni: 2,
            nj: 2,
            georef: georef.clone(),
            truncation: None,
            stats: Stats {
                min: Some(1.0),
                max: Some(4.0),
                valid_count: 4,
            },
            parameter: None,
            units: None,
        };
        let display = DisplayField {
            values: Values::F64(vec![1.0, 2.0, 3.0, 4.0]),
            mask: vec![1; 4],
            ni: 2,
            nj: 2,
            georef,
            reduction: 1,
            truncation: None,
            parameter: None,
            units: None,
        };
        let session = grib2_session();
        let options = PaletteOptions::default();

        assert_eq!(
            builds(|| {
                session.palette_with_range(&field, &options).expect("both");
            }),
            1
        );
        assert_eq!(
            builds(|| {
                session.palette(&field, &options).expect("a palette");
                session.palette_range(&field, &options).expect("a range");
            }),
            2,
            "the separate calls are the cost the combined one removes"
        );
        assert_eq!(
            builds(|| {
                session
                    .palette_display_with_range(&display, Some(&field.stats), &options)
                    .expect("both");
            }),
            1
        );
    }

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
            truncation: None,
            stats: Stats {
                min: None,
                max: None,
                valid_count: 0,
            },
            parameter: None,
            units: None,
        };
        // The session is irrelevant to `probe`; it reads only the field.
        if !crate::test_fixtures::outside_workspace() {
            assert!(grib2_session().probe(&field, 0.0, 0.0).is_none());
        }

        // The hazard the guard exists for, stated rather than assumed.
        assert!(
            std::panic::catch_unwind(|| 0.0_f64.clamp(0.0, -1.0)).is_err(),
            "f64::clamp is expected to panic on crossed bounds; if it ever \
             saturates instead, the guard above is redundant"
        );
    }

    /// A session over an arbitrary fixture, for the operations that read only
    /// the `Field` handed to them and never the reader behind it. The fixture
    /// is `fieldglass-grib2`'s, so a caller first asks
    /// [`outside_workspace`](crate::test_fixtures::outside_workspace).
    fn grib2_session() -> Session {
        Session {
            reader: Reader::Grib2(Box::new(
                fieldglass_grib2::Grib2Reader::from_source(Box::new(crate::test_fixtures::sibling(
                    "../fieldglass-grib2/tests/fixtures/gfs_c255_latlon.grib2",
                )) as Bytes)
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
            truncation: None,
            stats: Stats {
                min: Some(0.0),
                max: Some(15.0),
                valid_count: 128,
            },
            parameter: None,
            units: None,
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
        if crate::test_fixtures::outside_workspace() {
            return;
        }
        let session = grib2_session();
        let levels = [4.5, 9.5];
        let bounded: usize = session
            .contours(&field, &levels, &ContourOptions::default())
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
    const SWATH: &str = "../fieldglass-netcdf/tests/fixtures/mirs_swath_n21.nc";

    /// A real tripolar ocean mesh, whose fields carry a third axis — so a
    /// cross-section through one is a slice its 2-D coordinates do not span.
    #[cfg(feature = "netcdf")]
    const TRIPOLAR: &str = "../fieldglass-netcdf/tests/fixtures/rtofs_tripolar_arctic.nc";

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
        if crate::test_fixtures::outside_workspace() {
            return;
        }
        let session = Session::open(crate::test_fixtures::sibling(SWATH)).expect("the swath opens");
        let vars = session.variables();
        assert_eq!(vars.len(), 4, "the fixture's four swath variables");

        for (i, v) in vars.iter().enumerate() {
            let (Some(y), Some(x)) = (v.detected_y_dim, v.detected_x_dim) else {
                panic!("{}: a swath variable has detected axes", v.name);
            };
            let placed = session
                .place_slice(i as u32, y, x)
                .unwrap_or_else(|e| panic!("{}: {e}", v.name));
            // Placed by a coordinate lookup, and captioned by the name a user
            // knows it by (#808).
            assert_eq!(placed.family(), "curvilinear", "{} is curvilinear", v.name);
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
        if crate::test_fixtures::outside_workspace() {
            return;
        }
        let session =
            Session::open(crate::test_fixtures::sibling(TRIPOLAR)).expect("the mesh opens");
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
        assert_eq!(image.family(), "curvilinear");

        // The same array, cut the other way: the third axis against X.
        let cross = session
            .place_slice(index, other, x)
            .expect("the cross-section places");
        assert_ne!(
            cross.family(),
            "curvilinear",
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
    /// distinguishes "memoized" from "rebuilt and happened to agree", and it
    /// does it without timing anything.
    #[cfg(feature = "netcdf")]
    #[test]
    fn the_second_ask_borrows_the_first_answer() {
        if crate::test_fixtures::outside_workspace() {
            return;
        }
        let session = Session::open(crate::test_fixtures::sibling(SWATH)).expect("the swath opens");
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

    /// A dimensionless parameter reads the same whichever table resolved it
    /// (#873). One real entry per "no unit" spelling, from every table that
    /// uses it, all reaching a host as `None`, including one quantity that
    /// three tables spell two ways (the rime factor).
    #[cfg(all(feature = "grib1", feature = "grib2"))]
    #[test]
    fn every_table_spelling_of_no_unit_reads_as_none() {
        use fieldglass_grib2::Originator;

        // (table version, centre, parameter, spelling): NCEP's ON388 Table 2
        // and an ECMWF local table.
        for (table_version, centre, id, spelling) in [
            (2, 7, 252, "non-dim"),          // Drag coefficient
            (2, 7, 191, "numeric"),          // Probability from ensemble
            (2, 7, 21, "-"),                 // Radar spectra (1)
            (140, 98, 233, "dimensionless"), // Coefficient of drag with waves
        ] {
            let param = fieldglass_grib1::tables::lookup_parameter(id, table_version, centre, 0)
                .expect("a real GRIB1 entry");
            assert_eq!(param.units, spelling, "GRIB1 {centre}/{table_version}/{id}");
            let (_, _, units) = stated(param.abbreviation, param.name, param.units);
            assert_eq!(
                units, None,
                "GRIB1 {centre}/{table_version}/{id} {spelling:?}"
            );
        }

        // (centre, discipline, category, number, spelling): WMO, NCEP, DWD
        // and ECMWF.
        for (centre, d, c, n, spelling) in [
            (0, 0, 2, 29, "Numeric"),        // WMO: Drag coefficient
            (0, 0, 1, 44, "Numeric"),        // WMO: Rime factor
            (0, 0, 22, 0, "dimensionless"),  // WMO: Standard Precipitation Index
            (7, 0, 1, 203, "non-dim"),       // NCEP: Rime factor
            (7, 0, 1, 192, "-"),             // NCEP: Categorical rain
            (78, 0, 1, 44, "Numeric"),       // DWD: Rime factor
            (98, 0, 22, 0, "dimensionless"), // ECMWF: Standard Precipitation Index
        ] {
            let originator = Originator {
                centre,
                sub_centre: 0,
                local_tables_version: 0,
            };
            let (abbreviation, name, table_units) =
                fieldglass_grib2::lookup_parameter(originator, d, c, n)
                    .expect("a real GRIB2 entry");
            assert_eq!(table_units, spelling, "GRIB2 {centre} {d}.{c}.{n}");
            let (_, _, units) = stated(abbreviation, name, table_units);
            assert_eq!(units, None, "GRIB2 {centre} {d}.{c}.{n} {spelling:?}");
        }
    }
}

/// The refusal a build gets for a container it can recognize but not decode.
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

// A lat/lon box planned into the chunks of a Zarr store it touches, and decoded
// from them (#1083). A child module so it reaches the arrays arm without
// widening it, and written below everything else so the rest of this file is
// unchanged by it: a build without `zarr`, which is what the browser bundle is,
// compiles exactly what it did before.
#[cfg(feature = "zarr")]
mod window;
#[cfg(feature = "zarr")]
pub use window::{
    ChunkRef, IndexLocation, InnerChunkRef, ShardIndexRef, WindowPlan, WindowRequest,
};

/// The container [`Arrays`] holds. With `zarr` it is the array seam plus the
/// one question a window plan asks of a store and not of a file, which objects
/// a region read fetches (`window::Keyed`); without it, the seam alone.
#[cfg(feature = "zarr")]
type Source = dyn window::Keyed;
#[cfg(all(feature = "netcdf", not(feature = "zarr")))]
type Source = dyn ArraySource;
