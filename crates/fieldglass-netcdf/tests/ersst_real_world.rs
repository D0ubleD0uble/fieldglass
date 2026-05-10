//! End-to-end parse of a real NOAA ERSST v5 monthly SST file across all three
//! classic on-disk layouts. The CDF-1 file is the verbatim NCEI distribution;
//! the CDF-2 and CDF-5 copies are the same dataset re-encoded by the canonical
//! Unidata `netCDF4` library (see `tests/fixtures/NOTICE.md` and
//! `build_fixtures.py`).
//!
//! The same logical content parsing the same way across all three encodings is
//! the point: the asserts here are intentionally identical between variants,
//! and the only things that change file-to-file are the on-disk widths the
//! parser quietly negotiates (4-byte vs 8-byte `nelems`, `vsize`, `begin`).

use fieldglass_netcdf::{ClassicVersion, NcType, NetcdfBacking, NetcdfReader};

const ERSST_CDF1: &[u8] = include_bytes!("fixtures/ersst_v5_187001_cdf1.nc");
const ERSST_CDF2: &[u8] = include_bytes!("fixtures/ersst_v5_187001_cdf2.nc");
const ERSST_CDF5: &[u8] = include_bytes!("fixtures/ersst_v5_187001_cdf5.nc");

fn open(bytes: &[u8]) -> NetcdfReader {
    NetcdfReader::from_bytes(bytes.to_vec()).expect("ERSST fixture parses")
}

fn header(reader: &NetcdfReader) -> &fieldglass_netcdf::ClassicHeader {
    match &reader.backing {
        NetcdfBacking::Classic(h) => h,
        other => panic!("expected Classic backing, got {:?}", other.label()),
    }
}

/// The set of facts every ERSST v5 187001 fixture must agree on, regardless of
/// which classic on-disk variant encodes it. Lifted from the published
/// upstream file via `ncdump -h` — no synthesis.
fn assert_ersst_invariants(reader: &NetcdfReader) {
    let h = header(reader);

    // None of the dimensions are unlimited in this product.
    assert_eq!(h.numrecs, Some(0));

    let dims: std::collections::HashMap<&str, u64> = h
        .dimensions
        .iter()
        .map(|d| (d.name.as_str(), d.length))
        .collect();
    assert_eq!(dims.get("lat").copied(), Some(89));
    assert_eq!(dims.get("lev").copied(), Some(1));
    assert_eq!(dims.get("lon").copied(), Some(180));
    assert_eq!(dims.get("time").copied(), Some(1));
    for d in &h.dimensions {
        assert!(!d.is_record, "{} should not be the record dim", d.name);
    }

    // ERSST publishes 38 CF/ACDD globals; spot-check identity + license.
    let title = h
        .global_attributes
        .iter()
        .find(|a| a.name == "title")
        .expect("title global attr present");
    assert_eq!(title.nc_type, NcType::Char);
    assert_eq!(title.value, "NOAA ERSSTv5 (in situ only)");

    let license = h
        .global_attributes
        .iter()
        .find(|a| a.name == "license")
        .expect("license global attr present");
    assert_eq!(license.value, "No constraints on data access or use");

    // Numeric global attrs round-trip through `render_numeric_values` with
    // exact decimal text — these are floats that happen to be integer-valued.
    let lat_min = h
        .global_attributes
        .iter()
        .find(|a| a.name == "geospatial_lat_min")
        .expect("geospatial_lat_min present");
    assert_eq!(lat_min.nc_type, NcType::Float);
    assert_eq!(lat_min.value, "-89");
    let lat_max = h
        .global_attributes
        .iter()
        .find(|a| a.name == "geospatial_lat_max")
        .expect("geospatial_lat_max present");
    assert_eq!(lat_max.value, "89");

    // Variable identity: ERSST publishes coordinate vars + the SST product.
    let by_name: std::collections::HashMap<&str, &fieldglass_netcdf::Variable> =
        h.variables.iter().map(|v| (v.name.as_str(), v)).collect();
    let sst = by_name.get("sst").expect("sst variable present");
    assert_eq!(sst.nc_type, NcType::Float);
    assert_eq!(
        sst.dim_ids
            .iter()
            .map(|i| h.dimensions[*i as usize].name.as_str())
            .collect::<Vec<_>>(),
        vec!["time", "lev", "lat", "lon"]
    );
    // Spot-check one of `sst`'s typed attributes.
    let units = sst
        .attributes
        .iter()
        .find(|a| a.name == "units")
        .expect("sst:units present");
    assert_eq!(units.value, "degree_C");
    let scale = sst
        .attributes
        .iter()
        .find(|a| a.name == "scale_factor")
        .expect("sst:scale_factor present");
    assert_eq!(scale.nc_type, NcType::Float);

    let lat = by_name.get("lat").expect("lat coord var");
    assert_eq!(lat.nc_type, NcType::Double);

    // Every variable's dim refs land inside the dim list.
    for v in &h.variables {
        for &did in &v.dim_ids {
            assert!(
                (did as usize) < h.dimensions.len(),
                "{} references out-of-range dim id {did}",
                v.name
            );
        }
    }
}

#[test]
fn ersst_cdf1_parses() {
    let reader = open(ERSST_CDF1);
    assert_eq!(header(&reader).version, ClassicVersion::Cdf1);
    assert_ersst_invariants(&reader);
}

#[test]
fn ersst_cdf2_parses() {
    let reader = open(ERSST_CDF2);
    assert_eq!(header(&reader).version, ClassicVersion::Cdf2);
    assert_ersst_invariants(&reader);
}

#[test]
fn ersst_cdf5_parses() {
    let reader = open(ERSST_CDF5);
    assert_eq!(header(&reader).version, ClassicVersion::Cdf5);
    assert_ersst_invariants(&reader);
}

/// All three encodings should report the same backing label *family*, but the
/// version-specific label must change. This pins the user-visible string the
/// provider surfaces in the metadata view.
#[test]
fn version_labels_distinguish_the_three_layouts() {
    assert_eq!(open(ERSST_CDF1).backing.label(), "NetCDF classic (CDF-1)");
    assert_eq!(
        open(ERSST_CDF2).backing.label(),
        "NetCDF 64-bit offset (CDF-2)"
    );
    assert_eq!(
        open(ERSST_CDF5).backing.label(),
        "NetCDF 64-bit data (CDF-5)"
    );
}

/// `begin` for each variable must move past the header and stay inside the
/// file. This is the integration check on `read_offset`'s 32-bit (CDF-1) vs
/// 64-bit (CDF-2/5) branches against real on-disk values.
#[test]
fn variable_begins_are_in_bounds_across_all_layouts() {
    for (label, bytes) in [
        ("CDF-1", ERSST_CDF1),
        ("CDF-2", ERSST_CDF2),
        ("CDF-5", ERSST_CDF5),
    ] {
        let reader = open(bytes);
        let h = header(&reader);
        for v in &h.variables {
            assert!(
                v.begin > 0,
                "{label}: {} has begin=0 (must point past the header)",
                v.name
            );
            assert!(
                (v.begin as usize) < bytes.len(),
                "{label}: {} begin={} exceeds file size {}",
                v.name,
                v.begin,
                bytes.len()
            );
        }
    }
}
