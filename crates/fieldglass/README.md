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
is `#[non_exhaustive]`, so build an options struct from its default and adjust
it rather than with a struct literal.

## Feature flags

- **`schema`** *(default)* — `schemars::JsonSchema` on every API type, which is
  what a host's TypeScript or Python declarations are generated from. Off for
  `fieldglass-wasm`, whose declarations come from wasm-bindgen.

## Scope of this first cut

GRIB1 and GRIB2, and the four grid families NOAA NODD and ECMWF publish
(lat/lon, Gaussian, Lambert conformal, polar stereographic). Anything else
reports `Unsupported` with its own label rather than erroring, so a message can
still say which grid was declined.

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
