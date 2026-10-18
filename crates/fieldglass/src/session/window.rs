//! Windows: a lat/lon box and a position on every other axis, planned into the
//! chunks it touches and decoded from them alone (#1083).
//!
//! What a browser reading a remote Zarr store needs between "here is the
//! store's metadata" and "here is a field". It cannot fetch the whole variable,
//! which for a global forecast is every chunk of every step, so it asks three
//! things of a [`Session`] opened over what it has fetched so far:
//!
//! 1. [`Session::coordinate_chunks`]: which objects hold the coordinate arrays
//!    of a variable's axes. A box is matched against coordinates, so these come
//!    first, and they are small.
//! 2. [`Session::plan_window`]: given those, which cells a box and a position
//!    on the other axes select, and which chunks hold them.
//! 3. [`Session::decode_window`]: the field over exactly those cells, read from
//!    exactly those chunks.
//!
//! Every call is synchronous and reads only what the host already handed over
//! (ADR-0005): the host fetches between them and opens the store again with the
//! larger set of objects.
//!
//! # Which cells a box holds
//!
//! A cell is in the box when its centre is: latitude in `[lat_min, lat_max]`,
//! and longitude on the eastward arc from `lon_min` to `lon_max`, with both
//! edges included. The box follows the workspace convention for longitude
//! (`LonLatBox`): `lon_max >= lon_min`, and a box over the antimeridian runs
//! past 180 or below -180 rather than wrapping its bounds. An arc of 360 degrees
//! or more holds every column. A host that wants the cells a view only partly
//! covers widens its box by half a cell.
//!
//! The centres are the ones the slice is drawn at: the placement's own forward
//! map, so a window holds exactly the cells a render of the slice puts in the
//! box.
//!
//! On a grid that closes on itself in longitude, a box across its seam selects
//! the columns either side of it, and the window holds them as one run east
//! from the box's west edge, read as two regions. On a grid that does not close,
//! cells at both ends of a box with a gap between them are read as the one
//! rectangle that spans both.
//!
//! # What is windowed
//!
//! A slice placed on a regular lat/lon grid running west to east, which is how
//! 1-D latitude and longitude coordinates place one: the layout of the gridded
//! forecast and reanalysis stores (GFS, IFS, ERA5). Any other placement — a
//! curvilinear mesh, a projected domain, a slice nothing places — is
//! [`Error::Unsupported`], naming the family, rather than a window chosen by a
//! rule that does not hold for it.

use std::ops::Range;

use fieldglass_core::array::{ArraySource, CfUnpacking};
use fieldglass_core::bytes::ObjectSource;
use fieldglass_core::cf::RenderableArray;
use fieldglass_core::projection::LatLonParams;
use fieldglass_core::{FieldglassError, GridGeometry};

#[cfg(any(feature = "grib1", feature = "grib2"))]
use super::wrong_addressing;
use super::{Arrays, DecodeOptions, Reader, Session, array_units, build_field};
#[cfg(any(feature = "grib1", feature = "grib2"))]
use crate::api::Addressing;
use crate::api::{Field, Scan};
use crate::error::Error;

/// A window to plan or decode: a lat/lon box over two axes of a variable, and a
/// position on every other one.
///
/// The arguments [`Session::decode_slice`] takes, with the box added. The two
/// axes are positions in the variable's own axes, and the slice indices hold
/// one entry per axis in the same order, the two horizontal ones ignored
/// rather than absent, so a host passes the vector it already holds for the
/// slice on screen.
///
/// **On the wire** a host sends one as a camelCase object. Every key is
/// required and an unknown one is refused: a misspelt `sliceIndex` dropped
/// silently would plan the first time step and label it the one asked for.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[cfg_attr(feature = "schema", schemars(rename_all = "camelCase"))]
#[non_exhaustive]
pub struct WindowRequest {
    /// Which variable, as a position in [`Session::variables`].
    pub variable: u32,
    /// Which of its axes runs down the rows: a latitude.
    pub y_dim: u32,
    /// Which of its axes runs across the columns: a longitude.
    pub x_dim: u32,
    /// The box, `[latMin, latMax, lonMin, lonMax]` in degrees, the order a
    /// warp's `bounds` uses. A cell is in it when its centre is, edges
    /// included. Longitude runs east from `lonMin` to `lonMax`, which may not
    /// be smaller: a box over the antimeridian runs past 180 (`[.., 170, 190]`)
    /// rather than wrapping, and a span of 360 or more holds every column.
    pub bounds: [f64; 4],
    /// A position on every axis of the variable, the two horizontal ones
    /// ignored: the time step, the level, the member.
    pub slice_indices: Vec<u32>,
}

impl WindowRequest {
    /// A request, from its parts. A constructor for the reason
    /// [`DecodeOptions::new`] gives.
    #[must_use]
    pub fn new(
        variable: u32,
        y_dim: u32,
        x_dim: u32,
        bounds: [f64; 4],
        slice_indices: Vec<u32>,
    ) -> Self {
        Self {
            variable,
            y_dim,
            x_dim,
            bounds,
            slice_indices,
        }
    }
}

/// One stored object a read needs: which array it belongs to, the key it is
/// stored under from the store's root, and its index on that array's chunk
/// grid.
///
/// The key is what a host fetches, relative to the store's own URL, and what it
/// passes back as the object's name. For a sharded array it is a shard.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schema", schemars(rename_all = "camelCase"))]
#[non_exhaustive]
pub struct ChunkRef {
    /// The array, path-qualified as [`Session::variables`] names it.
    pub array: String,
    /// The object's key from the store's root: `t2m/c/1/2/0`, `t2m/1.2.0`.
    pub key: String,
    /// The chunk's position on the array's chunk grid, one entry per axis.
    pub index: Vec<u64>,
}

/// Which cells a window holds and which chunks hold them.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "camelCase")]
#[cfg_attr(feature = "schema", schemars(rename_all = "camelCase"))]
#[non_exhaustive]
pub struct WindowPlan {
    /// The rows the box holds, `[start, end)` along the request's Y axis, in
    /// storage order.
    /// `[0, 0]` when it holds none.
    pub rows: [u64; 2],
    /// The columns, as `[start, end)` runs along the request's X axis in the order the
    /// window holds them: one run, or two when the box crosses the seam of a
    /// grid that closes on itself (the run east of the seam second). Empty when
    /// the box holds no column.
    pub columns: Vec<[u64; 2]>,
    /// Columns in the window, the sum of the runs.
    pub ni: u32,
    /// Rows in the window.
    pub nj: u32,
    /// Every chunk of the variable the window's cells sit in, in row-major
    /// order of their index, each once. Fetch these, and only these, before
    /// [`Session::decode_window`]. `null` for a container that is not stored
    /// as keyed objects — a NetCDF file, which a host reads by range.
    pub chunks: Option<Vec<ChunkRef>>,
}

/// A request checked against the session and resolved to cells.
struct Window<'a> {
    arrays: &'a Arrays,
    var: RenderableArray,
    y: usize,
    x: usize,
    /// One position per axis, the horizontal ones `0`.
    fixed: Vec<u64>,
    rows: Range<u64>,
    columns: Vec<Range<u64>>,
    /// The slice's own placement, which the window's is cut from.
    whole: LatLonParams,
    scan: Scan,
    declared: String,
}

impl Window<'_> {
    fn ni(&self) -> u64 {
        self.columns.iter().map(|r| r.end - r.start).sum()
    }

    fn nj(&self) -> u64 {
        self.rows.end - self.rows.start
    }

    fn is_empty(&self) -> bool {
        self.ni() == 0 || self.nj() == 0
    }

    /// The region one column run reads, in the variable's declared axis order.
    fn region(&self, run: &Range<u64>) -> Vec<Range<u64>> {
        (0..self.var.dims.len())
            .map(|d| {
                if d == self.y {
                    self.rows.clone()
                } else if d == self.x {
                    run.clone()
                } else {
                    self.fixed[d]..self.fixed[d] + 1
                }
            })
            .collect()
    }
}

impl Session {
    /// The objects that hold the coordinate arrays of `variable`'s axes, in the
    /// order of its axes (#1083).
    ///
    /// What a host fetches before [`plan_window`](Self::plan_window), which
    /// matches the box against these coordinates. An axis's coordinate array
    /// is the 1-D array named after it, as CF has it and
    /// [`axis_values`](Self::axis_values) reads it; an axis with none
    /// contributes nothing. The time axis is included, so the same fetch is
    /// what labels the steps.
    ///
    /// `None` for a container that is not stored as keyed objects (a NetCDF
    /// file, which holds its coordinates already).
    ///
    /// # Errors
    ///
    /// [`Error::WrongAddressing`] for a message stream, and
    /// [`Error::NoSuchMessage`] for a variable index outside
    /// [`variables`](Self::variables).
    pub fn coordinate_chunks(&self, variable: u32) -> Result<Option<Vec<ChunkRef>>, Error> {
        let (arrays, var) = self.arrays_variable(variable, "coordinate_chunks")?;
        let source = arrays.source.as_ref();
        let mut out = Vec::new();
        for axis in &var.dims {
            let Some(coordinate) = source.array(&axis.name) else {
                continue;
            };
            if coordinate.dimensions.len() != 1 || coordinate.dimensions[0] != axis.name {
                continue;
            }
            let whole = 0..axis.length;
            match arrays.chunk_refs(&axis.name, std::slice::from_ref(&whole))? {
                Some(chunks) => out.extend(chunks),
                None => return Ok(None),
            }
        }
        Ok(arrays.is_keyed().then_some(out))
    }

    /// Which cells a window holds and which chunks they sit in (#1083).
    ///
    /// Reads the coordinates of the two horizontal axes and nothing else, so
    /// the objects [`coordinate_chunks`](Self::coordinate_chunks) names must be
    /// in the store; see the module docs for the rule a box is matched by. A
    /// box that holds no cell is an empty plan, not an error: a view panned off
    /// a regional grid has nothing to fetch.
    ///
    /// # Errors
    ///
    /// [`Error::WrongAddressing`] for a message stream;
    /// [`Error::NoSuchMessage`] for a variable index outside
    /// [`variables`](Self::variables); [`Error::InvalidOption`] for axes that
    /// are the same or outside the variable, a `slice_indices` of the wrong
    /// length or past an axis, and a box that is not finite or has an edge
    /// below its opposite; [`Error::Unsupported`] for a slice placed on
    /// anything but a regular lat/lon grid running west to east; and a decode
    /// failure reading the coordinates.
    pub fn plan_window(&self, request: &WindowRequest) -> Result<WindowPlan, Error> {
        let window = self.window(request, "plan_window")?;
        let chunks = if window.is_empty() {
            window.arrays.is_keyed().then(Vec::new)
        } else {
            let mut planned: Vec<ChunkRef> = Vec::new();
            let mut keyed = true;
            for run in &window.columns {
                match window
                    .arrays
                    .chunk_refs(&window.var.name, &window.region(run))?
                {
                    Some(chunks) => planned.extend(chunks),
                    None => keyed = false,
                }
            }
            planned.sort_by(|a, b| a.index.cmp(&b.index));
            planned.dedup_by(|a, b| a.index == b.index);
            keyed.then_some(planned)
        };
        Ok(WindowPlan {
            rows: if window.is_empty() {
                [0, 0]
            } else {
                [window.rows.start, window.rows.end]
            },
            columns: if window.is_empty() {
                Vec::new()
            } else {
                window.columns.iter().map(|r| [r.start, r.end]).collect()
            },
            ni: if window.is_empty() {
                0
            } else {
                u32::try_from(window.ni()).unwrap_or(u32::MAX)
            },
            nj: if window.is_empty() {
                0
            } else {
                u32::try_from(window.nj()).unwrap_or(u32::MAX)
            },
            chunks,
        })
    }

    /// The field over a window's cells, read from the chunks
    /// [`plan_window`](Self::plan_window) names and no others (#1083).
    ///
    /// The same field [`decode_slice`](Self::decode_slice) returns for the whole
    /// plane, cut to the window: physical values with the CF mask-and-scale
    /// applied once, `ni` columns by `nj` rows in the order the plan lists
    /// them, and a lat/lon georef for the window's own corners. Every other
    /// operation takes it as it takes any field.
    ///
    /// A chunk the host did not pass reads as the fill value, exactly as one
    /// the store never wrote: an object map cannot tell the two apart. Pass
    /// every key the plan names.
    ///
    /// # Errors
    ///
    /// Those of [`plan_window`](Self::plan_window), plus
    /// [`Error::InvalidOption`] for a box that holds no cell, and a decode
    /// failure reading a chunk.
    pub fn decode_window(
        &self,
        request: &WindowRequest,
        options: &DecodeOptions,
    ) -> Result<Field, Error> {
        let window = self.window(request, "decode_window")?;
        if window.is_empty() {
            return Err(Error::InvalidOption {
                detail: format!(
                    "the box {:?} holds no cell centre of `{}`, so there is no window to decode",
                    request.bounds, window.var.name
                ),
            });
        }
        let source = window.arrays.source.as_ref();
        let (ni, nj) = (window.ni(), window.nj());
        // Refused past the one-field cap before anything is allocated: the
        // axes' lengths are numbers out of somebody else's metadata.
        let cells = fieldglass_core::array::field_element_count(&[nj, ni])
            .map_err(FieldglassError::from)?;
        let width = usize::try_from(ni).map_err(|_| too_large())?;
        let height = usize::try_from(nj).map_err(|_| too_large())?;
        let mut raw: Vec<Option<f64>> = vec![None; cells];
        let mut offset = 0usize;
        for run in &window.columns {
            let run_width = usize::try_from(run.end - run.start).map_err(|_| too_large())?;
            let block = source.read_region(&window.var.name, &window.region(run))?;
            if block.len() != run_width * height {
                return Err(Error::Decode {
                    detail: format!(
                        "a {height} x {run_width} region of `{}` read back {} values",
                        window.var.name,
                        block.len()
                    ),
                });
            }
            for j in 0..height {
                for i in 0..run_width {
                    // The region comes back in declared axis order: rows outer
                    // when `y` precedes `x`, columns outer when it follows.
                    let at = if window.y < window.x {
                        j * run_width + i
                    } else {
                        i * height + j
                    };
                    raw[j * width + offset + i] = block[at];
                }
            }
            offset += run_width;
        }
        let attributes = source
            .array(&window.var.name)
            .map(|d| d.attributes.as_slice())
            .unwrap_or_default();
        let values = CfUnpacking::from_attributes(attributes).apply_owned(raw);
        let geometry = GridGeometry::LatLon(window_params(&window));
        let ni = u32::try_from(ni).map_err(|_| too_large())?;
        let nj = u32::try_from(nj).map_err(|_| too_large())?;
        Ok(build_field(
            &values,
            ni,
            nj,
            &geometry,
            window.scan,
            &window.declared,
            Some(window.var.name.clone()),
            array_units(source, &window.var.name),
            options,
        ))
    }

    /// The arrays arm and one of its renderable variables, or the refusal a
    /// message stream or an index past the list gets.
    fn arrays_variable(
        &self,
        variable: u32,
        called: &str,
    ) -> Result<(&Arrays, RenderableArray), Error> {
        #[cfg(not(any(feature = "grib1", feature = "grib2")))]
        let _ = called;
        match &self.reader {
            #[cfg(feature = "grib1")]
            Reader::Grib1(_) => Err(wrong_addressing(Addressing::Messages, called, "decode")),
            #[cfg(feature = "grib2")]
            Reader::Grib2(_) => Err(wrong_addressing(Addressing::Messages, called, "decode")),
            Reader::Arrays(a) => {
                let mut vars = fieldglass_core::cf::renderable_arrays(a.source.group());
                let count = u32::try_from(vars.len()).unwrap_or(u32::MAX);
                if (variable as usize) >= vars.len() {
                    return Err(Error::NoSuchMessage {
                        index: variable,
                        count,
                    });
                }
                Ok((a.as_ref(), vars.swap_remove(variable as usize)))
            }
        }
    }

    /// Check a request and resolve its box to rows and column runs.
    fn window(&self, request: &WindowRequest, called: &str) -> Result<Window<'_>, Error> {
        let (arrays, var) = self.arrays_variable(request.variable, called)?;
        let rank = var.dims.len();
        let (y, x) = (request.y_dim as usize, request.x_dim as usize);
        if y == x || y >= rank || x >= rank {
            return Err(Error::InvalidOption {
                detail: format!(
                    "`{}` has {rank} dimensions; yDim {y} and xDim {x} must be different and \
                     within them",
                    var.name
                ),
            });
        }
        if request.slice_indices.len() != rank {
            return Err(Error::InvalidOption {
                detail: format!(
                    "`{}` has {rank} dimensions, so `sliceIndices` needs {rank} entries (the two \
                     horizontal ones are ignored); {} were given",
                    var.name,
                    request.slice_indices.len()
                ),
            });
        }
        let mut fixed = Vec::with_capacity(rank);
        for (d, (&at, axis)) in request.slice_indices.iter().zip(&var.dims).enumerate() {
            if d == y || d == x {
                fixed.push(0);
                continue;
            }
            if u64::from(at) >= axis.length {
                return Err(Error::InvalidOption {
                    detail: format!(
                        "`sliceIndices[{d}]` is {at}, past the {} positions of `{}`",
                        axis.length, axis.name
                    ),
                });
            }
            fixed.push(u64::from(at));
        }
        let [lat_min, lat_max, lon_min, lon_max] = request.bounds;
        if !request.bounds.iter().all(|v| v.is_finite()) || lat_min > lat_max || lon_min > lon_max {
            return Err(Error::InvalidOption {
                detail: format!(
                    "`bounds` is [latMin, latMax, lonMin, lonMax] with each edge finite and no \
                     smaller than its opposite (a box over the antimeridian runs past 180); \
                     {:?} is not one",
                    request.bounds
                ),
            });
        }

        let placement = arrays.placement(&var, y, x)?;
        let scan = placement.scan.unwrap_or_else(Scan::north_down);
        let whole = match &placement.geometry {
            GridGeometry::LatLon(p) if !scan.i_negative && p.ni >= 2 && p.nj >= 2 => *p,
            other => {
                return Err(Error::Unsupported {
                    detail: format!(
                        "a window is planned over a slice placed on a regular lat/lon grid \
                         running west to east; `{}` on these axes is placed as `{}`",
                        var.name,
                        other.label()
                    ),
                });
            }
        };
        let forward = placement.geometry.forward_at();
        let lats: Vec<Option<f64>> = (0..whole.nj).map(|j| forward(0, j).map(|p| p.0)).collect();
        let lons: Vec<Option<f64>> = (0..whole.ni).map(|i| forward(i, 0).map(|p| p.1)).collect();
        let rows = span(
            &lats
                .iter()
                .map(|v| v.is_some_and(|lat| (lat_min..=lat_max).contains(&lat)))
                .collect::<Vec<_>>(),
        );
        let inside: Vec<bool> = lons
            .iter()
            .map(|v| v.is_some_and(|lon| on_arc(lon, lon_min, lon_max)))
            .collect();
        let columns = column_runs(&inside, placement.geometry.is_periodic_x());
        Ok(Window {
            arrays,
            var,
            y,
            x,
            fixed,
            rows,
            columns,
            whole,
            scan,
            declared: placement.geometry.label().to_string(),
        })
    }
}

impl Arrays {
    /// The objects a region read of `array` fetches, or `None` when this
    /// container is not stored as keyed objects.
    fn chunk_refs(
        &self,
        array: &str,
        region: &[Range<u64>],
    ) -> Result<Option<Vec<ChunkRef>>, FieldglassError> {
        let Some(keys) = self.source.chunk_keys(array, region) else {
            return Ok(None);
        };
        Ok(Some(
            keys?
                .into_iter()
                .map(|(index, key)| ChunkRef {
                    array: array.to_string(),
                    key,
                    index,
                })
                .collect(),
        ))
    }

    /// Whether this container is stored as keyed objects a host fetches one by
    /// one.
    fn is_keyed(&self) -> bool {
        self.source.is_keyed()
    }
}

/// The refusal for a window larger than this target can address.
fn too_large() -> Error {
    Error::Unsupported {
        detail: "the window holds more cells than this target can address".to_string(),
    }
}

/// Whether `lon` lies on the eastward arc from `west` to `east`, both included.
/// An arc of a full turn or more holds every longitude.
fn on_arc(lon: f64, west: f64, east: f64) -> bool {
    let arc = east - west;
    arc >= 360.0 || (lon - west).rem_euclid(360.0) <= arc
}

/// The smallest `start..end` holding every `true` in `selected`, or `0..0`.
fn span(selected: &[bool]) -> Range<u64> {
    let first = selected.iter().position(|&s| s);
    let last = selected.iter().rposition(|&s| s);
    match (first, last) {
        (Some(first), Some(last)) => first as u64..last as u64 + 1,
        _ => 0..0,
    }
}

/// The selected columns as the runs a window holds them in.
///
/// On a grid that closes on itself, the selection is read circularly: a run
/// that crosses the seam is split at it, its western part first. Anywhere a
/// circular reading finds more than one run, and on a grid that does not close,
/// the window is the one rectangle that spans the selection.
fn column_runs(selected: &[bool], periodic: bool) -> Vec<Range<u64>> {
    let n = selected.len();
    let count = selected.iter().filter(|&&s| s).count();
    if count == 0 {
        return Vec::new();
    }
    if count == n {
        return std::iter::once(0..n as u64).collect();
    }
    if periodic {
        // Where a run starts: selected, with the column before it (circularly)
        // not. There is exactly one when the selection is one circular run.
        let starts: Vec<usize> = (0..n)
            .filter(|&i| selected[i] && !selected[(i + n - 1) % n])
            .collect();
        if let [start] = starts[..] {
            let end = start + count;
            return if end <= n {
                std::iter::once(start as u64..end as u64).collect()
            } else {
                vec![start as u64..n as u64, 0..(end - n) as u64]
            };
        }
    }
    std::iter::once(span(selected)).collect()
}

/// The window's own placement: the slice's, cut to its first and last cell.
fn window_params(window: &Window<'_>) -> LatLonParams {
    let geometry = GridGeometry::LatLon(window.whole);
    let forward = geometry.forward_at();
    // Both are cells of the whole grid, which `window` resolved through this
    // same map, so neither is off it.
    let first_column = window.columns.first().map_or(0, |r| r.start);
    let last_column = window.columns.last().map_or(0, |r| r.end - 1);
    let corner = |i: u64, j: u64| {
        let (i, j) = (
            u32::try_from(i).unwrap_or(u32::MAX),
            u32::try_from(j).unwrap_or(u32::MAX),
        );
        forward(i, j).unwrap_or((f64::NAN, f64::NAN))
    };
    let (lat_first, lon_first) = corner(first_column, window.rows.start);
    let (lat_last, lon_last) = corner(last_column, window.rows.end - 1);
    LatLonParams {
        ni: u32::try_from(window.ni()).unwrap_or(u32::MAX),
        nj: u32::try_from(window.nj()).unwrap_or(u32::MAX),
        lat_first,
        lon_first,
        lat_last,
        lon_last,
    }
}

/// What [`Keyed::chunk_keys`] answers for a store: each object's index on the
/// chunk grid, and its key.
type Keys = Result<Vec<(Vec<u64>, String)>, FieldglassError>;

/// An [`ArraySource`] that can also say which stored objects a region read
/// fetches: what `Session`'s arrays arm holds when this crate reads Zarr.
///
/// A Zarr store answers from the code its own read spells keys with
/// ([`fieldglass_zarr::ZarrStore::chunk_keys`]), so a plan and the read it plans
/// for cannot name different objects. A NetCDF file answers `None`: a host reads
/// it by range, not by key.
pub(super) trait Keyed: ArraySource {
    /// Every object a read of `region` of `array` fetches, with its index on
    /// the chunk grid, or `None` for a container not stored as keyed objects.
    fn chunk_keys(&self, array: &str, region: &[Range<u64>]) -> Option<Keys>;

    /// Whether [`chunk_keys`](Self::chunk_keys) answers at all.
    fn is_keyed(&self) -> bool;
}

impl<O: ObjectSource> Keyed for fieldglass_zarr::ZarrStore<O> {
    fn chunk_keys(&self, array: &str, region: &[Range<u64>]) -> Option<Keys> {
        Some(fieldglass_zarr::ZarrStore::chunk_keys(self, array, region))
    }

    fn is_keyed(&self) -> bool {
        true
    }
}

#[cfg(feature = "netcdf")]
impl<R, V, S> Keyed for fieldglass_netcdf::NetcdfArrays<R, V, S>
where
    Self: ArraySource,
{
    fn chunk_keys(&self, _array: &str, _region: &[Range<u64>]) -> Option<Keys> {
        None
    }

    fn is_keyed(&self) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bits(s: &str) -> Vec<bool> {
        s.chars().map(|c| c == '#').collect()
    }

    #[test]
    fn an_arc_is_eastward_from_its_west_edge_and_holds_both_edges() {
        assert!(on_arc(10.0, 0.0, 10.0));
        assert!(on_arc(0.0, 0.0, 10.0));
        assert!(!on_arc(10.5, 0.0, 10.0));
        // Across the antimeridian, in both spellings of a longitude.
        assert!(on_arc(-175.0, 170.0, 190.0));
        assert!(on_arc(185.0, 170.0, 190.0));
        assert!(on_arc(300.0, -75.0, 75.0));
        assert!(!on_arc(180.0, -75.0, 75.0));
        // A full turn holds everything, a point holds itself.
        assert!(on_arc(123.0, -180.0, 180.0));
        assert!(on_arc(42.0, 42.0, 42.0));
        assert!(on_arc(402.0, 42.0, 42.0));
        assert!(!on_arc(42.5, 42.0, 42.0));
    }

    #[test]
    fn a_periodic_selection_across_the_seam_is_two_runs_western_first() {
        assert_eq!(column_runs(&bits("###.......##"), true), vec![10..12, 0..3]);
        assert_eq!(column_runs(&bits("..###......."), true), vec![2..5]);
        assert_eq!(
            column_runs(&bits("##########.#"), true),
            vec![11..12, 0..10]
        );
        assert_eq!(column_runs(&bits("############"), true), vec![0..12]);
        assert_eq!(
            column_runs(&bits("............"), true),
            Vec::<Range<u64>>::new()
        );
    }

    #[test]
    fn a_selection_that_is_not_one_run_is_the_rectangle_that_spans_it() {
        // A grid that does not close: both ends, one rectangle.
        assert_eq!(column_runs(&bits("##........##"), false), vec![0..12]);
        assert_eq!(column_runs(&bits("..##..##...."), false), vec![2..8]);
        // A closed grid read circularly still finds two runs here.
        assert_eq!(column_runs(&bits("..##..##...."), true), vec![2..8]);
    }

    #[test]
    fn a_span_is_first_to_last() {
        assert_eq!(span(&bits("..##.#..")), 2..6);
        assert_eq!(span(&bits("........")), 0..0);
        assert_eq!(span(&bits("#")), 0..1);
    }
}
