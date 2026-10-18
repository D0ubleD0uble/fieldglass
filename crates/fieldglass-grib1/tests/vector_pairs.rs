//! Which GRIB1 parameters are u/v pairs, read from the names this crate's
//! tables give them (#1049).
//!
//! The rule is `fieldglass_core::vector_pair`'s, held here to ON388 Table 2
//! (NCEP's 33/34 and the rest) and to every ECMWF local table this crate
//! carries (128's 165/166 and 131/132 among them).
//!
//! `cargo test -p fieldglass-grib1 --test vector_pairs -- --nocapture` prints
//! every pair the sweep finds.

use fieldglass_core::vector_pair::{VectorPairing, pairing_of_parameter_names};
use fieldglass_grib1::tables::lookup_parameter;

const NCEP: u8 = 7;
const ECMWF: u8 = 98;

fn pairing(table_version: u8, centre: u8, u: u8, v: u8) -> VectorPairing {
    let name = |id| {
        lookup_parameter(id, table_version, centre, 0)
            .unwrap_or_else(|| panic!("{centre}/{table_version}/{id} resolves"))
            .name
    };
    pairing_of_parameter_names(Some(name(u)), Some(name(v)))
}

#[test]
fn on388_table_2_pairs() {
    for (u, v) in [
        // UGRD / VGRD.
        (33, 34),
        // UOGRD / VOGRD.
        (49, 50),
        // Momentum flux, u and v.
        (124, 125),
    ] {
        assert_eq!(
            pairing(2, NCEP, u, v),
            VectorPairing::Recognised,
            "{u} / {v}"
        );
        assert_eq!(pairing(2, NCEP, v, u), VectorPairing::Swapped, "{v} / {u}");
    }
    for (u, v) in [
        // Wind direction with wind speed: CMC's 300 hPa wind is the second.
        (31, 32),
        (32, 32),
        // Temperature with a component.
        (11, 34),
        // The same component twice.
        (33, 33),
        // Wind with current.
        (33, 50),
    ] {
        assert_eq!(pairing(2, NCEP, u, v), VectorPairing::NotAPair, "{u} / {v}");
    }
}

#[test]
fn ecmwf_table_128_pairs() {
    for (u, v) in [
        // u / v.
        (131, 132),
        // 10u / 10v.
        (165, 166),
    ] {
        assert_eq!(
            pairing(128, ECMWF, u, v),
            VectorPairing::Recognised,
            "{u} / {v}"
        );
        assert_eq!(
            pairing(128, ECMWF, v, u),
            VectorPairing::Swapped,
            "{v} / {u}"
        );
    }
    for (u, v) in [
        // u at a level with v at 10 m: two different vectors.
        (131, 166),
        // 10 m wind speed with 10v.
        (207, 166),
        // Temperature with u.
        (130, 131),
    ] {
        assert_eq!(
            pairing(128, ECMWF, u, v),
            VectorPairing::NotAPair,
            "{u} / {v}"
        );
    }
}

/// Every pair in one table, in order, as `(u, v, u name, v name)`.
fn sweep(table_version: u8, centre: u8) -> Vec<(u8, u8, &'static str, &'static str)> {
    let named: Vec<(u8, &'static str)> = (0..=255u8)
        .filter_map(|id| lookup_parameter(id, table_version, centre, 0).map(|e| (id, e.name)))
        .collect();
    let mut pairs = Vec::new();
    for &(u, u_name) in &named {
        for &(v, v_name) in &named {
            if pairing_of_parameter_names(Some(u_name), Some(v_name)) == VectorPairing::Recognised {
                pairs.push((u, v, u_name, v_name));
            }
        }
    }
    pairs
}

#[test]
fn every_pair_the_tables_define() {
    let mut tables = vec![("ON388 Table 2".to_string(), 2, NCEP)];
    for version in 128..=254u8 {
        if (0..=255u8).any(|id| lookup_parameter(id, version, ECMWF, 0).is_some()) {
            tables.push((format!("ECMWF table {version}"), version, ECMWF));
        }
    }
    let mut total = 0;
    for (label, version, centre) in &tables {
        let pairs = sweep(*version, *centre);
        if pairs.is_empty() {
            continue;
        }
        println!("{label}: {} pairs", pairs.len());
        for (u, v, u_name, v_name) in &pairs {
            println!("  {u} / {v}: {u_name} / {v_name}");
            assert_eq!(
                pairing_of_parameter_names(Some(v_name), Some(u_name)),
                VectorPairing::Swapped,
                "{label}: {u} / {v}"
            );
        }
        total += pairs.len();
    }
    assert!(total >= 7, "the pairs named above at least: {total}");
}
