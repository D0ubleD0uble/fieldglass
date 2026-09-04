//! Each format crate, used the way a downstream crate would use it: as the
//! only Fieldglass dependency in the manifest.
//!
//! Every core type these tests name is reached through the format crate's own
//! re-export. `use fieldglass_core::…` does not compile here at all — the
//! crate is not a dependency — so a re-export that goes missing breaks the
//! build rather than quietly pushing `fieldglass-core = "…"` back into a
//! consumer's manifest, where forgetting `default-features = false` re-enables
//! `render` and `fs` for the whole graph (#537).

use std::borrow::Cow;
use std::cell::Cell;
use std::path::PathBuf;

// Aliased per crate rather than imported once: all three name the same type,
// and importing it from one crate would let a re-export dropped from either of
// the other two still compile.
use fieldglass_grib1::{FieldglassError as Grib1Error, Grib1Reader, GridGeometry as Grib1Geometry};
use fieldglass_grib2::{
    FieldglassError as Grib2Error, Grib2Reader, GridGeometry as Grib2Geometry, GridTemplate,
};
use fieldglass_netcdf::{
    ByteRange, ByteSource, FieldglassError as NetcdfError, NetcdfBacking, NetcdfReader, classic,
};

/// Fixtures live with the crate that owns them; this package borrows them
/// rather than committing a second copy of a real operational field.
fn fixture(relative: &str) -> Vec<u8> {
    let path: PathBuf = [env!("CARGO_MANIFEST_DIR"), "..", "..", relative]
        .iter()
        .collect();
    std::fs::read(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

/// The gate that would otherwise fail open. Every other test in this file is
/// vacuous the moment `fieldglass-core` becomes a dependency of this package,
/// because the re-exported names would resolve through it instead.
#[test]
fn the_manifest_names_no_direct_core_dependency() {
    let manifest = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
        .expect("this package has a manifest");
    // Comments stripped first, so the substring search catches every spelling
    // a dependency can take — `[dependencies.fieldglass-core]` and
    // `fieldglass-core.workspace = true` as well as the plain one — without
    // tripping over the manifest's own prose about why it is absent.
    for (n, line) in manifest.lines().enumerate() {
        let code = line.split('#').next().unwrap_or("");
        assert!(
            !code.contains("fieldglass-core"),
            "Cargo.toml:{}: this package must not depend on fieldglass-core — \
             its absence is what every other test in this file rests on",
            n + 1
        );
    }
}

#[test]
fn the_three_crates_re_export_one_error_type() {
    // Assigning across the aliases only compiles if they are the same type —
    // three separate error enums would be an API split, not a re-export.
    let from_grib2: Grib1Error = Grib2Error::OutOfRange;
    let from_netcdf: Grib1Error = NetcdfError::OutOfRange;
    assert!(matches!(from_grib2, Grib1Error::OutOfRange));
    assert!(matches!(from_netcdf, Grib1Error::OutOfRange));

    let grib2_geometry: Grib1Geometry = Grib2Geometry::Unsupported {
        label: "spherical_harmonics".into(),
    };
    assert_eq!(grib2_geometry.kind(), "unsupported");
}

// ── GRIB1 ────────────────────────────────────────────────────────────────────

#[test]
fn grib1_errors_are_matchable() {
    let whole = fixture("crates/fieldglass-grib1/tests/fixtures/reduced_gg_n32.grib1");

    // A message whose declared length runs past the end of the file. `let …
    // else` rather than `expect_err`, which would need `Grib1Reader: Debug` —
    // the readers do not derive it yet (#556).
    let Err(truncated) = Grib1Reader::from_bytes(whole[..40].to_vec()) else {
        panic!("a truncated message must not parse");
    };
    match truncated {
        Grib1Error::Parse(message) => assert!(message.contains("bytes remain"), "{message}"),
        other => panic!("expected a parse error, got {other:?}"),
    }

    let reader = Grib1Reader::from_bytes(whole).expect("the whole fixture parses");
    match reader.decode_message_values(reader.message_count()) {
        Err(Grib1Error::OutOfRange) => {}
        Err(other) => panic!("expected an out-of-range error, got {other:?}"),
        Ok(_) => panic!("decoding past the last message must not succeed"),
    }
}

#[test]
fn grib1_reduced_grid_decodes_and_places() {
    let reader = Grib1Reader::from_bytes(fixture(
        "crates/fieldglass-grib1/tests/fixtures/reduced_gg_n32.grib1",
    ))
    .expect("the fixture parses");
    let gds = reader.messages[0]
        .gds
        .as_ref()
        .expect("a reduced Gaussian message carries a GDS");

    let (ni, nj) = gds.dimensions().expect("a reduced grid reports its raster");
    let raster = reader.decode_message_raster(0).expect("the raster decodes");
    assert_eq!(raster.len(), ni as usize * nj as usize);

    // The `From` impl is the crate's public API, so `GridGeometry` is a name a
    // consumer has to be able to write.
    match Grib1Geometry::from(gds) {
        Grib1Geometry::Gaussian(params) => {
            assert_eq!((params.ni, params.nj), (ni, nj));
            // Not the `Lo2` the message declares: a reduced grid's east edge is
            // the one its widened rows reach (#543).
            assert_eq!(
                params.lon_last,
                gds.raster_bounds().expect("a placed grid has bounds").3
            );
        }
        other => panic!("expected a Gaussian geometry, got {other:?}"),
    }
}

// ── GRIB2 ────────────────────────────────────────────────────────────────────

#[test]
fn grib2_errors_are_matchable() {
    let whole = fixture("crates/fieldglass-grib2/tests/fixtures/octahedral_gaussian_o32.grib2");

    // A message whose declared length runs past the end of the file. `let …
    // else` rather than `expect_err`, which would need `Grib2Reader: Debug` —
    // the readers do not derive it yet (#556).
    let Err(truncated) = Grib2Reader::from_bytes(whole[..40].to_vec()) else {
        panic!("a truncated message must not parse");
    };
    match truncated {
        Grib2Error::Parse(message) => assert!(message.contains("bytes remain"), "{message}"),
        other => panic!("expected a parse error, got {other:?}"),
    }

    let reader = Grib2Reader::from_bytes(whole).expect("the whole fixture parses");
    match reader.decode_message_values(reader.message_count()) {
        Err(Grib2Error::OutOfRange) => {}
        Err(other) => panic!("expected an out-of-range error, got {other:?}"),
        Ok(_) => panic!("decoding past the last message must not succeed"),
    }
}

#[test]
fn grib2_octahedral_grid_decodes_and_places() {
    let reader = Grib2Reader::from_bytes(fixture(
        "crates/fieldglass-grib2/tests/fixtures/octahedral_gaussian_o32.grib2",
    ))
    .expect("the fixture parses");
    let gds = &reader.messages[0].gds;

    let (ni, nj) = gds.dimensions().expect("a reduced grid reports its raster");
    let raster = reader.decode_message_raster(0).expect("the raster decodes");
    assert_eq!(raster.len(), ni as usize * nj as usize);

    match Grib2Geometry::from(gds) {
        Grib2Geometry::Gaussian(params) => {
            assert_eq!((params.ni, params.nj), (ni, nj));
            assert_eq!(
                params.lon_last,
                gds.raster_bounds().expect("a placed grid has bounds").3
            );
        }
        other => panic!("expected a Gaussian geometry, got {other:?}"),
    }
}

/// Three §3 templates hand back a `core` parameter struct by value, so a
/// consumer that stores or forwards one has to be able to write the type.
/// These signatures are the claim; the fixtures below make one of each real.
fn azimuthal_ni(params: &fieldglass_grib2::LambertAzimuthalParams) -> u32 {
    params.ni
}

fn transverse_ni(params: &fieldglass_grib2::TransverseMercatorParams) -> u32 {
    params.ni
}

fn space_view_columns(params: &fieldglass_grib2::GeostationaryParams) -> u32 {
    params.ni
}

#[test]
fn grib2_projection_parameters_are_nameable() {
    let reader = Grib2Reader::from_bytes(fixture(
        "crates/fieldglass-grib2/tests/fixtures/lambert_azimuthal_efas.grib2",
    ))
    .expect("the fixture parses");
    let GridTemplate::LambertAzimuthal(template) = &reader.messages[0].gds.template else {
        panic!("the fixture is a §3.140 grid");
    };
    let (ni, _) = reader.messages[0].gds.dimensions().expect("a placed grid");
    assert_eq!(azimuthal_ni(&template.projection_params()), ni);

    let reader = Grib2Reader::from_bytes(fixture(
        "crates/fieldglass-grib2/tests/fixtures/transverse_mercator_ukv.grib2",
    ))
    .expect("the fixture parses");
    let GridTemplate::TransverseMercator(template) = &reader.messages[0].gds.template else {
        panic!("the fixture is a §3.12 grid");
    };
    let (ni, _) = reader.messages[0].gds.dimensions().expect("a placed grid");
    assert_eq!(transverse_ni(&template.projection_params()), ni);

    // No §3.90 fixture is committed, so the space-view parameters are proven
    // by the signature above and by naming the type here rather than by a
    // decode. `scan_grid` is `Option`: a template with no `Nr` has no grid.
    let no_grid: Option<fieldglass_grib2::GeostationaryParams> = None;
    assert!(no_grid.as_ref().map(space_view_columns).is_none());
}

// ── The shared WMO tables ────────────────────────────────────────────────────

#[test]
fn both_editions_read_centre_and_sub_centre_from_one_module() {
    // C-1 and C-11 give each edition its own centre table; C-12 keys the
    // sub-centre on the pair and is shared, so it lives in core. A consumer
    // rendering a message header needs both halves and should not have to know
    // which crate each one came from.
    assert_eq!(
        fieldglass_grib1::tables_cct::lookup_centre(7),
        Some("US National Weather Service - National Centres for Environmental Prediction (NCEP)")
    );
    assert_eq!(
        fieldglass_grib1::tables_cct::lookup_sub_centre(7, 4),
        Some("Environmental Modeling Center")
    );
    assert_eq!(
        fieldglass_grib2::tables_cct::lookup_sub_centre(7, 4),
        fieldglass_grib1::tables_cct::lookup_sub_centre(7, 4)
    );
}

// ── NetCDF ───────────────────────────────────────────────────────────────────

#[test]
fn netcdf_errors_are_matchable() {
    let Err(err) = NetcdfReader::from_bytes(b"neither CDF nor HDF5".to_vec()) else {
        panic!("garbage must not parse as NetCDF");
    };
    match err {
        NetcdfError::InvalidMagic => {}
        other => panic!("expected an invalid-magic error, got {other:?}"),
    }
}

/// A source that owns its bytes but refuses to hand out a borrow, the shape
/// ADR-0005 says a cache-backed remote source is forced into. Written here to
/// prove the trait is implementable from outside the workspace's own crates.
struct CountingSource {
    bytes: Vec<u8>,
    prefetched: Cell<usize>,
    reads: Cell<usize>,
}

impl ByteSource for CountingSource {
    fn size(&self) -> u64 {
        self.bytes.len() as u64
    }

    fn prefetch(&self, ranges: &[ByteRange]) -> Result<(), NetcdfError> {
        self.prefetched.set(self.prefetched.get() + ranges.len());
        Ok(())
    }

    fn read(&self, range: ByteRange) -> Result<Cow<'_, [u8]>, NetcdfError> {
        self.reads.set(self.reads.get() + 1);
        let end = range
            .end()
            .and_then(|e| usize::try_from(e).ok())
            .filter(|&e| e <= self.bytes.len())
            .ok_or(NetcdfError::OutOfRange)?;
        let start = usize::try_from(range.start).map_err(|_| NetcdfError::OutOfRange)?;
        Ok(Cow::Owned(self.bytes[start..end].to_vec()))
    }
}

#[test]
fn netcdf_classic_decodes_through_a_caller_supplied_byte_source() {
    let bytes = fixture("crates/fieldglass-netcdf/tests/fixtures/ersst_v5_187001_cdf1.nc");
    let header = classic::parse_header(&bytes).expect("the fixture is a classic CDF file");

    // The variable the plan is asked for is picked by name so the test does
    // not depend on the fixture's declaration order.
    let var_index = header
        .variables
        .iter()
        .position(|v| v.name == "sst")
        .expect("the fixture declares an `sst` variable");

    let plan = classic::variable_plan(&header, var_index).expect("the plan resolves");
    assert!(!plan.is_empty());
    let planned: u64 = plan.iter().map(|r| r.len).sum();

    let source = CountingSource {
        bytes: bytes.clone(),
        prefetched: Cell::new(0),
        reads: Cell::new(0),
    };
    let through_source = classic::decode_variable_values_from(&header, &source, var_index)
        .expect("decode through the source");
    assert_eq!(source.prefetched.get(), plan.len());
    assert_eq!(source.reads.get(), plan.len());
    assert_eq!(
        planned,
        through_source.len() as u64 * 4,
        "sst is 4-byte data"
    );

    // Same answer as the in-memory path, which is the only claim that makes
    // the seam worth having.
    let direct =
        classic::decode_variable_values(&header, &bytes, var_index).expect("in-memory decode");
    assert_eq!(direct, through_source);

    // The reader's own backing is reachable without naming a core type.
    let reader = NetcdfReader::from_bytes(bytes).expect("the fixture parses");
    assert!(matches!(reader.backing, NetcdfBacking::Classic(_)));
}
