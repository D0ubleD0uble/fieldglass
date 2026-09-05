//! Characterisation golden for the four planar inverses.
//!
//! #486 pulled `inverse` out of `LambertProjector`, `PolarStereoProjector`,
//! `LambertAzimuthalProjector` and `TransverseMercatorProjector` and made it a
//! provided `PlanarGridProjector` method, with what differed between them
//! moved into three hooks. Two of those differences were real and had drifted:
//! polar stereographic rejects the wrong hemisphere, and Lambert azimuthal
//! snaps grid edges in metres where the others snap in cell fractions.
//!
//! These hashes were captured from the four inherent bodies *before* the
//! extraction, so they pin it against a recording of the old code rather than
//! against the new code's own output. Each folds `inverse` over a 121x121
//! global lat/lon lattice, the grid's own four corners, a ring around each
//! corner at the scale of the edge snap, and the non-finite inputs — hashing
//! the raw `f64` bits of `i` and `j`, so "bit-identical" is literal.
//!
//! A failure is a behaviour change, not a test to re-baseline.
//!
//! Two rows *were* re-baselined once, deliberately: `polar_north` and
//! `polar_south` moved in #488, which removed the hemisphere reject that made a
//! polar grid refuse its own points across the equator. That change was
//! confined to exactly those two rows — the other twenty, every Lambert,
//! Lambert azimuthal, transverse Mercator and degenerate case among them, are
//! still the pre-#486 recording. `grid_round_trip.rs` is the test that pins
//! the new behaviour; this file only records that nothing else moved with it.
//!
//! # Two goldens, because "bit-identical" is not portable
//!
//! A conformal inverse runs on `ln`, `powf`, `tan` and `atan2`, and those are
//! the platform's libm, not ours. `wasm32` links Rust's own port instead of the
//! host's, so a browser build lands an ULP or two away from a native one. A
//! bit-exact hash turns that into a hard failure that says "behaviour change"
//! when nothing in this repository changed (#617).
//!
//! So the recording is kept twice, and the two say different things:
//!
//! * `Case::golden` is the pre-#486 bit-exact recording, unchanged. It is a
//!   property of *one libm*, so it runs only where the target's libm is the one
//!   that recorded it — identified positively by `libm_fingerprint`, not
//!   guessed from the target triple.
//! * `Case::portable` hashes the same probes in the same order at `QUANTUM`
//!   grid-index resolution, which every libm agrees on. It runs everywhere, and
//!   it is what says the browser and the native host place a point on the same
//!   grid cell.
//!
//! The portable constants were recorded from a tree whose bit-exact constants
//! still matched the pre-#486 recording, so the provenance carries across.
//! `docs/decisions/0009-cross-target-floating-point-agreement.md` has the
//! measurements and the reasoning.
use fieldglass_core::projection::*;

fn fnv(h: &mut u64, bytes: &[u8]) {
    for &b in bytes {
        *h ^= b as u64;
        *h = h.wrapping_mul(0x100000001b3);
    }
}
fn mix(h: &mut u64, r: Option<GridIndex>) {
    match r {
        None => fnv(h, &[0xff]),
        Some(g) => {
            fnv(h, &[0x01]);
            fnv(h, &g.i.to_bits().to_le_bytes());
            fnv(h, &g.j.to_bits().to_le_bytes());
        }
    }
}

/// The grid-index resolution the portable golden records, in cells.
///
/// A hundred-thousandth of a grid cell: eight orders of magnitude finer than
/// anything a raster, a probe or a contour can resolve, and — measured across
/// the whole probe set — 4,500 times coarser than the widest native/`wasm32`
/// disagreement, so no value sits near enough to a rounding boundary to fall on
/// different sides of it. `the_quantised_golden_has_room_for_a_different_libm`
/// asserts that margin rather than trusting it.
const QUANTUM: f64 = 1e-5;

/// How close to a `QUANTUM` boundary any recorded index may sit, in cells.
///
/// 1e-11 is a hundred times the widest native/`wasm32` disagreement measured
/// over this probe set (9.3e-14 cells) and forty times inside the closest
/// approach any value actually makes (4.2e-10 cells). A libm that pushed a
/// value past this floor would be about to flip a bucket, and the assertion
/// fires while the golden is still right rather than after it goes wrong.
const MARGIN_FLOOR: f64 = 1e-11;

/// Quantised mix: the same tag byte, then `i` and `j` in units of `QUANTUM`.
///
/// `f64::round` is exact for every finite input — there is no ULP freedom in
/// it — so two libms that agree on a value to well inside a quantum produce
/// the same integer here. A non-finite index has no quantised form and gets
/// its own tag; `inverse` is not expected to produce one, and this records
/// that it does not rather than silently rounding it to zero.
fn mix_quantised(h: &mut u64, r: Option<GridIndex>) {
    match r {
        None => fnv(h, &[0xff]),
        Some(g) => {
            fnv(h, &[0x01]);
            for v in [g.i, g.j] {
                if v.is_finite() {
                    fnv(h, &[0x02]);
                    fnv(h, &((v / QUANTUM).round() as i64).to_le_bytes());
                } else {
                    fnv(h, &[0x03]);
                    fnv(h, &v.to_bits().to_le_bytes());
                }
            }
        }
    }
}

/// A dense lattice plus the pathological inputs, in a fixed order.
fn probes(corners: &[(f64, f64); 4]) -> Vec<(f64, f64)> {
    let mut v = Vec::new();
    // 121 x 121 over the whole globe: inside, outside, and across every edge.
    for a in 0..=120 {
        for b in 0..=120 {
            v.push((-90.0 + a as f64 * 1.5, -180.0 + b as f64 * 3.0));
        }
    }
    // The corners themselves — the edge-snap cases this refactor must not move.
    v.extend_from_slice(corners);
    // And a ring immediately around each corner, at the scale of the snap.
    for &(lat, lon) in corners {
        for d in [-1e-9, -1e-12, 0.0, 1e-12, 1e-9, -1e-6, 1e-6] {
            v.push((lat + d, lon));
            v.push((lat, lon + d));
        }
    }
    for bad in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
        v.push((bad, 0.0));
        v.push((0.0, bad));
        v.push((bad, bad));
    }
    v
}

fn lambert() -> LambertParams {
    LambertParams {
        earth_radius_m: DEFAULT_EARTH_RADIUS_M,
        ni: 93,
        nj: 65,
        lat_first: 12.19,
        lon_first: -133.459,
        lad: 25.0,
        lov: -95.0,
        dx_metres: 81_271.0,
        dy_metres: 81_271.0,
        latin1: 25.0,
        latin2: 25.0,
    }
}
fn cmc_polar() -> PolarStereoParams {
    PolarStereoParams {
        earth_radius_m: DEFAULT_EARTH_RADIUS_M,
        ni: 135,
        nj: 95,
        lat_first: 11.43,
        lon_first: -110.27,
        lov: 247.0,
        lad: 60.0,
        dx_metres: 60_000.0,
        dy_metres: 60_000.0,
        south_pole: false,
    }
}
fn efas() -> LambertAzimuthalParams {
    LambertAzimuthalParams {
        semi_major_m: 6_378_137.0,
        semi_minor_m: 6_356_752.314,
        ni: 20,
        nj: 16,
        lat_first: 35.0,
        lon_first: -10.0,
        standard_parallel: 52.0,
        central_longitude: 10.0,
        dx_metres: 200_000.0,
        dy_metres: 200_000.0,
    }
}
fn ukv() -> TransverseMercatorParams {
    TransverseMercatorParams {
        semi_major_m: 6_377_563.0,
        semi_minor_m: 6_356_257.0,
        ni: 24,
        nj: 30,
        lat_ref: 49.0,
        lon_ref: -2.0,
        scale_factor: 0.999_601_244_926_452_6,
        false_easting_m: 400_000.0,
        false_northing_m: -100_000.0,
        x1_metres: -238_000.0,
        y1_metres: 1_222_000.0,
        dx_metres: 48_000.0,
        dy_metres: -48_000.0,
    }
}

/// The hash a projector that rejects every input produces. Named because half
/// the table is degenerate parameter sets that should reach it — and because a
/// live projector silently collapsing to "no point is on this grid" would
/// otherwise be an ordinary-looking hash change.
const ALL_REJECTED: u64 = 0xf69bbbe95fd1b93f;

/// A fold over the exact libm surface the planar inverses stand on, at fixed
/// inputs, so a target can say *which* libm it has rather than being guessed at
/// from its triple.
///
/// `target_arch = "wasm32"` would be the obvious gate and it is the wrong one
/// twice over: it assumes every other target agrees with the recording (musl
/// and macOS need not, and neither need a future glibc), and it says nothing
/// about which library is actually underneath. This asks.
///
/// Eleven of these fourteen already disagree between glibc 2.39 and the `libm`
/// Rust links on `wasm32`, so the fold separates them with room to spare; `ln`,
/// `exp` and `sqrt` agree today and are folded in anyway, for the next libm.
fn libm_fingerprint() -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    let fs: [fn(f64) -> f64; 14] = [
        |t| (t * 7.3 + 1e-3).ln(),
        |t| (t * 3.1 - 1.5).exp(),
        |t| (t * 2.0 + 0.5).powf(1.7 + t),
        |t| (t * 1.4).tan(),
        |t| (t * 9.0 - 4.5).atan(),
        |t| (t - 0.5).atan2(t * 2.0 - 1.3),
        |t| (t * 6.0).sin(),
        |t| (t * 6.0).cos(),
        |t| (t * 0.001 - 0.5).asin(),
        |t| (t * 2.0 - 1.0).sinh(),
        |t| (t * 2.0 - 1.0).cosh(),
        |t| (t - 0.4).hypot(t * 3.0 + 0.1),
        |t| (t * 5.0).sqrt(),
        |t| (t * 2.0 - 1.0).tanh(),
    ];
    for f in fs {
        for k in 0..2000i64 {
            fnv(&mut h, &f(k as f64 * 0.001 + 1e-9).to_bits().to_le_bytes());
        }
    }
    h
}

/// The fingerprint of the libm the bit-exact column was recorded against:
/// x86_64 Linux glibc 2.39, which is what CI's `ubuntu-latest` runners and the
/// pre-commit hooks run.
const REFERENCE_LIBM: u64 = 0x0951d9bc6bf359e4;

/// Whether a case's parameters describe a usable grid or a degenerate one.
#[derive(PartialEq)]
enum Kind {
    Live,
    Rejects,
}
use Kind::{Live, Rejects};

struct Case {
    name: &'static str,
    kind: Kind,
    projector: Box<dyn PlanarGridProjector>,
    /// Bit-exact, and therefore only true of `REFERENCE_LIBM`.
    golden: u64,
    /// The same probes at `QUANTUM` resolution. True of every target.
    portable: u64,
}

fn case(
    name: &'static str,
    kind: Kind,
    projector: impl PlanarGridProjector + 'static,
    golden: u64,
    portable: u64,
) -> Case {
    Case {
        name,
        kind,
        projector: Box::new(projector),
        golden,
        portable,
    }
}

/// One table, read by every test below. A projector added here is covered by
/// all of them — which is the point. The extraction made `inverse` free to
/// inherit, so the way a new planar projector now goes wrong is by *forgetting*
/// to override a hook, and nothing about writing one would remind you. The
/// Lambert azimuthal case is precedent: the default snap was three orders of
/// magnitude too tight for it, and the symptom was a missing outer row.
fn cases() -> Vec<Case> {
    vec![
        case(
            "lambert",
            Live,
            LambertProjector::new(lambert()),
            0x87ce7e8b13c2aa2a,
            0xcd6acdc0b8d20277,
        ),
        case(
            "lambert_degenerate_parallels",
            Rejects,
            LambertProjector::new(LambertParams {
                latin1: 25.0,
                latin2: -25.0,
                ..lambert()
            }),
            0xf69bbbe95fd1b93f,
            0xf69bbbe95fd1b93f,
        ),
        case(
            "lambert_ni1",
            Rejects,
            LambertProjector::new(LambertParams { ni: 1, ..lambert() }),
            0xf69bbbe95fd1b93f,
            0xf69bbbe95fd1b93f,
        ),
        case(
            "lambert_dx0",
            Rejects,
            LambertProjector::new(LambertParams {
                dx_metres: 0.0,
                ..lambert()
            }),
            0xf69bbbe95fd1b93f,
            0xf69bbbe95fd1b93f,
        ),
        case(
            "lambert_negdy",
            Live,
            LambertProjector::new(LambertParams {
                dy_metres: -81_271.0,
                ..lambert()
            }),
            0xe33d1c8c0e101c0c,
            0xc04a76209e76ebb3,
        ),
        case(
            "polar_north",
            Live,
            PolarStereoProjector::new(cmc_polar()),
            0x099824ae8940b504,
            0x121b065132907847,
        ),
        case(
            "polar_south",
            Live,
            PolarStereoProjector::new(PolarStereoParams {
                south_pole: true,
                lat_first: -11.43,
                ..cmc_polar()
            }),
            0x70140fd839b72cfc,
            0x8adbcae407e8989e,
        ),
        case(
            "polar_nj1",
            Rejects,
            PolarStereoProjector::new(PolarStereoParams {
                nj: 1,
                ..cmc_polar()
            }),
            0xf69bbbe95fd1b93f,
            0xf69bbbe95fd1b93f,
        ),
        case(
            "polar_dy0",
            Rejects,
            PolarStereoProjector::new(PolarStereoParams {
                dy_metres: 0.0,
                ..cmc_polar()
            }),
            0xf69bbbe95fd1b93f,
            0xf69bbbe95fd1b93f,
        ),
        case(
            "laea_efas",
            Live,
            LambertAzimuthalProjector::new(efas()),
            0x38c9da30a7138a4a,
            0x5cb6ec304ba6d550,
        ),
        case(
            "laea_ni1",
            Rejects,
            LambertAzimuthalProjector::new(LambertAzimuthalParams { ni: 1, ..efas() }),
            0xf69bbbe95fd1b93f,
            0xf69bbbe95fd1b93f,
        ),
        case(
            "laea_dx0",
            Rejects,
            LambertAzimuthalProjector::new(LambertAzimuthalParams {
                dx_metres: 0.0,
                ..efas()
            }),
            0xf69bbbe95fd1b93f,
            0xf69bbbe95fd1b93f,
        ),
        case(
            "laea_negdy",
            Live,
            LambertAzimuthalProjector::new(LambertAzimuthalParams {
                dy_metres: -200_000.0,
                ..efas()
            }),
            0x3a68ac1f214a5f4e,
            0x2e7b2604318eefe6,
        ),
        case(
            "laea_south_tangent",
            Live,
            LambertAzimuthalProjector::new(LambertAzimuthalParams {
                standard_parallel: -90.0,
                lat_first: -60.0,
                ..efas()
            }),
            0x7e17e890478be1d1,
            0xbfe7acca35304384,
        ),
        case(
            "tmerc_ukv",
            Live,
            TransverseMercatorProjector::new(ukv()),
            0xd1782da097605475,
            0x2e8bc10638f1f3ce,
        ),
        case(
            "tmerc_sf0",
            Rejects,
            TransverseMercatorProjector::new(TransverseMercatorParams {
                scale_factor: 0.0,
                ..ukv()
            }),
            0xf69bbbe95fd1b93f,
            0xf69bbbe95fd1b93f,
        ),
        case(
            "tmerc_sfnan",
            Rejects,
            TransverseMercatorProjector::new(TransverseMercatorParams {
                scale_factor: f64::NAN,
                ..ukv()
            }),
            0xf69bbbe95fd1b93f,
            0xf69bbbe95fd1b93f,
        ),
        case(
            "tmerc_nj1",
            Rejects,
            TransverseMercatorProjector::new(TransverseMercatorParams { nj: 1, ..ukv() }),
            0xf69bbbe95fd1b93f,
            0xf69bbbe95fd1b93f,
        ),
        case(
            "tmerc_dx0",
            Rejects,
            TransverseMercatorProjector::new(TransverseMercatorParams {
                dx_metres: 0.0,
                ..ukv()
            }),
            0xf69bbbe95fd1b93f,
            0xf69bbbe95fd1b93f,
        ),
        case(
            "tmerc_posdy",
            Live,
            TransverseMercatorProjector::new(TransverseMercatorParams {
                dy_metres: 48_000.0,
                ..ukv()
            }),
            0x49406a239fba2285,
            0x3a4ec071a91444c5,
        ),
        case(
            "laea_asymmetric_spacing",
            Live,
            LambertAzimuthalProjector::new(LambertAzimuthalParams {
                dx_metres: 250_000.0,
                dy_metres: -80_000.0,
                lat_first: 60.0,
                ..efas()
            }),
            0x3fbfdd8f1d9f4c3d,
            0x0f1365a69ba758ca,
        ),
        case(
            "lambert_asymmetric_spacing",
            Live,
            LambertProjector::new(LambertParams {
                dx_metres: 120_000.0,
                dy_metres: -40_000.0,
                ..lambert()
            }),
            0x75a4bb1ffaa325a2,
            0xe41eb1582d013cc7,
        ),
    ]
}

fn fold_with(p: &dyn PlanarGridProjector, mix: fn(&mut u64, Option<GridIndex>)) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    let corners = p.grid_corners_lonlat();
    for (lat, lon) in probes(&corners) {
        mix(&mut h, p.inverse(lat, lon));
    }
    h
}

/// The bit-exact pre-#486 recording, on the libm that recorded it.
///
/// Elsewhere this returns without asserting, and
/// `the_planar_inverses_place_every_point_where_the_recording_does` is what
/// covers the target instead. That is only safe because the fingerprint cannot
/// quietly stop matching where it should: on the reference toolchain
/// `the_reference_toolchain_is_the_recorded_libm` asserts it outright, so a
/// probe edited into never matching turns CI red rather than turning this test
/// into a no-op everywhere.
#[test]
fn the_planar_inverses_match_their_pre_extraction_recording() {
    if libm_fingerprint() != REFERENCE_LIBM {
        println!(
            "skipped: this target's libm is {:#018x}, and the bit-exact column \
             was recorded against {REFERENCE_LIBM:#018x}. The values are still \
             checked to {QUANTUM:e} of a grid cell by \
             the_planar_inverses_place_every_point_where_the_recording_does. \
             See docs/decisions/0009-cross-target-floating-point-agreement.md.",
            libm_fingerprint()
        );
        return;
    }
    for c in cases() {
        assert_eq!(
            fold_with(c.projector.as_ref(), mix),
            c.golden,
            "{} inverts differently than the pre-#486 recording",
            c.name
        );
    }
}

/// The same recording at `QUANTUM` resolution, which holds on every libm.
///
/// This is the one that runs in the `wasm32-wasip1` CI step, and so the one
/// that says the browser build places a point on the same grid cell as the
/// native host — including the `Some`/`None` decision, which is folded in as
/// the tag byte and is bit-identical on both.
#[test]
fn the_planar_inverses_place_every_point_where_the_recording_does() {
    for c in cases() {
        assert_eq!(
            fold_with(c.projector.as_ref(), mix_quantised),
            c.portable,
            "{} places points differently than the recording, by more than \
             {QUANTUM:e} of a grid cell",
            c.name
        );
    }
}

/// Quantising is only safe while no recorded index sits near a bucket boundary,
/// and "near" has to be measured against a libm this repository has never seen.
/// A value that drifted within `MARGIN_FLOOR` of a boundary would be one ULP
/// from flipping the hash above into a failure that reads as a behaviour
/// change — the exact confusion #617 was about. Fail here first, where the
/// message can say what actually happened.
#[test]
fn the_quantised_golden_has_room_for_a_different_libm() {
    let mut worst = f64::INFINITY;
    let mut worst_at = String::new();
    for c in cases() {
        let p = c.projector.as_ref();
        for (lat, lon) in probes(&p.grid_corners_lonlat()) {
            let Some(g) = p.inverse(lat, lon) else {
                continue;
            };
            for v in [g.i, g.j] {
                let q = v / QUANTUM;
                // Distance from the half-integer where `round` changes bucket.
                let margin = (0.5 - (q - q.round()).abs()) * QUANTUM;
                if margin < worst {
                    worst = margin;
                    worst_at = format!("{} at {lat},{lon} ({v})", c.name);
                }
            }
        }
    }
    assert!(
        worst > MARGIN_FLOOR,
        "{worst_at} sits {worst:e} of a cell from a {QUANTUM:e} bucket \
         boundary, inside the {MARGIN_FLOOR:e} floor"
    );
    println!("closest approach to a bucket boundary: {worst:e} cells ({worst_at})");
}

/// The fingerprint is a gate, and a gate that stops matching stops gating. On
/// the toolchain the bit-exact column was recorded against, assert it rather
/// than branching on it.
#[cfg(all(target_arch = "x86_64", target_os = "linux", target_env = "gnu"))]
#[test]
fn the_reference_toolchain_is_the_recorded_libm() {
    assert_eq!(
        libm_fingerprint(),
        REFERENCE_LIBM,
        "x86_64 glibc no longer computes what the bit-exact column was recorded \
         against; see docs/decisions/0009-cross-target-floating-point-agreement.md"
    );
}

/// Every degenerate parameter set must actually reach the reject path, and no
/// live one may. Without this the table above would still pass if a real
/// projector started refusing everything, since a hash is a hash.
///
/// Both columns are checked: an all-rejecting projector writes nothing but tag
/// bytes, so `mix` and `mix_quantised` fold it to the same `ALL_REJECTED`, and a
/// portable constant that disagreed would mean the table had been mis-edited.
#[test]
fn the_degenerate_grids_reject_and_the_live_ones_do_not() {
    for c in cases() {
        let refuses_all = c.golden == ALL_REJECTED;
        assert_eq!(
            refuses_all,
            c.portable == ALL_REJECTED,
            "{}: the two columns disagree about whether it rejects everything",
            c.name
        );
        assert_eq!(
            refuses_all,
            c.kind == Rejects,
            "{} is marked {} but {} every point",
            c.name,
            if c.kind == Rejects { "Rejects" } else { "Live" },
            if refuses_all { "refuses" } else { "places" }
        );
    }
}

/// The point of the extraction: `inverse` is the trait's, and every projector's
/// own `inverse` is the same call. A forwarder that stopped forwarding would
/// pass the hashes above only if it happened to reproduce the body exactly,
/// which is the drift this checks for directly.
#[test]
fn the_inherent_inverse_is_the_trait_inverse() {
    macro_rules! same {
        ($p:expr) => {{
            let p = $p;
            for (lat, lon) in probes(&p.grid_corners_lonlat()) {
                assert_eq!(
                    p.inverse(lat, lon),
                    PlanarGridProjector::inverse(&p, lat, lon),
                    "{lat},{lon}"
                );
            }
        }};
    }
    same!(LambertProjector::new(lambert()));
    same!(PolarStereoProjector::new(cmc_polar()));
    same!(LambertAzimuthalProjector::new(efas()));
    same!(TransverseMercatorProjector::new(ukv()));
}

/// The edge snap exists so the outermost row and column are not dropped to
/// background, and `snap_eps` is the one hook whose *unit* differs between
/// projectors. Inheriting the wrong default is silent until a render shows a
/// missing outer row, so every live grid in the table must place its own four
/// corners.
#[test]
fn every_live_grid_places_its_own_corners() {
    for c in cases().iter().filter(|c| c.kind == Live) {
        let p = c.projector.as_ref();
        let (ni, nj) = p.grid_dims();
        for (lat, lon) in p.grid_corners_lonlat() {
            let g = p
                .inverse(lat, lon)
                .unwrap_or_else(|| panic!("{} dropped its own corner {lat},{lon}", c.name));
            assert!(
                g.i >= 0.0 && g.i <= ni as f64 - 1.0 && g.j >= 0.0 && g.j <= nj as f64 - 1.0,
                "{} corner {lat},{lon} landed at {},{}",
                c.name,
                g.i,
                g.j
            );
        }
    }
}

/// `SnapEps::Metres` converts through the spacing, so a negative `dy` — the
/// scan direction, not a smaller cell — must not flip the tolerance negative
/// and turn the snap into a rejection.
#[test]
fn a_negative_spacing_does_not_invert_the_snap() {
    let p = LambertAzimuthalProjector::new(LambertAzimuthalParams {
        dy_metres: -200_000.0,
        lat_first: 60.0,
        ..efas()
    });
    for (lat, lon) in p.grid_corners_lonlat() {
        assert!(
            p.inverse(lat, lon).is_some(),
            "negative dy dropped corner {lat},{lon}"
        );
    }
}
