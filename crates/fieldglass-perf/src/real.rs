//! The real-data scenarios: one ERA5 field, twelve frames, three containers.
//!
//! Read from the cache `tools/fetch_perf_data.py` fills, never from the network
//! (ADR-0005: fetching is the host's, and here the harness is the host). Each
//! object goes from its cache file straight into memory — one copy on disk, one
//! while running.
//!
//! What the manifest pins and why is in `manifests/build_era5.py`. What these
//! report is the remote path's cost: how many bytes and round trips a scrub
//! through the twelve frames asks a transport for, and how long it takes once
//! the bytes are local.

use std::borrow::Cow;
use std::path::Path;
use std::rc::Rc;
use std::time::Instant;

use fieldglass::{DecodeOptions, Session};
use fieldglass_core::FieldglassError;
use fieldglass_core::bytes::{ByteRange, ByteSource, MemoryObjects, ObjectSource};
use fieldglass_core::testing::Recording;
use serde_json::Value;

use crate::io::{RangeLog, Recorder, SharedBytes, SharedObjects};

/// One container's scrub.
#[derive(Debug, Clone)]
pub struct RealRow {
    /// `era5-zarr`, `era5-netcdf` or `era5-grib1`.
    pub name: &'static str,
    /// Frames decoded.
    pub frames: u32,
    /// Cells per frame.
    pub cells: u64,
    /// Distinct bytes the scrub needed, open included.
    pub bytes: u64,
    /// Round trips the scrub needed, open included.
    pub requests: u64,
    /// Bytes the bound allows: the frames' own objects plus metadata.
    pub bound_bytes: u64,
    /// Time to open, in milliseconds.
    pub open_ms: f64,
    /// Median time per frame, in milliseconds.
    pub frame_ms: f64,
}

/// A range source holding only the ranges a manifest pinned.
///
/// Reports the whole remote object's size, as a transport would, and refuses a
/// read outside what it holds rather than inventing bytes: a reader that asks
/// for a range the manifest did not name is exactly what the report is for,
/// and it should fail where it asks.
pub struct Sparse {
    size: u64,
    pieces: Vec<(u64, Vec<u8>)>,
}

impl ByteSource for Sparse {
    fn size(&self) -> u64 {
        self.size
    }

    fn read(&self, range: ByteRange) -> Result<Cow<'_, [u8]>, FieldglassError> {
        let end = range.start.saturating_add(range.len);
        for (offset, bytes) in &self.pieces {
            let piece_end = offset + bytes.len() as u64;
            if *offset <= range.start && end <= piece_end {
                let from = (range.start - offset) as usize;
                return Ok(Cow::Borrowed(&bytes[from..from + range.len as usize]));
            }
        }
        Err(FieldglassError::Parse(format!(
            "read of {}+{} is outside every range the manifest pinned",
            range.start, range.len
        )))
    }
}

struct Cache<'a> {
    dir: &'a Path,
    manifest: &'a Value,
}

impl Cache<'_> {
    /// An object's bytes, or an error naming it. The fetcher verified every
    /// hash immediately before this run; the length is checked again here so
    /// a file truncated since then fails rather than decoding short.
    fn object(&self, digest: &str) -> Result<Vec<u8>, String> {
        let want = self.manifest["objects"][digest]["length"]
            .as_u64()
            .ok_or_else(|| format!("the manifest has no object {digest}"))?;
        let path = self.dir.join(digest);
        let bytes = std::fs::read(&path).map_err(|e| {
            format!(
                "{}: {e}; run python3 tools/fetch_perf_data.py",
                path.display()
            )
        })?;
        if bytes.len() as u64 != want {
            return Err(format!(
                "{} is {} bytes, the manifest says {want}",
                path.display(),
                bytes.len()
            ));
        }
        Ok(bytes)
    }
}

fn millis(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1e3
}

fn median(mut samples: Vec<f64>) -> f64 {
    samples.sort_by(f64::total_cmp);
    samples.get(samples.len() / 2).copied().unwrap_or(0.0)
}

fn variable(session: &Session, name: &str) -> Result<u32, String> {
    session
        .variables()
        .iter()
        .find(|v| v.name == name)
        .map(|v| v.index)
        .ok_or_else(|| format!("no variable {name}"))
}

/// Scrub all three containers.
///
/// # Errors
///
/// A manifest that does not parse, an object missing from the cache or of the
/// wrong length, or a reader that fails on the data.
pub fn era5(manifest_path: &Path, cache_dir: &Path) -> Result<Vec<RealRow>, String> {
    let text = std::fs::read_to_string(manifest_path)
        .map_err(|e| format!("{}: {e}", manifest_path.display()))?;
    let manifest: Value =
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", manifest_path.display()))?;
    let cache = Cache {
        dir: cache_dir,
        manifest: &manifest,
    };
    Ok(vec![zarr(&cache)?, netcdf(&cache)?, grib1(&cache)?])
}

fn frames(section: &Value) -> Result<u32, String> {
    section["frames"]
        .as_u64()
        .and_then(|f| u32::try_from(f).ok())
        .ok_or_else(|| "the manifest names no frame count".to_string())
}

fn zarr(cache: &Cache<'_>) -> Result<RealRow, String> {
    let section = &cache.manifest["zarr"];
    let frames = frames(section)?;
    let mut objects = MemoryObjects::new();
    for (key, digest) in section["objects"].as_object().into_iter().flatten() {
        let bytes = cache.object(digest.as_str().unwrap_or_default())?;
        objects.insert(key.clone(), bytes);
    }
    for (key, document) in section["documents"].as_object().into_iter().flatten() {
        let bytes = serde_json::to_vec(document).map_err(|e| e.to_string())?;
        objects.insert(key.clone(), bytes);
    }
    for (key, encoded) in section["binary"].as_object().into_iter().flatten() {
        let bytes = fieldglass_zarr::base64::decode(encoded.as_str().unwrap_or_default())
            .ok_or_else(|| format!("{key}: not base64"))?;
        objects.insert(key.clone(), bytes);
    }
    // Everything the subset store holds except the time coordinate's data. The
    // latitude and longitude arrays are in: a decoded field carries where its
    // cells are, so reading them is part of the decode, not a stray fetch.
    let bound_bytes = objects
        .list("")
        .map_err(|e| e.to_string())?
        .iter()
        .filter(|key| key.as_str() != "time/0")
        .filter_map(|key| objects.get(key).ok().flatten().map(|b| b.len() as u64))
        .sum();

    let recording = Rc::new(Recording::new(objects));
    let recorder = Recorder::Objects(Rc::clone(&recording));
    let start = Instant::now();
    let session = Session::open_store(SharedObjects(recording)).map_err(|e| e.to_string())?;
    let open_ms = millis(start);
    let var = variable(&session, section["variable"].as_str().unwrap_or_default())?;
    let (cells, frame_ms) = scrub(frames, |k| {
        session.decode_slice(var, 1, 2, &[k, 0, 0], &DecodeOptions::default())
    })?;
    let io = recorder.tally();
    Ok(RealRow {
        name: "era5-zarr",
        frames,
        cells,
        bytes: io.bytes,
        requests: io.requests,
        bound_bytes,
        open_ms,
        frame_ms,
    })
}

fn netcdf(cache: &Cache<'_>) -> Result<RealRow, String> {
    let section = &cache.manifest["netcdf"];
    let frames = frames(section)?;
    let bytes = cache.object(section["object"].as_str().unwrap_or_default())?;
    let size = bytes.len() as u64;
    let start = Instant::now();
    let session = Session::open(bytes).map_err(|e| e.to_string())?;
    let open_ms = millis(start);
    let var = variable(&session, section["variable"].as_str().unwrap_or_default())?;
    let (cells, frame_ms) = scrub(frames, |k| {
        session.decode_slice(var, 1, 2, &[k, 0, 0], &DecodeOptions::default())
    })?;
    // The day's file holds 24 frames after a small header; the twelve asked
    // for are half of its data. The element size is the manifest's, checked
    // against the file: a plane layout that does not fit is an error rather
    // than a bound computed from a guess.
    let planes = session
        .variables()
        .iter()
        .find(|v| v.index == var)
        .and_then(|v| v.dims.first().map(|d| d.length))
        .unwrap_or(u64::from(frames).max(1));
    let element = section["element_bytes"]
        .as_u64()
        .ok_or("the manifest's netcdf section names no element_bytes")?;
    let plane_bytes = cells * element;
    let header = size.checked_sub(planes * plane_bytes).ok_or_else(|| {
        format!("{planes} planes of {plane_bytes} bytes do not fit a {size}-byte file")
    })?;
    Ok(RealRow {
        name: "era5-netcdf",
        frames,
        cells,
        bytes: size,
        requests: 1,
        bound_bytes: header + u64::from(frames) * plane_bytes,
        open_ms,
        frame_ms,
    })
}

fn grib1(cache: &Cache<'_>) -> Result<RealRow, String> {
    let section = &cache.manifest["grib1"];
    let size = section["size"]
        .as_u64()
        .ok_or("the manifest names no GRIB file size")?;
    let mut pieces = Vec::new();
    let mut offsets = Vec::new();
    for message in section["messages"].as_array().into_iter().flatten() {
        let offset = message["offset"]
            .as_u64()
            .ok_or("a message has no offset")?;
        pieces.push((
            offset,
            cache.object(message["object"].as_str().unwrap_or_default())?,
        ));
        offsets.push(offset);
    }
    let bound_bytes = pieces.iter().map(|(_, b)| b.len() as u64).sum();
    if offsets.is_empty() {
        return Err("the manifest's grib1 section lists no messages".to_string());
    }
    let frames = u32::try_from(offsets.len()).map_err(|e| e.to_string())?;
    let recording = Rc::new(Recording::new(Sparse { size, pieces }));
    let recorder = Recorder::Ranges(Rc::clone(&recording) as Rc<dyn RangeLog>);
    // A sidecar index names each message's offset, so each frame is opened at
    // it: the remote path a host with an index takes.
    let start = Instant::now();
    let first = Session::open_message_at(SharedBytes(Rc::clone(&recording)), offsets[0])
        .map_err(|e| e.to_string())?;
    let open_ms = millis(start);
    drop(first);
    let (cells, frame_ms) = scrub(frames, |k| {
        let session =
            Session::open_message_at(SharedBytes(Rc::clone(&recording)), offsets[k as usize])?;
        session.decode(0, &DecodeOptions::default())
    })?;
    let io = recorder.tally();
    Ok(RealRow {
        name: "era5-grib1",
        frames,
        cells,
        bytes: io.bytes,
        requests: io.requests,
        bound_bytes,
        open_ms,
        frame_ms,
    })
}

fn scrub(
    frames: u32,
    mut decode: impl FnMut(u32) -> Result<fieldglass::Field, fieldglass::Error>,
) -> Result<(u64, f64), String> {
    let mut times = Vec::new();
    let mut cells = 0;
    for k in 0..frames {
        let start = Instant::now();
        let field = decode(k).map_err(|e| format!("frame {k}: {e}"))?;
        times.push(millis(start));
        cells = field.values.len() as u64;
    }
    Ok((cells, median(times)))
}
