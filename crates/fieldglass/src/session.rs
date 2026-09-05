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

use fieldglass_core::{
    Format as CoreFormat, GridGeometry, LonLatBox, Resampling, SourceGrid, TargetRaster,
    colormap::{Colormap, Palette, ScaleMode, default_colormap},
    contour_segments, contour_segments_global, detect_from_bytes, nice_levels,
    units::normalize_units,
    warp,
};

use crate::api::{
    Dtype, Field, Georef, Isoline, MessageInfo, Probe, Scan, SourceFormat, Stats, Values, Warped,
};
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

/// How a field should be resampled onto a geographic box.
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
}

fn yes() -> bool {
    true
}

impl Default for WarpOptions {
    fn default() -> Self {
        Self {
            bilinear: true,
            bounds: None,
        }
    }
}

/// Colour, decided once in Rust and exported as data.
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

/// A painted raster: RGBA bytes plus the dimensions they cover.
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

#[derive(Debug)]
enum Reader {
    Grib1(Box<fieldglass_grib1::Grib1Reader>),
    Grib2(Box<fieldglass_grib2::Grib2Reader>),
}

impl Session {
    /// Open a container from its bytes.
    ///
    /// The format is detected from the bytes, never from a name: ADR-0005 hands
    /// the host's fetched buffer straight in, and a range-fetched GRIB message
    /// has no filename to guess from.
    pub fn open(bytes: Vec<u8>) -> Result<Self, Error> {
        let reader = match detect_from_bytes(&bytes) {
            CoreFormat::Grib1 => {
                Reader::Grib1(Box::new(fieldglass_grib1::Grib1Reader::from_bytes(bytes)?))
            }
            CoreFormat::Grib2 => {
                Reader::Grib2(Box::new(fieldglass_grib2::Grib2Reader::from_bytes(bytes)?))
            }
            CoreFormat::NetCdf => {
                return Err(Error::UnsupportedFormat {
                    detail: "NetCDF; this build carries the GRIB decoders only".to_string(),
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
            Reader::Grib1(_) => SourceFormat::Grib1,
            Reader::Grib2(_) => SourceFormat::Grib2,
        }
    }

    /// How many messages the container holds. Message indices run
    /// `0..count()`; anything outside is [`Error::NoSuchMessage`].
    pub fn count(&self) -> u32 {
        let n = match &self.reader {
            Reader::Grib1(r) => r.message_count(),
            Reader::Grib2(r) => r.message_count(),
        };
        // A file with more messages than a `u32` counts does not exist; the
        // saturating cast is here so the index type and the count type agree
        // rather than because the clamp is reachable.
        u32::try_from(n).unwrap_or(u32::MAX)
    }

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
        let i = self.check_index(index)?;
        Ok(match &self.reader {
            Reader::Grib1(r) => grib1_message(r, i),
            Reader::Grib2(r) => grib2_message(r, i),
        })
    }

    /// Decode one message into a field: values, mask, geometry, and the range
    /// a palette is built from.
    pub fn decode(&self, index: u32, options: &DecodeOptions) -> Result<Field, Error> {
        let i = self.check_index(index)?;
        let (raw, geometry, parameter, units) = match &self.reader {
            Reader::Grib1(r) => {
                let msg = &r.messages[i];
                let gds = msg.gds.as_ref().ok_or_else(|| Error::Unsupported {
                    detail: "the message carries no grid description".to_string(),
                })?;
                let geometry = GridGeometry::from(gds);
                let raw = r.decode_message_raster(i)?;
                let (parameter, units) = grib1_parameter(&r.messages[i]);
                (raw, geometry, parameter, units)
            }
            Reader::Grib2(r) => {
                let msg = &r.messages[i];
                let geometry = GridGeometry::from(&msg.gds);
                let raw = r.decode_message_raster(i)?;
                let (_, parameter, units) = grib2_parameter(msg);
                (raw, geometry, parameter, units)
            }
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

        let mut values = Vec::with_capacity(raw.len());
        let mut mask = Vec::with_capacity(raw.len());
        let mut min = f64::INFINITY;
        let mut max = f64::NEG_INFINITY;
        let mut valid_count = 0u32;
        for cell in &raw {
            match cell {
                // A non-finite decoded value is not a value: it cannot be
                // ranged, coloured, or interpolated, so it joins the masked
                // cells rather than poisoning the field's min / max.
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

        let scan = match &self.reader {
            Reader::Grib1(r) => grib1_scan(&r.messages[i]),
            Reader::Grib2(r) => grib2_scan(&r.messages[i]),
        };

        Ok(Field {
            values: Values::build(values, &mask, options.dtype.clone()),
            mask,
            ni,
            nj,
            georef: Georef::from_geometry(&geometry, scan),
            stats,
            parameter,
            units,
        })
    }

    /// Resample a field onto a geographic box, without painting it.
    ///
    /// This is the render pipeline split at the paint step: a GPU host wants
    /// the resampled *values*, so restyling never re-decodes. The output is the
    /// source `ni × nj` until #465 lets a caller size it.
    pub fn warp(&self, field: &Field, options: &WarpOptions) -> Result<Warped, Error> {
        warp_field(field, options)
    }

    /// The colour decision, as data (ADR-0006 decision 3). The CPU painter
    /// reads the same value, so it is the oracle a GPU path is checked against
    /// rather than a second colour implementation.
    pub fn palette(&self, field: &Field, options: &PaletteOptions) -> Result<Palette, Error> {
        build_palette(field, options)
    }

    /// Paint a field to RGBA on the CPU. The fallback path: a GPU host colours
    /// from [`Session::palette`] instead.
    pub fn render(
        &self,
        field: &Field,
        options: &PaletteOptions,
        flip_y: bool,
    ) -> Result<Raster, Error> {
        let palette = build_palette(field, options)?;
        let values = field.values.to_f64();
        let rgba = palette.paint(&values, Some(&field.mask), field.ni, field.nj, flip_y);
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

    /// Isolines through a field, in fractional grid coordinates.
    ///
    /// `levels` empty asks for a nice set spanning the field's own range.
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
        let raw = if field.georef.periodic_x {
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
fn optional_values(field: &Field) -> Vec<Option<f64>> {
    (0..field.mask.len())
        .map(|k| (field.mask[k] == 1).then(|| field.values.get(k)).flatten())
        .collect()
}

fn warp_field(field: &Field, options: &WarpOptions) -> Result<Warped, Error> {
    let geometry = &field.georef.geometry;
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
    let target = TargetRaster {
        width: field.ni,
        height: field.nj,
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

fn grib1_scan(msg: &fieldglass_grib1::Grib1Message) -> Scan {
    match &msg.gds {
        Some(gds) => scan_of_grib1(gds).unwrap_or_else(Scan::north_down),
        None => Scan::north_down(),
    }
}

fn scan_of_grib1(gds: &fieldglass_grib1::GridDescription) -> Option<Scan> {
    gds.scanning_mode()
        .map(|m| Scan::new(m.i_negative, m.j_positive, m.j_consecutive))
}

fn grib2_scan(msg: &fieldglass_grib2::Grib2Message) -> Scan {
    match msg.gds.scanning_mode() {
        Some(sm) => Scan::new(sm & 0x80 != 0, sm & 0x40 != 0, sm & 0x20 != 0),
        None => Scan::north_down(),
    }
}

/// `(name, units)` for one GRIB1 message.
///
/// Split out of [`grib1_message`] so [`Session::decode`] does not build a whole
/// `MessageInfo` for two strings: that would build the `Georef` too, and a
/// projected family's `lonlat_bbox` walks its perimeter 512 times per edge.
fn grib1_parameter(msg: &fieldglass_grib1::Grib1Message) -> (String, String) {
    let param = fieldglass_grib1::tables::lookup_parameter(
        msg.pds.parameter_id,
        msg.pds.table_version,
        msg.pds.originating_centre,
    );
    // Normalised at the display seam, the same way the napi host does it: the
    // ECMWF local tables are generated from eccodes' Fortran-style exponents
    // and ON388 chains solidi, so the raw strings disagree about the same unit.
    (
        param.name.to_string(),
        normalize_units(param.units).into_owned(),
    )
}

fn grib1_message(reader: &fieldglass_grib1::Grib1Reader, index: usize) -> MessageInfo {
    let msg = &reader.messages[index];
    let param = fieldglass_grib1::tables::lookup_parameter(
        msg.pds.parameter_id,
        msg.pds.table_version,
        msg.pds.originating_centre,
    );
    let grid = msg
        .gds
        .as_ref()
        .map(|gds| Georef::from_geometry(&GridGeometry::from(gds), grib1_scan(msg)));
    let (parameter, units) = grib1_parameter(msg);
    MessageInfo {
        // Round-trips the `u32` handle `Session::message` was given and
        // `check_index` widened, so it cannot be a narrowing in practice.
        index: index as u32,
        offset_bytes: msg.byte_offset as u64,
        parameter,
        abbreviation: param.abbreviation.to_string(),
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
fn grib2_parameter(msg: &fieldglass_grib2::Grib2Message) -> (String, String, String) {
    let discipline = msg.is.discipline;
    match msg.pds.common().and_then(|c| {
        fieldglass_grib2::lookup_parameter(
            msg.ids.originator(),
            discipline,
            c.parameter_category,
            c.parameter_number,
        )
    }) {
        Some((abbr, long, units)) => (
            abbr.to_string(),
            long.to_string(),
            normalize_units(units).into_owned(),
        ),
        None => (String::new(), String::new(), String::new()),
    }
}

fn grib2_message(reader: &fieldglass_grib2::Grib2Reader, index: usize) -> MessageInfo {
    let msg = &reader.messages[index];
    let common = msg.pds.common();
    let (abbreviation, parameter, units) = grib2_parameter(msg);
    let (level, level_type) = match common {
        Some(c) => {
            let surface = &c.first_surface;
            let label = fieldglass_grib2::lookup_fixed_surface(surface.surface_type).to_string();
            // A surface with no scaled value is named rather than numbered —
            // "Ground or water surface" has no height to print — and the WMO
            // missing sentinel is neither.
            let level = if surface.is_missing() {
                "—".to_string()
            } else {
                match surface.value() {
                    Some(v) => format!("{v}"),
                    None => label.clone(),
                }
            };
            (level, label)
        }
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
        // The producer's own unit, not hours: MRMS states its lead time in
        // minutes, and normalising to hours would report `0` for every step of
        // a nowcast series. `+30 Minute` is what the napi host shows too.
        forecast: common
            .map(|c| {
                format!(
                    "+{} {}",
                    c.forecast_time,
                    fieldglass_grib2::lookup_time_range_unit(c.forecast_time_unit)
                )
            })
            .unwrap_or_else(|| "—".to_string()),
        packing: msg.drs.template_name(),
        grid: Some(Georef::from_geometry(
            &GridGeometry::from(&msg.gds),
            grib2_scan(msg),
        )),
        size_label: msg.gds.size_label(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fieldglass_core::{LatLonParams, projection::GridGeometry};

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
        let session = Session {
            reader: Reader::Grib2(Box::new(
                fieldglass_grib2::Grib2Reader::from_bytes(
                    std::fs::read("../fieldglass-grib2/tests/fixtures/gfs_c255_latlon.grib2")
                        .expect("fixture"),
                )
                .expect("parse"),
            )),
        };
        assert!(session.probe(&field, 0.0, 0.0).is_none());

        // The hazard the guard exists for, stated rather than assumed.
        assert!(
            std::panic::catch_unwind(|| 0.0_f64.clamp(0.0, -1.0)).is_err(),
            "f64::clamp is expected to panic on crossed bounds; if it ever \
             saturates instead, the guard above is redundant"
        );
    }
}
