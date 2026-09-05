//! Characterisation golden for render, probe, contours and CSV (#570).
//!
//! The existing oracles stop at decoded values: `.eccodes.ref.json` and
//! `tools/preflight_samples.js` pin metadata and the numbers a message decodes
//! to. Nothing pinned what the *display* half does with those numbers — the
//! warp, the palette, the point probe, the contour tracer, the CSV writer —
//! which is exactly the layer #571 and #572 lift out of this crate and onto
//! `fieldglass::Session`. This file records that layer's output so those moves
//! can be proved rather than argued, and it is the seed of the ADR-0006
//! conformance suite (#573).
//!
//! # What is recorded
//!
//! `golden/render_characterisation.tsv`, one line per case:
//!
//! ```text
//! <field>\t<case>\t<portable>\t<exact>
//! ```
//!
//! `<field>` names a fixture file, one of its messages, or one NetCDF slice of
//! it; `<case>` names one operation with its inputs in it. `<portable>` is the
//! case's *discrete* result written out in the open — raster size,
//! opaque-pixel count, probe hit or miss and the grid cell it landed on,
//! contour run and vertex counts, CSV line count, and for a refusal the reason
//! with its numbers elided. `<exact>` is an FNV fold over everything the call
//! produced, RGBA bytes, `f64` bits and the unelided reason included.
//!
//! # Why two columns
//!
//! ADR-0009: cross-target agreement is a tolerance, and bit-identical results
//! are a property of one libm rather than of this repository. A warp runs the
//! planar inverses per output pixel, so an ULP of difference in `atan2` can in
//! principle move a sampled cell and repaint a pixel. The `<exact>` column is
//! therefore asserted only where `support::libm_fingerprint()` recognises the
//! libm that recorded it; `<portable>` is asserted everywhere.
//!
//! `<portable>` is not a weaker copy of `<exact>`. It is deliberately made of
//! the *discrete* outputs — the `Some`/`None`-shaped decisions, the counts and
//! which refusal a case produces — which is the class ADR-0009 named as "the
//! assumption most worth re-checking, because it is the one that would actually
//! be visible". A grid cell index or an opaque-pixel count that started
//! disagreeing between targets is a real product defect, and this column is
//! where it would surface.
//!
//! Two limits of that column, stated rather than left to be discovered:
//!
//! * **ADR-0009 did not measure these outputs.** Its 323,620-probe measurement
//!   covers the four planar inverses. The opaque counts here are millions of
//!   per-pixel decisions across Mollweide, Robinson, Equal Earth, orthographic,
//!   the geostationary scan grid and a curvilinear nearest-neighbour search,
//!   and a raster height like 503 is a rounded result on a libm-computed
//!   extent. This column extends the ADR's finding to the whole display path on
//!   purpose, so that a target where it does *not* hold says so. If one ever
//!   reddens here, the response is ADR-0009's "a discrete output starts
//!   disagreeing across targets" revisit bullet, not a re-baseline.
//! * **`fieldglass-napi` does not build for `wasm32`**, so unlike the planar
//!   golden this column has no second target in CI today. It is here for a
//!   downstream running `cargo test` on macOS or musl, and for #572, which
//!   moves this code onto `fieldglass::Session` — a crate the wasm job does
//!   build and test.
//!
//! The gate cannot quietly stop matching: `planar_inverse_golden.rs`'s
//! `the_reference_toolchain_is_the_recorded_libm` asserts the same shared
//! constant outright on x86_64 glibc.
//!
//! # Why the committed fixtures and not `samples/`
//!
//! #464 asked for "every `samples/` and fixture field". `samples/` cannot be
//! part of a golden that also has to run in the default gate and reproduce on a
//! fresh clone: `.gitignore` keeps every file under it but the README, the
//! corpus is fetched by `tools/fetch_samples.sh`, and CI has none of it. A
//! golden keyed on it would degrade to a no-op that passes while pinning
//! nothing — the failure the issue's own "a fixture leaving the corpus fails
//! here" criterion exists to prevent. So the input is the committed corpus
//! under `crates/*/tests/fixtures/`. Nothing in the repository enumerated that
//! whole set before: `every_fixture_places_its_own_first_point` walks the 53
//! GRIB2 files only, and `grid_geometry_proj.rs` reads no fixture directory at
//! all — it checks three hand-written grids against a PROJ-generated JSON
//! golden. `every_fixture_file_is_classified` is what holds the corpus to the
//! extension allow-list this file reaches it by, so a fixture arriving as
//! `.grb2` or in a subdirectory fails instead of quietly staying outside.
//!
//! A `samples/`-wide sweep, if it is ever wanted, has to be a separate check
//! that *asserts* the corpus is present rather than skipping when it is not.
//!
//! # Two tiers, and why
//!
//! All 105 fixture files are recorded, each with the number of fields it
//! yielded. Six of them yield none — a NetCDF-4 or HDF5 file with no renderable
//! variable — and without their own line they would be absent from the
//! recording rather than named in it, which is the difference between a corpus
//! that is enumerated and one that is merely counted. Every one of the 144
//! fields is recorded too, with its resolved geometry, a source-projection
//! render and an equirectangular render.
//!
//! [`DEEP_FIELDS`] then names 15 of them and gives each the full matrix: every
//! target projection under both resamplings, three manual render windows, the
//! flipped source view, probes, contours and both CSV formats. That is where
//! the per-family warp setups actually differ. Running the matrix over all 144
//! fields instead of 15 was measured at 59 s against 10 s in
//! release, 3 min 31 s against 38 s in the debug `cargo test` the hook
//! runs, and the extra cases differ only in the data flowing through the same
//! code path.
//!
//! The 15 are one per *source path*, which is a finer partition than the render
//! family: spectral synthesis, HEALPix resampling and two ordinary lat/lon
//! grids all report `family=latlon`, and the fifteen entries cover twelve
//! families. `every_grid_family_in_the_golden_has_a_deep_field` enforces the
//! coarser half — a family arriving with no representative fails rather than
//! being covered shallowly — while the finer half is the named list itself,
//! held in place by `every_deep_field_is_still_in_the_recording`.
//!
//! Two of the fifteen trace no contours at all under any target: the regular
//! Gaussian and the rotated lat/lon fixtures are constant fields, so the auto
//! levels have nothing to cross. They are each the only fixture of their family
//! in the corpus, so there is nothing to swap them for; the tracer is covered
//! by the other thirteen.
//!
//! # Re-recording
//!
//! ```sh
//! FIELDGLASS_UPDATE_GOLDEN=1 cargo test -p fieldglass-napi characterisation
//! ```
//!
//! That run **fails on purpose** after writing: a run that re-recorded has
//! verified nothing, so it must not be mistaken for a green one, and an
//! environment with the variable left set cannot silently turn the golden into
//! a no-op that re-baselines every diff it exists to catch.
//!
//! A diff to this file is a behaviour change in the display path. During #571
//! and #572 it should be empty; anywhere else it needs a reason in the commit
//! message.

use super::*;
use std::collections::{BTreeMap, BTreeSet};

// The FNV fold and the libm fingerprint, shared with
// `fieldglass-core/tests/planar_inverse_golden.rs` so one `REFERENCE_LIBM`
// serves both goldens. This crate is a cdylib whose tests are unit tests, so it
// has no integration-test directory to hold a copy of its own.
#[path = "../../fieldglass-core/tests/support/mod.rs"]
mod support;

/// The recording, relative to this crate's manifest directory (which is what
/// cargo makes the working directory of a test).
const GOLDEN_PATH: &str = "golden/render_characterisation.tsv";

/// Set to re-record rather than compare. Deliberately not a `--ignored` test:
/// the golden must run in the ordinary gate, and only writing is opt-in.
const UPDATE_ENV: &str = "FIELDGLASS_UPDATE_GOLDEN";

/// Every target projection the picker offers, in `RenderOptions::projection`
/// spelling. Written out rather than derived from the parser so that a target
/// being added, renamed or dropped shows up here as a decision.
const PROJECTIONS: [&str; 8] = [
    "source",
    "equirectangular",
    "web_mercator",
    "orthographic",
    "polar_stereographic",
    "mollweide",
    "robinson",
    "equal_earth",
];

/// The pixels every deep field is probed at, in output-raster coordinates.
///
/// A reprojected raster is at least 720 wide; a source raster is the source
/// grid, which for a deep field is as small as 6x5. So (0, 0) hits every
/// non-empty raster, (7, 3) and (359, 180) hit the reprojected ones and fall
/// off the smallest source views, and (719, 719) is off the end of nearly
/// everything. Every one of the four is recorded as a hit or a miss, so "off
/// the raster" is a result here rather than an untested path.
const PROBE_PIXELS: [(u32, u32); 4] = [(0, 0), (7, 3), (359, 180), (719, 719)];

/// A manual render window as `RenderOptions` states one:
/// `(lat_min, lat_max, lon_min, lon_max)`, in degrees.
type Window = (f64, f64, f64, f64);

/// One of [`WINDOWS`]: its name in a case id, and the box itself.
type NamedWindow = (&'static str, Window);

/// One fixture directory: the prefix its fields carry in a field id, the path,
/// the extensions this golden reads as data, and the companion extensions it
/// knows are not data.
type CorpusDir = (
    &'static str,
    &'static str,
    &'static [&'static str],
    &'static [&'static str],
);

/// The manual render windows the bounds cases ask for, by name.
///
/// `world` is the whole globe, `afro_eurasia` is 20 degrees south to 40 north
/// and 30 west to 60 east, `conus` is 25 to 50 north and 125 to 65 west. None
/// is degenerate and the middle one crosses both the equator and the prime
/// meridian, so a target that ignores a window and one that honours it cannot
/// agree by accident.
///
/// Three rather than one because a window that misses records only
/// `opaque = 0`, which pins the *exclusion* and nothing else, and no single
/// fixed box intersects a corpus spread over Europe, North America, a
/// geostationary disc and a polar swath. `world` is the one every field that
/// renders at all paints under, so the clipping is pinned positively for each
/// of them; the two regional boxes are what make a grid that ignores its
/// window look different from one that honours it.
const WINDOWS: [NamedWindow; 3] = [
    ("world", (-90.0, 90.0, -180.0, 180.0)),
    ("afro_eurasia", (-20.0, 40.0, -30.0, 60.0)),
    ("conus", (25.0, 50.0, -125.0, -65.0)),
];

/// The fields given the full matrix: one per source path.
///
/// A finer partition than the render family, and deliberately so — spectral
/// synthesis, HEALPix resampling and two ordinary lat/lon grids all report
/// `family=latlon`, so these fifteen cover twelve families. Chosen as the
/// cheapest fixture in each family, plus the two source paths that synthesise a
/// lat/lon grid instead of describing one, plus the two refusals: a §3.20 whose
/// stated radius places no point (#603) and a §3.51 whose geometry never
/// resolves at all. `every_grid_family_in_the_golden_has_a_deep_field` asserts
/// the family half of that coverage; the source-path half is this list, held in
/// place by `every_deep_field_is_still_in_the_recording`.
const DEEP_FIELDS: [&str; 15] = [
    // A lat/lon grid whose row order is the awkward one, from GRIB1.
    "grib1/j_consecutive_latlon.grib1#00",
    // Spectral: no grid of its own, synthesised onto a global 0.5° lat/lon.
    "grib1/spectral_simple_t63.grib1#00",
    // A §3.51 bi-Fourier grid: `resolved_meta` refuses it, so this is the
    // `family=unresolved` representative — what every target does with a field
    // whose geometry never arrives.
    "grib2/bifourier_ellipse_ieee32.grib2#00",
    "grib2/eta_lambert_msg0.grib2#00",
    "grib2/healpix_n4_ring.grib2#00",
    "grib2/lambert_azimuthal_efas.grib2#00",
    "grib2/octahedral_gaussian_o32.grib2#00",
    // A §3.20 whose stated radius places no point: every planar target refuses.
    "grib2/polar_stereographic_surface.grib2#00",
    "grib2/regular_gaussian_f32.grib2#00",
    "grib2/rotated_latlon_surface.grib2#00",
    "grib2/runlength_4bit_regular_latlon.grib2#00",
    "grib2/transverse_mercator_ukv.grib2#00",
    // A geostationary scan-angle grid, and a swath with a cell-centre index.
    "netcdf/goes_geostationary.nc#Rad",
    "netcdf/mirs_swath_n21.nc#TPW",
    // WRF's Mercator projection: the only Mercator grid in the corpus, and a
    // geometry synthesised from projection attributes rather than declared.
    "netcdf/wrf_mercator.nc#T2",
];

/// One recorded case.
#[derive(PartialEq, Eq)]
struct Row {
    /// The discrete result, asserted on every target.
    portable: String,
    /// A fold over everything the call produced, asserted on the reference
    /// libm only.
    exact: u64,
}

/// The recording, keyed by `(field, case)` so a missing or extra case is a
/// key difference rather than a line-number difference.
type Golden = BTreeMap<(String, String), Row>;

/// A `RenderOptions` with **every** field stated.
///
/// Nothing here is left to a default. A golden that moved when
/// `default_colormap()` changed would fail on the wrong commit, and the four
/// `None`-defaulting knobs (colormap, reverse, scale, presets) are exactly the
/// ones a future change is most likely to redefine.
fn options(projection: &str, resampling: &str) -> RenderOptions {
    RenderOptions {
        projection: projection.to_string(),
        // The azimuthal and world targets read `center_lat`/`center_lon`, which
        // are pinned below, so the preset is never consulted; stated anyway.
        projection_preset: Some("atlantic".to_string()),
        center_lat: Some(0.0),
        center_lon: Some(0.0),
        resampling: resampling.to_string(),
        flip_y: false,
        range_min: None,
        range_max: None,
        bounds_lat_min: None,
        bounds_lat_max: None,
        bounds_lon_min: None,
        bounds_lon_max: None,
        colormap: Some("viridis".to_string()),
        reverse_colormap: Some(false),
        scale_mode: Some("linear".to_string()),
    }
}

/// `options`, with one of [`WINDOWS`] filled in.
fn windowed(projection: &str, resampling: &str, window: Window) -> RenderOptions {
    let (lat_min, lat_max, lon_min, lon_max) = window;
    RenderOptions {
        bounds_lat_min: Some(lat_min),
        bounds_lat_max: Some(lat_max),
        bounds_lon_min: Some(lon_min),
        bounds_lon_max: Some(lon_max),
        ..options(projection, resampling)
    }
}

/// One field of the corpus, borrowed from the handle that owns its file.
enum Subject<'a> {
    Grib1(&'a Grib1Handle, u32),
    Grib2(&'a Grib2Handle, u32),
    /// A NetCDF slice: variable, image axes, and the held index of every other
    /// dimension.
    Netcdf(&'a NetcdfHandle, u32, u32, u32, Vec<u32>),
}

impl Subject<'_> {
    /// The geometry the display path will actually run on — the resolved one,
    /// so a spectral or HEALPix message reports its synthesis grid.
    fn meta(&self) -> napi::Result<MessageMeta> {
        match self {
            Self::Grib1(h, i) => h.resolved_meta(*i),
            Self::Grib2(h, i) => h.resolved_meta(*i),
            Self::Netcdf(h, v, y, x, _) => {
                let var = h.renderable(*v)?;
                h.slice_meta(&var, *y as usize, *x as usize)
            }
        }
    }

    fn render(&self, o: RenderOptions) -> napi::Result<RenderedGrid> {
        match self {
            Self::Grib1(h, i) => h.render_grid(*i, o),
            Self::Grib2(h, i) => h.render_grid(*i, o),
            Self::Netcdf(h, v, y, x, idx) => h.render_slice(*v, *y, *x, idx.clone(), o),
        }
    }

    fn probe(&self, o: RenderOptions, px: u32, py: u32) -> napi::Result<Option<ProbeResult>> {
        match self {
            Self::Grib1(h, i) => h.probe(*i, o, px, py),
            Self::Grib2(h, i) => h.probe(*i, o, px, py),
            Self::Netcdf(h, v, y, x, idx) => h.probe(*v, *y, *x, idx.clone(), o, px, py),
        }
    }

    fn contours(&self, o: RenderOptions) -> napi::Result<ProjectedOverlay> {
        // `interval: None` — the eight auto levels over the field's own used
        // range. A fixed absolute interval cannot serve a corpus that mixes
        // kelvin, pascals and metres per second: it would trace nothing for one
        // field and hundreds of thousands of segments for the next.
        match self {
            Self::Grib1(h, i) => h.project_contours(*i, o, None),
            Self::Grib2(h, i) => h.project_contours(*i, o, None),
            Self::Netcdf(h, v, y, x, idx) => h.project_contours(*v, *y, *x, idx.clone(), o, None),
        }
    }

    fn csv(&self, format: &str) -> napi::Result<napi::bindgen_prelude::Buffer> {
        let format = format.to_string();
        match self {
            Self::Grib1(h, i) => h.export_csv(*i, format),
            Self::Grib2(h, i) => h.export_csv(*i, format),
            Self::Netcdf(h, v, y, x, idx) => h.export_csv(*v, *y, *x, idx.clone(), format),
        }
    }
}

/// The three fixture directories, with the extensions this golden treats as
/// data and the ones it knows are not.
///
/// Written as data so `every_fixture_file_is_classified` can hold the corpus to
/// it. Without that, a fixture added as `.grb2`, `.nc4` or `.hdf5` would simply
/// never enter the recording, and nothing would say so — the same fail-open as
/// a `samples/`-keyed golden, arriving by a different door.
const CORPUS: [CorpusDir; 3] = [
    (
        "grib1",
        "../fieldglass-grib1/tests/fixtures",
        &["grib", "grib1"],
        &["json", "txt", "md"],
    ),
    (
        "grib2",
        "../fieldglass-grib2/tests/fixtures",
        &["grib2"],
        &["json", "txt", "md", "j2k"],
    ),
    (
        "netcdf",
        "../fieldglass-netcdf/tests/fixtures",
        &["h5", "nc"],
        &["json", "txt", "md", "py"],
    ),
];

/// Fixture files of one extension in one crate's corpus, in path order.
fn fixtures(dir: &str, extension: &str) -> Vec<std::path::PathBuf> {
    let mut paths: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{dir} is the committed fixture corpus: {e}"))
        .map(|entry| entry.expect("directory entry").path())
        .filter(|p| p.extension().and_then(|e| e.to_str()) == Some(extension))
        .collect();
    paths.sort();
    paths
}

/// The file's name, for the field id.
fn stem(path: &std::path::Path) -> String {
    path.file_name()
        .expect("fixture file name")
        .to_string_lossy()
        .into_owned()
}

/// What a visitor is handed: one file, then each of the fields it yielded.
enum Visit<'a> {
    /// A fixture file, with whether it parsed and how many fields it produced.
    File { parsed: bool, fields: usize },
    /// One message or slice of that file.
    Field(&'a Subject<'a>),
}

/// Visit the committed corpus in a fixed order: every file, and every field
/// each file yields.
///
/// **Every file is visited, including the ones that yield nothing.** Six
/// NetCDF-4 and HDF5 fixtures have no renderable variable at all. Recording
/// fields alone would leave those six out of the recording entirely, so nobody
/// reading it could tell them apart from files that are not in the corpus —
/// and a seventh joining them would be a silent shrink rather than a diff.
fn for_each_visit(mut f: impl FnMut(String, Visit<'_>)) {
    for extension in CORPUS[0].2 {
        for path in fixtures(CORPUS[0].1, extension) {
            let file = format!("grib1/{}", stem(&path));
            let bytes = std::fs::read(&path).expect("fixture bytes");
            let Ok(reader) = Grib1Reader::from_bytes(bytes) else {
                f(
                    file,
                    Visit::File {
                        parsed: false,
                        fields: 0,
                    },
                );
                continue;
            };
            let count = reader.messages.len();
            f(
                file.clone(),
                Visit::File {
                    parsed: true,
                    fields: count,
                },
            );
            let handle = Grib1Handle {
                reader,
                decoded: Mutex::new(std::collections::HashMap::new()),
                synthesized: Mutex::new(std::collections::HashMap::new()),
            };
            for i in 0..count {
                f(
                    format!("{file}#{i:02}"),
                    Visit::Field(&Subject::Grib1(&handle, i as u32)),
                );
            }
        }
    }
    for path in fixtures(CORPUS[1].1, CORPUS[1].2[0]) {
        let file = format!("grib2/{}", stem(&path));
        let bytes = std::fs::read(&path).expect("fixture bytes");
        let Ok(reader) = Grib2Reader::from_bytes(bytes) else {
            f(
                file,
                Visit::File {
                    parsed: false,
                    fields: 0,
                },
            );
            continue;
        };
        let count = reader.messages.len();
        f(
            file.clone(),
            Visit::File {
                parsed: true,
                fields: count,
            },
        );
        let handle = Grib2Handle {
            reader,
            decoded: Mutex::new(std::collections::HashMap::new()),
            synthesized: Mutex::new(std::collections::HashMap::new()),
        };
        for i in 0..count {
            f(
                format!("{file}#{i:02}"),
                Visit::Field(&Subject::Grib2(&handle, i as u32)),
            );
        }
    }
    for extension in CORPUS[2].2 {
        for path in fixtures(CORPUS[2].1, extension) {
            let file = format!("netcdf/{}", stem(&path));
            let bytes = std::fs::read(&path).expect("fixture bytes");
            let Ok(reader) = NetcdfReader::from_bytes(bytes) else {
                f(
                    file,
                    Visit::File {
                        parsed: false,
                        fields: 0,
                    },
                );
                continue;
            };
            // `unwrap_or_default` mirrors the handle's own constructor: a view
            // that will not resolve leaves the picker empty rather than failing
            // the open. The file's line records that it resolved to nothing.
            let view = reader.view().unwrap_or_default();
            let handle = NetcdfHandle {
                reader,
                view,
                decoded: Mutex::new(std::collections::HashMap::new()),
                curvilinear: Mutex::new(std::collections::HashMap::new()),
            };
            let variables = handle.variables();
            f(
                file.clone(),
                Visit::File {
                    parsed: true,
                    fields: variables.len(),
                },
            );
            for v in variables {
                let (y, x) = slice_axes(&v);
                let indices = vec![0u32; v.dims.len()];
                let subject = Subject::Netcdf(&handle, v.variable_index as u32, y, x, indices);
                f(format!("{file}#{}", v.name), Visit::Field(&subject));
            }
        }
    }
}

/// The image axes of a NetCDF variable: the CF-detected pair, or the trailing
/// two dimensions when detection found none.
///
/// This golden pins the engine, not the picker. The host falls back to
/// dimensions 0 and 1, which for a WRF file is `Time` × `south_north` — a 5x1
/// strip that reaches none of the four `synth_*_meta` builders (measured, and
/// recorded on #549). The trailing pair is the plane those grids actually lie
/// on, so choosing it here is what puts WRF's Lambert, polar stereographic and
/// Mercator geometry in the recording at all. Every other variable in the
/// corpus has either a detected pair or exactly two dimensions, where the two
/// rules agree.
fn slice_axes(v: &NetcdfVariableMeta) -> (u32, u32) {
    match (v.detected_y_dim, v.detected_x_dim) {
        (Some(y), Some(x)) => (y as u32, x as u32),
        _ => {
            // `renderable_variables` only offers variables of two dimensions or
            // more, so the trailing pair always exists. Assert it rather than
            // saturating into `(0, 0)`, which would silently record a
            // degenerate slice whose two axes are the same dimension.
            assert!(
                v.dims.len() >= 2,
                "{} has {} dimensions; a renderable variable has at least two",
                v.name,
                v.dims.len()
            );
            let last = (v.dims.len() - 1) as u32;
            (last - 1, last)
        }
    }
}

/// Start a fold.
fn hasher() -> u64 {
    support::FNV_OFFSET
}

/// Fold a `&str`, length-prefixed so `"ab" + "c"` and `"a" + "bc"` differ.
fn mix_str(h: &mut u64, s: &str) {
    support::fnv(h, &(s.len() as u64).to_le_bytes());
    support::fnv(h, s.as_bytes());
}

/// Fold an optional `f64` by its bits, with a presence tag.
fn mix_opt_f64(h: &mut u64, v: Option<f64>) {
    match v {
        None => support::fnv(h, &[0xff]),
        Some(v) => {
            support::fnv(h, &[0x01]);
            support::fnv(h, &v.to_bits().to_le_bytes());
        }
    }
}

/// Fold an optional `i32`, with a presence tag.
fn mix_opt_i32(h: &mut u64, v: Option<i32>) {
    match v {
        None => support::fnv(h, &[0xff]),
        Some(v) => {
            support::fnv(h, &[0x01]);
            support::fnv(h, &v.to_le_bytes());
        }
    }
}

/// Fold an optional `bool`, with a presence tag.
fn mix_opt_bool(h: &mut u64, v: Option<bool>) {
    match v {
        None => support::fnv(h, &[0xff]),
        Some(v) => {
            support::fnv(h, &[0x01]);
            support::fnv(h, &[u8::from(v)]);
        }
    }
}

/// Fold an optional string, with a presence tag.
fn mix_opt_str(h: &mut u64, v: Option<&str>) {
    match v {
        None => support::fnv(h, &[0xff]),
        Some(v) => {
            support::fnv(h, &[0x01]);
            mix_str(h, v);
        }
    }
}

/// An error reason with every run of digits replaced by `#`, tabs and newlines
/// flattened to spaces.
///
/// Which error a case produces is a discrete fact and belongs in the portable
/// column — without it, 98 of the 999 rows assert only "this still fails", and
/// on a foreign libm the recording cannot tell "slice has no x-axis size" from
/// "unknown colormap". The reasons that interpolate an `f64` are what kept them
/// out; eliding the digits keeps the sentence and drops the only part a
/// different libm could move.
fn elided(reason: &str) -> String {
    let mut out = String::with_capacity(reason.len());
    let mut in_number = false;
    for c in reason.chars() {
        if c.is_ascii_digit() {
            if !in_number {
                out.push('#');
                in_number = true;
            }
            continue;
        }
        in_number = false;
        out.push(if c == '\t' || c == '\n' || c == '\r' {
            ' '
        } else {
            c
        });
    }
    out
}

/// How an error is recorded: the reason verbatim in the fold, because the
/// message a host surfaces is part of what #571 and #572 must preserve, and
/// the same reason with its numbers elided in the portable column, so a
/// foreign libm still checks *which* refusal this is.
fn error_row(e: &napi::Error) -> Row {
    let mut h = hasher();
    mix_str(&mut h, "err");
    mix_str(&mut h, &e.reason);
    Row {
        portable: format!("err: {}", elided(&e.reason)),
        exact: h,
    }
}

/// The resolved geometry: family, raster shape and reprojection offer in the
/// open, every geometry-defining field in the fold.
///
/// The fold destructures `MetaGeometry` exhaustively with no rest pattern, for
/// the same reason `MessageMeta::geometry()` does: a field added to the
/// geometry is a compile error here until someone decides where it goes.
/// Folding each field by its own bits rather than through `Debug` also keeps
/// the recording out of reach of two things that are not behaviour — the
/// toolchain's float formatting, and the field names.
fn meta_row(subject: &Subject<'_>) -> Row {
    let meta = match subject.meta() {
        Ok(m) => m,
        // A geometry that will not resolve is its own family as far as the
        // coverage guard is concerned: `family=unresolved` keeps those fields
        // inside `every_grid_family_in_the_golden_has_a_deep_field` instead of
        // being skipped by it, which is how the bi-Fourier grids were invisible
        // to a check written to notice exactly that.
        Err(e) => {
            let row = error_row(&e);
            return Row {
                portable: format!("family=unresolved {}", row.portable),
                ..row
            };
        }
    };
    let mut h = hasher();
    mix_str(&mut h, "meta");
    let MetaGeometry {
        grid_type,
        grid_ni,
        grid_nj,
        lat_first,
        lon_first,
        lat_last,
        lon_last,
        earth_radius_metres,
        lambert_lad,
        lambert_lov,
        lambert_dx_metres,
        lambert_dy_metres,
        lambert_latin1,
        lambert_latin2,
        gaussian_n_parallels,
        polar_stereo_lov,
        polar_stereo_lad,
        polar_stereo_dx_metres,
        polar_stereo_dy_metres,
        polar_stereo_south_pole,
        lambert_azimuthal_semi_major_metres,
        lambert_azimuthal_semi_minor_metres,
        lambert_azimuthal_standard_parallel,
        lambert_azimuthal_central_longitude,
        lambert_azimuthal_dx_metres,
        lambert_azimuthal_dy_metres,
        transverse_mercator_semi_major_metres,
        transverse_mercator_semi_minor_metres,
        transverse_mercator_lat_ref,
        transverse_mercator_lon_ref,
        transverse_mercator_scale_factor,
        transverse_mercator_false_easting_metres,
        transverse_mercator_false_northing_metres,
        transverse_mercator_x1_metres,
        transverse_mercator_y1_metres,
        transverse_mercator_dx_metres,
        transverse_mercator_dy_metres,
        rotated_south_pole_lat,
        rotated_south_pole_lon,
        rotated_angle_of_rotation,
        geos_sub_lon,
        geos_height,
        geos_r_eq,
        geos_r_pol,
        geos_sweep_x,
        geos_x0,
        geos_dx_rad,
        geos_y0,
        geos_dy_rad,
        j_scans_positive,
    } = meta.geometry();
    mix_opt_str(&mut h, grid_type.as_deref());
    mix_opt_i32(&mut h, *grid_ni);
    mix_opt_i32(&mut h, *grid_nj);
    mix_opt_f64(&mut h, *lat_first);
    mix_opt_f64(&mut h, *lon_first);
    mix_opt_f64(&mut h, *lat_last);
    mix_opt_f64(&mut h, *lon_last);
    mix_opt_f64(&mut h, *earth_radius_metres);
    mix_opt_f64(&mut h, *lambert_lad);
    mix_opt_f64(&mut h, *lambert_lov);
    mix_opt_f64(&mut h, *lambert_dx_metres);
    mix_opt_f64(&mut h, *lambert_dy_metres);
    mix_opt_f64(&mut h, *lambert_latin1);
    mix_opt_f64(&mut h, *lambert_latin2);
    mix_opt_i32(&mut h, *gaussian_n_parallels);
    mix_opt_f64(&mut h, *polar_stereo_lov);
    mix_opt_f64(&mut h, *polar_stereo_lad);
    mix_opt_f64(&mut h, *polar_stereo_dx_metres);
    mix_opt_f64(&mut h, *polar_stereo_dy_metres);
    mix_opt_bool(&mut h, *polar_stereo_south_pole);
    mix_opt_f64(&mut h, *lambert_azimuthal_semi_major_metres);
    mix_opt_f64(&mut h, *lambert_azimuthal_semi_minor_metres);
    mix_opt_f64(&mut h, *lambert_azimuthal_standard_parallel);
    mix_opt_f64(&mut h, *lambert_azimuthal_central_longitude);
    mix_opt_f64(&mut h, *lambert_azimuthal_dx_metres);
    mix_opt_f64(&mut h, *lambert_azimuthal_dy_metres);
    mix_opt_f64(&mut h, *transverse_mercator_semi_major_metres);
    mix_opt_f64(&mut h, *transverse_mercator_semi_minor_metres);
    mix_opt_f64(&mut h, *transverse_mercator_lat_ref);
    mix_opt_f64(&mut h, *transverse_mercator_lon_ref);
    mix_opt_f64(&mut h, *transverse_mercator_scale_factor);
    mix_opt_f64(&mut h, *transverse_mercator_false_easting_metres);
    mix_opt_f64(&mut h, *transverse_mercator_false_northing_metres);
    mix_opt_f64(&mut h, *transverse_mercator_x1_metres);
    mix_opt_f64(&mut h, *transverse_mercator_y1_metres);
    mix_opt_f64(&mut h, *transverse_mercator_dx_metres);
    mix_opt_f64(&mut h, *transverse_mercator_dy_metres);
    mix_opt_f64(&mut h, *rotated_south_pole_lat);
    mix_opt_f64(&mut h, *rotated_south_pole_lon);
    mix_opt_f64(&mut h, *rotated_angle_of_rotation);
    mix_opt_f64(&mut h, *geos_sub_lon);
    mix_opt_f64(&mut h, *geos_height);
    mix_opt_f64(&mut h, *geos_r_eq);
    mix_opt_f64(&mut h, *geos_r_pol);
    mix_opt_bool(&mut h, *geos_sweep_x);
    mix_opt_f64(&mut h, *geos_x0);
    mix_opt_f64(&mut h, *geos_dx_rad);
    mix_opt_f64(&mut h, *geos_y0);
    mix_opt_f64(&mut h, *geos_dy_rad);
    mix_opt_bool(&mut h, *j_scans_positive);
    Row {
        portable: format!(
            "family={} ni={} nj={} reprojectable={}",
            meta.grid_type.as_deref().unwrap_or("-"),
            meta.grid_ni.unwrap_or(-1),
            meta.grid_nj.unwrap_or(-1),
            meta.reprojectable,
        ),
        exact: h,
    }
}

/// A render: raster shape and opaque-pixel count in the open, the RGBA bytes,
/// the used range, the echoed window and the summary string in the fold.
///
/// The opaque count is the discrete half of a warp — for every output pixel,
/// whether the inverse landed on the grid at all. ADR-0009 measured that
/// decision as identical across libms over 323,620 probes; recording it here
/// is what would catch it ceasing to be.
fn render_row(subject: &Subject<'_>, o: RenderOptions) -> Row {
    let rendered = match subject.render(o) {
        Ok(r) => r,
        Err(e) => return error_row(&e),
    };
    let mut h = hasher();
    mix_str(&mut h, "render");
    support::fnv(&mut h, &rendered.width.to_le_bytes());
    support::fnv(&mut h, &rendered.height.to_le_bytes());
    support::fnv(&mut h, &rendered.rgba);
    mix_opt_f64(&mut h, Some(rendered.used_min));
    mix_opt_f64(&mut h, Some(rendered.used_max));
    mix_opt_f64(&mut h, rendered.used_lat_min);
    mix_opt_f64(&mut h, rendered.used_lat_max);
    mix_opt_f64(&mut h, rendered.used_lon_min);
    mix_opt_f64(&mut h, rendered.used_lon_max);
    mix_str(&mut h, &rendered.projection_summary);
    let opaque = rendered
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|p| p[3] != 0)
        .count();
    Row {
        portable: format!(
            "ok w={} h={} opaque={opaque}",
            rendered.width, rendered.height
        ),
        exact: h,
    }
}

/// A point probe: hit or miss and the source cell in the open, the coordinates
/// and the value in the fold.
///
/// `grid_i`/`grid_j` are in the portable column on purpose. They are the
/// floored index of a per-pixel inverse, so they are the most sensitive
/// *discrete* output the display path has, and a libm that started moving one
/// is a libm that is about to move a pixel.
fn probe_row(subject: &Subject<'_>, o: RenderOptions, px: u32, py: u32) -> Row {
    let probed = match subject.probe(o, px, py) {
        Ok(p) => p,
        Err(e) => return error_row(&e),
    };
    let mut h = hasher();
    mix_str(&mut h, "probe");
    let Some(p) = probed else {
        mix_str(&mut h, "miss");
        return Row {
            portable: "miss".to_string(),
            exact: h,
        };
    };
    mix_str(&mut h, "hit");
    mix_opt_f64(&mut h, p.lat);
    mix_opt_f64(&mut h, p.lon);
    mix_opt_f64(&mut h, p.value);
    mix_opt_i32(&mut h, p.grid_i);
    mix_opt_i32(&mut h, p.grid_j);
    let cell = match (p.grid_i, p.grid_j) {
        (Some(i), Some(j)) => format!("{i},{j}"),
        _ => "-".to_string(),
    };
    Row {
        portable: format!(
            "hit cell={cell} geo={} value={}",
            if p.lat.is_some() && p.lon.is_some() {
                "yes"
            } else {
                "no"
            },
            if p.value.is_some() { "yes" } else { "no" },
        ),
        exact: h,
    }
}

/// Contours: run and vertex counts in the open, every vertex in the fold.
fn contour_row(subject: &Subject<'_>, o: RenderOptions) -> Row {
    let overlay = match subject.contours(o) {
        Ok(c) => c,
        Err(e) => return error_row(&e),
    };
    let mut h = hasher();
    mix_str(&mut h, "contours");
    for length in overlay.seg_lengths.as_ref() {
        support::fnv(&mut h, &length.to_le_bytes());
    }
    for v in overlay.xy.as_ref() {
        support::fnv(&mut h, &v.to_bits().to_le_bytes());
    }
    Row {
        portable: format!(
            "ok runs={} points={}",
            overlay.seg_lengths.len(),
            overlay.xy.len() / 2
        ),
        exact: h,
    }
}

/// CSV: the line count in the open (header included, so it is one more than
/// the number of data rows), every byte in the fold.
fn csv_row(subject: &Subject<'_>, format: &str) -> Row {
    let csv = match subject.csv(format) {
        Ok(c) => c,
        Err(e) => return error_row(&e),
    };
    let mut h = hasher();
    mix_str(&mut h, "csv");
    support::fnv(&mut h, &csv);
    Row {
        portable: format!("ok lines={}", csv.iter().filter(|&&b| b == b'\n').count()),
        exact: h,
    }
}

/// Every case of one field, in a fixed order.
fn cases(id: &str, subject: &Subject<'_>, into: &mut Golden) {
    let mut record = |case: String, row: Row| {
        let previous = into.insert((id.to_string(), case.clone()), row);
        assert!(previous.is_none(), "{id}: duplicate case {case}");
    };
    record("meta".to_string(), meta_row(subject));
    record(
        "render/source/nearest".to_string(),
        render_row(subject, options("source", "nearest")),
    );
    record(
        "render/equirectangular/nearest".to_string(),
        render_row(subject, options("equirectangular", "nearest")),
    );
    if !DEEP_FIELDS.contains(&id) {
        return;
    }
    for projection in PROJECTIONS {
        for resampling in ["nearest", "bilinear"] {
            // `source/nearest` is already recorded above for every field.
            if projection == "source" && resampling == "nearest" {
                continue;
            }
            if projection == "equirectangular" && resampling == "nearest" {
                continue;
            }
            record(
                format!("render/{projection}/{resampling}"),
                render_row(subject, options(projection, resampling)),
            );
        }
    }
    // The manual render window, and the source view painted bottom-up — the
    // two render inputs that are neither a projection nor a resampling.
    for (name, window) in WINDOWS {
        record(
            format!("render/equirectangular/nearest+window/{name}"),
            render_row(subject, windowed("equirectangular", "nearest", window)),
        );
    }
    record(
        "render/source/nearest+flip_y".to_string(),
        render_row(
            subject,
            RenderOptions {
                flip_y: true,
                ..options("source", "nearest")
            },
        ),
    );
    for (px, py) in PROBE_PIXELS {
        for projection in ["source", "equirectangular", "orthographic"] {
            record(
                format!("probe/{projection}/{px},{py}"),
                probe_row(subject, options(projection, "nearest"), px, py),
            );
        }
    }
    for projection in ["source", "equirectangular", "mollweide"] {
        record(
            format!("contours/{projection}/auto"),
            contour_row(subject, options(projection, "nearest")),
        );
    }
    for format in ["matrix", "long"] {
        record(format!("csv/{format}"), csv_row(subject, format));
    }
}

/// Replay the whole corpus.
fn observed() -> Golden {
    let mut golden = Golden::new();
    for_each_visit(|id, visit| match visit {
        Visit::Field(subject) => cases(&id, subject, &mut golden),
        Visit::File { parsed, fields } => {
            let portable = format!("parsed={parsed} fields={fields}");
            let mut h = hasher();
            mix_str(&mut h, "file");
            mix_str(&mut h, &portable);
            let previous =
                golden.insert((id.clone(), "file".to_string()), Row { portable, exact: h });
            assert!(previous.is_none(), "{id}: visited twice");
        }
    });
    golden
}

/// Parse the recording. Every line must have four tab-separated columns; a
/// malformed one is a failure rather than a skipped line.
fn parse(text: &str) -> Golden {
    let mut golden = Golden::new();
    for (n, line) in text.lines().enumerate() {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        let columns: Vec<&str> = line.split('\t').collect();
        assert_eq!(
            columns.len(),
            4,
            "{GOLDEN_PATH}:{}: expected 4 tab-separated columns, got {}",
            n + 1,
            columns.len()
        );
        let exact = u64::from_str_radix(columns[3], 16)
            .unwrap_or_else(|e| panic!("{GOLDEN_PATH}:{}: bad hash {:?}: {e}", n + 1, columns[3]));
        let key = (columns[0].to_string(), columns[1].to_string());
        let previous = golden.insert(
            key.clone(),
            Row {
                portable: columns[2].to_string(),
                exact,
            },
        );
        // A hand-edited file with the same case twice would otherwise keep the
        // last one and drop the other without a word, which is a case that
        // looks recorded and is not.
        assert!(
            previous.is_none(),
            "{GOLDEN_PATH}:{}: {key:?} is recorded twice",
            n + 1
        );
    }
    golden
}

/// Render the recording back out.
fn render_golden(golden: &Golden) -> String {
    let mut out = String::from(
        "# Characterisation golden for render, probe, contours and CSV (#570).\n\
         # field\tcase\tportable\texact\n\
         # Re-record: FIELDGLASS_UPDATE_GOLDEN=1 cargo test -p fieldglass-napi characterisation\n\
         # `portable` is asserted on every target; `exact` only on the recorded\n\
         # libm (ADR-0009). See src/characterisation.rs for what each column holds.\n",
    );
    for ((field, case), row) in golden {
        out.push_str(&format!(
            "{field}\t{case}\t{}\t{:016x}\n",
            row.portable, row.exact
        ));
    }
    out
}

/// Whether this run is re-recording rather than comparing.
///
/// The three tests run concurrently under cargo's harness and the re-record
/// truncates the file in place, so the two that only read it stand aside
/// rather than racing a half-written recording and failing with a confusing
/// parse error instead of the deliberate one.
fn updating() -> bool {
    std::env::var(UPDATE_ENV).is_ok()
}

/// The recording as committed.
fn recorded() -> Golden {
    let text = std::fs::read_to_string(GOLDEN_PATH).unwrap_or_else(|e| {
        panic!(
            "{GOLDEN_PATH} is the recording this test replays and it must be \
             committed: {e}"
        )
    });
    let golden = parse(&text);
    assert!(
        !golden.is_empty(),
        "{GOLDEN_PATH} parsed to no cases at all — an empty recording would \
         pass every comparison below while pinning nothing"
    );
    golden
}

/// The corpus, replayed against its recording.
///
/// The comparison is in three parts, and each says something different: the
/// case *set* catches a fixture leaving the corpus or a case being dropped, the
/// portable column catches a behaviour change on any target, and the exact
/// column catches one down to the last RGBA byte where the libm is the recorded
/// one.
#[test]
fn the_display_path_matches_its_recording() {
    let observed = observed();
    if updating() {
        std::fs::write(GOLDEN_PATH, render_golden(&observed)).expect("write the golden");
        // Fail rather than pass. A run that rewrote the recording has verified
        // nothing, and an environment with this variable left set would
        // otherwise turn the whole golden into a green no-op that quietly
        // re-baselines every diff it was supposed to catch.
        panic!(
            "re-recorded {} cases into {GOLDEN_PATH}. Re-run without {UPDATE_ENV} \
             to verify, and read the diff before committing it.",
            observed.len()
        );
    }
    let recorded = recorded();

    let observed_keys: BTreeSet<&(String, String)> = observed.keys().collect();
    let recorded_keys: BTreeSet<&(String, String)> = recorded.keys().collect();
    let missing: Vec<&&(String, String)> = recorded_keys.difference(&observed_keys).collect();
    let extra: Vec<&&(String, String)> = observed_keys.difference(&recorded_keys).collect();
    assert!(
        missing.is_empty() && extra.is_empty(),
        "the corpus no longer produces the recorded set of cases.\n\
         recorded but not produced ({}): {missing:?}\n\
         produced but not recorded ({}): {extra:?}",
        missing.len(),
        extra.len()
    );

    let mut differences = Vec::new();
    for (key, want) in &recorded {
        let got = &observed[key];
        if got.portable != want.portable {
            differences.push(format!(
                "{} {}: recorded {:?}, produced {:?}",
                key.0, key.1, want.portable, got.portable
            ));
        }
    }
    assert!(
        differences.is_empty(),
        "the display path changed what it produces ({} cases):\n{}",
        differences.len(),
        differences.join("\n")
    );

    if !support::is_reference_libm() {
        println!(
            "skipped the exact column: this target's libm is {:#018x}, and the \
             recording was made against {:#018x}. The discrete results above are \
             still compared. See \
             docs/decisions/0009-cross-target-floating-point-agreement.md.",
            support::libm_fingerprint(),
            support::REFERENCE_LIBM,
        );
        return;
    }
    let mut exact = Vec::new();
    for (key, want) in &recorded {
        let got = &observed[key];
        if got.exact != want.exact {
            exact.push(format!(
                "{} {}: recorded {:016x}, produced {:016x}",
                key.0, key.1, want.exact, got.exact
            ));
        }
    }
    assert!(
        exact.is_empty(),
        "the display path produced the same shape but different bytes ({} cases):\n{}",
        exact.len(),
        exact.join("\n")
    );
}

/// Every source grid family in the corpus has a field with the full matrix.
///
/// The shallow tier covers each family's *existence*; only the deep tier covers
/// its warp setup under every target. A new family arriving with no
/// representative would otherwise be recorded — and left untested where it
/// differs. Reads the recording rather than the corpus, so it costs nothing.
/// `meta_geometry` declines exactly the fields the warp declines.
///
/// That equivalence is the whole argument that moving the four grid questions
/// into `core` (#571) preserved behaviour: every one of them is now asked of the
/// geometry this mapping produces, and `GridGeometry::Unsupported` answers "not
/// periodic, no seam wrap, not reprojectable". Those are three plausible
/// answers, none of them a panic, so a family that quietly stopped mapping would
/// move the golden only where a fixture's *output* moved with it — and for a
/// grid that was already source-only, nothing would.
///
/// So it is asserted directly, over all 144 fields: the mapping produces a
/// geometry if and only if `warp_setup_for` produces a setup. One family is
/// excluded and states why — a lookup grid's geometry *is* its cell-centre
/// index, which the DTO does not carry and which `meta_geometry` therefore
/// declines by design while the warp, holding the index, accepts.
#[test]
fn meta_geometry_declines_exactly_what_the_warp_declines() {
    let mut checked = 0usize;
    let mut disagreed: Vec<String> = Vec::new();
    for_each_visit(|id, visit| {
        let Visit::Field(subject) = visit else {
            return;
        };
        let Ok(meta) = subject.meta() else {
            return;
        };
        if meta.grid_type.as_deref() == Some("curvilinear") {
            return;
        }
        let (Ok(ni), Ok(nj)) = (grid_ni(&meta), grid_nj(&meta)) else {
            return;
        };
        checked += 1;
        let mapped = meta_geometry(&meta).kind() != "unsupported";
        let warps = warp_setup_for(&meta, ni, nj, None).is_ok();
        if mapped != warps {
            disagreed.push(format!(
                "{id}: grid_type {:?}, mapped={mapped}, warp_setup_for ok={warps}",
                meta.grid_type
            ));
        }
    });
    assert!(
        checked > 0,
        "the corpus yielded no field with a raster shape"
    );
    assert!(
        disagreed.is_empty(),
        "{} of {checked} fields map to a geometry the warp disagrees about, so \
         the four grid questions are being asked of something the render is not: \
         {disagreed:#?}",
        disagreed.len()
    );
}

#[test]
fn every_grid_family_in_the_golden_has_a_deep_field() {
    if updating() {
        return;
    }
    let golden = recorded();
    let mut families: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for ((field, case), row) in &golden {
        if case != "meta" {
            continue;
        }
        let Some(family) = row
            .portable
            .strip_prefix("family=")
            .and_then(|rest| rest.split(' ').next())
        else {
            continue;
        };
        families
            .entry(family.to_string())
            .or_default()
            .insert(field.clone());
    }
    assert!(
        !families.is_empty(),
        "no field in {GOLDEN_PATH} reported a grid family"
    );
    let deep: BTreeSet<&str> = DEEP_FIELDS.into_iter().collect();
    let uncovered: Vec<&String> = families
        .iter()
        .filter(|(_, fields)| !fields.iter().any(|f| deep.contains(f.as_str())))
        .map(|(family, _)| family)
        .collect();
    assert!(
        uncovered.is_empty(),
        "these grid families have no field in DEEP_FIELDS, so nothing records \
         them under every target projection: {uncovered:?}"
    );
}

/// Every named deep field is still in the recording.
///
/// `DEEP_FIELDS` is a list of names, and a name that stopped matching anything
/// would silently take its family's full matrix with it while every other
/// assertion stayed green. The case-set comparison above is what fails when a
/// fixture leaves the corpus; this is what fails once that has been re-recorded,
/// or when the list is edited to a name that was never there.
#[test]
fn every_deep_field_is_still_in_the_recording() {
    if updating() {
        return;
    }
    let golden = recorded();
    let recorded_fields: BTreeSet<&str> = golden.keys().map(|(field, _)| field.as_str()).collect();
    let gone: Vec<&&str> = DEEP_FIELDS
        .iter()
        .filter(|f| !recorded_fields.contains(*f))
        .collect();
    assert!(
        gone.is_empty(),
        "DEEP_FIELDS names fields that are not in {GOLDEN_PATH}: {gone:?}"
    );
    // Every deep field must actually carry deep cases: a name that is in the
    // corpus but whose matrix was never recorded is the same hole by another
    // route.
    for field in DEEP_FIELDS {
        assert!(
            golden.contains_key(&(field.to_string(), "csv/long".to_string())),
            "{field} is named in DEEP_FIELDS but has no deep cases recorded"
        );
    }
}

/// Every file in the three fixture directories is either data this golden
/// records or a companion this golden knows about.
///
/// The corpus is reached by an extension allow-list, so a fixture added under
/// an unlisted extension — `.grb2`, `.nc4`, `.hdf5` — or in a subdirectory
/// would never enter the recording and no other assertion would notice: the
/// case set would simply not gain it. This is the check that turns that from a
/// silence into a failure.
#[test]
fn every_fixture_file_is_classified() {
    for (label, dir, data, companions) in CORPUS {
        let mut unclassified = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{dir}: {e}")) {
            let path = entry.expect("directory entry").path();
            if path.is_dir() {
                unclassified.push(format!(
                    "{} (a directory; the walk is flat)",
                    path.display()
                ));
                continue;
            }
            let extension = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or_default()
                .to_string();
            if !data.contains(&extension.as_str()) && !companions.contains(&extension.as_str()) {
                unclassified.push(path.display().to_string());
            }
        }
        assert!(
            unclassified.is_empty(),
            "{label}: these files are neither recorded as data ({data:?}) nor \
             known companions ({companions:?}), so they are silently outside \
             the golden: {unclassified:?}"
        );
    }
}
