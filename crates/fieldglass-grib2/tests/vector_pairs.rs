//! Which GRIB2 parameters are u/v pairs, read from the names this crate's
//! tables give them (#1049).
//!
//! The rule is `fieldglass_core::vector_pair`'s: a name with a direction word
//! pairs with the same name holding its partner word. This holds it to the
//! tables it reads: the pairs the WMO master table and the NCEP, ECMWF and DWD
//! local tables define are recognised in order and swapped out of it, and the
//! neighbours that are not a pair are not.
//!
//! `cargo test -p fieldglass-grib2 --test vector_pairs -- --nocapture` prints
//! every pair the sweep finds.

use fieldglass_core::vector_pair::{VectorPairing, pairing_of_parameter_names};
use fieldglass_grib2::{Originator, lookup_parameter};

const WMO: Originator = Originator {
    centre: 0,
    sub_centre: 0,
    local_tables_version: 0,
};
const NCEP: Originator = Originator {
    centre: 7,
    sub_centre: 0,
    local_tables_version: 1,
};
const ECMWF: Originator = Originator {
    centre: 98,
    sub_centre: 0,
    local_tables_version: 0,
};
const DWD: Originator = Originator {
    centre: 78,
    sub_centre: 0,
    local_tables_version: 1,
};

fn name(originator: Originator, (d, c, n): (u8, u8, u8)) -> &'static str {
    lookup_parameter(originator, d, c, n)
        .unwrap_or_else(|| panic!("{d}-{c}-{n} resolves"))
        .1
}

fn pairing(originator: Originator, u: (u8, u8, u8), v: (u8, u8, u8)) -> VectorPairing {
    pairing_of_parameter_names(Some(name(originator, u)), Some(name(originator, v)))
}

#[test]
fn wind_currents_storm_motion_and_their_variants_pair() {
    for (u, v) in [
        // UGRD / VGRD.
        ((0, 2, 2), (0, 2, 3)),
        // The variants in the same category: shear, momentum flux, gust,
        // storm motion, geostrophic and neutral wind, wind stress.
        ((0, 2, 15), (0, 2, 16)),
        ((0, 2, 17), (0, 2, 18)),
        ((0, 2, 23), (0, 2, 24)),
        ((0, 2, 27), (0, 2, 28)),
        ((0, 2, 41), (0, 2, 42)),
        ((0, 2, 50), (0, 2, 51)),
        ((0, 2, 52), (0, 2, 53)),
        ((0, 2, 56), (0, 2, 57)),
        // UOGRD / VOGRD, and the eastward / northward current.
        ((10, 1, 2), (10, 1, 3)),
        ((10, 1, 5), (10, 1, 6)),
        // Ice drift, surface Stokes drift.
        ((10, 2, 4), (10, 2, 5)),
        ((10, 0, 21), (10, 0, 22)),
    ] {
        assert_eq!(
            pairing(WMO, u, v),
            VectorPairing::Recognised,
            "{u:?} / {v:?}"
        );
        assert_eq!(pairing(WMO, v, u), VectorPairing::Swapped, "{v:?} / {u:?}");
    }
}

#[test]
fn neighbours_that_are_not_a_pair() {
    for (u, v) in [
        // Wind speed with wind direction.
        ((0, 2, 1), (0, 2, 0)),
        // Temperature with a component.
        ((0, 0, 0), (0, 2, 3)),
        // The same component twice.
        ((0, 2, 2), (0, 2, 2)),
        // Components of two different vectors.
        ((0, 2, 2), (0, 2, 28)),
        ((0, 2, 2), (10, 1, 3)),
        // One vector's u-component with the other spelling's northward one.
        ((10, 1, 2), (10, 1, 6)),
    ] {
        assert_eq!(pairing(WMO, u, v), VectorPairing::NotAPair, "{u:?} / {v:?}");
    }
}

#[test]
fn local_tables_pair_by_the_same_rule() {
    // NCEP's own spelling of the master pair, and its local storm motion and
    // Bunkers motion pairs.
    assert_eq!(
        pairing(NCEP, (0, 2, 2), (0, 2, 3)),
        VectorPairing::Recognised
    );
    assert_eq!(pairing(NCEP, (0, 2, 3), (0, 2, 2)), VectorPairing::Swapped);
    for originator in [NCEP, ECMWF, DWD] {
        let pairs = sweep(originator);
        let local = pairs
            .iter()
            .filter(|(u, v, ..)| {
                [u.0, u.1, u.2, v.0, v.1, v.2]
                    .iter()
                    .any(|c| (192..=254).contains(c))
            })
            .count();
        assert!(
            local > 0,
            "centre {} defines local u/v pairs",
            originator.centre
        );
    }
}

/// Every pair of parameters in one discipline whose names pair, in order, as
/// `(u, v, u name, v name)`.
#[allow(clippy::type_complexity)]
fn sweep(originator: Originator) -> Vec<((u8, u8, u8), (u8, u8, u8), &'static str, &'static str)> {
    let mut named = Vec::new();
    for d in 0..=255u8 {
        for c in 0..=255u8 {
            for n in 0..=255u8 {
                if let Some((_, name, _)) = lookup_parameter(originator, d, c, n)
                    && has_direction_word(name)
                {
                    named.push(((d, c, n), name));
                }
            }
        }
    }
    let mut pairs = Vec::new();
    for &(u, u_name) in &named {
        for &(v, v_name) in &named {
            if u.0 == v.0
                && pairing_of_parameter_names(Some(u_name), Some(v_name))
                    == VectorPairing::Recognised
            {
                pairs.push((u, v, u_name, v_name));
            }
        }
    }
    pairs
}

/// A cheap filter ahead of the sweep's quadratic step: whether a name holds a
/// word the rule could read as a direction.
fn has_direction_word(name: &str) -> bool {
    name.split(|c: char| !c.is_alphanumeric()).any(|w| {
        matches!(
            w.to_lowercase().as_str(),
            "u" | "v" | "eastward" | "northward" | "zonal" | "meridional" | "x" | "y"
        )
    })
}

#[test]
fn every_pair_the_tables_define() {
    for (label, originator) in [("WMO", WMO), ("NCEP", NCEP), ("ECMWF", ECMWF), ("DWD", DWD)] {
        let pairs = sweep(originator);
        println!("{label}: {} pairs", pairs.len());
        for ((ud, uc, un), (vd, vc, vn), u, v) in &pairs {
            println!("  {ud}-{uc}-{un} / {vd}-{vc}-{vn}: {u} / {v}");
        }
        // Each pair found is found once each way round: recognised in its
        // order, swapped out of it.
        for (u, v, u_name, v_name) in &pairs {
            assert_eq!(
                pairing_of_parameter_names(Some(v_name), Some(u_name)),
                VectorPairing::Swapped,
                "{u:?} / {v:?}"
            );
        }
        assert!(pairs.len() >= 13, "{label}: the master pairs at least");
    }
}
