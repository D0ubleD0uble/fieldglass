//! What the performance harness measures, shared by every tier (#743).
//!
//! One catalogue of scenarios, built from the generated corpus, and one way to
//! run each of them. The `measure` binary runs a scenario under the heap
//! profiler and again over a recording source; the Gungraun bench runs the same
//! scenario under Callgrind; the `report` binary times it. Keeping the scenario
//! in one place is what lets a row in `docs/performance.md` mean the same work
//! in every column.
//!
//! A scenario is **prepared** first and **executed** second. Preparing reads
//! the input, opens the session and decodes whatever the operation takes as
//! given (a render scenario starts from a decoded field). Only execution is
//! measured: the heap profiler is started after `prepare` returns, and Gungraun
//! runs `prepare` as its setup. So a row costs the operation it names and not
//! the loading around it.

mod corpus;
mod io;
pub mod real;

use std::hint::black_box;
use std::rc::Rc;

use fieldglass::{DecodeOptions, Field, PaletteOptions, Session, WarpOptions};
use fieldglass_core::bytes::{FIRST_WINDOW_BYTES, MemoryObjects};
use fieldglass_core::testing::Recording;

pub use corpus::Corpus;
pub use io::Io;
use io::{Recorder, SharedBytes, SharedObjects};

/// The time step a slice decodes. Inside every variable, and not the first, so
/// a reader that only ever looks at plane 0 is not flattered.
pub const SLICE_PLANE: u32 = 3;

/// How many frames a scrub decodes. The same at every size, so the `D` input's
/// scrub differs from `S`'s only in how much variable lies beyond the frames.
pub const SCRUB_FRAMES: u32 = 8;

/// Contour levels, spanning the generated field's 265–300 K.
pub const CONTOUR_LEVELS: &[f64] = &[270.0, 275.0, 280.0, 285.0, 290.0, 295.0];

/// The inputs the render-side operations run on: one message container and one
/// store, at both sizes. The decode varies by format and the render does not,
/// so repeating these over every packing would measure the same work 20 times.
const RENDER_INPUTS: &[&str] = &["grib2-5.0", "zarr-v3-zstd"];

/// What a scenario does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// Open the container and count what it holds.
    Open,
    /// Decode message 0 as a whole field.
    Decode,
    /// Place message 0 on the Earth without decoding it.
    Place,
    /// List an array container's variables and dimensions.
    Variables,
    /// Decode plane [`SLICE_PLANE`] of the variable.
    Slice,
    /// Decode planes `0..SCRUB_FRAMES` in turn, as a time slider does.
    Scrub,
    /// Place the variable's plane without decoding it.
    PlaceSlice,
    /// Resample a decoded field onto its own extent.
    Warp,
    /// Build the colour decision for a decoded field.
    Palette,
    /// Paint a decoded field to RGBA.
    Render,
    /// Trace [`CONTOUR_LEVELS`] through a decoded field.
    Contours,
    /// The input's decompressor alone, over the bytes the decode hands it.
    Codec(Codec),
}

/// A decompressor measured on its own, for the codec-ceiling rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    /// `rust_j2k`, over GRIB2 §7 of a 5.40 message.
    Jpeg2000,
    /// `rust_aec`, over GRIB2 §7 of a 5.42 message.
    Aec,
    /// `png`, over GRIB2 §7 of a 5.41 message.
    Png,
    /// `miniz_oxide`, over the plane's NetCDF-4 chunk.
    Zlib,
    /// `ruzstd`, over the plane's Zarr v3 chunk.
    Zstd,
    /// This workspace's own blosc, over the plane's Zarr v2 chunk.
    Blosc,
}

impl Op {
    /// The operation's name in a scenario id.
    pub fn name(self) -> &'static str {
        match self {
            Op::Open => "open",
            Op::Decode => "decode",
            Op::Place => "place",
            Op::Variables => "variables",
            Op::Slice => "slice",
            Op::Scrub => "scrub",
            Op::PlaceSlice => "place",
            Op::Warp => "warp",
            Op::Palette => "palette",
            Op::Render => "render",
            Op::Contours => "contours",
            Op::Codec(_) => "codec",
        }
    }
}

/// One row: an input and an operation on it.
#[derive(Debug, Clone)]
pub struct Scenario {
    /// `<input>/<operation>`, the key every tier reports under.
    pub id: String,
    /// The corpus input, e.g. `grib2-5.40-L`.
    pub input: String,
    /// What is done to it.
    pub op: Op,
}

/// Every scenario the corpus supports, in a stable order.
pub fn catalogue(corpus: &Corpus) -> Vec<Scenario> {
    let mut out = Vec::new();
    for name in corpus.input_names() {
        let family = family(&name);
        let ops: Vec<Op> = match corpus.format(&name) {
            "grib1" | "grib2" => vec![Op::Open, Op::Decode, Op::Place],
            _ => vec![
                Op::Open,
                Op::Variables,
                Op::Slice,
                Op::Scrub,
                Op::PlaceSlice,
            ],
        };
        let mut ops = ops;
        if RENDER_INPUTS.contains(&family) && !name.ends_with("-D") {
            ops.extend([Op::Warp, Op::Palette, Op::Render, Op::Contours]);
        }
        if let Some(codec) = codec_of(family) {
            ops.push(Op::Codec(codec));
        }
        for op in ops {
            out.push(Scenario {
                id: format!("{name}/{}", op.name()),
                input: name.clone(),
                op,
            });
        }
    }
    out
}

/// The input's name without its size suffix: `grib2-5.40-L` → `grib2-5.40`.
pub fn family(input: &str) -> &str {
    input.rsplit_once('-').map_or(input, |(family, _)| family)
}

fn codec_of(family: &str) -> Option<Codec> {
    Some(match family {
        "grib2-5.40" => Codec::Jpeg2000,
        "grib2-5.42" => Codec::Aec,
        "grib2-5.41" => Codec::Png,
        "netcdf4-zlib" => Codec::Zlib,
        "zarr-v3-zstd" => Codec::Zstd,
        "zarr-v2-blosc" => Codec::Blosc,
        _ => return None,
    })
}

/// Where a prepared scenario's bytes come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Via {
    /// The input in memory, behind the plainest source the reader takes. What
    /// the heap and instruction tiers measure, so the recording's own
    /// bookkeeping is not counted as the reader's.
    Memory,
    /// The same input behind [`Recording`], for the I/O tier.
    Recorded,
}

/// A scenario with everything but its operation done.
pub struct Prepared {
    op: Op,
    state: State,
    recorder: Recorder,
    /// Kept until the prepared scenario is dropped, so nothing the operation
    /// produced is freed inside the measurement.
    output: Option<Box<dyn std::any::Any>>,
    /// Bytes per value in the decoded field, once a decoding operation has run.
    value_width: Option<u32>,
}

enum State {
    /// Bytes not yet opened: the open scenarios.
    Bytes {
        format: String,
        bytes: Vec<u8>,
    },
    /// A store not yet opened.
    Objects(MemoryObjects),
    Session {
        session: Session,
        variable: u32,
    },
    // Boxed: a decoded field's metadata makes this variant ten times the
    // others, and the enum is moved into and out of `execute`.
    Field(Box<(Session, Field)>),
    Codec {
        codec: Codec,
        bytes: Vec<u8>,
        aec: Option<[u32; 4]>,
        cells: usize,
    },
    Taken,
}

impl Prepared {
    /// Load, open and pre-decode `scenario` so only its operation remains.
    ///
    /// # Panics
    ///
    /// On an input the corpus does not hold or a reader that refuses it: the
    /// corpus is generated, so either is a broken harness, not a finding.
    pub fn new(corpus: &Corpus, scenario: &Scenario, via: Via) -> Self {
        let name = scenario.input.as_str();
        let format = corpus.format(name).to_string();
        let mut recorder = Recorder::None;
        let state = match scenario.op {
            Op::Codec(codec) => codec_input(corpus, name, codec),
            Op::Open => {
                if format == "zarr" {
                    State::Objects(corpus.objects(name))
                } else {
                    State::Bytes {
                        format,
                        bytes: corpus.bytes(name),
                    }
                }
            }
            op => {
                let session = open(corpus, name, via, &mut recorder);
                let variable = if format.starts_with("grib") {
                    0
                } else {
                    variable_index(&session, corpus.variable(name))
                };
                if matches!(op, Op::Warp | Op::Palette | Op::Render | Op::Contours) {
                    let field = if format.starts_with("grib") {
                        session.decode(0, &DecodeOptions::default())
                    } else {
                        session.decode_slice(
                            variable,
                            1,
                            2,
                            &[SLICE_PLANE, 0, 0],
                            &DecodeOptions::default(),
                        )
                    }
                    .expect("a generated input decodes");
                    State::Field(Box::new((session, field)))
                } else {
                    State::Session { session, variable }
                }
            }
        };
        if via == Via::Recorded && matches!(scenario.op, Op::Open) {
            // An open scenario owns its bytes until it runs, so the recording is
            // wrapped around them here rather than by `open`.
            recorder = Recorder::Pending;
        }
        recorder.forget();
        Self {
            op: scenario.op,
            state,
            recorder,
            output: None,
            value_width: None,
        }
    }

    /// Run the operation. Returns how many cells (or decompressed elements) one
    /// output holds, which is what a per-cell bound divides by. A scrub
    /// returns one frame's cells: its frames are alive one at a time, so the
    /// peak is a frame's and not the sum of eight.
    ///
    /// # Panics
    ///
    /// When called twice, or when the reader fails on a generated input.
    pub fn execute(&mut self) -> u64 {
        let options = DecodeOptions::default();
        let state = std::mem::replace(&mut self.state, State::Taken);
        let (cells, output): (u64, Box<dyn std::any::Any>) = match (self.op, state) {
            (Op::Open, State::Bytes { format, bytes }) => {
                let pending = matches!(self.recorder, Recorder::Pending);
                let session = if format == "netcdf" {
                    if pending {
                        self.recorder = Recorder::Whole(bytes.len() as u64);
                    }
                    Session::open(bytes)
                } else if pending {
                    let recording = Rc::new(Recording::new(bytes));
                    self.recorder = Recorder::Ranges(Rc::clone(&recording) as Rc<dyn io::RangeLog>);
                    Session::open_source(SharedBytes(recording))
                } else {
                    Session::open_source(bytes)
                }
                .expect("a generated input opens");
                black_box(session.count());
                (0, Box::new(session))
            }
            (Op::Open, State::Objects(objects)) => {
                let session = if matches!(self.recorder, Recorder::Pending) {
                    let recording = Rc::new(Recording::new(objects));
                    self.recorder = Recorder::Objects(Rc::clone(&recording));
                    Session::open_store(SharedObjects(recording))
                } else {
                    Session::open_store(objects)
                }
                .expect("a generated store opens");
                black_box(session.variables().len());
                (0, Box::new(session))
            }
            (Op::Decode, State::Session { session, .. }) => {
                let field = session
                    .decode(0, &options)
                    .expect("a generated message decodes");
                self.value_width = Some(width(&field));
                (field.values.len() as u64, Box::new((session, field)))
            }
            (Op::Place, State::Session { session, .. }) => {
                let georef = session
                    .place_message(0)
                    .expect("a generated message places");
                (0, Box::new((session, georef)))
            }
            (Op::Variables, State::Session { session, .. }) => {
                let listed = (session.variables(), session.dimensions());
                (0, Box::new((session, listed)))
            }
            (Op::Slice, State::Session { session, variable }) => {
                let field = session
                    .decode_slice(variable, 1, 2, &[SLICE_PLANE, 0, 0], &options)
                    .expect("a generated plane decodes");
                self.value_width = Some(width(&field));
                (field.values.len() as u64, Box::new((session, field)))
            }
            (Op::Scrub, State::Session { session, variable }) => {
                let mut cells = 0;
                for frame in 0..SCRUB_FRAMES {
                    // Each frame is dropped before the next is decoded, as a
                    // slider that shows one frame at a time does.
                    let field = session
                        .decode_slice(variable, 1, 2, &[frame, 0, 0], &options)
                        .expect("a generated plane decodes");
                    cells = field.values.len() as u64;
                    self.value_width = Some(width(&field));
                    black_box(&field);
                }
                (cells, Box::new(session))
            }
            (Op::PlaceSlice, State::Session { session, variable }) => {
                let placed = session
                    .place_slice(variable, 1, 2)
                    .expect("a generated plane places");
                (0, Box::new((session, placed)))
            }
            (Op::Warp, State::Field(boxed)) => {
                let (session, field) = *boxed;
                let warped = session.warp(&field, &WarpOptions::new(true)).expect("warp");
                (
                    warped.values.len() as u64,
                    Box::new((session, field, warped)),
                )
            }
            (Op::Palette, State::Field(boxed)) => {
                let (session, field) = *boxed;
                let palette = session
                    .palette(&field, &PaletteOptions::new(None, None))
                    .expect("palette");
                (0, Box::new((session, field, palette)))
            }
            (Op::Render, State::Field(boxed)) => {
                let (session, field) = *boxed;
                let raster = session
                    .render(&field, &PaletteOptions::new(None, None), false)
                    .expect("render");
                (
                    raster.rgba.len() as u64 / 4,
                    Box::new((session, field, raster)),
                )
            }
            (Op::Contours, State::Field(boxed)) => {
                let (session, field) = *boxed;
                let lines = session.contours(&field, CONTOUR_LEVELS).expect("contours");
                let cells = field.values.len() as u64;
                (cells, Box::new((session, field, lines)))
            }
            (
                Op::Codec(_),
                State::Codec {
                    codec,
                    bytes,
                    aec,
                    cells,
                },
            ) => {
                let (elements, out) = run_codec(codec, &bytes, aec, cells);
                (elements, Box::new((bytes, out)))
            }
            (op, _) => panic!("scenario {op:?} executed twice or prepared for another operation"),
        };
        self.output = Some(output);
        cells
    }

    /// Bytes per value of the field a decode, slice or scrub produced: 4 when
    /// `Dtype::Auto` narrowed it to `f32`, 8 otherwise. `None` for the
    /// operations that produce no decoded field.
    pub fn value_width(&self) -> Option<u32> {
        self.value_width
    }

    /// What the operation asked its source for. Meaningful after
    /// [`execute`](Self::execute) on a scenario prepared [`Via::Recorded`].
    pub fn io(&self) -> Io {
        self.recorder.tally()
    }
}

fn width(field: &Field) -> u32 {
    match field.values {
        fieldglass::Values::F32(_) => 4,
        _ => 8,
    }
}

fn open(corpus: &Corpus, name: &str, via: Via, recorder: &mut Recorder) -> Session {
    let opened = match (corpus.format(name), via) {
        ("zarr", Via::Memory) => Session::open_store(corpus.objects(name)),
        ("zarr", Via::Recorded) => {
            let recording = Rc::new(Recording::new(corpus.objects(name)));
            *recorder = Recorder::Objects(Rc::clone(&recording));
            Session::open_store(SharedObjects(recording))
        }
        ("netcdf", via) => {
            // NetCDF has no source seam: the reader takes the whole file as a
            // `Vec`. Every byte of it is therefore something a host had to
            // fetch before any operation could run, and the I/O tier says so.
            let bytes = corpus.bytes(name);
            if via == Via::Recorded {
                *recorder = Recorder::Whole(bytes.len() as u64);
            }
            Session::open(bytes)
        }
        (_, Via::Memory) => Session::open_source(corpus.bytes(name)),
        (_, Via::Recorded) => {
            let recording = Rc::new(Recording::new(corpus.bytes(name)));
            *recorder = Recorder::Ranges(Rc::clone(&recording) as Rc<dyn io::RangeLog>);
            Session::open_source(SharedBytes(recording))
        }
    };
    opened.unwrap_or_else(|e| panic!("generated input {name} does not open: {e}"))
}

fn variable_index(session: &Session, name: &str) -> u32 {
    session
        .variables()
        .iter()
        .find(|v| v.name == name)
        .map(|v| v.index)
        .unwrap_or_else(|| panic!("no variable {name:?} in a generated input"))
}

fn codec_input(corpus: &Corpus, name: &str, codec: Codec) -> State {
    let cells = corpus.cells(name);
    let bytes = match codec {
        Codec::Jpeg2000 | Codec::Aec | Codec::Png => {
            // §7's payload: the section less its five-byte header.
            let (offset, length) = corpus.data_section(name);
            corpus.bytes(name)[offset + 5..offset + length].to_vec()
        }
        Codec::Zlib => {
            let (offset, length) = corpus.plane_extent(name, SLICE_PLANE);
            corpus.bytes(name)[offset..offset + length].to_vec()
        }
        Codec::Zstd | Codec::Blosc => corpus.plane_object(name, SLICE_PLANE),
    };
    let aec = (codec == Codec::Aec).then(|| corpus.aec(name));
    State::Codec {
        codec,
        bytes,
        aec,
        cells,
    }
}

/// Decompress `bytes` with `codec` alone, the way the reader invokes it.
/// Returns the samples (or bytes) produced and the output itself.
fn run_codec(
    codec: Codec,
    bytes: &[u8],
    aec: Option<[u32; 4]>,
    cells: usize,
) -> (u64, Box<dyn std::any::Any>) {
    let out = match codec {
        Codec::Jpeg2000 => {
            let image = rust_j2k::decode(bytes).expect("a generated codestream decodes");
            let samples = u64::from(image.width) * u64::from(image.height);
            return (samples, Box::new(image));
        }
        Codec::Aec => {
            let [bits, block, rsi, grib_flags] = aec.expect("aec parameters");
            let mut flags = rust_aec::flags_from_grib2_ccsds_flags(grib_flags as u8);
            flags.insert(rust_aec::AecFlags::MSB);
            flags.remove(rust_aec::AecFlags::DATA_3BYTE);
            let params = rust_aec::AecParams::new(bits as u8, block, rsi, flags);
            rust_aec::decode(bytes, params, cells).expect("a generated AEC stream decodes")
        }
        Codec::Png => {
            let mut reader = png::Decoder::new(std::io::Cursor::new(bytes))
                .read_info()
                .expect("a generated PNG stream decodes");
            let mut buf = vec![0; reader.output_buffer_size().expect("a bounded image")];
            reader
                .next_frame(&mut buf)
                .expect("a generated PNG frame decodes");
            buf
        }
        Codec::Zlib => {
            miniz_oxide::inflate::decompress_to_vec_zlib(bytes).expect("a generated zlib chunk")
        }
        Codec::Zstd => {
            use std::io::Read;
            let mut out = Vec::new();
            ruzstd::decoding::StreamingDecoder::new(bytes)
                .expect("a generated zstd frame")
                .read_to_end(&mut out)
                .expect("a generated zstd frame decodes");
            out
        }
        Codec::Blosc => {
            fieldglass_zarr::blosc::decompress(bytes, usize::MAX).expect("a generated blosc frame")
        }
    };
    (out.len() as u64, Box::new(out))
}

/// The most bytes the operation could need, from the corpus's facts alone.
///
/// Written as "the input, less what the operation has no reason to touch", so
/// each is one subtraction a reader can check against the format:
///
/// * a message's **decode** needs the whole message;
/// * its **open** and **place** need everything but the data section's payload;
/// * an array's **open**, **variables** and **place** need everything but the
///   variable's planes;
/// * a **slice** needs everything but the other planes, and a **scrub** everything
///   but the planes past its frames. For a sharded store the unit is the shard,
///   since an object source fetches whole objects;
/// * a render-side operation starts from a decoded field and needs nothing.
///
/// `None` for the codec rows, which read no source.
pub fn bound_bytes(corpus: &Corpus, scenario: &Scenario) -> Option<u64> {
    let name = scenario.input.as_str();
    let format = corpus.format(name);
    let messages = format.starts_with("grib");
    let total = corpus.total_bytes(name);
    Some(match scenario.op {
        Op::Codec(_) => return None,
        Op::Warp | Op::Palette | Op::Render | Op::Contours => 0,
        Op::Decode => corpus.message_length(name),
        Op::Open | Op::Place if messages => {
            // The data section's own header stays in: 5 octets in GRIB2, 11 in
            // GRIB1's binary data section. So does one look-ahead window per
            // section: the scanner reads a section's length through a
            // `FileCursor`, whose first window is deliberately wider than the
            // length field so a short section costs one request, not two. That
            // is a trade of bytes for round trips made on purpose, and the
            // bound allows exactly it and no more.
            let (header, sections) = if format == "grib2" { (5, 9) } else { (11, 6) };
            let (_, length) = corpus.data_section(name);
            let window = sections * FIRST_WINDOW_BYTES as u64;
            corpus.message_length(name) - (length as u64).saturating_sub(header) + window
        }
        Op::Open | Op::Variables | Op::PlaceSlice | Op::Place => {
            total - planes_bytes(corpus, name, &mut |_| true)
        }
        Op::Slice => total - planes_bytes(corpus, name, &mut |p| p != SLICE_PLANE),
        Op::Scrub => total - planes_bytes(corpus, name, &mut |p| p >= SCRUB_FRAMES),
    })
}

/// Bytes of the planes `keep` selects, counting a shared object once and only
/// when *no* plane outside the selection also needs it.
fn planes_bytes(corpus: &Corpus, name: &str, select: &mut dyn FnMut(u32) -> bool) -> u64 {
    let planes = u32::try_from(corpus.planes(name)).expect("a small plane count");
    if corpus.format(name) == "zarr" {
        let sizes: std::collections::BTreeMap<String, u64> =
            corpus.object_sizes(name).into_iter().collect();
        let needed: std::collections::BTreeSet<String> = (0..planes)
            .filter(|&p| !select(p))
            .flat_map(|p| corpus.plane_keys(name, p))
            .collect();
        let excluded: std::collections::BTreeSet<String> = (0..planes)
            .filter(|&p| select(p))
            .flat_map(|p| corpus.plane_keys(name, p))
            .filter(|key| !needed.contains(key))
            .collect();
        excluded
            .iter()
            .map(|key| sizes.get(key).copied().unwrap_or(0))
            .sum()
    } else {
        (0..planes)
            .filter(|&p| select(p))
            .flat_map(|p| corpus.plane_extents(name, p))
            .map(|(_, length)| length)
            .sum()
    }
}
