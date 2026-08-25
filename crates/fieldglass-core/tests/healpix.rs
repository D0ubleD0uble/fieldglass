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

// ---------------------------------------------------------------------------
// Resampling onto a lat/lon grid (#443)
// ---------------------------------------------------------------------------

use fieldglass_core::healpix::resample_to_latlon;

/// A grid at the same resolution as the source, so no pixel is skipped.
fn latlon_grid(ni: usize, nj: usize) -> (Vec<f64>, Vec<f64>) {
    let lats = (0..nj)
        .map(|j| 90.0 - j as f64 * 180.0 / (nj as f64 - 1.0))
        .collect();
    let lons = (0..ni).map(|i| i as f64 * 360.0 / ni as f64).collect();
    (lats, lons)
}

/// Every resampled point must hold the value of the pixel that contains it —
/// which is the same question `ang2pix` answers, so this checks the two agree
/// rather than restating one in terms of the other.
#[test]
fn each_output_point_takes_the_pixel_that_contains_it() {
    let nside = 8;
    // A ramp, so a misindexed pixel shows as the wrong number rather than as
    // plausible noise.
    let values: Vec<Option<f64>> = (0..npix(nside)).map(|k| Some(k as f64)).collect();
    let (lats, lons) = latlon_grid(64, 33);
    let out = resample_to_latlon(nside, false, &values, &lats, &lons).expect("resamples");
    assert_eq!(out.len(), lats.len() * lons.len());

    for (j, &lat) in lats.iter().enumerate() {
        for (i, &lon) in lons.iter().enumerate() {
            let want = ang2pix_ring(nside, lat, lon).expect("in range") as f64;
            assert_eq!(
                out[j * lons.len() + i],
                Some(want),
                "point ({lat}, {lon}) should hold pixel {want}"
            );
        }
    }
}

/// A NESTED field must resample to the same picture as the RING field holding
/// the same values — the ordering is bookkeeping, not geometry. This is what
/// catches a reindex applied in the wrong direction, which is self-consistent
/// and completely wrong.
#[test]
fn nested_and_ring_fields_resample_to_the_same_picture() {
    let nside = 8;
    // Give each pixel its *position*, not its index, so the two orderings hold
    // genuinely the same field rather than the same numbers.
    let ring_values: Vec<Option<f64>> = (0..npix(nside))
        .map(|p| Some(pix2ang_ring(nside, p).unwrap().0))
        .collect();
    let nested_values: Vec<Option<f64>> = (0..npix(nside))
        .map(|p| Some(pix2ang(nside, p, true).unwrap().0))
        .collect();

    let (lats, lons) = latlon_grid(48, 25);
    let from_ring = resample_to_latlon(nside, false, &ring_values, &lats, &lons).unwrap();
    let from_nested = resample_to_latlon(nside, true, &nested_values, &lats, &lons).unwrap();
    assert_eq!(
        from_ring, from_nested,
        "ordering must not change the picture"
    );

    // And the picture is right: each point holds its own pixel's latitude.
    for (j, &lat) in lats.iter().enumerate() {
        for (i, &lon) in lons.iter().enumerate() {
            let p = ang2pix_ring(nside, lat, lon).unwrap();
            let want = pix2ang_ring(nside, p).unwrap().0;
            assert_eq!(from_ring[j * lons.len() + i], Some(want));
        }
    }
}

#[test]
fn a_masked_pixel_stays_masked_through_the_resample() {
    let nside = 4;
    let mut values: Vec<Option<f64>> = (0..npix(nside)).map(|k| Some(k as f64)).collect();
    values[100] = None;
    let (lats, lons) = latlon_grid(64, 33);
    let out = resample_to_latlon(nside, false, &values, &lats, &lons).expect("resamples");
    for (j, &lat) in lats.iter().enumerate() {
        for (i, &lon) in lons.iter().enumerate() {
            if ang2pix_ring(nside, lat, lon) == Some(100) {
                assert_eq!(
                    out[j * lons.len() + i],
                    None,
                    "a masked pixel must not become a value"
                );
            }
        }
    }
}

#[test]
fn a_field_of_the_wrong_length_is_refused() {
    let (lats, lons) = latlon_grid(8, 5);
    let short = vec![Some(1.0); 47];
    assert!(
        resample_to_latlon(2, false, &short, &lats, &lons).is_none(),
        "48 pixels are needed at Nside 2; pairing a field with the wrong \
         geometry must not resample into a plausible picture"
    );
    let right = vec![Some(1.0); 48];
    assert!(resample_to_latlon(2, false, &right, &lats, &lons).is_some());
    assert!(resample_to_latlon(0, false, &[], &lats, &lons).is_none());
}
