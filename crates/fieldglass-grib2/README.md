# fieldglass-grib2

Reads [GRIB edition 2](https://www.nco.ncep.noaa.gov/pmb/docs/grib2/grib2_doc/) weather files in pure Rust: every message's metadata, its
values for every registered packing, and where its grid sits on the Earth.
This crate is usable on its own, and is part of
[Fieldglass](https://github.com/D0ubleD0uble/fieldglass).

## What it reads

GRIB is the format forecast centers use to share model output, and edition 2
is the one most of them publish today. A GRIB2 file is a list of messages.
Each message holds one field, such as the temperature at the surface over the
United States for one forecast hour, and describes itself in nine numbered
sections, 0 to 8: the edition and length, who made it and when, an optional
section for the center's own use, the grid it sits on, what the field is, how
its values are packed, which points have a value, the packed values
themselves, and an end marker.

- It reads **all nine sections** of every message, so you can list a file's
  contents without decoding any values.
- It **decodes every packing** in [WMO's register](https://github.com/wmo-im/GRIB2), with no C libraries and no
  build flags. The [GRIB2 packing modes
  table](https://github.com/D0ubleD0uble/fieldglass#grib2-packing-modes) shows
  each one and how it's checked. The compressed ones use pure-Rust decoders,
  so the crate cross-compiles anywhere Rust does.
- It **places the grid on the Earth**: regular and rotated latitude and
  longitude, Mercator, transverse Mercator, polar stereographic, Lambert
  conformal, regular and reduced Gaussian, Lambert azimuthal equal-area,
  geostationary satellite views, and [HEALPix](https://www.nco.ncep.noaa.gov/pmb/docs/grib2/grib2_doc/grib2_temp3-150.shtml), the equal-area tiling ECMWF
  publishes its higher-resolution open data on.
- It reads **what each field is**, including ensemble members, derived fields
  such as an ensemble mean, probabilities, percentiles, and chemistry and
  aerosol constituents.
- It hands back **a plain rectangle of values** whatever the message stored.
  Reduced rows are widened and a grid stored column by column is reordered
  into rows, so you don't have to know either rule.
- It turns **spectral fields**, which store waves around the globe instead of
  grid values, back into a latitude and longitude grid, and resamples HEALPix
  fields onto one, so they can be drawn like any other field.

What it doesn't do:

- It doesn't turn **every non-grid form** back into a grid. Bi-Fourier
  fields, the limited-area spectral form, decode to their coefficients only.
- It doesn't place **every grid type**. A grid template it doesn't model still
  has its metadata read, and its dimensions come back as `None`.
- It doesn't **write** GRIB. It only reads it.

## Example

```rust
use fieldglass_grib2::Grib2Reader;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Any GRIB2 file's bytes. This one is committed with the crate, so the
    // snippet runs as written from a checkout.
    let bytes = std::fs::read("tests/fixtures/regular_latlon_surface.grib2")?;
    let reader = Grib2Reader::from_bytes(bytes)?;

    for index in 0..reader.message_count() {
        let msg = &reader.messages[index];
        // `None` for the three families that are not a rectangle of values at
        // all — spherical-harmonic and bi-Fourier coefficients, and HEALPix
        // pixels, which have their own entry points and would refuse the call
        // below — and for a §3 template this build does not model.
        let Some((ni, nj)) = msg.gds.dimensions() else {
            continue;
        };

        // Every scalar §5 packing reaches this one call — a caller never
        // branches on `msg.drs.template_number`. `decode_message_raster` hands
        // back `ni · nj` values in row-major order whatever the message stored
        // (reduced rows widened, a `j`-consecutive grid transposed);
        // `decode_message_values` is the field exactly as stored.
        let values: Vec<Option<f64>> = reader.decode_message_raster(index)?;
        println!("§5.{} {ni}x{nj}: {} values", msg.drs.template_number, values.len());
    }

    Ok(())
}
```

A fuller version, which also prints the stored and raster counts side by side,
is `examples/decode.rs`. Run it with `cargo run -p fieldglass-grib2 --example
decode` from a checkout of the repository. Each value is an `Option<f64>`, with
`None` for a point the message marks as missing.

The packings that don't produce one value per grid point have their own entry
points. A spectral field goes through `decode_spectral_message`, or
`synthesize_spectral_message` to get a grid back. A bi-Fourier field goes
through `decode_bifourier_message`, and a field with a small matrix at every
point goes through `decode_matrix_message`. `synthesize_message_global` puts
either a spectral or a HEALPix field on a latitude and longitude grid, and
returns `None` for any other message.

### A coarse view of a JPEG 2000 field

Some fields are compressed with [JPEG 2000](https://www.nco.ncep.noaa.gov/pmb/docs/grib2/grib2_doc/grib2_temp5-40.shtml), and that compression stores the
picture as a stack of levels, each half the size of the one below. A zoomed-out
map only needs a coarse level, and reading one costs much less.
`decode_message_raster_with(index, DecodeOptions::new(1))` returns a half-size
raster in about a quarter of the time, with the grid that places it. On the
451×337 RAP test file, run natively in release, that's 39.9 ms down to
10.5 ms. `cargo run --release -p fieldglass-grib2 --example bench_reduce`
prints what each level costs.

Those coarse values are averages the message doesn't contain, so the result
is a different type, a `ReducedRaster`, which nothing that takes a decoded
field (probing, contouring, statistics) will accept. This crate's
`DecodeOptions` is unrelated to the type of the same name in the `fieldglass`
crate, so a program that uses both should import one of them by its full
path.

## What it is not

It's a reader for one format. To open GRIB1, GRIB2, NetCDF and Zarr through one
interface, or to draw a field as a map, use
[`fieldglass`](https://crates.io/crates/fieldglass). The grids it returns use
the shared types from
[`fieldglass-core`](https://crates.io/crates/fieldglass-core), which this crate
re-exports, so you don't need a second dependency to name them.

## How it is checked

Every test file that ecCodes, ECMWF's GRIB library, can read is decoded and
compared with ecCodes. Where ecCodes can't help (it crashes on a true matrix
field, can't turn a spectral field back into a grid, and has no definition for
one early NCEP PNG form), the decode is checked against the specification and
independent implementations instead. Where ecCodes reads a file but gets part
of it wrong (the values of a reduced Gaussian grid whose alternate rows run
backwards, and some keys of an aerosol product template), the test expects the
specification's answer, and the divergence is recorded with the test
files.

## Related crates

| Crate | Use it to |
|---|---|
| [`fieldglass`](https://crates.io/crates/fieldglass) | Open any supported file and draw it |
| [`fieldglass-grib1`](https://crates.io/crates/fieldglass-grib1) | Read GRIB edition 1 |
| [`fieldglass-fetchplan`](https://crates.io/crates/fieldglass-fetchplan) | Work out which bytes of a remote GRIB file hold the field you want |
| [`fieldglass-aec`](https://crates.io/crates/fieldglass-aec) | Decode the [CCSDS compression](https://www.nco.ncep.noaa.gov/pmb/docs/grib2/grib2_doc/grib2_temp5-42.shtml) this crate uses |
| [`fieldglass-core`](https://crates.io/crates/fieldglass-core) | Name the types every Fieldglass reader shares |

## License

MIT OR Apache-2.0

## AI disclosure

Much of this crate's code, tests and documentation was written with AI
assistance (Anthropic's Claude), under the maintainer's direction and review.

### How do I maintain quality?

I treat the AI as a fast contributor whose work I don't take on trust:

- Fieldglass is designed against the specifications and reference libraries are
  used to check against the resulting behaviors. If behaviors differ between
  Fieldglass and a reference, Fieldglass follows the specification and records
  why.
- Every example comes from as close to real-world files as possible while also
  trying not to bloat the size of our libraries with data. In
  `fieldglass-grib2`, the files come from NOAA and ECMWF, or are built with
  ecCodes and our own scripts for the packings and grids no small public file
  covers, and each is checked against ecCodes wherever ecCodes can read it.
- I take a personal hand in testing each feature and delivery before a new tag
  is released. I maintain a testing plan in which I test each change.
- I have many strict automatic checks for formatting, lints, the test suite,
  dependency and security scans, and fuzzing with damaged or hostile files to
  make sure we refuse them safely.

### Where do I currently notice gaps?

- I dislike the current documentation. There's a lot of it, which would be fine
  if it made the docs more readable but instead it's hard to understand. I am
  undertaking efforts to rewrite the user-facing documentation with my own
  words. This file has been rewritten by me.
- While I have been ensuring proper functionality, behaviors, and
  larger-picture dependency graphs, I haven't been reading over every line of
  code. As the shape of things starts to become a bit more stable, I hope to
  review more of the code closely, but to be totally transparent this is more
  of a 'desire' than a reality currently.
