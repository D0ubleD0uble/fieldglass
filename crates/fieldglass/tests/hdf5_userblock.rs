//! A NetCDF-4 file that begins with an HDF5 userblock opens through
//! `Session`, which detects its format from the bytes, and reads as its twin
//! without one (#936). The reader-level tests are `fieldglass-netcdf`'s
//! `tests/hdf5_userblock.rs`, where the fixtures are described.

use fieldglass::{DecodeOptions, Session};

const FIXTURES: &str = "../fieldglass-netcdf/tests/fixtures";

#[test]
fn a_file_with_a_userblock_opens_and_reads_as_its_twin() {
    for format in ["earliest", "latest"] {
        let read = |name: &str| std::fs::read(format!("{FIXTURES}/{name}_{format}.h5")).unwrap();
        let session = Session::open(read("hdf5_userblock"))
            .unwrap_or_else(|e| panic!("{format}: the file opens: {e}"));
        let twin = Session::open(read("hdf5_no_userblock")).expect("the twin opens");

        let variables = session.variables();
        let names: Vec<&str> = variables.iter().map(|v| v.name.as_str()).collect();
        assert!(
            names.contains(&"v") && names.contains(&"c"),
            "{format}: {names:?}"
        );
        assert_eq!(
            format!("{variables:?}"),
            format!("{:?}", twin.variables()),
            "{format}"
        );

        for v in &variables {
            let field = |s: &Session| {
                s.decode_slice(v.index, 0, 1, &[0, 0], &DecodeOptions::default())
                    .unwrap_or_else(|e| panic!("{format}: {} decodes: {e}", v.name))
            };
            let got = field(&session);
            assert_eq!(
                format!("{got:?}"),
                format!("{:?}", field(&twin)),
                "{format}: {}",
                v.name
            );
            assert!(
                got.mask.iter().all(|&m| m != 0),
                "{format}: {} is all present",
                v.name
            );
        }
    }
}
