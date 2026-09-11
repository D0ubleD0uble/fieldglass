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
#[cfg(any(feature = "grib1", feature = "grib2"))]
use fieldglass_core::units::normalize_units;
use fieldglass_core::{Format as CoreFormat, GridGeometry, detect_from_bytes};
#[cfg(feature = "render")]
use fieldglass_core::{
    LonLatBox, Resampling, SourceGrid, TargetRaster,
    colormap::{Colormap, Palette, ScaleMode, default_colormap},
    warp,
};
#[cfg(feature = "analysis")]
use fieldglass_core::{contour_segments, contour_segments_global, nice_levels};

#[cfg(feature = "analysis")]
use crate::api::Isoline;
#[cfg(feature = "render")]
use crate::api::Warped;
use crate::api::{
    Addressing, DimensionInfo, Dtype, Field, Georef, MessageInfo, Probe, Scan, SourceFormat, Stats,
    Values, VariableInfo,
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
#[derive(Debug)]
enum Reader {
    #[cfg(feature = "grib1")]
    Grib1(Box<fieldglass_grib1::Grib1Reader>),
    #[cfg(feature = "grib2")]
    Grib2(Box<fieldglass_grib2::Grib2Reader>),
    /// An array dataset. The `DatasetView` is resolved once on open rather than
    /// per call: for a NetCDF-4 backing it walks the whole object model, and
    /// every variable and slice question is asked of it afterwards.
    #[cfg(feature = "netcdf")]
    Netcdf(
        Box<(
            fieldglass_netcdf::NetcdfReader,
            fieldglass_netcdf::DatasetView,
        )>,
    ),
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
    let mut values = Vec::with_capacity(raw.len());
    let mut mask = Vec::with_capacity(raw.len());
    let mut min = f64::INFINITY;
    let mut max = f64::NEG_INFINITY;
    let mut valid_count = 0u32;
    for cell in raw {
        match cell {
            // A non-finite decoded value is not a value: it cannot be ranged,
            // coloured, or interpolated, so it joins the masked cells rather
            // than poisoning the field's min / max.
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
    Field {
        values: Values::build(values, &mask, options.dtype.clone()),
        mask,
        ni,
        nj,
        georef: Georef::from_declared(geometry, scan, declared),
        stats,
        parameter,
        units,
    }
}

/// The refusal a message call gets from a variable dataset, and vice versa.
///
/// One place so the two halves cannot word it differently, and so the `detail`
/// always names the call to make instead — an error that only says "no" costs
/// the caller a trip to the docs.
fn wrong_addressing(called: &str, instead: &str) -> Error {
    Error::WrongAddressing {
        expected: "variables".to_string(),
        detail: format!("`{called}` addresses messages; call `{instead}` instead"),
    }
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
            CoreFormat::Grib1 => {
                Reader::Grib1(Box::new(fieldglass_grib1::Grib1Reader::from_bytes(bytes)?))
            }
            #[cfg(not(feature = "grib1"))]
            CoreFormat::Grib1 => {
                return Err(Error::UnsupportedFormat {
                    detail: "GRIB1; this build was compiled without the `grib1` feature"
                        .to_string(),
                });
            }
            #[cfg(feature = "grib2")]
            CoreFormat::Grib2 => {
                Reader::Grib2(Box::new(fieldglass_grib2::Grib2Reader::from_bytes(bytes)?))
            }
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
                // Resolved here so a variable list costs one walk per file
                // rather than one per question.
                let view = reader.view()?;
                Reader::Netcdf(Box::new((reader, view)))
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

    /// Which container the bytes turned out to be. Detected at
    /// [`Session::open`], never re-sniffed.
    pub fn format(&self) -> SourceFormat {
        match self.reader {
            #[cfg(feature = "grib1")]
            Reader::Grib1(_) => SourceFormat::Grib1,
            #[cfg(feature = "grib2")]
            Reader::Grib2(_) => SourceFormat::Grib2,
            #[cfg(feature = "netcdf")]
            Reader::Netcdf(_) => SourceFormat::NetCdf,
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
            #[cfg(feature = "netcdf")]
            Reader::Netcdf(_) => Addressing::Variables,
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
            #[cfg(feature = "netcdf")]
            Reader::Netcdf(_) => 0,
        };
        // A file with more messages than a `u32` counts does not exist; the
        // saturating cast is here so the index type and the count type agree
        // rather than because the clamp is reachable.
        u32::try_from(n).unwrap_or(u32::MAX)
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
        #[cfg(feature = "netcdf")]
        if matches!(self.reader, Reader::Netcdf(_)) {
            return Err(wrong_addressing("message", "variables"));
        }
        #[cfg(any(feature = "grib1", feature = "grib2"))]
        {
            let i = self.check_index(index)?;
            Ok(match &self.reader {
                #[cfg(feature = "grib1")]
                Reader::Grib1(r) => grib1_message(r, i),
                #[cfg(feature = "grib2")]
                Reader::Grib2(r) => grib2_message(r, i),
                #[cfg(feature = "netcdf")]
                Reader::Netcdf(_) => return Err(wrong_addressing("message", "variables")),
            })
        }
        // A build with no GRIB decoder has no message path at all. Answering
        // rather than panicking: the guard above already returned for the only
        // reader such a build can hold, so this is unreachable in fact and
        // total in type.
        #[cfg(not(any(feature = "grib1", feature = "grib2")))]
        {
            let _ = index;
            Err(wrong_addressing("message", "variables"))
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
        #[cfg(feature = "netcdf")]
        if matches!(self.reader, Reader::Netcdf(_)) {
            return Err(wrong_addressing("decode", "decode_slice"));
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
                #[cfg(feature = "netcdf")]
                Reader::Netcdf(_) => return Err(wrong_addressing("decode", "decode_slice")),
            };
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
                #[cfg(feature = "netcdf")]
                Reader::Netcdf(_) => return Err(wrong_addressing("decode", "decode_slice")),
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
                    #[cfg(feature = "netcdf")]
                    Reader::Netcdf(_) => return Err(wrong_addressing("decode", "decode_slice")),
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
            Ok(build_field(
                &raw, ni, nj, &geometry, scan, &declared, parameter, units, options,
            ))
        }
        // As in `message`: with no GRIB decoder compiled there is no
        // message path, and the guard above has already answered.
        #[cfg(not(any(feature = "grib1", feature = "grib2")))]
        {
            let _ = (index, options);
            Err(wrong_addressing("decode", "decode_slice"))
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
            #[cfg(feature = "netcdf")]
            Reader::Netcdf(b) => {
                b.1.dims
                    .iter()
                    .map(|d| DimensionInfo {
                        name: d.name.clone(),
                        length: d.length,
                    })
                    .collect()
            }
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
            #[cfg(feature = "netcdf")]
            Reader::Netcdf(b) => {
                b.1.renderable_variables()
                    .into_iter()
                    .enumerate()
                    .map(|(i, v)| VariableInfo {
                        // Position in *this* list, not the decode index the crate
                        // uses: a host should never have to know that the reader
                        // numbers every dataset in the file while this offers only
                        // the renderable ones.
                        index: u32::try_from(i).unwrap_or(u32::MAX),
                        name: v.name.clone(),
                        dims: v
                            .dims
                            .iter()
                            .map(|d| DimensionInfo {
                                name: d.name.clone(),
                                length: d.length,
                            })
                            .collect(),
                        dtype: format!("{:?}", v.nc_type).to_lowercase(),
                        units: b
                            .1
                            .var(v.decode_index)
                            .and_then(|s| s.units())
                            .map(str::to_string)
                            .unwrap_or_default(),
                        detected_y_dim: v.detected_y_dim.and_then(|d| u32::try_from(d).ok()),
                        detected_x_dim: v.detected_x_dim.and_then(|d| u32::try_from(d).ok()),
                    })
                    .collect()
            }
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
    // Every parameter is read by the `netcdf` arm alone, so a GRIB-only build
    // sees a signature it cannot use. Kept in the signature regardless: the API
    // a host compiles against must not change shape with the feature set, or a
    // build without NetCDF would not be the same crate.
    #[cfg_attr(not(feature = "netcdf"), allow(unused_variables))]
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
            Reader::Grib1(_) => Err(wrong_addressing("decode_slice", "decode")),
            #[cfg(feature = "grib2")]
            Reader::Grib2(_) => Err(wrong_addressing("decode_slice", "decode")),
            #[cfg(feature = "netcdf")]
            Reader::Netcdf(b) => {
                let (reader, view) = (&b.0, &b.1);
                let vars = view.renderable_variables();
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
                let source = view
                    .vars
                    .iter()
                    .find(|s| s.decode_index == var.decode_index)
                    .ok_or_else(|| Error::Decode {
                        detail: format!("`{}` is not in the dataset view", var.name),
                    })?;
                // `decode_plane` is the whole chain — decode, extract the
                // plane, then the CF mask-and-scale — so a packed `int16`
                // variable arrives here already in physical units, the way a
                // GRIB field does. It must **not** be unpacked again: a second
                // `scale_factor` / `add_offset` pass computes
                // `(raw·s + o)·s + o`, which for the committed CF fixture turns
                // 250 K into 265.625 K and for a GOES or ERA5 archive is wrong
                // by about two orders of magnitude. Every number is finite and
                // plausible, so nothing downstream can tell.
                let values = reader.decode_plane(source, y, x, &fixed)?;
                let placement = reader.slice_placement(view, var, y, x)?;
                let ni = u32::try_from(var.dims[x].length).unwrap_or(u32::MAX);
                let nj = u32::try_from(var.dims[y].length).unwrap_or(u32::MAX);
                let units = source.units().map(str::to_string).unwrap_or_default();
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
    let colormap = match &options.colormap {
        Some(name) => Colormap::by_name(name).ok_or_else(|| Error::InvalidOption {
            detail: format!("no colormap named {name:?}"),
        })?,
        None => default_colormap(),
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
    Ok(Palette::build(colormap, options.reversed, min, max, scale))
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
fn grib1_message(reader: &fieldglass_grib1::Grib1Reader, index: usize) -> MessageInfo {
    let msg = &reader.messages[index];
    let grid = msg.gds.as_ref().map(|gds| {
        Georef::from_declared(
            &GridGeometry::from(gds),
            grib1_scan(msg),
            gds.grid_type_name(),
        )
    });
    let (abbreviation, parameter, units) = grib1_parameter(msg);
    MessageInfo {
        // Round-trips the `u32` handle `Session::message` was given and
        // `check_index` widened, so it cannot be a narrowing in practice.
        index: index as u32,
        offset_bytes: msg.byte_offset as u64,
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
fn grib2_message(reader: &fieldglass_grib2::Grib2Reader, index: usize) -> MessageInfo {
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
        offset_bytes: msg.byte_offset as u64,
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
        grid: Some(Georef::from_declared(
            &GridGeometry::from(&msg.gds),
            grib2_scan(msg),
            &msg.gds.template_name(),
        )),
        size_label: msg.gds.size_label(),
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
                fieldglass_grib2::Grib2Reader::from_bytes(
                    std::fs::read("../fieldglass-grib2/tests/fixtures/gfs_c255_latlon.grib2")
                        .expect("fixture"),
                )
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
