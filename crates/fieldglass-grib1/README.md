# fieldglass-grib1

Reads [GRIB edition 1](https://www.nco.ncep.noaa.gov/pmb/docs/on388/) weather files in pure Rust: every message's metadata, its
values, and where its grid sits on the Earth. This crate is usable on its own,
and is part of [Fieldglass](https://github.com/D0ubleD0uble/fieldglass).

## What it reads

GRIB is the format forecast centers use to share model output, and edition 1
is the older of its two editions, still common in archives and some
operational feeds. A GRIB1 file is a list of messages. Each message holds one
field, such as the temperature at 2 meters over Europe for one forecast hour,
and describes itself in a few short sections: what the field is and when it's
for, the grid it sits on, which points have a value, and the packed values
themselves.

- It reads **every message's metadata**: the parameter, level, reference time
  and forecast time, the center that made it, and its grid.
- It **decodes the values** of the packings in real use: simple packing, IEEE
  floating point, ECMWF's complex packing, and second-order packing in both
  its classic and general extended forms, including spatial differencing and
  rows that alternate direction.
- It **places the grid on the Earth** for regular and rotated latitude and
  longitude, Gaussian, polar stereographic and Lambert conformal grids, and
  for reduced grids whose rows have different numbers of points. A message
  with no grid description at all is matched to the common [predefined global
  grids](https://www.nco.ncep.noaa.gov/pmb/docs/on388/tableb.html) by their catalog number.
- It hands back **a plain rectangle of values** whatever the message stored.
  Reduced rows are widened and a grid stored column by column is reordered
  into rows, so you don't have to know either rule.
- It reads **spectral fields**, which store waves around the globe instead of
  grid values, and turns them back into a latitude and longitude grid so they
  can be drawn like any other field.
- It reads **matrix fields**, which hold a small matrix of values at every
  grid point.

What it doesn't do:

- It doesn't place **every grid type**. A grid family it doesn't model, such
  as Mercator, is reported by its number (`unsupported(1)`), and the rest of
  the message's metadata is still read.
- It doesn't read a **[predefined bitmap](https://www.nco.ncep.noaa.gov/pmb/docs/on388/section3.html)**, one a message names by number
  instead of carrying. It refuses that message with a reason, instead of
  treating every point as present.
- It doesn't **write** GRIB. It only reads it.

## Example

```rust
use fieldglass_grib1::{Grib1MessageKind, Grib1Reader};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Any GRIB1 file's bytes. This one is committed with the crate, so the
    // snippet runs as written from a checkout.
    let bytes = std::fs::read("tests/fixtures/cmc_wind_300_2010052400_p012.grib")?;
    let reader = Grib1Reader::from_bytes(bytes)?;

    for index in 0..reader.message_count() {
        // Which entry point applies is not a property of the packing label
        // alone, so ask rather than guess.
        if reader.message_kind(index) != Grib1MessageKind::Grid {
            continue;
        }
        let gds = reader.messages[index].gds.as_ref().expect("a grid description");
        let (ni, nj) = gds.dimensions().expect("a raster shape");

        // `decode_message_raster` hands back `ni · nj` values in row-major
        // order whatever the message stored — reduced rows widened, a
        // `j`-consecutive grid transposed. `decode_message_values` is the field
        // exactly as stored, when that is what you want.
        let values: Vec<Option<f64>> = reader.decode_message_raster(index)?;
        println!("{} {ni}x{nj}: {} values", gds.grid_type_name(), values.len());
    }

    Ok(())
}
```

A fuller version, which also prints the stored and raster counts side by side,
is `examples/decode.rs`. Run it with `cargo run -p fieldglass-grib1 --example
decode` from a checkout of the repository. Each value is an `Option<f64>`, with
`None` for a point the message marks as missing.

A message is one of three decodable kinds, and each kind has its own way to
decode. `message_kind` tells you which: a grid message goes through
`decode_message_raster`, a spectral one through `decode_spectral_message` (or
`synthesize_spectral_message` to get a grid back), and a matrix one through
`decode_matrix_message`. For a message none of these can decode,
`message_kind` answers `Unsupported`.

## What it is not

It's a reader for one format. To open GRIB1, GRIB2, NetCDF and Zarr through one
interface, or to draw a field as a map, use
[`fieldglass`](https://crates.io/crates/fieldglass). The grids it returns use
the shared types from
[`fieldglass-core`](https://crates.io/crates/fieldglass-core), which this crate
re-exports, so you don't need a second dependency to name them.

## How it is checked

Every test file that ecCodes, ECMWF's GRIB library, can read is decoded and
compared with a saved ecCodes snapshot of its metadata and values. The
spectral transform and the matrix layout, which ecCodes can't produce, are
checked against [the specification](https://www.nco.ncep.noaa.gov/pmb/docs/on388/) instead.

## Related crates

| Crate | Use it to |
|---|---|
| [`fieldglass`](https://crates.io/crates/fieldglass) | Open any supported file and draw it |
| [`fieldglass-grib2`](https://crates.io/crates/fieldglass-grib2) | Read GRIB edition 2 |
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
  `fieldglass-grib1`, the files come from the Canadian Meteorological Centre
  and ECMWF, or are built from ecCodes' samples or by hand, and each is
  checked against ecCodes wherever ecCodes can read it.
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
