//! A NetCDF-4 variable whose `DIMENSION_LIST` is damaged opens through
//! `Session` on anonymous axes, and its row says so (#1695). From #1557 the
//! damage failed the whole file, which 0.6.0 had opened. The reader-level
//! cases are `fieldglass-netcdf`'s `tests/hdf5_dimension_scales.rs`, which
//! describes the patch made here.

use fieldglass::{DecodeOptions, Session};

const EARLIEST: &[u8] =
    include_bytes!("../../fieldglass-netcdf/tests/fixtures/hdf5_no_userblock_earliest.h5");

/// `EARLIEST` with `v`'s second heap ID naming object 1, and `x`'s object
/// renumbered 1, so two objects carry the index it names.
fn repeated_index() -> Vec<u8> {
    let mut file = EARLIEST.to_vec();
    for (at, was, bytes) in [
        (2200, &2u16.to_le_bytes()[..], &1u16.to_le_bytes()[..]),
        (1792 + 12, &2u32.to_le_bytes()[..], &1u32.to_le_bytes()[..]),
    ] {
        assert_eq!(
            &file[at..at + was.len()],
            was,
            "the fixture's bytes at {at}"
        );
        file[at..at + bytes.len()].copy_from_slice(bytes);
    }
    file
}

/// What the row of a variable whose `DIMENSION_LIST` is damaged says,
/// whatever the damage: the parse error is left to the library (#1604).
const DAMAGED_NOTE: &str = "attribute \"DIMENSION_LIST\" not read: the file's record of \
    the variable's dimensions is damaged, so its axes are unnamed";

#[test]
fn a_variable_whose_dimension_list_is_damaged_opens_on_anonymous_axes_with_a_note() {
    let session = Session::open(repeated_index()).expect("the file opens");
    let whole = Session::open(EARLIEST.to_vec()).expect("the whole file opens");
    let variables = session.variables();
    let row = |name: &str| {
        variables
            .iter()
            .find(|v| v.name == name)
            .unwrap_or_else(|| panic!("{name} is listed"))
    };

    let v = row("v");
    let dims: Vec<(&str, u64)> = v.dims.iter().map(|d| (d.name.as_str(), d.length)).collect();
    assert_eq!(dims, [("phony_dim_2", 4), ("phony_dim_3", 5)]);
    assert_eq!(v.attributes_not_read, Some(DAMAGED_NOTE.to_string()));
    // `c` was always on anonymous axes, and keeps them, with no note.
    let c = row("c");
    let whole_c = whole
        .variables()
        .into_iter()
        .find(|v| v.name == "c")
        .unwrap();
    assert_eq!(format!("{c:?}"), format!("{whole_c:?}"));
    assert_eq!(c.attributes_not_read, None);
    assert_eq!(session.global_attributes_not_read(), None);

    // Its values are untouched. With its axes unnamed it has no coordinates,
    // so it is drawn unplaced rather than on `y` and `x`.
    let field = |s: &Session, index: u32| {
        s.decode_slice(index, 0, 1, &[0, 0], &DecodeOptions::default())
            .expect("v decodes")
    };
    let whole_v = whole
        .variables()
        .into_iter()
        .find(|v| v.name == "v")
        .unwrap();
    let (got, want) = (field(&session, v.index), field(&whole, whole_v.index));
    assert_eq!(format!("{:?}", got.values), format!("{:?}", want.values));
    assert_eq!(got.mask, want.mask);
    assert_eq!(got.georef.kind, "unsupported");
}

/// The structured names a host reads instead of the note (#1687) name the
/// damaged `DIMENSION_LIST` too, beside `variables`' note.
#[test]
fn a_damaged_dimension_list_is_among_the_names_the_variable_lost() {
    use fieldglass_core::array::ArraySource;
    let reader = fieldglass::netcdf::NetcdfReader::from_bytes(repeated_index()).expect("opens");
    let arrays = fieldglass::netcdf::NetcdfArrays::open(reader).expect("the view resolves");
    let lost = arrays.unread_attribute_names("v").expect("v lost one");
    assert_eq!(lost.names().collect::<Vec<_>>(), ["DIMENSION_LIST"]);
    assert_eq!(lost.read_in_part().count(), 0);
    assert_eq!(arrays.unread_attribute_names("c"), None);
}
