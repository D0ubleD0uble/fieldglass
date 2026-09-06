//! The grid family a message declares survives into `Georef::label` (#645).
//!
//! `GridGeometry` describes **how points are placed**, and both decoders widen
//! a reduced grid's ragged rows onto its regular sibling's raster before
//! anything places a point on it. So `GridGeometry::kind` — which is the serde
//! tag and must stay the variant's own name — answers `"gaussian"` for a
//! `reduced_gg`, and a `Georef` that took its family from the geometry lost the
//! only word that says the file was reduced.
//!
//! `fieldglass-napi` never lost it: it reads `GridDescription::grid_type_name`
//! / `GridDefinitionSection::template_name` straight off the decoder. So the
//! two hosts disagreed in public — the extension's grid-type column said
//! `reduced_gaussian` and the umbrella, and therefore the browser host, said
//! `gaussian`. These tests hold the umbrella to the decoder's own answer.
//!
//! Deliberately over the **committed** fixtures rather than `samples/`, which
//! is git-ignored and would let this pass vacuously in a fresh clone.

use std::path::{Path, PathBuf};

use fieldglass::Session;

/// The two committed fixture directories, relative to this crate's directory,
/// and the extensions each holds.
const CORPORA: &[(&str, &[&str])] = &[
    ("../fieldglass-grib1/tests/fixtures", &["grib1", "grib"]),
    ("../fieldglass-grib2/tests/fixtures", &["grib2"]),
];

/// Every committed GRIB file, sorted so a failure names the same file run to
/// run.
fn corpus() -> Vec<PathBuf> {
    let mut out = Vec::new();
    for (dir, exts) in CORPORA {
        let entries = std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{dir}: {e}"));
        for entry in entries {
            let path = entry.expect("a readable directory entry").path();
            let is_grib = path
                .extension()
                .and_then(|e| e.to_str())
                .is_some_and(|e| exts.contains(&e));
            if is_grib {
                out.push(path);
            }
        }
    }
    out.sort();
    out
}

/// What the decoder itself calls each message's grid, message by message, or
/// `None` for a message with no grid section at all.
fn decoder_families(path: &Path, bytes: Vec<u8>) -> Vec<Option<String>> {
    let is_grib1 = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e != "grib2");
    if is_grib1 {
        fieldglass_grib1::Grib1Reader::from_bytes(bytes)
            .expect("the fixture parses")
            .messages
            .iter()
            .map(|m| m.gds.as_ref().map(|g| g.grid_type_name().to_string()))
            .collect()
    } else {
        fieldglass_grib2::Grib2Reader::from_bytes(bytes)
            .expect("the fixture parses")
            .messages
            .iter()
            .map(|m| Some(m.gds.template_name()))
            .collect()
    }
}

/// `MessageInfo::grid.label` is the decoder's own family string, for every
/// message of every committed fixture.
///
/// The sweep rather than a list of the two reduced fixtures: the defect was a
/// *seam* silently re-deriving a name the decoder already owned, so what is
/// worth pinning is that no message anywhere disagrees. A corpus check can also
/// pass by checking nothing, so the file count is asserted too.
#[test]
fn every_committed_message_reports_the_family_its_decoder_names() {
    let corpus = corpus();
    assert!(
        corpus.len() >= 40,
        "only {} committed GRIB fixtures found — the corpus paths are wrong and \
         this test is checking almost nothing",
        corpus.len()
    );

    let mut checked = 0usize;
    let mut families = std::collections::BTreeSet::new();
    for path in &corpus {
        let bytes = std::fs::read(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        let expected = decoder_families(path, bytes.clone());
        let session = Session::open(bytes).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert_eq!(
            session.count() as usize,
            expected.len(),
            "{}: the session and the decoder disagree about the message count",
            path.display()
        );
        for (i, want) in expected.iter().enumerate() {
            let info = session
                .message(i as u32)
                .unwrap_or_else(|e| panic!("{} message {i}: {e}", path.display()));
            let got = info.grid.as_ref().map(|g| g.label.clone());
            assert_eq!(
                got.as_deref(),
                want.as_deref(),
                "{} message {i}: the umbrella renamed the grid the decoder declared",
                path.display()
            );
            if let Some(name) = want {
                families.insert(name.clone());
            }
            checked += 1;
        }
    }

    assert!(
        checked >= 70,
        "only {checked} messages checked — the corpus is not being walked"
    );
    // The whole point is the pair whose declared family is not their geometry's,
    // so a corpus that no longer carries one has stopped testing the defect.
    assert!(
        families.contains("reduced_gaussian"),
        "no committed fixture declares a reduced Gaussian grid any more; \
         families seen: {families:?}",
    );
}

/// The reduced families by name: `kind` is the raster's family and `label` is
/// the file's, and they differ.
///
/// `reduced_latlon` has no committed fixture of its own, so it is made here the
/// way #633's did — one byte of a committed one. GRIB1 tells the two apart by
/// GDS octet 6 alone (`4` Gaussian, `0` lat/lon); every other octet the two
/// parsers read is at the same offset, `Nj`, the corners and the `PL` list
/// included, so flipping that byte is exactly the difference between the two
/// grids and nothing else.
#[test]
fn a_reduced_grid_keeps_its_own_family_beside_its_rasters() {
    let n32 = std::fs::read("../fieldglass-grib1/tests/fixtures/reduced_gg_n32.grib1")
        .expect("the committed reduced Gaussian fixture");

    for (why, bytes, kind, label) in [
        (
            "GRIB1 reduced Gaussian",
            n32.clone(),
            "gaussian",
            "reduced_gaussian",
        ),
        (
            "GRIB2 reduced Gaussian",
            std::fs::read(
                "../fieldglass-grib2/tests/fixtures/reduced_gaussian_pressure_level.grib2",
            )
            .expect("the committed GRIB2 reduced Gaussian fixture"),
            "gaussian",
            "reduced_gaussian",
        ),
        (
            "GRIB1 reduced lat/lon",
            patched_to_reduced_latlon(&n32),
            "latlon",
            "reduced_latlon",
        ),
    ] {
        let session = Session::open(bytes).unwrap_or_else(|e| panic!("{why}: {e}"));
        let info = session.message(0).unwrap_or_else(|e| panic!("{why}: {e}"));
        let grid = info
            .grid
            .as_ref()
            .unwrap_or_else(|| panic!("{why}: no grid"));
        assert_eq!(grid.kind, kind, "{why}: the raster's family");
        assert_eq!(grid.label, label, "{why}: the file's own family");
        assert_ne!(
            grid.kind, grid.label,
            "{why}: this case exists because the two differ"
        );
    }
}

/// A field's own georef reports the grid its **values** are on, and for a
/// reduced grid that is the raster the rows were widened onto — so `kind` is
/// the regular sibling's. The family the file declared still travels beside it,
/// because a host showing "which grid is this" reads the field as often as it
/// reads the message list.
#[test]
fn a_decoded_reduced_field_carries_the_declaration_too() {
    let bytes = std::fs::read("../fieldglass-grib1/tests/fixtures/reduced_gg_n32.grib1")
        .expect("the committed reduced Gaussian fixture");
    let session = Session::open(bytes).expect("opens");
    let field = session
        .decode(0, &fieldglass::DecodeOptions::default())
        .expect("decodes");
    assert_eq!(field.georef.kind, "gaussian");
    assert_eq!(field.georef.label, "reduced_gaussian");
    assert_eq!(
        field.values.len(),
        (field.ni as usize) * (field.nj as usize),
        "the field is on the widened raster, which is why `kind` is `gaussian`"
    );
}

/// A **synthesised** grid is the case where the geometry is the honest answer:
/// nothing of the declared family survives an inverse spherical-harmonic
/// transform, so the field really is on a lat/lon raster and says so, while the
/// message list still describes the file.
#[test]
fn a_synthesised_field_reports_the_grid_it_is_actually_on() {
    let bytes = std::fs::read("../fieldglass-grib1/tests/fixtures/spectral_simple_t63.grib1")
        .expect("the committed spectral fixture");
    let session = Session::open(bytes).expect("opens");

    let info = session.message(0).expect("message 0");
    let declared = info.grid.as_ref().expect("a declared grid");
    assert_eq!(declared.kind, "unsupported");
    assert_eq!(declared.label, "spherical_harmonic");

    let field = session
        .decode(0, &fieldglass::DecodeOptions::default())
        .expect("synthesises");
    assert_eq!(field.georef.kind, "latlon");
    assert_eq!(
        field.georef.label, "latlon",
        "the values are on the synthesised raster, not on a spectral grid"
    );
}

/// The same GRIB1 message with GDS octet 6 — the ON388 Table 6 data
/// representation type — changed from `4` (Gaussian) to `0` (lat/lon).
///
/// §0 is 8 octets and §1's length is its first three, so the GDS begins at
/// `8 + pds_len` and the type is five octets into it. Computed rather than
/// written down so it stays right if the fixture is ever rebuilt.
fn patched_to_reduced_latlon(bytes: &[u8]) -> Vec<u8> {
    const IS_LEN: usize = 8;
    let pds_len = usize::from(bytes[IS_LEN]) << 16
        | usize::from(bytes[IS_LEN + 1]) << 8
        | usize::from(bytes[IS_LEN + 2]);
    let grid_type = IS_LEN + pds_len + 5;
    assert_eq!(
        bytes[grid_type], 4,
        "the fixture is no longer a Gaussian grid at the offset this patch assumes"
    );
    let mut out = bytes.to_vec();
    out[grid_type] = 0;
    out
}
