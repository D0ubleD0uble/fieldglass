//! Unit-notation normalisation over the whole NetCDF corpus (#453).
//!
//! NetCDF units differ from every other seam this module serves. A GRIB unit
//! comes from a table generated from a WMO tag; a NetCDF unit comes from the
//! **file's author**. ADR-0007 records why they are normalised anyway — CF
//! requires the attribute to be parsable by UDUNITS, so the author picked one
//! machine-readable spelling among equivalents rather than writing prose — and
//! where the line falls: notation is restored, names are left alone.
//!
//! The snapshot below is the deliverable, not scaffolding, for the same reason
//! the GRIB1 and GRIB2 ones are: normalisation is the kind of transform where a
//! rule that looks obviously right mangles a handful of real inputs. Every
//! distinct `units` string the committed corpus contains is pinned with its
//! exact output, so a rule change shows up as a diff across all of them.
//!
//! It also closes the gap #450 hit and #453 named: a string whose every token
//! is *almost* recognised passes through whole and silently, and looks
//! identical to a string that was deliberately left alone. Here that shows as a
//! snapshot line rather than as nothing at all.
//!
//! Regenerate with `UPDATE_UNIT_SNAPSHOT=1 cargo test -p fieldglass-netcdf
//! --test unit_notation` and read the diff before committing it.

use fieldglass_core::units::normalize_units;
use fieldglass_netcdf::{DatasetView, NetcdfBacking, NetcdfReader};
use std::collections::BTreeSet;

const SNAPSHOT_PATH: &str = "tests/fixtures/unit_notation.snapshot.txt";
const SNAPSHOT: &str = include_str!("fixtures/unit_notation.snapshot.txt");

/// Every distinct `units` attribute in every committed fixture.
///
/// Read from the fixture directory rather than a list, so a fixture added later
/// joins the sweep without anyone remembering to add it — which is exactly the
/// omission that made the GRIB2 sweep miss 59 of 69 strings (#476).
fn distinct_units() -> BTreeSet<String> {
    let dir = std::path::Path::new("tests/fixtures");
    let mut units = BTreeSet::new();
    let mut files = 0usize;
    for entry in std::fs::read_dir(dir).expect("fixture directory") {
        let path = entry.expect("dir entry").path();
        if !matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("nc") | Some("h5")
        ) {
            continue;
        }
        let bytes = std::fs::read(&path).expect("read fixture");
        // A fixture whose HDF5 layout this crate cannot resolve has no
        // variables to read units from; that is decision 0003's territory, not
        // this test's, so it is skipped rather than failed.
        let Ok(reader) = NetcdfReader::from_bytes(bytes) else {
            continue;
        };
        let view = match &reader.backing {
            NetcdfBacking::Classic(h) => DatasetView::from_classic(h),
            NetcdfBacking::Hdf5(_) => match reader.hdf5_metadata() {
                Ok(meta) => DatasetView::from_hdf5(&meta),
                Err(_) => continue,
            },
        };
        files += 1;
        for var in &view.vars {
            for (name, value) in &var.attrs {
                if name == "units" && !value.is_empty() {
                    units.insert(value.clone());
                }
            }
        }
    }
    assert!(
        files >= 10,
        "only {files} fixtures were read — the corpus is not loaded, so this \
         proves nothing"
    );
    units
}

fn render_snapshot(units: &BTreeSet<String>) -> String {
    let mut out = String::new();
    for unit in units {
        // Debug-quoted on both sides, as the GRIB sweeps do: it makes a
        // trailing space visible instead of mysterious, and keeps the
        // trailing-whitespace hook from editing the snapshot underneath us.
        out.push_str(&format!("{unit:?}\t{:?}\n", normalize_units(unit)));
    }
    out
}

#[test]
fn the_whole_corpus_normalises_as_pinned() {
    let units = distinct_units();
    assert!(
        units.len() > 25,
        "only {} distinct units — the corpus is not loaded, so this proves \
         nothing",
        units.len()
    );

    let rendered = render_snapshot(&units);
    if std::env::var("UPDATE_UNIT_SNAPSHOT").is_ok() {
        std::fs::write(SNAPSHOT_PATH, &rendered).expect("write snapshot");
        return;
    }
    assert_eq!(
        rendered, SNAPSHOT,
        "unit normalisation changed. Re-read the diff, then regenerate with \
         UPDATE_UNIT_SNAPSHOT=1"
    );
}

/// A time encoding is never touched.
///
/// `minutes since 1870-01-01 00:00` states an epoch, and RTOFS writes a date as
/// `day as %Y%m%d.%f`. Neither is a unit. Both survived before this issue only
/// because `since`, `as` and a date are not recognised symbols — true, but
/// incidental, and it stops being true the moment the vocabulary grows. Both
/// begin with a token that *is* a real unit, so a wider vocabulary would
/// half-rewrite them. ADR-0007 makes it a rule; this is the rule's test, and it
/// asserts on spellings the corpus does *not* contain as well as ones it does.
///
/// The second form was found by the corpus sweep above, not by anyone thinking
/// of it, which is the argument for having the sweep.
#[test]
fn a_time_encoding_is_refused_by_rule() {
    for encoding in [
        "minutes since 1870-01-01 00:00",
        "hours since 2020-01-01 00:00:00",
        "seconds since 1970-01-01T00:00:00Z",
        "days since 1978-01-01 12:00:00",
        "day as %Y%m%d.%f",
        // Spellings no fixture carries, where every leading token *is* a
        // recognised symbol — the case the incidental protection would miss.
        "s since 1970-01-01",
        "d since 2000-01-01",
        "h SINCE 2000-01-01",
        "d as %Y%m%d",
    ] {
        assert_eq!(
            normalize_units(encoding),
            encoding,
            "a time encoding must pass through untouched"
        );
    }
}

/// Names are the author's; notation is not.
///
/// The line ADR-0007 draws, asserted directly so a future widening of the
/// vocabulary has to argue with it. `um` becomes `µm` because `u` is what CF
/// writes where the symbol is `µ` and the attribute is ASCII — the same act as
/// turning `-1` into `⁻¹`. `meters` stays `meters` because that is a different
/// *word*, and which word to use is the file author's business.
#[test]
fn notation_is_restored_and_names_are_left_alone() {
    for (input, expected) in [
        // Notation: restored.
        ("m/s", "m s⁻¹"),
        ("cm/s", "cm s⁻¹"),
        ("mm/hr", "mm hr⁻¹"),
        ("W m-2 sr-1 um-1", "W m⁻² sr⁻¹ µm⁻¹"),
        // Names: untouched.
        ("meters", "meters"),
        ("kelvin", "kelvin"),
        ("Kelvin", "Kelvin"),
        ("Celsius", "Celsius"),
        ("degree_C", "degree_C"),
        ("degree_Celsius", "degree_Celsius"),
        ("degrees_north", "degrees_north"),
        ("percent", "percent"),
        ("counts", "counts"),
    ] {
        assert_eq!(normalize_units(input), expected, "for {input:?}");
    }
}
