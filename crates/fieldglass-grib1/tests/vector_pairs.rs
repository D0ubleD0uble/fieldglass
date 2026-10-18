//! Which GRIB1 parameters are u/v pairs, read from the names this crate's
//! tables give them (#1049).
//!
//! The rule is `fieldglass_core::vector_pair`'s, held here to ON388 Table 2
//! (NCEP's 33/34 and the rest) and to every ECMWF local table this crate
//! carries (128's 165/166 and 131/132 among them).
//!
//! `cargo test -p fieldglass-grib1 --test vector_pairs -- --nocapture` prints
//! every pair the sweep finds.

use std::collections::BTreeSet;

use fieldglass_core::vector_pair::{
    VectorPairing, mentions_a_direction, pairing_of_parameter_names,
};
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
fn ecmwf_east_west_and_longitudinal_spellings_pair() {
    for (table, u, v) in [
        // East-West / North-South gravity wave drag tendency.
        (128, 220, 221),
        // East-West / North-South surface stress.
        (160, 180, 181),
        // Longitudinal / Meridional component of gravity wave stress.
        (172, 195, 196),
    ] {
        assert_eq!(
            pairing(table, ECMWF, u, v),
            VectorPairing::Recognised,
            "{table}: {u} / {v}"
        );
        assert_eq!(
            pairing(table, ECMWF, v, u),
            VectorPairing::Swapped,
            "{table}: {v} / {u}"
        );
    }
    // The variance of each component is two scalars, not a vector.
    assert_eq!(pairing(162, ECMWF, 215, 220), VectorPairing::NotAPair);
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

/// Every name ON388 Table 2 and the ECMWF tables hold that mentions a
/// direction.
fn directional_names() -> BTreeSet<&'static str> {
    let mut names = BTreeSet::new();
    for (version, centre) in std::iter::once((2, NCEP)).chain((128..=254u8).map(|v| (v, ECMWF))) {
        for id in 0..=255u8 {
            if let Some(e) = lookup_parameter(id, version, centre, 0)
                && mentions_a_direction(e.name)
            {
                names.insert(e.name);
            }
        }
    }
    names
}

/// The names that mention a direction and pair with nothing, reviewed one by
/// one (#1049). Each is either no component at all or a component whose
/// partner no table carries.
const UNPARTNERED: &[&str] = &[
    // Both axes in one name: a cross-covariance or product of u with v, not one component.
    "Covariance between meridional and zonal wind",
    "Covariance of V component and U component",
    "Covariance of v component/u component",
    "U*V product",
    "UV - U~V~",
    // A scalar statistic of one component, which the rule stops on purpose (see `fieldglass_core::vector_pair`).
    "East-West component of sub-gridscale orographic variance",
    "East-West component of sub-gridscale orographic variance anomaly",
    "East-West component of sub-gridscale orographic variance difference",
    "East-West component of sub-gridscale orographic variance gradient",
    "North-South component of sub-gridscale orographic variance",
    "North-South component of sub-gridscale orographic variance anomaly",
    "North-South component of sub-gridscale orographic variance difference",
    "North-South component of sub-gridscale orographic variance gradient",
    "Standard deviation of U velocity",
    "Standard deviation of V component",
    "Variance of u component",
    "Variance of v component",
    "U*T product",
    "U*U product",
    "V*T product",
    "V*V product",
    // ECMWF's eddy products and covariances, whose u and v spellings differ inside a word (`UT` and `VT`, `[uT]` and `[vT]`). Not drawn as arrows; left unpaired.
    "UT - U~T~",
    "UU - U~U~",
    "VT - V~T~",
    "VV - V~V~",
    // A component whose partner no table carries.
    "U velocity maximum",
];

/// Every name that mentions a direction either finds its partner or is on
/// [`UNPARTNERED`]: a table regeneration that adds a spelling the rule does
/// not read fails here instead of drawing arrows with no note.
#[test]
fn every_directional_name_pairs_or_is_reviewed() {
    let names = directional_names();
    let unpartnered: Vec<&str> = names
        .iter()
        .copied()
        .filter(|&a| {
            !names
                .iter()
                .any(|&b| pairing_of_parameter_names(Some(a), Some(b)) != VectorPairing::NotAPair)
        })
        .collect();
    for name in &unpartnered {
        println!("UNPARTNERED {name:?}");
    }
    let unpartnered: BTreeSet<&str> = unpartnered.into_iter().collect();
    let reviewed: BTreeSet<&str> = UNPARTNERED.iter().copied().collect();
    assert_eq!(reviewed.len(), UNPARTNERED.len(), "a name is listed twice");
    let new: Vec<_> = unpartnered.difference(&reviewed).collect();
    let stale: Vec<_> = reviewed.difference(&unpartnered).collect();
    assert!(
        new.is_empty() && stale.is_empty(),
        "names that pair with nothing and are not reviewed: {new:?}; \
         reviewed names that now pair or are gone: {stale:?}"
    );
}
