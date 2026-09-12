//! `Session::place_message` says where a message's values will land;
//! `Session::decode` puts them there. The two must not disagree (#726).
//!
//! They are computed by different routes on purpose. `decode` learns the
//! synthesised grid from `synthesize_message_global`, which runs the inverse
//! transform; `place_message` asks `synthesis_grid`, which reads the GDS and
//! runs nothing. That is the whole value of the second call — a host painting an
//! overlay before it decodes does not pay seconds for a spectral field — and it
//! is also the risk: two routes to one answer can drift.
//!
//! So the agreement is **asserted over the corpus** rather than argued for. For
//! every message of every committed GRIB fixture, the `Georef` `place_message`
//! returns must equal the one `decode` puts on the field: geometry, family,
//! dimensions, scan and bounds.
//!
//! The spectral and HEALPix fixtures are the cases that matter. For an ordinary
//! grid both routes read the same GDS and agreeing proves little; for a
//! synthesised family they agree only if `synthesis_grid` and
//! `synthesize_message_global` really do describe one grid.
//!
//! # Where they legitimately differ
//!
//! A message whose values are **not one scalar per grid point** — the GRIB1 true
//! `matrixOfValues` form, GRIB2 bi-Fourier coefficients — has a perfectly real
//! grid and no single 2-D field. `place_message` answers about the grid, so it
//! succeeds; `decode` returns one scalar per point, so it refuses and names the
//! call that does read them. Both answers are right: they are different
//! questions, and the conventions already carve these families out of
//! `decode_message_values`.
//!
//! That set is pinned below by **packing family** rather than by fixture name,
//! so it survives the corpus growing. A message outside those families that
//! places but will not decode is a real disagreement and fails.

use std::borrow::Cow;
use std::rc::Rc;

use fieldglass::{DecodeOptions, Session};
use fieldglass_core::bytes::{ByteRange, ByteSource, SourceIdentity};
use fieldglass_core::error::FieldglassError;
use fieldglass_core::testing::Recording;

/// The recording source, kept reachable after the session has taken it.
///
/// `Session::open_source` takes its source by value, so counting what a call
/// read means handing over a handle rather than the recorder. Otherwise
/// transparent: every method forwards, `identity` included, so the decode
/// through this is the decode without it.
struct Shared(Rc<Recording<Vec<u8>>>);

impl ByteSource for Shared {
    fn size(&self) -> u64 {
        self.0.size()
    }

    fn identity(&self) -> Option<SourceIdentity> {
        self.0.identity()
    }

    fn prefetch(&self, ranges: &[ByteRange]) -> Result<(), FieldglassError> {
        self.0.prefetch(ranges)
    }

    fn read(&self, range: ByteRange) -> Result<Cow<'_, [u8]>, FieldglassError> {
        self.0.read(range).map(|b| Cow::Owned(b.into_owned()))
    }
}

/// Total bytes a recording source was asked for.
fn bytes_read(recorder: &Recording<Vec<u8>>) -> u64 {
    recorder.reads().iter().map(|r| r.len).sum()
}

/// Every GRIB fixture in the workspace, as `(label, path)`.
fn corpus() -> Vec<(String, std::path::PathBuf)> {
    let mut out = Vec::new();
    for (label, dir, extensions) in [
        (
            "grib1",
            "../fieldglass-grib1/tests/fixtures",
            &["grib", "grib1"][..],
        ),
        (
            "grib2",
            "../fieldglass-grib2/tests/fixtures",
            &["grib2"][..],
        ),
    ] {
        for extension in extensions {
            let mut paths: Vec<_> = std::fs::read_dir(dir)
                .unwrap_or_else(|e| panic!("{dir} is the committed corpus: {e}"))
                .map(|e| e.expect("entry").path())
                .filter(|p| p.extension().and_then(|e| e.to_str()) == Some(*extension))
                .collect();
            paths.sort();
            for path in paths {
                let stem = path
                    .file_name()
                    .expect("a file")
                    .to_string_lossy()
                    .into_owned();
                out.push((format!("{label}/{stem}"), path));
            }
        }
    }
    out
}

/// Packings whose values are not one scalar per grid point, so the message
/// places but does not decode as a single 2-D field. See the module docs.
const MULTI_VALUE_PACKINGS: [&str; 2] = ["grid_simple_matrix", "bifourier_complex"];

#[test]
fn the_placement_is_the_georef_decode_puts_on_the_field() {
    let mut wrong: Vec<String> = Vec::new();
    let (mut files, mut messages, mut synthesised, mut carved_out) = (0, 0, 0, 0);

    for (label, path) in corpus() {
        let bytes = std::fs::read(&path).expect("fixture bytes");
        // A fixture the session refuses to open has no message to place. The
        // decoders' own suites are what hold those to their parse outcome.
        let Ok(session) = Session::open(bytes) else {
            continue;
        };
        files += 1;
        for i in 0..session.count() {
            let placed = session.place_message(i);
            let decoded = session.decode(i, &DecodeOptions::default());
            match (placed, decoded) {
                (Ok(placed), Ok(field)) => {
                    messages += 1;
                    // A field whose family is not the message's declared one is
                    // a synthesised family: the count is here so a corpus that
                    // stopped containing one would say so rather than passing.
                    if placed.label == "latlon"
                        && session
                            .message(i)
                            .ok()
                            .and_then(|m| m.grid)
                            .is_some_and(|g| g.label != "latlon")
                    {
                        synthesised += 1;
                    }
                    let pairs: [(&str, String, String); 5] = [
                        (
                            "geometry",
                            format!("{:?}", placed.geometry),
                            format!("{:?}", field.georef.geometry),
                        ),
                        ("label", placed.label.clone(), field.georef.label.clone()),
                        (
                            "dims",
                            format!("{}x{}", placed.ni, placed.nj),
                            format!("{}x{}", field.ni, field.nj),
                        ),
                        (
                            "scan",
                            format!("{:?}", placed.scan),
                            format!("{:?}", field.georef.scan),
                        ),
                        (
                            "boundsLonlat",
                            format!("{:?}", placed.bounds_lonlat),
                            format!("{:?}", field.georef.bounds_lonlat),
                        ),
                    ];
                    for (what, a, b) in pairs {
                        if a != b {
                            wrong.push(format!("  {label}#{i} {what}: placed {a} vs decoded {b}"));
                        }
                    }
                }
                // Both refuse, and for the same reason: a message `decode`
                // cannot place is one `place_message` must not claim to.
                (Err(p), Err(d)) => {
                    if p.to_string() != d.to_string() {
                        wrong.push(format!(
                            "  {label}#{i} refusals differ: placed {p:?} vs decoded {d:?}"
                        ));
                    }
                }
                // The carve-out: a grid that is real, holding values that are
                // not one per point.
                (Ok(placed), Err(d)) => {
                    let packing = session.message(i).map(|m| m.packing).unwrap_or_default();
                    if MULTI_VALUE_PACKINGS.contains(&packing.as_str()) {
                        carved_out += 1;
                    } else {
                        wrong.push(format!(
                            "  {label}#{i} placed {} ({packing}) but decode refused: {d:?}",
                            placed.label
                        ));
                    }
                }
                (Err(p), Ok(field)) => wrong.push(format!(
                    "  {label}#{i} decoded as {} but placing refused: {p:?}",
                    field.georef.label
                )),
            }
        }
    }

    assert!(files > 0 && messages > 0, "the GRIB corpus is present");
    assert!(
        synthesised > 0,
        "the corpus must still contain a synthesised family, or this test's \
         interesting half is silently gone"
    );
    assert!(
        wrong.is_empty(),
        "{} disagreement(s) between placing and decoding:\n{}",
        wrong.len(),
        wrong.join("\n")
    );
    // A ratchet in the other direction: if the corpus stopped holding a
    // multi-value packing, the carve-out above would be excusing nothing and
    // should be questioned rather than left in place.
    assert!(
        carved_out > 0,
        "the corpus must still hold a message whose values are not one per grid \
         point, or MULTI_VALUE_PACKINGS is excusing nothing"
    );
    eprintln!(
        "{files} files, {messages} messages agree \
         ({synthesised} synthesised, {carved_out} placed but not a single field)"
    );
}

/// Placing reads far less of the file than decoding it.
///
/// The claim `place_message` exists for, measured at the seam it is paid at
/// rather than on the clock. Both readers reach their bytes through
/// `ByteSource`, so `Recording` can count what each call actually asked for.
#[test]
fn placing_a_message_reads_less_than_decoding_it() {
    // A spectral fixture: the family where not decoding is worth the most,
    // since decoding runs an inverse spherical-harmonic transform.
    let path = "../fieldglass-grib1/tests/fixtures/spectral_simple_t63.grib1";
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        // Named rather than skipped: a fixture that moved must fail this test,
        // not quietly turn it into a no-op.
        Err(e) => panic!("{path} is the spectral fixture this test is about: {e}"),
    };

    let for_placing = Rc::new(Recording::new(bytes.clone()));
    let placing =
        Session::open_source(Shared(Rc::clone(&for_placing))).expect("the spectral fixture opens");
    // Measured from here, so the open's own indexing reads are not counted
    // against either side.
    for_placing.clear();
    let _ = placing.place_message(0).expect("places");

    let for_decoding = Rc::new(Recording::new(bytes));
    let decoding =
        Session::open_source(Shared(Rc::clone(&for_decoding))).expect("the spectral fixture opens");
    for_decoding.clear();
    let _ = decoding
        .decode(0, &DecodeOptions::default())
        .expect("decodes");

    let placed_bytes = bytes_read(&for_placing);
    let decoded_bytes = bytes_read(&for_decoding);
    assert!(
        placed_bytes < decoded_bytes,
        "placing read {placed_bytes} bytes and decoding {decoded_bytes}: \
         placing is supposed to be the cheap question"
    );
    eprintln!("spectral: placing read {placed_bytes} bytes, decoding {decoded_bytes}");
}
