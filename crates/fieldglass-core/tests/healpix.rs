//! HEALPix pixel positions, against eccodes.
//!
//! The formulas are not obvious and a wrong one still produces plausible
//! numbers — a dropped stagger term puts every other ring half a pixel out,
//! which looks like a grid. So the oracle is eccodes' own geoiterator, over
//! every pixel of two resolutions in both orderings, not a spot check.
//!
//! RING comes from the pinned eccodes 2.34.1; NESTED from a newer wheel,
//! because 2.34.1's HEALPix geoiterator supports RING only. The goldens record
//! which produced them; see the GRIB2 fixtures' `NOTICE.md`.

use fieldglass_core::healpix::{MAX_NSIDE, ang2pix_ring, nest2ring, npix, pix2ang, pix2ang_ring};

const N2_RING: &str =
    include_str!("../../fieldglass-grib2/tests/fixtures/healpix_n2_ring.grib2.coords.json");
const N2_NESTED: &str =
    include_str!("../../fieldglass-grib2/tests/fixtures/healpix_n2_nested.grib2.coords.json");
const N4_RING: &str =
    include_str!("../../fieldglass-grib2/tests/fixtures/healpix_n4_ring.grib2.coords.json");
const N4_NESTED: &str =
    include_str!("../../fieldglass-grib2/tests/fixtures/healpix_n4_nested.grib2.coords.json");

/// eccodes prints nine decimals, so it contributes about 5e-10 degrees; the
/// rest is the two implementations' own arithmetic. Measured worst is printed
/// by the test.
const TOL_DEG: f64 = 1e-6;

fn check(golden: &str, name: &str) -> f64 {
    let v: serde_json::Value = serde_json::from_str(golden).expect("golden parses");
    let nside = v["nside"].as_u64().expect("nside") as u32;
    let nested = v["ordering"].as_str().expect("ordering") == "nested";
    let coords = v["coords"].as_array().expect("coords");
    assert_eq!(
        coords.len() as u64,
        npix(nside),
        "{name}: golden must cover every pixel"
    );

    let mut worst = 0.0f64;
    for (ipix, c) in coords.iter().enumerate() {
        let want_lat = c[0].as_f64().expect("lat");
        let want_lon = c[1].as_f64().expect("lon");
        let (lat, lon) = pix2ang(nside, ipix as u64, nested)
            .unwrap_or_else(|| panic!("{name}: no position for pixel {ipix}"));
        // Longitudes are both in [0, 360), but 0 and 360 are the same meridian.
        let dlon = (lon - want_lon).abs().min(360.0 - (lon - want_lon).abs());
        worst = worst.max((lat - want_lat).abs()).max(dlon);
        assert!(
            (lat - want_lat).abs() < TOL_DEG && dlon < TOL_DEG,
            "{name}: pixel {ipix} at ({lat}, {lon}), eccodes says ({want_lat}, {want_lon})"
        );
    }
    println!("{name}: {} pixels, worst {worst:e} deg", coords.len());
    worst
}

#[test]
fn ring_positions_match_eccodes() {
    check(N2_RING, "Nside=2 ring");
    check(N4_RING, "Nside=4 ring");
}

#[test]
fn nested_positions_match_eccodes() {
    check(N2_NESTED, "Nside=2 nested");
    check(N4_NESTED, "Nside=4 nested");
}

/// Every pixel's centre must land back in that same pixel. This is what would
/// catch a forward map and an inverse that are each self-consistent but
/// disagree with one another.
#[test]
fn ang2pix_inverts_pix2ang_for_every_pixel() {
    for nside in [1u32, 2, 3, 4, 8, 16] {
        for ipix in 0..npix(nside) {
            let (lat, lon) = pix2ang_ring(nside, ipix).expect("in range");
            let back = ang2pix_ring(nside, lat, lon).expect("in range");
            assert_eq!(
                back, ipix,
                "Nside={nside}: pixel {ipix} at ({lat}, {lon}) came back as {back}"
            );
        }
    }
}

/// `nest2ring` must be a bijection: every RING pixel is some NESTED pixel's
/// image, exactly once. A reindexing bug that merely permutes wrongly would
/// still pass a spot check but not this.
#[test]
fn nest2ring_is_a_permutation() {
    for nside in [1u32, 2, 4, 8, 16] {
        let n = npix(nside);
        let mut seen = vec![false; n as usize];
        for ipnest in 0..n {
            let ring = nest2ring(nside, ipnest).expect("in range");
            assert!(
                ring < n,
                "Nside={nside}: {ipnest} mapped out of range to {ring}"
            );
            assert!(
                !seen[ring as usize],
                "Nside={nside}: ring pixel {ring} claimed twice"
            );
            seen[ring as usize] = true;
        }
        assert!(
            seen.iter().all(|&s| s),
            "Nside={nside}: some ring pixel unreachable"
        );
    }
}

#[test]
fn the_twelve_base_pixels_tile_the_sphere() {
    // Nside=1 is the twelve base faces themselves: four round the north, four
    // on the equator, four round the south. A sanity check that needs no
    // oracle — if the latitudes are not symmetric the scheme is wrong.
    let lats: Vec<f64> = (0..12)
        .map(|p| (pix2ang_ring(1, p).unwrap().0 * 1e9).round() / 1e9)
        .collect();
    assert_eq!(
        lats[0..4].iter().filter(|&&l| l > 0.0).count(),
        4,
        "north four"
    );
    assert_eq!(
        lats[4..8].iter().filter(|&&l| l == 0.0).count(),
        4,
        "equatorial four"
    );
    assert_eq!(
        lats[8..12].iter().filter(|&&l| l < 0.0).count(),
        4,
        "south four"
    );
    for k in 0..4 {
        assert!(
            (lats[k] + lats[11 - k]).abs() < 1e-9,
            "north and south faces must mirror: {} vs {}",
            lats[k],
            lats[11 - k]
        );
    }
}

#[test]
fn out_of_range_and_degenerate_input_is_refused() {
    assert_eq!(npix(0), 0);
    assert!(pix2ang_ring(0, 0).is_none(), "nside 0 is not a grid");
    assert!(pix2ang_ring(2, 48).is_none(), "one past the last pixel");
    assert!(ang2pix_ring(2, f64::NAN, 0.0).is_none());
    assert!(ang2pix_ring(2, 0.0, f64::INFINITY).is_none());
    // NESTED is a quadtree per face, so it needs a power-of-two nside; RING
    // does not, and 3 is a legal HEALPix resolution.
    assert!(nest2ring(3, 0).is_none(), "nested needs a power of two");
    assert!(pix2ang_ring(3, 0).is_some(), "ring does not");
    assert!(nest2ring(4, 192).is_none(), "one past the last pixel");
}

/// `Nside` arrives as four untrusted octets, and `12·Nside²` passes `u64::MAX`
/// above 2³¹. These functions must answer rather than panic, whatever they are
/// handed — the parse and decode fuzz targets reach here.
#[test]
fn a_hostile_nside_does_not_panic() {
    for nside in [u32::MAX, 1 << 31, (1 << 31) + 1, MAX_NSIDE + 1] {
        // The count stays computable, so a caller using it as a bound is not
        // handed a wrapped value that makes an out-of-range pixel look valid.
        assert!(
            npix(nside) > 0,
            "Nside {nside} must report some pixel count"
        );
        // Past the sound range every map declines rather than computing.
        assert!(pix2ang_ring(nside, 0).is_none(), "Nside {nside}");
        assert!(nest2ring(nside, 0).is_none(), "Nside {nside}");
        assert!(ang2pix_ring(nside, 45.0, 45.0).is_none(), "Nside {nside}");
    }
    // And the boundary itself still works, so the cap is not quietly lower
    // than it says.
    assert!(pix2ang_ring(MAX_NSIDE, 0).is_some());
    assert!(ang2pix_ring(MAX_NSIDE, 45.0, 45.0).is_some());
}
