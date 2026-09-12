//! The dataset view is `fieldglass-core`'s array model (#684): what each
//! backing reads a file into, checked against the files themselves rather than
//! against how the view happens to be built.

use fieldglass_netcdf::array::{AttributeValue, ElementType};
use fieldglass_netcdf::{
    Attribute, ClassicHeader, ClassicVersion, DatasetView, Dimension, NcType, NetcdfReader,
    Variable,
};

const ERSST: &[u8] = include_bytes!("fixtures/ersst_v5_187001_cdf1.nc");
const GROUPED: &[u8] = include_bytes!("fixtures/netcdf4_grouped.nc");
const GOES: &[u8] = include_bytes!("fixtures/goes16_abi_cmip.nc");

fn view(bytes: &[u8]) -> DatasetView {
    NetcdfReader::from_bytes(bytes.to_vec())
        .expect("the fixture parses")
        .view()
        .expect("the fixture has a view")
}

/// Looking an array up by name finds what listing the tree finds, on every
/// fixture — including the nested NetCDF-4 groups, which are where a descent by
/// path segment can go wrong.
///
/// `Group::array_qualified` descends where `arrays_qualified` builds the whole
/// list, and they have to agree exactly or a cheap lookup is a wrong lookup
/// (#709). A NetCDF root group is *unnamed*, and `walk` contributes no path
/// segment for one — so the descent must not either, which is the case this
/// fixture set exercises and a synthetic tree would not.
///
/// Equality by address, not by value: two variables can describe the same shape,
/// type and attributes, and a descent landing on the wrong one would compare
/// equal.
#[test]
fn array_lookup_agrees_with_listing_on_every_fixture() {
    use fieldglass_netcdf::array::ArraySource;

    let mut checked = 0usize;
    for (name, bytes) in [("ersst", ERSST), ("grouped", GROUPED), ("goes", GOES)] {
        let reader = NetcdfReader::from_bytes(bytes.to_vec()).expect("parses");
        let arrays = fieldglass_netcdf::NetcdfArrays::open(reader).expect("a view");
        let group = arrays.group();
        let listed = group.arrays_qualified();
        assert!(!listed.is_empty(), "{name} lists no arrays");
        for (qualified, array) in &listed {
            let found = group.array_qualified(qualified).unwrap_or_else(|| {
                panic!("{name}: listing spells {qualified:?}, lookup misses it")
            });
            assert!(
                std::ptr::eq(found, *array),
                "{name}: {qualified:?} resolved to a different array than the listing names"
            );
            // And the seam's own accessor, which is what `read_region_physical`
            // calls, agrees with both.
            assert!(
                std::ptr::eq(arrays.array(qualified).expect("through the trait"), *array),
                "{name}: ArraySource::array disagrees for {qualified:?}"
            );
            checked += 1;
        }
        assert!(group.array_qualified("no/such/array").is_none(), "{name}");
    }
    assert!(checked > 10, "only {checked} names compared");
}

/// The group a host walks names exactly what the view holds, on both backings
/// and through a nested NetCDF-4 group.
#[test]
fn the_group_holds_every_name_the_view_does() {
    for (label, bytes) in [("classic", ERSST), ("grouped", GROUPED), ("goes", GOES)] {
        let view = view(bytes);
        let group = view.group();

        let arrays: Vec<String> = group
            .arrays_qualified()
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        let vars: Vec<String> = view.vars.iter().map(|v| v.name().to_string()).collect();
        assert_eq!(arrays, vars, "{label}: arrays");
        assert!(!arrays.is_empty(), "{label}: the fixture has variables");

        let dimensions: Vec<(String, u64)> = group
            .dimensions_qualified()
            .into_iter()
            .map(|(name, d)| (name, d.length))
            .collect();
        let dims: Vec<(String, u64)> = view
            .dims
            .iter()
            .map(|d| (d.name.clone(), d.length))
            .collect();
        assert_eq!(dimensions, dims, "{label}: dimensions");

        assert_eq!(group.attributes, view.global_attrs, "{label}: attributes");
    }
    // Not vacuous: the grouped fixture really does nest.
    assert!(
        view(GROUPED).vars.iter().any(|v| v.name().contains('/')),
        "netcdf4_grouped.nc has a variable in a nested group"
    );
}

/// Text stays text and numbers stay numbers, as the file typed them: a
/// classic global of each kind, and an HDF5 attribute with more than one
/// element.
#[test]
fn attributes_keep_the_type_the_file_gave_them() {
    let ersst = view(ERSST);
    let global = |name: &str| {
        ersst
            .global_attrs
            .iter()
            .find(|a| a.name == name)
            .map(|a| &a.value)
            .unwrap_or_else(|| panic!("ERSST carries {name}"))
    };
    assert_eq!(
        global("title"),
        &AttributeValue::Text("NOAA ERSSTv5 (in situ only)".to_string())
    );
    assert_eq!(
        global("geospatial_lat_min"),
        &AttributeValue::Numbers(vec![-89.0])
    );

    let sst = ersst.vars.iter().find(|v| v.name() == "sst").expect("sst");
    assert_eq!(sst.array.element_type, ElementType::Float(32));
    assert_eq!(sst.nc_type(), Some(NcType::Float));

    let goes = view(GOES);
    let cmi = goes.vars.iter().find(|v| v.name() == "CMI").expect("CMI");
    assert_eq!(
        cmi.attribute("valid_range"),
        Some(&AttributeValue::Numbers(vec![0.0, 4095.0]))
    );
}

fn attribute(name: &str, nc_type: NcType, values: &[f64]) -> Attribute {
    Attribute {
        name: name.to_string(),
        nc_type,
        nelems: values.len() as u64,
        value: String::new(),
        values: values.to_vec(),
    }
}

/// Two things the display-string view got wrong, through the one path a host
/// takes to physical units.
///
/// A `missing_value` declaring two codes masks both: decode masks only the
/// first. And a `float` bound is the stored value widened, not its printed
/// form parsed back — `0.1f32` prints as `0.1`, which as an `f64` is *below*
/// the stored bound, so a value sitting exactly on it was thrown away.
#[test]
fn unpacking_through_the_view_masks_every_sentinel_and_keeps_float_bounds_exact() {
    let bound = f64::from(0.1f32);
    let header = ClassicHeader {
        version: ClassicVersion::Cdf1,
        numrecs: Some(0),
        dimensions: vec![Dimension {
            name: "x".to_string(),
            length: 4,
            is_record: false,
        }],
        global_attributes: Vec::new(),
        variables: vec![Variable {
            name: "v".to_string(),
            dim_ids: vec![0],
            nc_type: NcType::Float,
            attributes: vec![
                attribute("missing_value", NcType::Float, &[-1.0, -2.0]),
                attribute("valid_range", NcType::Float, &[-5.0, bound]),
            ],
            vsize: 16,
            begin: 0,
        }],
    };
    let view = DatasetView::from_classic(&header);
    let v = view.var(0).expect("the variable");
    assert_eq!(
        v.attribute("valid_range"),
        Some(&AttributeValue::Numbers(vec![-5.0, bound]))
    );
    assert_eq!(
        v.unpack(&[Some(bound), Some(-1.0), Some(-2.0), Some(-3.0)]),
        vec![Some(bound), None, None, Some(-3.0)]
    );
}
