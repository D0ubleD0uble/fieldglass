# fieldglass

The host-neutral Fieldglass API: bytes in, plain data out.

This is the crate a Rust consumer reaches for, and the one every host binds.
The format crates (`fieldglass-grib1`, `-grib2`, `-netcdf`) stay independently
usable; this one sits above them and below a binding, so a host carries only
four things — buffer conversion, error mapping, method forwarding, packaging.
See [ADR-0006](../../docs/decisions/0006-hosts-are-bindings-over-a-plain-data-api.md).

```rust
let bytes = std::fs::read("forecast.grib2")?;
let session = fieldglass::Session::open(bytes)?;

let info = session.message(0)?;          // lazy: one message, not all of them
let field = session.decode(0, &Default::default())?;
let palette = session.palette(&field, &Default::default())?;
let rgba = session.render(&field, &Default::default(), false)?;

// Two fields at once — the difference map and its siblings.
let other = session.decode(1, &Default::default())?;
let anomaly = session.combine(&field, &other, fieldglass::CombineOp::Difference)?;
```

## What a `Field` is

Contiguous values, a separate `u8` mask, and scalar georeferencing. Not
`Vec<Option<f64>>`: that is the engine's shape, it costs a branch per element to
cross a language seam, and it is not a typed array. Not `NaN` for absent cells
either — `isnan()` is unreliable on some mobile GPUs and a `NaN` poisons linear
filtering in a texture.

**The element type follows the source.** `Dtype::Auto` narrows to `f32` only
when every present value survives the round trip. That is stricter than "the
packing used 24 bits or fewer": a simple-packed value is `(R + X·2ᴱ)·10⁻ᴰ`, and
once the reference value sits far from zero relative to the quantum — or `D` is
non-zero, making the quantum a negative power of ten — the ordinals fitting an
`f32` mantissa says nothing about the values fitting. A host that wants an
`R32F` texture regardless asks for `Dtype::F32` and gets the loss it chose.

**`Georef` carries both halves of the placement.** A CRS it can name (`proj4`)
and an affine placing the raster in that CRS (`x0`, `y0`, `dx`, `dy`), in
degrees for the geographic families and projection-plane metres for the
projected ones. A family that cannot state something says `None` rather than
guessing — a Gaussian grid's rows are Gauss–Legendre nodes, so its `dy` is
absent, and inventing a mean one would misplace every row but the middle.

## Colour is decided once, here

`Palette` is the painter's own 256-entry lookup table plus the transformed
domain, exported as data. A GPU host uploads it as a 256 × 1 texture and pastes
`fieldglass::GLSL` into its fragment shader; `shader_values` prepares the field
the shader reads. The CPU painter consumes the same `Palette`, so it is the
**oracle** a GPU path is tested against rather than a second colour
implementation.

The field is rebased by `t0` **in Rust**, in `f64`, and this is not an
optimisation. A shader that subtracts `t0` itself, in `f32`, loses the domain
rather than the result: once the `f32` gap at `t0` exceeds one lookup step the
error is unbounded within the ramp — measured at up to 127 entries, half the
table, for a 1.0 range over values near 1e7, which geopotential in m²/s² and
pressure in Pa both reach under a tight manual range.

## Wire format

Structs are `camelCase` on the wire and `snake_case` in Rust; enum *variants*
stay `snake_case`, because a variant tag is a value a host compares strings
against and `"polar_stereo"` is the one `core` already reports. Every API type
is `#[non_exhaustive]`, so build an options struct through its `new` — every
option type has one — and assign the rest.

`render(field, options, flip_y)` composes `flip_y` with the message's own scan
order rather than replacing it, so `false` means **north up** and not "rows as
stored". `Georef::scan` still carries the flag, because a host drawing
something other than a raster needs it; a host painting one passes the user's
request straight through.

## Combining two fields

`Session::combine(a, b, op)` is the difference map and its siblings, and the one
operation here that takes two fields. Its precondition is that they align cell
for cell, which is `PartialEq` on the grid geometry plus the raster shape and
the scan order; a mismatch is `Error::Unsupported` naming which of the three
differs. The result is a `Field` on **A's** placement, so `warp`, `palette`,
`render`, `probe` and `contours` take it like any other, and it carries A's
`parameter` and `units` verbatim — the caption `A − B` is the host's to compose.
`combine_ops()` is the vocabulary a Compare picker is built from, and
`op_from_wire` parses a tag back.

## The conformance suite

`conformance/suite.json` records what every operation answers over a set of
committed GRIB fixtures, and `fieldglass::conformance` is the machinery a
runner needs (ADR-0006 decision 3). Three runners replay it: this crate's own
tests, on the native target and on `wasm32-wasip1`; `fieldglass-napi`'s
`conformance_host`, through the napi handles; and
`crates/fieldglass-wasm/tests/node/conformance.mjs`, through the built browser
bundle from Node. Adding a host is then a checklist — implement the buffer
handoff, map the error, pass the suite.

Discrete answers (counts, lengths, raster sizes, the mask, every error code)
are compared exactly and the geolocated numbers to a tolerance, which is what
[ADR-0009](../../docs/decisions/0009-cross-target-floating-point-agreement.md)
measured. Re-record with
`FIELDGLASS_UPDATE_CONFORMANCE=1 cargo test -p fieldglass --test conformance`;
that run fails afterwards on purpose.

## Feature flags

Every one is on by default. A consumer that says `default-features = false` is
asking to pay for less, and names what it wants back.

- **`grib1`**, **`grib2`** *(default)* — the decoders `Session::open`
  dispatches to. At least one is required; a build with neither is a compile
  error rather than a session that can only refuse. Format *detection* stays
  unconditional, so a GRIB1 file opened by a GRIB2-only build is refused as
  "GRIB1, compiled out" and not as "unknown bytes".
- **`render`** *(default)* — the projection and paint pipeline: `warp`,
  `palette`, `render`, `project`, `probe_pixel`, `overlay_polylines`, and the
  GLSL snippet.
- **`analysis`** *(default)* — operations that take values and return values:
  `combine`, `contours`, and CSV. Independent of `render`, so a values-first
  host takes contours without the painter; `contour_polylines`, which projects
  its isolines onto the render raster, is the one member that needs both.
- **`schema`** *(default)* — `schemars::JsonSchema` on every API type, which is
  what a host's TypeScript or Python declarations are generated from. Off for
  `fieldglass-wasm`, whose declarations come from wasm-bindgen.
- **`conformance`** *(default)* — the suite above. Both hosts take this crate
  with `default-features = false`, so neither the addon nor the browser bundle
  carries it; it is on by default because `cargo test --workspace` does not
  enable optional features, and a suite skipped there would pass while checking
  nothing. It turns on `grib1`, `grib2`, `render` and `analysis` with it — but
  not `schema` — because the suite is one recorded expectation per case over
  every format and both surfaces, and a partial build has no honest subset of
  it to run.

What the formats cost is most of the weight, because each decoder brings its
own codecs:

| Features | Crates linked |
| --- | --- |
| default | 34 |
| `grib2,render` | 23 |
| `grib1,render` | 12 |


## Scope of this first cut

GRIB1 and GRIB2 — whichever of the two this build compiled in — and every grid
family the engine can project: lat/lon,
Gaussian, Mercator, rotated lat/lon, Lambert conformal, polar stereographic,
transverse Mercator, Lambert azimuthal equal-area, and the geostationary space
view.

Two families are not rasters at all and are put on one at decode: a spherical
harmonic message is evaluated onto a global 0.5° lat/lon grid, and a HEALPix
message is resampled onto one sized from its `Nside`. Both come back as
ordinary `latlon` fields, so warp, palette, render, probe, contours and combine
need no special case; `message()` keeps reporting the grid the file declares
and `sizeLabel` its native shape (`T63`, `Nside 4`). Bi-Fourier is the one that
still declines — recovering its grid needs an inverse bi-Fourier transform this
build does not have — and its message reports `Unsupported` with its own label
rather than erroring, so it can still say which grid was declined.

Every projected family also names a PROJ CRS and the affine placing its raster
in it, checked against PROJ itself rather than against a golden of our own
output. Rotated lat/lon is the exception: it places its points, but its axes
are degrees in a rotated frame and it does not yet name that frame as a CRS.

Filed under
[#460](https://github.com/D0ubleD0uble/fieldglass/issues/460) so
`fieldglass-wasm` has something to bind.
[#464](https://github.com/D0ubleD0uble/fieldglass/issues/464) fixes the
`Session` surface against a second real consumer, moves the render
orchestration out of `fieldglass-napi`, and is when this goes to crates.io;
NetCDF, caller-sized output (#465), and reduced-resolution decode (#463) arrive
with their own issues.

## Licence

MIT OR Apache-2.0.
