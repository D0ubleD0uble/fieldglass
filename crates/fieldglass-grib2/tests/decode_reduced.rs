//! Reduced-resolution decode of JPEG 2000 (§5.40) fields (#463).
//!
//! Three things are checked here, and they are different kinds of claim.
//!
//! **The coarse field is the pyramid's own low-pass.** Its shape comes from the
//! §3 grid (`ceil(n / 2^r)`) and its placement from
//! [`GridGeometry::subsampled`], so `the_coarse_grid_places_the_points_it_keeps`
//! compares the derived geometry against the message's own, point by point,
//! rather than against a formula restated here. There is no eccodes oracle for
//! any of this: eccodes has no reduced-resolution `grid_jpeg` decode, and the
//! fixtures' `.eccodes.ref.json` value blocks are full-resolution. The value
//! check that *is* possible — and the one the issue asks for — is that the
//! coarse mean tracks the full field's.
//!
//! **Reduction zero is today's decode.** `decode_message_raster_with` at
//! reduction zero must equal `decode_message_raster` exactly, on every packing,
//! or the new entry point is a second decoder rather than the same one.
//!
//! **Every refusal is reachable.** A coarse decode is refused for five reasons
//! (see [`Grib2Reader::decode_message_raster_with`]), and four of the five
//! cannot occur in any committed fixture — no producer ships JPEG 2000 on a
//! reduced grid, with a bitmap, or under an exotic scanning mode. Rather than
//! leave those guards untested, each is reached by substituting one parsed
//! section, or by rebuilding one section's bytes, and the substitution is named
//! in the test. `a_bitmap_is_refused` builds its message rather than mutating a
//! parse, and proves it built a valid one by decoding it at full resolution
//! first.

use fieldglass_grib2::{
    DecodeOptions, Grib2Reader, GridGeometry, GridTemplate, SCAN_ALTERNATE_ROWS, SCAN_J_CONSECUTIVE,
};
use std::path::Path;

/// Every committed fixture carrying JPEG 2000 packing.
const JPEG2000_FIXTURES: &[&str] = &[
    "jpeg2000_regular_latlon.grib2",
    "jpeg2000_local_40000.grib2",
    "rap_jpeg2000_lambert.grib2",
];

fn read(fixture: &str) -> Vec<u8> {
    std::fs::read(Path::new("tests/fixtures").join(fixture))
        .unwrap_or_else(|e| panic!("read fixture {fixture}: {e}"))
}

fn reader(fixture: &str) -> Grib2Reader {
    Grib2Reader::from_bytes(read(fixture)).unwrap_or_else(|e| panic!("{fixture} parses: {e:?}"))
}

fn mean(values: &[Option<f64>]) -> f64 {
    let present: Vec<f64> = values.iter().flatten().copied().collect();
    assert!(!present.is_empty(), "a field with no present values");
    present.iter().sum::<f64>() / present.len() as f64
}

/// Reduction zero is the decode this crate already does, carried beside the
/// message's own geometry — on every packing, not only the one with a pyramid.
#[test]
fn reduction_zero_is_the_message_field() {
    // Two JPEG 2000 fixtures and four other packings, so "the same decode" is
    // a claim about the entry point rather than about one codec.
    for fixture in [
        "jpeg2000_regular_latlon.grib2",
        "rap_jpeg2000_lambert.grib2",
        "regular_latlon_surface.grib2",
        "ccsds_regular_latlon.grib2",
        "png_local_40010.grib2",
        // The three layouts `decode_message_raster` regularises, which is what
        // the claim above is actually about: rows widened from `PL`, a
        // transposed `j`-consecutive field, and alternate rows put back.
        "reduced_gaussian_pressure_level.grib2",
        "j_consecutive_latlon.grib2",
        "alternate_row_lambert.grib2",
    ] {
        let r = reader(fixture);
        for i in 0..r.message_count() {
            let want = r
                .decode_message_raster(i)
                .unwrap_or_else(|e| panic!("{fixture}[{i}] decodes: {e:?}"));
            let got = r
                .decode_message_raster_with(i, DecodeOptions::default())
                .unwrap_or_else(|e| panic!("{fixture}[{i}] displays: {e:?}"));
            assert_eq!(
                got.display_values(),
                want.as_slice(),
                "{fixture}[{i}]: reduction 0 must be bit-identical to decode_message_raster"
            );
            assert_eq!(
                got.exact_values(),
                Some(want.as_slice()),
                "{fixture}[{i}]: at reduction 0 the values are the message's own"
            );
            assert_eq!(got.resolution_reduction(), 0);
            let (ni, nj) = r.messages[i].gds.dimensions().expect("a raster fixture");
            assert_eq!(
                (got.ni(), got.nj()),
                (ni, nj),
                "{fixture}[{i}]: raster shape"
            );
            assert_eq!(
                got.geometry(),
                &GridGeometry::from(&r.messages[i].gds),
                "{fixture}[{i}]: reduction 0 carries the message's own geometry"
            );
        }
    }
}

/// The coarse raster is the shape the §3 grid reduces to, and its values are
/// declared inexact: `exact_values` is the seam that keeps them away from a
/// probe or a statistic.
#[test]
fn a_coarse_raster_is_the_shape_the_grid_reduces_to() {
    for fixture in JPEG2000_FIXTURES {
        let r = reader(fixture);
        let (ni, nj) = r.messages[0].gds.dimensions().expect("a raster fixture");
        for reduction in 1u8..=2 {
            let step = 1u32 << reduction;
            let coarse = r
                .decode_message_raster_with(0, DecodeOptions::new(reduction))
                .unwrap_or_else(|e| panic!("{fixture} at reduction {reduction}: {e:?}"));
            assert_eq!(
                (coarse.ni(), coarse.nj()),
                (ni.div_ceil(step), nj.div_ceil(step)),
                "{fixture} at reduction {reduction}: raster shape"
            );
            assert_eq!(
                coarse.display_values().len(),
                coarse.ni() as usize * coarse.nj() as usize,
                "{fixture} at reduction {reduction}: value count matches the raster"
            );
            assert_eq!(coarse.resolution_reduction(), reduction);
            assert_eq!(
                coarse.exact_values(),
                None,
                "{fixture} at reduction {reduction}: a low-pass is not the message's data"
            );
            assert!(
                coarse.display_values().iter().all(Option::is_some),
                "{fixture}: a bitmapless field has no missing points at any resolution"
            );
        }
    }
}

/// The one value claim a coarse decode can make: the low-pass has the field's
/// mean, to within the wavelet's own smoothing. There is no eccodes oracle for
/// a reduced `grid_jpeg` decode — see the module docs — so this is the check,
/// stated as such.
///
/// The bound is a tenth of a percent of the field's own range, and each fixture
/// is checked only as deep as it stays a picture of something. Measured drift
/// at the levels below is 0.020 %–0.054 %; a real mistake — the wrong pyramid
/// level, samples read as signed, the `R`/`E`/`D` transform applied twice —
/// moves the mean by whole multiples of the range, so the margin is two-fold
/// against noise and four orders against a defect.
///
/// The two 16×31 fixtures stop at reduction 1 on purpose. They are 8×16 there
/// and 4×8 one level down, where the low-pass is mostly boundary: the measured
/// drift is 1.3 % at reduction 2 and 4.4 % at reduction 3, which is the filter
/// being asked for a display of thirty-two points rather than anything wrong.
/// RAP, the 451×337 operational field this feature is for, holds 0.04 % to
/// reduction 3.
#[test]
fn the_coarse_mean_tracks_the_full_field() {
    for (fixture, deepest) in [
        ("jpeg2000_regular_latlon.grib2", 1u8),
        ("jpeg2000_local_40000.grib2", 1),
        ("rap_jpeg2000_lambert.grib2", 3),
    ] {
        let r = reader(fixture);
        let full = r.decode_message_raster(0).expect("full decode");
        let full_mean = mean(&full);
        let present: Vec<f64> = full.iter().flatten().copied().collect();
        let range = present.iter().copied().fold(f64::MIN, f64::max)
            - present.iter().copied().fold(f64::MAX, f64::min);
        assert!(
            range > 0.0,
            "{fixture}: a constant field has no scale to bound by"
        );
        for reduction in 1..=deepest {
            let coarse = r
                .decode_message_raster_with(0, DecodeOptions::new(reduction))
                .expect("coarse decode");
            let coarse_mean = mean(coarse.display_values());
            assert!(
                (coarse_mean - full_mean).abs() <= 0.001 * range,
                "{fixture} at reduction {reduction}: coarse mean {coarse_mean} against full \
                 {full_mean}, range {range}"
            );
        }
    }
}

/// The load-bearing geometry claim: coarse point `(i, j)` is where the message
/// puts point `(i·2^r, j·2^r)`. A field paired with the message's own GDS
/// instead would draw at `1/2^r` of its size, which is the defect the derived
/// geometry exists to prevent — and which no value check would catch.
#[test]
fn the_coarse_grid_places_the_points_it_keeps() {
    for fixture in JPEG2000_FIXTURES {
        let r = reader(fixture);
        let source = GridGeometry::from(&r.messages[0].gds);
        for reduction in 1u8..=2 {
            let step = 1u32 << reduction;
            let coarse = r
                .decode_message_raster_with(0, DecodeOptions::new(reduction))
                .expect("coarse decode");
            let mut placed = 0usize;
            for j in 0..coarse.nj() {
                for i in 0..coarse.ni() {
                    let want = source.forward(i * step, j * step);
                    let got = coarse.geometry().forward(i, j);
                    match (want, got) {
                        (None, None) => {}
                        (Some(w), Some(g)) => {
                            placed += 1;
                            assert!(
                                (g.0 - w.0).abs() < 1e-8 && (g.1 - w.1).abs() < 1e-8,
                                "{fixture} at reduction {reduction}: coarse ({i}, {j}) at {g:?}, \
                                 source ({}, {}) at {w:?}",
                                i * step,
                                j * step
                            );
                        }
                        (w, g) => panic!(
                            "{fixture} at reduction {reduction}: source ({}, {}) at {w:?} but \
                             coarse ({i}, {j}) at {g:?}",
                            i * step,
                            j * step
                        ),
                    }
                }
            }
            assert!(placed > 0, "{fixture}: nothing placed, so nothing compared");
        }
    }
}

/// A reduction the codestream has no level for is refused by the codec and the
/// refusal is surfaced, not clamped: a caller asking for a field the file does
/// not carry gets told so.
///
/// RAP rather than the 16×31 fixtures, and this is the trap: those are 16 wide,
/// so reduction 4 leaves them one column and the *geometry* declines first —
/// a test written on them would pass without ever reaching the codec. RAP is
/// 451×337 with six resolutions, so reduction 5 is the deepest level that
/// exists and the geometry is still a grid three levels past that.
#[test]
fn a_reduction_past_the_pyramid_is_refused() {
    let r = reader("rap_jpeg2000_lambert.grib2");
    let deepest = r
        .decode_message_raster_with(0, DecodeOptions::new(5))
        .expect("the codestream carries six resolutions, so five levels can be discarded");
    assert_eq!((deepest.ni(), deepest.nj()), (15, 11));

    let err = r
        .decode_message_raster_with(0, DecodeOptions::new(6))
        .expect_err("one past the deepest level has nothing to decode");
    let text = err.to_string();
    assert!(
        text.contains("decode at resolution reduction 6 failed"),
        "the refusal is the codec's, surfaced with the level asked for: {text}"
    );
    // Not the geometry declining: at reduction 6 the grid is still 8×6.
    assert!(
        GridGeometry::from(&r.messages[0].gds)
            .subsampled(6)
            .is_some(),
        "the geometry is not what refused"
    );
}

/// Only §5.40 carries a pyramid, and every other packing says so rather than
/// decoding in full and calling the result coarse.
#[test]
fn every_other_packing_refuses_a_reduction() {
    for fixture in [
        "regular_latlon_surface.grib2",
        "ccsds_regular_latlon.grib2",
        "png_local_40010.grib2",
        "complex_spd2_regular_latlon.grib2",
        "ieee32_regular_latlon.grib2",
    ] {
        let r = reader(fixture);
        let err = r
            .decode_message_raster_with(0, DecodeOptions::new(1))
            .expect_err("only JPEG 2000 reduces");
        let text = err.to_string();
        assert!(text.contains("only JPEG 2000"), "{fixture}: {text}");
        // And reduction zero on the same message is fine, so the refusal is
        // about the level asked for and not about the packing as such.
        assert!(
            r.decode_message_raster_with(0, DecodeOptions::default())
                .is_ok(),
            "{fixture}: reduction 0 is every packing's own decode"
        );
    }
}

/// A layout that is not a rectangle has no raster to display at any
/// resolution. The two rasterless families refuse at different places, and both
/// are checked because only one of them is this method's own guard.
///
/// A spherical-harmonic message never reaches it: `decode_message_values`
/// already refuses coefficients and names the two calls that decode them, which
/// is the answer a caller wants and is unchanged here. HEALPix *does* decode —
/// it is a list of `12·Nside²` pixels — and it is the raster guard that
/// declines to call that list a rectangle.
#[test]
fn a_layout_with_no_raster_is_refused() {
    let spectral = reader("spectral_complex_t63.grib2")
        .decode_message_raster_with(0, DecodeOptions::default())
        .expect_err("coefficients are not values on a grid");
    assert!(
        spectral
            .to_string()
            .contains("spherical-harmonic coefficients"),
        "{spectral}"
    );

    let healpix = reader("healpix_n4_ring.grib2")
        .decode_message_raster_with(0, DecodeOptions::default())
        .expect_err("a pixel list is not a raster");
    let text = healpix.to_string();
    assert!(
        text.contains("healpix") && text.contains("raster"),
        "the refusal names the family and what it lacks: {text}"
    );

    // And above reduction zero the answer must be the same one. The packing is
    // also wrong for a coarse decode on both of these, but naming that would
    // point at the one thing a caller could change instead of at the one it
    // cannot — so the raster question is asked first.
    for (fixture, family) in [
        ("spectral_complex_t63.grib2", "spherical_harmonic"),
        ("healpix_n4_ring.grib2", "healpix"),
    ] {
        let err = reader(fixture)
            .decode_message_raster_with(0, DecodeOptions::new(1))
            .expect_err("still no raster");
        let text = err.to_string();
        assert!(
            text.contains(family) && text.contains("no coarse one either"),
            "{fixture}: {text}"
        );
    }
}

/// The two scanning-mode guards, reached by setting the flag on a parsed
/// message — no producer ships JPEG 2000 under either, and the guards must
/// hold anyway. Both orders are undone *after* the packing decodes, so a
/// wavelet low-pass of the stored raster has no undo.
#[test]
fn an_exotic_scan_order_is_refused() {
    for (flag, expected) in [
        (SCAN_ALTERNATE_ROWS, "alternate-row"),
        (SCAN_J_CONSECUTIVE, "j-consecutive"),
    ] {
        let mut r = reader("jpeg2000_regular_latlon.grib2");
        let GridTemplate::LatLon(t) = &mut r.messages[0].gds.template else {
            panic!("the fixture is a regular lat/lon grid")
        };
        t.scanning_mode |= flag;
        let err = r
            .decode_message_raster_with(0, DecodeOptions::new(1))
            .expect_err("an exotic scan order has no coarse form");
        assert!(err.to_string().contains(expected), "{}", err);
    }
}

/// A reduced grid's rows differ in width, so its codestream is not the raster
/// the rows expand into. Reached by giving a reduced Gaussian message the RAP
/// fixture's §5 — no file pairs the two, and the guard must hold anyway.
#[test]
fn a_reduced_grid_is_refused() {
    let jpeg = reader("rap_jpeg2000_lambert.grib2").messages[0].drs.clone();
    let mut r = reader("reduced_gaussian_pressure_level.grib2");
    assert!(
        r.messages[0].gds.points_per_row().is_some(),
        "the fixture is a reduced grid"
    );
    r.messages[0].drs = jpeg;
    let err = r
        .decode_message_raster_with(0, DecodeOptions::new(1))
        .expect_err("a ragged grid has no coarse raster");
    assert!(err.to_string().contains("reduced grid"), "{}", err);
}

/// A family with no derivable coarse geometry is refused by name. A regular
/// Gaussian grid is the case: its rows are Gauss–Legendre nodes, and every
/// other node is not the half-order quadrature — see
/// [`GridGeometry::subsampled`]. Reached the same way, by giving it the RAP
/// fixture's §5.
#[test]
fn a_family_with_no_coarse_geometry_is_refused() {
    let jpeg = reader("rap_jpeg2000_lambert.grib2").messages[0].drs.clone();
    let mut r = reader("regular_gaussian_f32.grib2");
    assert!(
        r.messages[0].gds.points_per_row().is_none(),
        "the fixture is a regular Gaussian grid, not a reduced one"
    );
    r.messages[0].drs = jpeg;
    let err = r
        .decode_message_raster_with(0, DecodeOptions::new(1))
        .expect_err("a Gaussian grid has no derivable coarse geometry");
    let text = err.to_string();
    assert!(
        text.contains("no derivable geometry") && text.contains("gaussian"),
        "{text}"
    );
}

/// A §6 bitmap is one flag per full-resolution point and has no low-pass.
///
/// No committed fixture pairs a bitmap with JPEG 2000, so this builds one: the
/// 16×31 lat/lon fixture with its empty §6 replaced by an all-present bitmap.
/// That is a *valid* message — every point is present, so the codestream still
/// holds one sample per point — which the full-resolution decode below proves
/// by returning the original values. The reduced decode must still refuse it.
#[test]
fn a_bitmap_is_refused() {
    let fixture = "jpeg2000_regular_latlon.grib2";
    let original = reader(fixture)
        .decode_message_raster(0)
        .expect("the fixture decodes");
    let bitmapped = with_all_present_bitmap(&read(fixture), original.len());
    let r = Grib2Reader::from_bytes(bitmapped).expect("the rebuilt message parses");
    assert_eq!(
        r.decode_message_raster(0).expect("it still decodes"),
        original,
        "the rebuilt message is the same field, so the refusal below is about the bitmap"
    );
    let err = r
        .decode_message_raster_with(0, DecodeOptions::new(1))
        .expect_err("a bitmap has no low-pass");
    assert!(err.to_string().contains("§6 declares a bitmap"), "{}", err);
}

/// Rebuild a GRIB2 message with its §6 replaced by an all-ones inline bitmap
/// over `points` grid points, fixing §0's total-length field to match.
///
/// Deliberately narrow: it asserts the message's §6 currently declares no
/// bitmap, so it cannot quietly do nothing.
fn with_all_present_bitmap(bytes: &[u8], points: usize) -> Vec<u8> {
    const BMS_INDICATOR_NONE: u8 = 255;
    let mut out = bytes[..16].to_vec();
    let mut offset = 16usize;
    let mut replaced = false;
    while &bytes[offset..offset + 4] != b"7777" {
        let length =
            u32::from_be_bytes(bytes[offset..offset + 4].try_into().expect("4 bytes")) as usize;
        let number = bytes[offset + 4];
        if number == 6 {
            assert_eq!(
                bytes[offset + 5],
                BMS_INDICATOR_NONE,
                "the fixture's §6 already carries a bitmap"
            );
            let mut bitmap = vec![0xffu8; points.div_ceil(8)];
            // The trailing bits of the last octet are padding; §6 leaves them
            // unspecified, and zeroing them keeps the section unambiguous.
            if !points.is_multiple_of(8) {
                let last = bitmap.len() - 1;
                bitmap[last] = 0xffu8 << (8 - points % 8);
            }
            let section_length = 6 + bitmap.len();
            out.extend_from_slice(&(section_length as u32).to_be_bytes());
            out.push(6);
            out.push(0);
            out.extend_from_slice(&bitmap);
            replaced = true;
        } else {
            out.extend_from_slice(&bytes[offset..offset + length]);
        }
        offset += length;
    }
    assert!(replaced, "the message carried no §6 to replace");
    out.extend_from_slice(b"7777");
    let total = out.len() as u64;
    out[8..16].copy_from_slice(&total.to_be_bytes());
    out
}

/// §3's own point count must agree with the shape its template declares, and
/// the shape must stay under the raster cap — the two guards
/// `decode_message_values` applies before it sizes anything, restated on the
/// coarse path because that path does not go through it.
///
/// Both are reached by editing the parsed §3 of a real JPEG 2000 message. The
/// cap is the one a decode fuzz target found: a corrupted `ni`/`nj` naming a
/// hundred-million-point grid the file carries no data for, whose constant
/// field then allocates gigabytes.
#[test]
fn a_section_that_disagrees_with_itself_is_refused() {
    let mut r = reader("jpeg2000_regular_latlon.grib2");
    r.messages[0].gds.num_data_points += 1;
    let err = r
        .decode_message_raster_with(0, DecodeOptions::new(1))
        .expect_err("the section disagrees with its own template");
    let text = err.to_string();
    assert!(
        text.contains("disagree with the GDS-declared 497 data points"),
        "{text}"
    );

    let mut r = reader("jpeg2000_regular_latlon.grib2");
    let GridTemplate::LatLon(t) = &mut r.messages[0].gds.template else {
        panic!("the fixture is a regular lat/lon grid")
    };
    (t.ni, t.nj) = (20_000, 20_000);
    // Agreeing with itself, so the cap is what refuses and not the check above.
    r.messages[0].gds.num_data_points = 400_000_000;
    let err = r
        .decode_message_raster_with(0, DecodeOptions::new(1))
        .expect_err("400 million points is past the cap");
    assert!(err.to_string().contains("exceeds cap of"), "{}", err);
}

/// Which coarse cell each value lands in — which nothing else here pins.
///
/// Every other check in this file is about a shape, a geometry or a mean, and a
/// mean survives any permutation of the raster: a coarse field returned with
/// its rows reversed, its columns reversed, or shifted a cell passes all of
/// them. That is a defect a display would show and a test would not.
///
/// The oracle is the full-resolution field itself. A wavelet low-pass is not a
/// block mean — the 5/3 filter has a five-tap support and reads across block
/// boundaries — so the two do not agree to a tolerance worth writing down: the
/// worst single point is a third of the field's range, at an edge. What they do
/// is agree *far better than any other arrangement does*, and that is the
/// claim. The mean absolute deviation from the aligned block means is within a
/// twentieth of the field's range (measured 0.98 %–2.26 %), and at least half
/// again smaller than for the raster flipped in either axis or shifted one
/// coarse cell in either direction (measured 1.70×–18×).
///
/// Reduction 1 only, deliberately. The comparison weakens as the blocks grow:
/// at reduction 2 a four-by-four block of a smooth field is close to its
/// neighbour's, and the shift margin falls to 1.2×. A margin that held at every
/// level would have to be loose enough to hold at the worst one, which is the
/// level that discriminates least.
#[test]
fn the_coarse_values_land_in_the_cells_they_claim() {
    /// Where a coarse cell's value would come from under one wrong
    /// arrangement, given the raster's own `(ci, cj)`.
    type Rearrange = fn(u32, u32, u32, u32) -> (u32, u32);

    /// The arrangements this distinguishes the returned raster from. Each is a
    /// real defect: the first two are a flipped raster, the last two are the
    /// displacement an offset codestream origin would produce.
    const WRONG: &[(&str, Rearrange)] = &[
        ("rows flipped", |i, j, _, cj| (i, cj - 1 - j)),
        ("columns flipped", |i, j, ci, _| (ci - 1 - i, j)),
        ("shifted one coarse column", |i, j, ci, _| {
            ((i + 1).min(ci - 1), j)
        }),
        ("shifted one coarse row", |i, j, _, cj| {
            (i, (j + 1).min(cj - 1))
        }),
    ];

    for fixture in JPEG2000_FIXTURES {
        let r = reader(fixture);
        let (ni, nj) = r.messages[0].gds.dimensions().expect("a raster fixture");
        let full = r.decode_message_raster(0).expect("full decode");
        let present: Vec<f64> = full.iter().flatten().copied().collect();
        let range = present.iter().copied().fold(f64::MIN, f64::max)
            - present.iter().copied().fold(f64::MAX, f64::min);

        let coarse = r
            .decode_message_raster_with(0, DecodeOptions::new(1))
            .expect("coarse decode");
        let (ci, cj) = (coarse.ni(), coarse.nj());
        let got: Vec<f64> = coarse
            .display_values()
            .iter()
            .map(|v| v.expect("a bitmapless field"))
            .collect();

        // The aligned 2×2 block means of the full field, in the coarse
        // raster's own order. A block at the far edge is clipped, which is
        // what `div_ceil` leaves it.
        let block_mean = |i: u32, j: u32| {
            let (mut sum, mut n) = (0.0, 0usize);
            for dj in 0..2 {
                for di in 0..2 {
                    let (x, y) = (i * 2 + di, j * 2 + dj);
                    if x < ni
                        && y < nj
                        && let Some(v) = full[(y * ni + x) as usize]
                    {
                        sum += v;
                        n += 1;
                    }
                }
            }
            assert!(n > 0, "{fixture}: coarse ({i}, {j}) covers no source point");
            sum / n as f64
        };

        let deviation = |place: &dyn Fn(u32, u32) -> (u32, u32)| {
            let mut sum = 0.0;
            for j in 0..cj {
                for i in 0..ci {
                    let (si, sj) = place(i, j);
                    sum += (got[(sj * ci + si) as usize] - block_mean(i, j)).abs();
                }
            }
            sum / got.len() as f64
        };

        let aligned = deviation(&|i, j| (i, j));
        assert!(
            aligned <= 0.05 * range,
            "{fixture}: the coarse raster is {aligned} from the block means of a field whose \
             range is {range}"
        );
        for (label, rearrange) in WRONG {
            let other = deviation(&|i, j| rearrange(i, j, ci, cj));
            assert!(
                other >= 1.5 * aligned,
                "{fixture}: {label} is {other} from the block means and the raster as returned \
                 is {aligned}; the two are too close for this to be evidence about placement"
            );
        }
    }
}
