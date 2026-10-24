//! An array at a store's root is named by the host (#1468).
//!
//! It has no name inside the store, so `Session::open_store` lists it as the
//! empty string. `Session::open_store_named` takes the name the host knows it
//! by, such as the folder it was opened from, and the array is listed and
//! decoded under it.
//!
//! Paths are relative for the `wasm32-wasip1` run's sandbox, which preopens the
//! package directory and its parent.

#![cfg(feature = "zarr")]

use fieldglass::{DecodeOptions, MemoryObjects, Session};

/// The array folder `v3_v2keys/k`, as the objects a host would hand over.
fn array_folder() -> MemoryObjects {
    let dir = "../fieldglass-zarr/tests/fixtures/stores/v3_v2keys/k";
    MemoryObjects::from_iter(std::fs::read_dir(dir).expect("the fixture").map(|entry| {
        let path = entry.expect("entry").path();
        let key = path.file_name().unwrap().to_string_lossy().into_owned();
        (key, std::fs::read(&path).expect("read"))
    }))
}

#[test]
fn a_named_root_array_is_listed_and_decodes_under_its_name() {
    let session = Session::open_store_named(array_folder(), "k").expect("opens");
    let variables = session.variables();
    assert_eq!(variables.len(), 1);
    assert_eq!(variables[0].name, "k");
    let v = &variables[0];
    let field = session
        .decode_slice(v.index, 0, 1, &[0, 0], &DecodeOptions::default())
        .expect("decodes under the new name");
    assert!(field.ni > 0 && field.nj > 0);
}

#[test]
fn an_empty_name_is_as_open_store() {
    for session in [
        Session::open_store_named(array_folder(), "").expect("opens"),
        Session::open_store(array_folder()).expect("opens"),
    ] {
        let names: Vec<String> = session.variables().into_iter().map(|v| v.name).collect();
        assert_eq!(names, [""]);
    }
}
