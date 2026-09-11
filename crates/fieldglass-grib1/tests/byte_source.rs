//! GRIB1 reads through the `ByteSource` seam (#697, ADR-0005).
//!
//! What this file checks rather than assumes: a decode prefetches exactly the
//! sections it then reads, in one batch; the scan's reads grow with the
//! messages rather than with the bytes, garbage and a GRIB2 message included;
//! one message decodes from a source that holds nothing but its own range; and
//! a source that serves short fails the parse rather than panicking or
//! answering with a field that looks complete.

use fieldglass_core::{ByteRange, ByteSource, FieldglassError};
use fieldglass_grib1::{Grib1Message, Grib1MessageKind, Grib1Reader};
use std::borrow::Cow;
use std::cell::{Cell, RefCell};

/// Records every range read and every prefetch batch, so a test can say what
/// an operation touched rather than that it succeeded.
struct Recording<'a> {
    bytes: &'a [u8],
    reads: RefCell<Vec<ByteRange>>,
    prefetches: RefCell<Vec<Vec<ByteRange>>>,
}

impl<'a> Recording<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            reads: RefCell::new(Vec::new()),
            prefetches: RefCell::new(Vec::new()),
        }
    }

    fn clear(&self) {
        self.reads.borrow_mut().clear();
        self.prefetches.borrow_mut().clear();
    }
}

impl ByteSource for Recording<'_> {
    fn size(&self) -> u64 {
        self.bytes.len() as u64
    }

    fn prefetch(&self, ranges: &[ByteRange]) -> Result<(), FieldglassError> {
        self.prefetches.borrow_mut().push(ranges.to_vec());
        Ok(())
    }

    fn read(&self, range: ByteRange) -> Result<Cow<'_, [u8]>, FieldglassError> {
        self.reads.borrow_mut().push(range);
        self.bytes.read(range)
    }
}

/// A file of which only one range was fetched — what a host holds after
/// fetching the range a sidecar index gave for one message. It knows the whole
/// file's size, and refuses any read outside what it holds.
struct OneRange<'a> {
    bytes: &'a [u8],
    held: ByteRange,
}

impl ByteSource for OneRange<'_> {
    fn size(&self) -> u64 {
        self.bytes.len() as u64
    }

    fn read(&self, range: ByteRange) -> Result<Cow<'_, [u8]>, FieldglassError> {
        let inside = range.start >= self.held.start
            && range
                .end()
                .is_some_and(|e| e <= self.held.start + self.held.len);
        if !inside {
            return Err(FieldglassError::Parse(format!(
                "{range:?} was never fetched; only {:?} was",
                self.held
            )));
        }
        self.bytes.read(range)
    }
}

/// A response cut off at `cut`: a read running past it comes back short, the
/// way a truncated transfer would, while the size still claims the whole file.
struct CutOff<'a> {
    bytes: &'a [u8],
    cut: u64,
}

impl ByteSource for CutOff<'_> {
    fn size(&self) -> u64 {
        self.bytes.len() as u64
    }

    fn read(&self, range: ByteRange) -> Result<Cow<'_, [u8]>, FieldglassError> {
        let served = range.len.min(self.cut.saturating_sub(range.start));
        self.bytes.read(ByteRange::new(range.start, served))
    }
}

/// Serves every read in full until armed, then half of every read: a source
/// whose later transfers came back truncated.
struct Starved<'a> {
    bytes: &'a [u8],
    armed: Cell<bool>,
}

impl ByteSource for Starved<'_> {
    fn size(&self) -> u64 {
        self.bytes.len() as u64
    }

    fn read(&self, range: ByteRange) -> Result<Cow<'_, [u8]>, FieldglassError> {
        let got = self.bytes.read(range)?;
        if self.armed.get() {
            return Ok(Cow::Owned(got[..got.len() / 2].to_vec()));
        }
        Ok(got)
    }
}

fn fixture(path: &str) -> Vec<u8> {
    // Relative, like every other fixture read here: the wasm32-wasip1 run in
    // CI preopens only the crate directory and its parent.
    std::fs::read(path).expect("fixture")
}

/// Every GRIB1 fixture, by name.
fn corpus() -> Vec<(String, Vec<u8>)> {
    let mut paths: Vec<_> = std::fs::read_dir("tests/fixtures")
        .expect("fixture dir")
        .map(|e| e.expect("entry").path())
        .filter(|p| p.extension().is_some_and(|e| e == "grib1" || e == "grib"))
        .collect();
    paths.sort();
    paths
        .into_iter()
        .map(|p| {
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            (name, std::fs::read(&p).expect("fixture"))
        })
        .collect()
}

/// Several real messages behind a kilobyte of garbage, with a false start
/// between each pair and a GRIB2 message the scan has to step over.
fn multi_message_file() -> Vec<u8> {
    let mut file = vec![0x5a_u8; 1024];
    for name in [
        "cmc_wind_300_2010052400_p012.grib",
        "reduced_gg_n32.grib1",
        "spectral_complex_t63.grib1",
        "ecmwf_lfpw_msg0.grib1",
    ] {
        file.extend(fixture(&format!("tests/fixtures/{name}")));
        // The magic with edition 2 in its eighth byte: not a GRIB1 message.
        file.extend_from_slice(b"GRIB\0\0\0\x02 and then some padding");
    }
    file.extend(fixture(
        "../fieldglass-grib2/tests/fixtures/regular_latlon_surface.grib2",
    ));
    file.extend(fixture("tests/fixtures/ieee32_cmc_wind.grib1"));
    file
}

/// Decoded values as bit patterns, so a `NaN` compares equal to itself.
fn bits(values: &[Option<f64>]) -> Vec<Option<u64>> {
    values.iter().map(|v| v.map(f64::to_bits)).collect()
}

#[test]
fn the_scan_over_a_source_finds_what_the_buffer_scan_finds() {
    let file = multi_message_file();
    let from_buffer = Grib1Reader::from_bytes(file.clone()).expect("buffer scan");
    let recording = Recording::new(&file);
    let from_source = Grib1Reader::from_source(&recording).expect("source scan");
    assert_eq!(from_buffer.message_count(), 5);
    assert_eq!(from_buffer.messages, from_source.messages);
}

/// The scan is a chain — each message's length says where the next begins —
/// so it cannot be one batch. What it can be is bounded by the messages: a few
/// reads each, and never the data sections themselves.
#[test]
fn the_scan_reads_a_few_windows_per_message_and_never_the_data() {
    let file = multi_message_file();
    let recording = Recording::new(&file);
    let reader = Grib1Reader::from_source(&recording).expect("scan");
    let n = reader.message_count();

    let reads = recording.reads.borrow();
    // Per message: the search window that finds it, the indicator, the
    // trailing `7777`, and a few growing windows over the PDS, GDS and BMS
    // headers. Plus a handful of windows over the garbage, the false starts and
    // the GRIB2 message the search steps across.
    assert!(
        reads.len() <= 8 * n + 24,
        "{} reads to scan {n} messages",
        reads.len()
    );
    let fetched: u64 = reads.iter().map(|r| r.len).sum();
    assert!(
        fetched < file.len() as u64 / 2,
        "the scan fetched {fetched} of {} bytes",
        file.len()
    );
    assert!(
        recording.prefetches.borrow().is_empty(),
        "a scan has no plan to state"
    );
}

/// A megabyte of garbage before one message is a couple of dozen reads, not a
/// million.
#[test]
fn leading_garbage_costs_windows_not_bytes() {
    let mut file = vec![0_u8; 1 << 20];
    file.extend(fixture("tests/fixtures/ieee32_cmc_wind.grib1"));
    let recording = Recording::new(&file);
    let reader = Grib1Reader::from_source(&recording).expect("scan");
    assert_eq!(reader.message_count(), 1);
    assert_eq!(reader.messages[0].byte_offset, 1 << 20);
    let reads = recording.reads.borrow().len();
    assert!(reads <= 40, "{reads} reads to step over 1 MiB of garbage");
}

/// The sections a message's decode reads: the BMS when there is one, then the
/// BDS.
fn plan(m: &Grib1Message) -> Vec<ByteRange> {
    m.bms_range.into_iter().chain([m.bds_range]).collect()
}

/// Every decode entry point resolves the sections it reads, prefetches them in
/// one batch, then reads exactly those: not a superset, which would fetch bytes
/// nobody wants, and not one request per section.
#[test]
fn a_decode_prefetches_its_sections_once_then_reads_exactly_them() {
    let (mut decoded, mut with_bitmap) = (0, 0);
    for (name, bytes) in corpus() {
        let recording = Recording::new(&bytes);
        let reader = Grib1Reader::from_source(&recording).expect("fixture scans");
        for (i, m) in reader.messages.iter().enumerate() {
            // Routing a gridded message reads the BDS header too, and is held
            // to the same rule. A spectral one is decided by the GDS alone.
            recording.clear();
            let kind = reader.message_kind(i);
            if matches!(kind, Grib1MessageKind::Grid | Grib1MessageKind::Matrix) {
                assert_eq!(*recording.prefetches.borrow(), vec![vec![m.bds_range]]);
                assert_eq!(*recording.reads.borrow(), vec![m.bds_range]);
            }

            recording.clear();
            let (expected, ok) = match kind {
                Grib1MessageKind::Grid => (plan(m), reader.decode_message_values(i).is_ok()),
                Grib1MessageKind::Matrix => (plan(m), reader.decode_matrix_message(i).is_ok()),
                Grib1MessageKind::Spectral => {
                    (vec![m.bds_range], reader.decode_spectral_message(i).is_ok())
                }
                Grib1MessageKind::Unsupported => continue,
            };
            if !ok {
                continue;
            }
            decoded += 1;
            if m.bms_range.is_some() {
                with_bitmap += 1;
            }
            assert_eq!(
                *recording.prefetches.borrow(),
                vec![expected.clone()],
                "{name} message {i}: one batch, naming the sections"
            );
            assert_eq!(
                *recording.reads.borrow(),
                expected,
                "{name} message {i}: reads exactly the batch"
            );
        }
    }
    // A count near zero would mean the loop above checked nothing, and the
    // two-section batch has to have been exercised at least once.
    assert!(decoded >= 15, "only {decoded} messages decoded");
    assert!(with_bitmap > 0, "no fixture exercised a BMS + BDS batch");
}

/// What a sidecar index makes possible: the host fetches one message's range,
/// and the message decodes from that alone, with no scan and no read outside
/// it — matching the whole-file decode of the same message.
#[test]
fn one_message_decodes_from_a_source_holding_only_its_range() {
    let file = multi_message_file();
    let whole = Grib1Reader::from_bytes(file.clone()).expect("scan");
    for (i, m) in whole.messages.iter().enumerate() {
        let held = ByteRange::new(m.byte_offset, u64::from(m.is.total_length));
        let source = OneRange { bytes: &file, held };
        let one = Grib1Reader::from_message_at(&source, m.byte_offset)
            .unwrap_or_else(|e| panic!("message {i} alone: {e}"));
        assert_eq!(one.message_count(), 1);
        let mut expected = m.clone();
        expected.message_index = 0;
        assert_eq!(
            one.messages[0], expected,
            "message {i}: the offsets are still the file's"
        );
        match whole.message_kind(i) {
            Grib1MessageKind::Spectral => assert_eq!(
                one.decode_spectral_message(0).expect("alone"),
                whole.decode_spectral_message(i).expect("whole"),
            ),
            _ => assert_eq!(
                bits(&one.decode_message_raster(0).expect("alone")),
                bits(&whole.decode_message_raster(i).expect("whole")),
                "message {i}"
            ),
        }
    }
    // And that source really holds only the one range: the scan, which reads
    // from the start of the file, cannot run over it.
    let m = &whole.messages[1];
    let source = OneRange {
        bytes: &file,
        held: ByteRange::new(m.byte_offset, u64::from(m.is.total_length)),
    };
    assert!(Grib1Reader::from_source(&source).is_err());
}

/// Handed an offset directly, the reader has not been through the scan, so it
/// holds the offset to what the scan would have: an edition-1 indicator there.
#[test]
fn an_offset_that_is_not_a_grib1_message_is_refused_not_searched_from() {
    let file = multi_message_file();
    let whole = Grib1Reader::from_bytes(file.clone()).expect("scan");
    let first = whole.messages[0].byte_offset;
    assert!(Grib1Reader::from_message_at(file.as_slice(), first + 1).is_err());
    assert!(
        Grib1Reader::from_message_at(file.as_slice(), 0).is_err(),
        "garbage"
    );
    // The GRIB2 message sits just before the last GRIB1 one.
    let grib2_len =
        fixture("../fieldglass-grib2/tests/fixtures/regular_latlon_surface.grib2").len();
    let grib2_at = whole.messages[4].byte_offset - grib2_len as u64;
    let err = Grib1Reader::from_message_at(file.as_slice(), grib2_at).expect_err("edition 2");
    assert!(err.to_string().contains("edition"), "{err}");
    assert!(Grib1Reader::from_message_at(file.as_slice(), file.len() as u64 + 1).is_err());
}

/// A response cut off anywhere in the message fails the scan, at every cut
/// point and without a panic. The scan reads the trailing `7777` as well as
/// the headers, so no truncation of the message gets past it to a decode.
#[test]
fn a_source_cut_off_anywhere_fails_the_scan_cleanly() {
    let bytes = fixture("tests/fixtures/ieee32_cmc_wind.grib1");
    for cut in 0..bytes.len() as u64 {
        let source = CutOff { bytes: &bytes, cut };
        assert!(
            Grib1Reader::from_source(&source).is_err(),
            "a cut at {cut} of {} scanned",
            bytes.len()
        );
    }
    let whole = CutOff {
        bytes: &bytes,
        cut: bytes.len() as u64,
    };
    assert_eq!(
        Grib1Reader::from_source(&whole)
            .expect("uncut")
            .message_count(),
        1
    );
}

/// A source whose sections come back short after the scan — a data transfer
/// that was truncated — fails every call that reads them, rather than decoding
/// a short buffer into a field that looks complete.
#[test]
fn a_decode_whose_sections_come_back_short_fails_cleanly() {
    let bytes = fixture("tests/fixtures/ieee32_cmc_wind.grib1");
    let source = Starved {
        bytes: &bytes,
        armed: Cell::new(false),
    };
    let reader = Grib1Reader::from_source(&source).expect("scan");
    assert_eq!(reader.message_kind(0), Grib1MessageKind::Grid);
    source.armed.set(true);
    let err = reader.decode_message_values(0).expect_err("short BDS");
    assert!(err.to_string().contains("served"), "{err}");
    assert!(reader.decode_message_raster(0).is_err());
    assert_eq!(reader.packing_label(0), None);
    assert_eq!(reader.message_kind(0), Grib1MessageKind::Unsupported);
}
