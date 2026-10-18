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

use std::collections::BTreeSet;

use fieldglass_core::vector_pair::{
    VectorPairing, is_placeholder_name, mentions_a_direction, pairing_of_parameter_names,
};
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
fn per_axis_statistics_are_not_pairs() {
    // NCEP's latitude and longitude of the u and v points: positions, not
    // a vector.
    for (u, v) in [((0, 2, 198), (0, 2, 200)), ((0, 2, 199), (0, 2, 201))] {
        assert_eq!(
            pairing(NCEP, u, v),
            VectorPairing::NotAPair,
            "{u:?} / {v:?}"
        );
        assert_eq!(
            pairing(NCEP, v, u),
            VectorPairing::NotAPair,
            "{v:?} / {u:?}"
        );
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
                    && mentions_a_direction(name)
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

/// Every name the WMO, NCEP, ECMWF and DWD tables give.
fn all_names() -> BTreeSet<&'static str> {
    let mut names = BTreeSet::new();
    for originator in [WMO, NCEP, ECMWF, DWD] {
        for d in 0..=255u8 {
            for c in 0..=255u8 {
                for n in 0..=255u8 {
                    if let Some((_, name, _)) = lookup_parameter(originator, d, c, n) {
                        names.insert(name);
                    }
                }
            }
        }
    }
    names
}

/// Every name the four originators' tables hold that mentions a direction.
fn directional_names() -> BTreeSet<&'static str> {
    all_names()
        .into_iter()
        .filter(|n| mentions_a_direction(n))
        .collect()
}

/// The names that mention a direction and pair with nothing, reviewed one by
/// one (#1049). Each is either no component at all or a component whose
/// partner no table carries.
const UNPARTNERED: &[&str] = &[
    // Both axes in one name: a cross-covariance or product of u with v, not one component.
    "Covariance between Meridional and Zonal Components of the wind",
    "Covariance between zonal and meridional components of the wind. Defined as [uv]-[u][v], where [] indicates the mean over the indicated time span",
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
    "Latitude of U Wind Component of Velocity",
    "Latitude of V Wind Component of Velocity",
    "Longitude of U Wind Component of Velocity",
    "Longitude of V Wind Component of Velocity",
    "Standard deviation of U velocity",
    "Standard deviation of V component",
    "Variance of u component",
    "Variance of v component",
    "U*T product",
    "U*U product",
    "V*T product",
    "V*V product",
    "Zonal Lagrangian time scale (direct) (Hanna)",
    "Meridional Lagrangian time scale (direct) (Hanna)",
    "Zonal velocity fluctuation (Hanna)",
    "Meridional velocity fluctuation (Hanna)",
    // ECMWF's eddy products and covariances, whose u and v spellings differ inside a word (`UT` and `VT`, `[uT]` and `[vT]`). Not drawn as arrows; left unpaired.
    "UT - U~T~",
    "UU - U~U~",
    "VT - V~T~",
    "VV - V~V~",
    "Covariance between meridional and meridional components of the wind. Defined as [vv]-[v][v], where [] indicates the mean over the indicated time span",
    "Covariance between zonal and zonal components of the wind. Defined as [uu]-[u][u], where [] indicates the mean over the indicated time span",
    "Covariance between meridional component of the wind and temperature. Defined as [vT]-[v][T], where [] indicates the mean over the indicated time span",
    "Covariance between zonal component of the wind and temperature. Defined as [uT]-[u][T], where [] indicates the mean over the indicated time span",
    "Covariance between specific humidity and meridional components of the wind. Defined as [vq]-[v][q], where [] indicates the mean over the indicated time span",
    "Covariance between specific humidity and zonal components of the wind. Defined as [uq]-[u][q], where [] indicates the mean over the indicated time span",
    // A component whose partner no table carries.
    "Amplitude function for Rossby wave envelope for meridional wind",
    "Meridional overturning stream function",
    "U velocity maximum",
    // A letter or phrase that is not a direction here.
    "Solar X-ray flux (XRS long)",
    "Solar X-ray flux (XRS short)",
    "X-ray radiance",
    "Volumetric heat capacity of urban material (capacity x density)",
    "Gauss Boaga Coordinates West to East, West Sector",
    "Surface exchange coefficients for U and V divided by delta z",
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

/// Words a table uses for a code it reserves or leaves open (#1142).
const PLACEHOLDER_WORDS: &[&str] = &[
    "experimental",
    "local",
    "missing",
    "reserved",
    "spare",
    "undefined",
    "unknown",
    "unused",
];

/// The names [`is_placeholder_name`] reads as placeholders here, reviewed one
/// by one (#1142), so a real name it would swallow fails the sweep.
const PLACEHOLDERS: &[&str] = &[
    // None: these tables hold no placeholder names.
];

/// Names that hold a [`PLACEHOLDER_WORDS`] word and still name a quantity,
/// reviewed one by one (#1142).
const NAMED_DESPITE_THE_WORD: &[&str] = &[
    // DWD: "local" names the table the codes come from.
    "Soil type (1...9, local soilType.table)",
];

/// Every name that holds a word tables use for a reserved or open code is
/// either read as a placeholder (no name, so a pair with it is `Unknown`) or
/// reviewed as a real name: a table regeneration that adds a new placeholder
/// spelling fails here instead of being reported as not a pair (#1142).
#[test]
fn every_placeholder_spelling_is_read_as_no_name() {
    let mut named = BTreeSet::new();
    let mut placeholders = BTreeSet::new();
    for name in all_names() {
        let words: Vec<String> = name
            .split(|c: char| !c.is_alphanumeric())
            .map(str::to_lowercase)
            .collect();
        if !words
            .iter()
            .any(|w| PLACEHOLDER_WORDS.contains(&w.as_str()))
        {
            continue;
        }
        if is_placeholder_name(name) {
            placeholders.insert(name);
        } else {
            named.insert(name);
        }
    }
    for name in &placeholders {
        println!("PLACEHOLDER {name:?}");
    }
    let expected: BTreeSet<&str> = PLACEHOLDERS.iter().copied().collect();
    assert_eq!(
        expected.len(),
        PLACEHOLDERS.len(),
        "a placeholder is listed twice"
    );
    assert_eq!(
        placeholders, expected,
        "the names read as placeholders are not the reviewed ones"
    );
    let reviewed: BTreeSet<&str> = NAMED_DESPITE_THE_WORD.iter().copied().collect();
    assert_eq!(
        reviewed.len(),
        NAMED_DESPITE_THE_WORD.len(),
        "a name is listed twice"
    );
    let new: Vec<_> = named.difference(&reviewed).collect();
    let stale: Vec<_> = reviewed.difference(&named).collect();
    assert!(
        new.is_empty() && stale.is_empty(),
        "names with a placeholder word that are neither placeholders nor reviewed: {new:?}; \
         reviewed names that are now placeholders or gone: {stale:?}"
    );
}
