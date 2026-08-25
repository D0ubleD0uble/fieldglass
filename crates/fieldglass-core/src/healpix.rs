//! HEALPix pixel geometry: pixel number to position, and back.
//!
//! HEALPix (Górski et al. 2005) tiles the sphere into `12·Nside²` pixels of
//! *equal area*, arranged on rings of constant latitude. GRIB2 §3.150 carries a
//! field on one, and unlike every other grid the project reads it is a single
//! list of pixels rather than a raster: there is no `(ni, nj)`, only an index.
//!
//! Two orderings exist for that index, and a message states which:
//!
//! - **RING** walks pixels along each ring in turn, north to south. The
//!   closed-form maps below are written for this ordering.
//! - **NESTED** groups each of the twelve base faces into a quadtree, so a
//!   pixel's children are contiguous. It is the ordering that makes
//!   multi-resolution work cheap, and [`nest2ring`] converts it to RING rather
//!   than duplicating the position maps.
//!
//! Positions are checked against eccodes: RING against the pinned 2.34.1
//! `grib_get_data`, NESTED against a newer wheel, because 2.34.1's HEALPix
//! geoiterator supports RING only. See
//! `crates/fieldglass-grib2/tests/fixtures/NOTICE.md`.

use std::f64::consts::PI;

/// Face-to-ring offsets from the HEALPix reference implementation: for each of
/// the twelve base faces, the ring row and the phi column its corner sits on.
/// The four north faces, four equatorial, four south, in face order.
const JRLL: [u64; 12] = [2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4];
const JPLL: [u64; 12] = [1, 3, 5, 7, 0, 2, 4, 6, 1, 3, 5, 7];

/// Largest `Nside` these maps will work on.
///
/// `Nside` reaches them from four untrusted octets, and the pixel arithmetic
/// runs to about `12·Nside²`, which passes `u64::MAX` above 2³¹. Rather than
/// widen every intermediate, the range where `u64` is sound is stated once and
/// everything above it is refused. `2²⁴` leaves four orders of magnitude of
/// headroom in `u64` and is still some six orders above any published grid —
/// ECMWF's HEALPix open data is Nside 1024 — so the only messages this turns
/// away are malformed.
pub const MAX_NSIDE: u32 = 1 << 24;

/// Total pixels on a HEALPix sphere of this `nside`. Zero for `nside = 0`,
/// which is not a grid.
///
/// Computed in `u128` and clamped, because `nside` reaches this straight from
/// a message's four untrusted octets and `12·Nside²` passes `u64::MAX` above
/// `Nside = 2³¹`. Clamping keeps the bounds checks below sound — every pixel
/// number is then "in range", so nothing indexes past an array — while the
/// real refusal happens where the bytes are parsed.
pub fn npix(nside: u32) -> u64 {
    // `u128` so this stays total for an `nside` past `MAX_NSIDE`: callers use
    // it as a bound, and a wrapped count would make an out-of-range pixel look
    // in range.
    let n = nside as u128;
    u64::try_from(12 * n * n).unwrap_or(u64::MAX)
}

/// Pixels in the north polar cap — those on rings above the equatorial belt.
fn ncap(nside: u32) -> u64 {
    let n = nside as u128;
    u64::try_from(2 * n * n.saturating_sub(1)).unwrap_or(u64::MAX)
}

/// Centre of RING-ordered pixel `ipix` as `(lat, lon)` in degrees, longitude in
/// `[0, 360)`.
///
/// `None` when `nside` is zero or the pixel is off the sphere.
///
/// The sphere is in three parts and each has its own closed form: two polar
/// caps, where ring `i` holds `4i` pixels and the rings crowd towards the pole,
/// and the equatorial belt, where every ring holds `4·Nside` and alternate
/// rings are offset by half a pixel — the `fodd` term, and the thing most
/// easily got wrong, since a grid with it dropped still looks plausible.
pub fn pix2ang_ring(nside: u32, ipix: u64) -> Option<(f64, f64)> {
    let npix = npix(nside);
    if nside == 0 || nside > MAX_NSIDE || ipix >= npix {
        return None;
    }
    let n = nside as u64;
    let nf = nside as f64;
    let ncap = ncap(nside);

    let (z, phi) = if ipix < ncap {
        // North polar cap. Inverting `ipix = 2·i·(i−1)` for the ring index.
        let iring = (((1.0 + (1.0 + 2.0 * ipix as f64).sqrt()) * 0.5).floor()) as u64;
        let iphi = ipix - 2 * iring * (iring - 1);
        (
            1.0 - (iring * iring) as f64 / (3.0 * nf * nf),
            (iphi as f64 + 0.5) * PI / (2.0 * iring as f64),
        )
    } else if ipix < npix - ncap {
        // Equatorial belt: every ring holds 4·Nside pixels.
        let ip = ipix - ncap;
        let iring = ip / (4 * n) + n;
        // One-based within the ring, matching the reference implementation:
        // the stagger below is *subtracted*, so an off-by-one here shifts every
        // other ring by a whole pixel and still looks like a grid.
        let iphi = ip % (4 * n) + 1;
        // Alternate rings are offset by half a pixel, which is what makes the
        // pixels interlock rather than stack.
        let fodd = if (iring + n) % 2 == 1 { 1.0 } else { 0.5 };
        (
            (2 * n) as f64 - iring as f64,
            (iphi as f64 - fodd) * PI / (2.0 * nf),
        )
    } else {
        // South polar cap: the north form, counted from the other pole.
        let ip = npix - ipix;
        let iring = (((1.0 + (2.0 * ip as f64 - 1.0).sqrt()) * 0.5).floor()) as u64;
        let iphi = 4 * iring + 1 - (ip - 2 * iring * (iring - 1));
        (
            -1.0 + (iring * iring) as f64 / (3.0 * nf * nf),
            (iphi as f64 - 0.5) * PI / (2.0 * iring as f64),
        )
    };

    // The equatorial branch above returns `2n − iring`, still to be scaled.
    let z = if ipix >= ncap && ipix < npix - ncap {
        z * 2.0 / (3.0 * nf)
    } else {
        z
    };

    Some((
        z.clamp(-1.0, 1.0).asin().to_degrees(),
        phi.rem_euclid(2.0 * PI).to_degrees(),
    ))
}

/// RING pixel containing `(lat, lon)` in degrees — the inverse of
/// [`pix2ang_ring`]. `None` when `nside` is zero or the position is not finite.
pub fn ang2pix_ring(nside: u32, lat: f64, lon: f64) -> Option<u64> {
    if nside == 0 || nside > MAX_NSIDE || !lat.is_finite() || !lon.is_finite() {
        return None;
    }
    let n = nside as u64;
    let nf = nside as f64;
    let z = lat.to_radians().sin().clamp(-1.0, 1.0);
    let za = z.abs();
    // Longitude in units of 90 degrees, wrapped into [0, 4).
    let tt = (lon.rem_euclid(360.0) / 90.0).rem_euclid(4.0);

    if za <= 2.0 / 3.0 {
        // Equatorial belt: the pixel is where two diagonal edge families cross.
        let temp1 = nf * (0.5 + tt);
        let temp2 = nf * z * 0.75;
        let jp = (temp1 - temp2) as i64; // ascending edge index
        let jm = (temp1 + temp2) as i64; // descending edge index
        // `jp - jm` is signed — negative for the northern half of the belt — so
        // this stays in `i64`. Clamping it at zero instead shifts the ring by
        // one and returns a neighbouring pixel, which still looks like an
        // answer.
        let ir = n as i64 + 1 + jp - jm; // ring, counted from the top of the belt
        let kshift = 1 - (ir & 1);
        let ip = ((jp + jm + kshift + 1 - n as i64) / 2).rem_euclid(4 * n as i64);
        Some(ncap(nside) + (ir as u64 - 1) * 4 * n + ip as u64)
    } else {
        // Polar cap: rings crowd towards the pole, so the ring index comes from
        // the distance to it rather than from a linear scale.
        let tp = tt - tt.floor();
        let tmp = nf * (3.0 * (1.0 - za)).sqrt();
        let jp = (tp * tmp) as u64;
        let jm = ((1.0 - tp) * tmp) as u64;
        let ir = jp + jm + 1;
        let ip = ((tt * ir as f64) as u64) % (4 * ir);
        Some(if z > 0.0 {
            2 * ir * (ir - 1) + ip
        } else {
            npix(nside) - 2 * ir * (ir + 1) + ip
        })
    }
}

/// Take every other bit of `v` (bits 0, 2, 4, …) and pack them down.
///
/// A NESTED pixel number interleaves its within-face `x` and `y` bits, so
/// pulling them apart is what turns it back into a position on its face. An
/// explicit loop rather than the usual chain of masks and shifts: it runs once
/// per pixel at decode, never in an inner loop, and this way it is checkable by
/// eye.
fn compress_bits(v: u64) -> u64 {
    let mut out = 0u64;
    for b in 0..32u32 {
        out |= ((v >> (2 * b)) & 1) << b;
    }
    out
}

/// Convert a NESTED pixel number to its RING equivalent.
///
/// `None` when `nside` is not a power of two — NESTED is a quadtree over each
/// base face, so it is only defined there, while RING works for any `nside` —
/// or when the pixel is off the sphere.
pub fn nest2ring(nside: u32, ipnest: u64) -> Option<u64> {
    if nside == 0 || nside > MAX_NSIDE || !nside.is_power_of_two() || ipnest >= npix(nside) {
        return None;
    }
    let n = nside as u64;
    let npface = n * n;
    let face = (ipnest / npface) as usize;
    let pix = ipnest % npface;
    // The interleaved bits are the pixel's (x, y) within its face.
    let ix = compress_bits(pix);
    let iy = compress_bits(pix >> 1);

    let nl4 = 4 * n;
    let jr = JRLL[face] * n - ix - iy - 1;
    let (nr, n_before, kshift) = if jr < n {
        (jr, 2 * jr * (jr - 1), 0)
    } else if jr > 3 * n {
        let nr = nl4 - jr;
        (nr, npix(nside) - 2 * (nr + 1) * nr, 0)
    } else {
        (n, ncap(nside) + (jr - n) * nl4, (jr - n) & 1)
    };

    // `ix - iy` is genuinely signed — it is negative for every pixel below its
    // face's diagonal — so this step leaves unsigned arithmetic rather than
    // wrapping through it.
    let mut jp = ((JPLL[face] * nr) as i64 + ix as i64 - iy as i64 + 1 + kshift as i64) / 2;
    let nl4 = nl4 as i64;
    if jp > nl4 {
        jp -= nl4;
    } else if jp < 1 {
        jp += nl4;
    }
    Some(n_before + jp as u64 - 1)
}

/// Centre of a pixel in whichever ordering the message declared.
///
/// NESTED goes through [`nest2ring`] rather than getting its own position map,
/// so there is one implementation of where a pixel is and only the indexing
/// differs.
pub fn pix2ang(nside: u32, ipix: u64, nested: bool) -> Option<(f64, f64)> {
    let ring = if nested {
        nest2ring(nside, ipix)?
    } else {
        ipix
    };
    pix2ang_ring(nside, ring)
}
