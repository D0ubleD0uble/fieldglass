//! GRIB2 reads through the `ByteSource` seam (#697, ADR-0005).
//!
//! What this file checks rather than assumes: a decode prefetches exactly the
//! sections it then reads, in one batch; the scan's reads grow with the
//! messages rather than with the bytes, garbage and a GRIB1 message included;
//! one message decodes from a source that holds nothing but its own range; and
//! a source that serves short fails the parse rather than panicking or
//! answering with a field that looks complete.

use fieldglass_core::testing::{CutOff, OneRange, Recording, Starved};
use fieldglass_core::{ByteRange, FieldglassError};
use fieldglass_grib2::{Grib2Message, Grib2Reader};

fn fixture(path: &str) -> Vec<u8> {
    // Relative, like every other fixture read here: the wasm32-wasip1 run in
    // CI preopens only the crate directory and its parent.
    std::fs::read(path).expect("fixture")
}

/// Every GRIB2 fixture, by name.
fn corpus() -> Vec<(String, Vec<u8>)> {
    let mut paths: Vec<_> = std::fs::read_dir("tests/fixtures")
        .expect("fixture dir")
        .map(|e| e.expect("entry").path())
        .filter(|p| p.extension().is_some_and(|e| e == "grib2"))
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
/// between each pair and a GRIB1 message the scan has to step over.
fn multi_message_file() -> Vec<u8> {
    let mut file = vec![0x5a_u8; 1024];
    for name in [
        "gfs_c255_latlon.grib2",
        "rap_jpeg2000_lambert.grib2",
        "ecmwf_ccsds_latlon.grib2",
        "regular_latlon_surface.grib2",
    ] {
        file.extend(fixture(&format!("tests/fixtures/{name}")));
        // The magic with edition 1 in its eighth byte: not a GRIB2 message.
        file.extend_from_slice(b"GRIB\0\0\0\x01 and then some padding");
    }
    file.extend(fixture(
        "../fieldglass-grib1/tests/fixtures/ieee32_cmc_wind.grib1",
    ));
    file.extend(fixture("tests/fixtures/healpix_n2_nested.grib2"));
    file
}

/// Where a message sits and what it records, for comparing two readers'
/// answers without depending on the parsed sections being comparable.
fn layout(m: &Grib2Message) -> (u64, Option<ByteRange>, ByteRange, ByteRange) {
    (m.byte_offset, m.lus_range, m.bms_range, m.ds_range)
}

/// Decoded values as bit patterns, so a `NaN` compares equal to itself.
fn bits(values: &[Option<f64>]) -> Vec<Option<u64>> {
    values.iter().map(|v| v.map(f64::to_bits)).collect()
}

#[test]
fn the_scan_over_a_source_finds_what_the_buffer_scan_finds() {
    let file = multi_message_file();
    let from_buffer = Grib2Reader::from_bytes(file.clone()).expect("buffer scan");
    let recording = Recording::new(&file);
    let from_source = Grib2Reader::from_source(&recording).expect("source scan");
    assert_eq!(
        from_buffer.message_count(),
        5,
        "four GRIB2 messages and HEALPix"
    );
    let a: Vec<_> = from_buffer.messages.iter().map(layout).collect();
    let b: Vec<_> = from_source.messages.iter().map(layout).collect();
    assert_eq!(a, b);
}

/// The scan is a chain — each message's length says where the next begins —
/// so it cannot be one batch. What it can be is bounded by the messages: a few
/// reads each, and never the data sections themselves. Before #697 there were
/// no reads to count; over a transport the naive form of the same scan is one
/// read per skipped byte, which here would be thousands.
#[test]
fn the_scan_reads_a_few_windows_per_message_and_never_the_data() {
    let file = multi_message_file();
    let recording = Recording::new(&file);
    let reader = Grib2Reader::from_source(&recording).expect("scan");
    let n = reader.message_count();

    let reads = recording.reads();
    // Per message: the search window that finds it, the indicator, the
    // trailing `7777`, and a few growing windows over §1–§7's headers. Plus a
    // handful of windows over the garbage, the false starts and the GRIB1
    // message the search steps across.
    assert!(
        reads.len() <= 8 * n + 24,
        "{} reads to scan {n} messages",
        reads.len()
    );
    let fetched: u64 = reads.iter().map(|r| r.len).sum();
    assert!(
        fetched < file.len() as u64 / 8,
        "the scan fetched {fetched} of {} bytes",
        file.len()
    );
    // No read starts inside a data section: the scan reads §7's header from
    // where it begins and then jumps to the trailing `7777`. The payload is a
    // decode's to fetch, not the scan's.
    for m in &reader.messages {
        let (start, end) = (m.ds_range.start, m.ds_range.start + m.ds_range.len);
        for r in reads.iter() {
            assert!(
                !(r.start > start && r.start < end),
                "{r:?} starts inside the data section {:?}",
                m.ds_range
            );
        }
    }
    assert!(
        recording.prefetches().is_empty(),
        "a scan has no plan to state"
    );
}

/// A megabyte of garbage before one message is a couple of dozen reads, not a
/// million.
#[test]
fn leading_garbage_costs_windows_not_bytes() {
    let mut file = vec![0_u8; 1 << 20];
    file.extend(fixture("tests/fixtures/regular_latlon_surface.grib2"));
    let recording = Recording::new(&file);
    let reader = Grib2Reader::from_source(&recording).expect("scan");
    assert_eq!(reader.message_count(), 1);
    assert_eq!(reader.messages[0].byte_offset, 1 << 20);
    let reads = recording.reads().len();
    assert!(reads <= 40, "{reads} reads to step over 1 MiB of garbage");
}

/// Every decode entry point resolves the sections it reads, prefetches them in
/// one batch, then reads exactly those: not a superset, which would fetch bytes
/// nobody wants, and not one request per section.
#[test]
fn a_decode_prefetches_its_sections_once_then_reads_exactly_them() {
    let mut decoded = 0;
    for (name, bytes) in corpus() {
        let recording = Recording::new(&bytes);
        let reader = Grib2Reader::from_source(&recording).expect("fixture scans");
        for (i, m) in reader.messages.iter().enumerate() {
            let (plan, ok) = if m.gds.spherical_harmonic().is_some() {
                recording.clear();
                (vec![m.ds_range], reader.decode_spectral_message(i).is_ok())
            } else if m.gds.bifourier().is_some() {
                recording.clear();
                (vec![m.ds_range], reader.decode_bifourier_message(i).is_ok())
            } else {
                recording.clear();
                let ok = reader.decode_message_values(i).is_ok();
                (vec![m.bms_range, m.ds_range], ok)
            };
            if !ok {
                continue;
            }
            decoded += 1;
            assert_eq!(
                recording.prefetches(),
                vec![plan.clone()],
                "{name} message {i}: one batch, naming the sections"
            );
            assert_eq!(
                recording.reads(),
                plan,
                "{name} message {i}: reads exactly the batch"
            );
        }
    }
    // The corpus is 53 fixtures; a count near zero would mean the loop above
    // checked nothing.
    assert!(decoded >= 45, "only {decoded} messages decoded");
}

/// What a sidecar index makes possible: the host fetches one message's range,
/// and the message decodes from that alone, with no scan and no read outside
/// it — matching the whole-file decode of the same message.
#[test]
fn one_message_decodes_from_a_source_holding_only_its_range() {
    let file = multi_message_file();
    let whole = Grib2Reader::from_bytes(file.clone()).expect("scan");
    for (i, m) in whole.messages.iter().enumerate() {
        let held = ByteRange::new(m.byte_offset, m.is.total_length);
        let source = OneRange::new(&file, held);
        let one = Grib2Reader::from_message_at(&source, m.byte_offset)
            .unwrap_or_else(|e| panic!("message {i} alone: {e}"));
        assert_eq!(one.message_count(), 1);
        assert_eq!(one.messages[0].message_index, 0);
        assert_eq!(
            layout(&one.messages[0]),
            layout(m),
            "message {i}: the offsets are still the file's"
        );
        if m.gds.dimensions().is_none() {
            // HEALPix: no raster, but its stored field still decodes.
            assert_eq!(
                bits(&one.decode_message_values(0).expect("healpix")),
                bits(&whole.decode_message_values(i).expect("healpix"))
            );
            continue;
        }
        assert_eq!(
            bits(&one.decode_message_raster(0).expect("alone")),
            bits(&whole.decode_message_raster(i).expect("whole")),
            "message {i}"
        );
    }
    // And that source really holds only the one range: the scan, which reads
    // from the start of the file, cannot run over it.
    let m = &whole.messages[1];
    let source = OneRange::new(&file, ByteRange::new(m.byte_offset, m.is.total_length));
    assert!(Grib2Reader::from_source(&source).is_err());
}

/// Handed an offset directly, the reader has not been through the scan, so it
/// holds the offset to what the scan would have: an edition-2 indicator there.
#[test]
fn an_offset_that_is_not_a_grib2_message_is_refused_not_searched_from() {
    let file = multi_message_file();
    let whole = Grib2Reader::from_bytes(file.clone()).expect("scan");
    let first = whole.messages[0].byte_offset;
    assert!(Grib2Reader::from_message_at(file.as_slice(), first + 1).is_err());
    assert!(
        Grib2Reader::from_message_at(file.as_slice(), 0).is_err(),
        "garbage"
    );
    // The GRIB1 message sits just before the last GRIB2 one.
    let grib1_len = fixture("../fieldglass-grib1/tests/fixtures/ieee32_cmc_wind.grib1").len();
    let grib1_at = whole.messages[4].byte_offset - grib1_len as u64;
    let err = Grib2Reader::from_message_at(file.as_slice(), grib1_at).expect_err("edition 1");
    assert!(err.to_string().contains("edition"), "{err}");
    assert!(Grib2Reader::from_message_at(file.as_slice(), file.len() as u64 + 1).is_err());
}

/// A response cut off anywhere in the message fails the scan, at every cut
/// point and without a panic. The scan reads the trailing `7777` as well as
/// the headers, so no truncation of the message gets past it to a decode.
#[test]
fn a_source_cut_off_anywhere_fails_the_scan_cleanly() {
    let bytes = fixture("tests/fixtures/regular_latlon_surface.grib2");
    for cut in 0..bytes.len() as u64 {
        let source = CutOff::new(&bytes, cut);
        assert!(
            Grib2Reader::from_source(&source).is_err(),
            "a cut at {cut} of {} scanned",
            bytes.len()
        );
    }
    let whole = CutOff::new(&bytes, bytes.len() as u64);
    assert_eq!(
        Grib2Reader::from_source(&whole)
            .expect("uncut")
            .message_count(),
        1
    );
}

/// A source whose sections come back short after the scan — a data transfer
/// that was truncated — fails every decode that reads them, rather than
/// decoding a short buffer into a field that looks complete.
#[test]
fn a_decode_whose_sections_come_back_short_fails_cleanly() {
    let bytes = fixture("tests/fixtures/regular_latlon_surface.grib2");
    let source = Starved::new(&bytes);
    let reader = Grib2Reader::from_source(&source).expect("scan");
    source.arm();
    let err = reader.decode_message_values(0).expect_err("short sections");
    assert!(
        matches!(err, FieldglassError::ShortRead { .. }),
        "a truncated transfer is its own error, not a malformed file: {err}"
    );
    assert!(reader.decode_message_raster(0).is_err());
}
