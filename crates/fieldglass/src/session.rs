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
    Format as CoreFormat, GridGeometry, Resampling, SourceGrid, TargetRaster,
    colormap::{Colormap, Palette, ScaleMode, default_colormap},
    contour_segments, contour_segments_global, detect_from_bytes, nice_levels,
    units::normalize_units,
    warp,
};

use crate::api::{
    ContourLevel, Dtype, Field, Format, Georef, MessageInfo, Probe, Scan, Stats, Values, Warped,
};
use crate::error::Error;

/// How a decode should be shaped.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct DecodeOptions {
    #[serde(default)]
    pub dtype: Dtype,
}

/// How a field should be resampled onto a geographic box.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
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
#[non_exhaustive]
pub struct PaletteOptions {
    /// A colormap name `core` knows. Unknown names are an error rather than a
    /// silent fallback: a host that misspells one should hear about it.
    #[serde(default)]
    pub colormap: Option<String>,
    #[serde(default)]
    pub reversed: bool,
    /// Display range. `None` takes the field's own min / max.
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
    /// `"linear"` (default) or `"log10"`.
    #[serde(default)]
    pub scale: Option<String>,
}

/// A painted raster: RGBA bytes plus the dimensions they cover.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub struct Raster {
    pub rgba: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

/// An open file.
pub struct Session {
    reader: Reader,
}

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

    pub fn format(&self) -> Format {
        match self.reader {
            Reader::Grib1(_) => Format::Grib1,
            Reader::Grib2(_) => Format::Grib2,
        }
    }

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
                let raw = grib1_decode_regular(r, i)?;
                let info = grib1_message(r, i);
                (raw, geometry, info.parameter, info.units)
            }
            Reader::Grib2(r) => {
                let msg = &r.messages[i];
                let geometry = GridGeometry::from(&msg.gds);
                let raw = grib2_decode_regular(r, i)?;
                let info = grib2_message(r, i);
                (raw, geometry, info.parameter, info.units)
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
        Ok(Raster {
            rgba: palette.paint(&values, Some(&field.mask), field.ni, field.nj, flip_y),
            width: field.ni,
            height: field.nj,
        })
    }

    /// Sample one geographic point out of a field.
    pub fn probe(&self, field: &Field, lat: f64, lon: f64) -> Option<Probe> {
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
    pub fn contours(&self, field: &Field, levels: &[f64]) -> Result<Vec<ContourLevel>, Error> {
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
            .map(|level| ContourLevel {
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
    let bounds = match options.bounds {
        Some(b) => b,
        None => {
            let (lat_min, lat_max, lon_min, lon_max) =
                geometry.lonlat_bbox().ok_or_else(|| Error::Unsupported {
                    detail: format!("a {} grid states no extent to warp onto", geometry.label()),
                })?;
            // A global grid's default window runs the full turn, not to its
            // last declared column. The gap between the last column and the
            // first belongs to the grid — the periodic sampler fills it — and
            // stopping at `lon_last` leaves the seam meridian as a stripe of
            // background one cell wide. `lonlat_bbox` reports the corners,
            // which is the right answer to a different question.
            let lon_max = if field.georef.periodic_x {
                lon_min + 360.0
            } else {
                lon_max
            };
            [lat_min, lat_max, lon_min, lon_max]
        }
    };
    let [lat_min, lat_max, lon_min, lon_max] = bounds;
    if !(lat_min.is_finite() && lat_max.is_finite() && lon_min.is_finite() && lon_max.is_finite())
        || lat_max <= lat_min
        || lon_max <= lon_min
    {
        return Err(Error::InvalidOption {
            detail: format!("the warp window {bounds:?} encloses no area"),
        });
    }

    let cells = optional_values(field);
    let ni = field.ni as usize;
    let sample =
        move |i: usize, j: usize| -> Option<f64> { cells.get(j * ni + i).copied().flatten() };
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
        bounds,
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
    Ok(Palette::build(colormap, options.reversed, min, max, scale))
}

// ---------------------------------------------------------------------------
// Per-format decode boundaries
// ---------------------------------------------------------------------------

/// GRIB1 decode with the reduced-row expansion applied.
///
/// The expansion lives here rather than in the reader for now, mirroring what
/// `fieldglass-napi` does; #543 moves it inside `decode_message_values`, and
/// this function disappears when it does.
fn grib1_decode_regular(
    reader: &fieldglass_grib1::Grib1Reader,
    index: usize,
) -> Result<Vec<Option<f64>>, Error> {
    let raw = reader.decode_message_values(index)?;
    let Some(gds) = reader.messages[index].gds.as_ref() else {
        return Ok(raw);
    };
    Ok(match (gds.points_per_row(), gds.dimensions()) {
        (Some(pl), Some((width, _))) => {
            fieldglass_core::expand_reduced_to_regular(&raw, pl, width as usize)
        }
        _ => raw,
    })
}

/// GRIB2 decode with the alternate-row scan undone and reduced rows expanded.
///
/// Both steps mirror `fieldglass-napi`'s single decode boundary. #541 moves the
/// alternate-row undo into `decode_message_values` and #543 the expansion;
/// until then a second host has to do the same two things or draw NBM's
/// Lambert fields with every other row reversed.
fn grib2_decode_regular(
    reader: &fieldglass_grib2::Grib2Reader,
    index: usize,
) -> Result<Vec<Option<f64>>, Error> {
    let mut raw = reader.decode_message_values(index)?;
    let gds = &reader.messages[index].gds;
    if let Some(sm) = gds.scanning_mode()
        && sm & fieldglass_grib2::SCAN_ALTERNATE_ROWS != 0
        && sm & fieldglass_grib2::SCAN_J_CONSECUTIVE == 0
    {
        match gds.points_per_row() {
            Some(pl) => fieldglass_grib2::undo_alternate_reduced_rows(&mut raw, pl),
            None => {
                if let Some((ni, _)) = gds.dimensions() {
                    fieldglass_grib2::undo_alternate_rows(&mut raw, ni as usize);
                }
            }
        }
    }
    if let (Some(pl), Some((width, _))) = (gds.points_per_row(), gds.dimensions()) {
        raw = fieldglass_core::expand_reduced_to_regular(&raw, pl, width as usize);
    }
    Ok(raw)
}

// ---------------------------------------------------------------------------
// Per-format metadata
// ---------------------------------------------------------------------------

fn grib1_scan(msg: &fieldglass_grib1::Grib1Message) -> Scan {
    match &msg.gds {
        Some(gds) => match scan_of_grib1(gds) {
            Some(s) => s,
            None => Scan::default_north_down(),
        },
        None => Scan::default_north_down(),
    }
}

fn scan_of_grib1(gds: &fieldglass_grib1::GridDescription) -> Option<Scan> {
    use fieldglass_grib1::GridDescription as G;
    let m = match gds {
        G::LatLon(g) => &g.scanning_mode,
        G::RotatedLatLon(g) => &g.scanning_mode,
        G::ReducedLatLon(g) => &g.scanning_mode,
        G::Gaussian(g) => &g.scanning_mode,
        G::ReducedGaussian(g) => &g.scanning_mode,
        G::PolarStereographic(g) => &g.scanning_mode,
        G::LambertConformal(g) => &g.scanning_mode,
        G::SphericalHarmonic(_) | G::Unsupported { .. } => return None,
    };
    Some(Scan {
        i_negative: m.i_negative,
        j_positive: m.j_positive,
        j_consecutive: m.j_consecutive,
    })
}

fn grib2_scan(msg: &fieldglass_grib2::Grib2Message) -> Scan {
    match msg.gds.scanning_mode() {
        Some(sm) => Scan {
            i_negative: sm & 0x80 != 0,
            j_positive: sm & 0x40 != 0,
            j_consecutive: sm & 0x20 != 0,
        },
        None => Scan::default_north_down(),
    }
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
    MessageInfo {
        index: index as u32,
        offset_bytes: msg.byte_offset as u64,
        parameter: param.name.to_string(),
        abbreviation: param.abbreviation.to_string(),
        // Normalised at the display seam, the same way the napi host does it:
        // the ECMWF local tables are generated from eccodes' Fortran-style
        // exponents and ON388 chains solidi, so the raw strings disagree about
        // the same unit.
        units: normalize_units(param.units).into_owned(),
        level: fieldglass_grib1::level_value_str(&msg.pds),
        level_type: fieldglass_grib1::level_type_str(&msg.pds),
        reference_time: Some(fieldglass_grib1::reference_time(&msg.pds)),
        forecast: fieldglass_grib1::forecast_display(&msg.pds),
        packing: reader.packing_label(index).unwrap_or("unknown").to_string(),
        size_label: msg.gds.as_ref().and_then(|g| g.size_label()),
        grid,
    }
}

fn grib2_message(reader: &fieldglass_grib2::Grib2Reader, index: usize) -> MessageInfo {
    let msg = &reader.messages[index];
    let common = msg.pds.common();
    let discipline = msg.is.discipline;
    let (abbreviation, parameter, units) = match common.and_then(|c| {
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
    };
    let (level, level_type) = match common {
        Some(c) => (
            c.first_surface
                .value()
                .map(|v| format!("{v}"))
                .unwrap_or_else(|| "—".to_string()),
            fieldglass_grib2::lookup_fixed_surface(c.first_surface.surface_type).to_string(),
        ),
        None => ("—".to_string(), "—".to_string()),
    };
    MessageInfo {
        index: index as u32,
        offset_bytes: msg.byte_offset as u64,
        parameter,
        abbreviation,
        units,
        level,
        level_type,
        reference_time: Some(msg.ids.reference_time_iso8601()),
        forecast: common
            .map(|c| format!("{}", c.forecast_time))
            .unwrap_or_else(|| "—".to_string()),
        packing: msg.drs.template_name(),
        grid: Some(Georef::from_geometry(
            &GridGeometry::from(&msg.gds),
            grib2_scan(msg),
        )),
        size_label: msg.gds.size_label(),
    }
}

impl Scan {
    /// The operational default — west-to-east, north-to-south, row-major — used
    /// where a message states no scan at all.
    fn default_north_down() -> Self {
        Self {
            i_negative: false,
            j_positive: false,
            j_consecutive: false,
        }
    }
}
