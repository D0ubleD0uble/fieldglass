# fieldglass-core

`fieldglass-core` holds the abstraction pieces every Fieldglass reader shares:
the error type, reading bits and bytes, map projections and grid placement, and
the model that describes a file's arrays and dimensions. You only need it
directly if you are writing your own reader or want to name these shared types.
To read weather data, use [`fieldglass`](https://crates.io/crates/fieldglass)
or one of the format crates instead. This crate is part of
[Fieldglass](https://github.com/D0ubleD0uble/fieldglass).

## What it holds

The format readers are built on a common set of modules, so that GRIB, NetCDF
and Zarr data are read, placed and described the same way:

<!-- parsing-surface: the set of core modules the format crate libraries name,
     checked by tools/check_parsing_surface.py. The crate documentation states
     it again; both regions have to match the code. -->
| Module | What it is for |
|---|---|
| `error` | Reporting what went wrong with the one error type every reader returns |
| `bits`, `bytes` | Reading bits and bytes from a file, a buffer or a remote source |
| `bitmap` | Reading GRIB's bitmap of which grid points have a value |
| `unpacking` | Turning the whole numbers GRIB stores back into real values with each field's reference value and scale factors |
| `packing_groups` | Expanding the groups of values in GRIB's second-order and complex packing |
| `spatial_differencing` | Undoing the spatial differencing both GRIB editions can apply |
| `center_tables` | Naming the sub-center (the group within a weather center) that made a GRIB file, from the WMO table |
| `storage_order` | Putting a grid's values in one standard order no matter how the file stored them |
| `projection` | Placing a grid on the Earth with the map projections GRIB and NetCDF use |
| `lead_time` | Working out a GRIB field's forecast time with the rules both editions share |
| `matrix` | Reading GRIB fields that hold a small matrix of values at every grid point |
| `sht` | Turning a spectral field's wave coefficients into values on a grid |
| `healpix` | Placing the equal-area pixels of a HEALPix field on the Earth |
| `global_grid` | Choosing the one world grid that spectral and HEALPix fields are drawn onto |
| `array` | Describing a file's dimensions, attributes and arrays |
| `cf` | Following the [CF conventions](https://cfconventions.org/cf-conventions/cf-conventions.html) to find which arrays can be drawn and where a slice sits |
| `byte_shuffle` | Undoing the byte shuffle HDF5 and Zarr apply before compressing |
| `output_limits` | Limiting how much a decompressor can produce so a damaged file can't run away |
| `detect` | Recognizing a file's format from its first bytes |
<!-- /parsing-surface -->

On top of that, with features on, it holds the drawing layer (reprojecting a
field onto a map, coloring it, and placing coastlines and other lines on it)
and the analysis layer (contours, CSV export and combining two fields). Three more
modules are always there that no reader uses directly. `spatial_index` finds
the nearest cell in a grid stored as a list of cell centers, and `cf` uses it
to place those grids. `units` tidies unit strings for display, and
`vector_pair` tells whether two fields are the two halves of one wind or
current. The `fieldglass` crate uses `units` and `vector_pair`.

This crate also defines the traits where you plug in your own behavior: where
the bytes come from (`ByteSource` for one file, `ObjectSource` for a store of
named objects), how a projected grid turns a latitude and longitude back into
a row and column (`PlanarGridProjector`), and, with drawing on, how a field is
reprojected onto a picture (`TargetProjection`, `ForwardMap`).

## Feature flags

| Feature | Default | What it adds |
|---|---|---|
| `render` | on | Reprojecting, coloring and placing lines on a picture. Map projections themselves are always available. |
| `analysis` | on | Contours, CSV export and combining two fields. It doesn't need `render`. |
| `serde` | on | Saving and loading the geometry, color and placement types with serde. |
| `fs` | on | Recognizing a file's format from a path on disk. Turn it off for a target with no file system, such as a browser. |
| `testing` | off | Byte sources that behave the way a transport does, for testing readers. Only tests turn it on. |

Turn off the ones you don't need with `default-features = false`, and your
program won't include their code.

## License

MIT OR Apache-2.0

## AI disclosure

Much of this crate's code, tests and documentation was written with AI
assistance (Anthropic's Claude), under the maintainer's direction and review.

### How do I maintain quality?

I treat the AI as a fast contributor whose work I don't take on trust:

- Fieldglass is designed against the specifications and reference libraries
  are used to check against the resulting behaviors. If behaviors differ
  between Fieldglass and a reference, Fieldglass follows the specification and
  records why.
- Every example comes from as close to real-world files as possible while also
  trying not to bloat the size of our libraries with data. In
  `fieldglass-core`, the map projections are compared point by point against
  PROJ, the standard projection library.
- I take a personal hand in testing each feature and delivery before a new tag
  is released. I maintain a testing plan in which I test each change.
- I have many strict automatic checks for formatting, lints, the test suite,
  dependency and security scans, and fuzzing with damaged or hostile files to
  make sure we refuse them safely.

### Where do I currently notice gaps?

- I dislike the current documentation. There's a lot of it, and it's hard to
  understand. I am undertaking efforts to rewrite the user-facing documentation
  with my own words. This file has been rewritten by me.
- While I have been ensuring proper functionality, behaviors, and larger-picture
  dependency graphs, I haven't been reading over every line of code. As the
  shape of things starts to become a bit more stable, I hope to review more of
  the code closely, but to be totally transparent this is more of a 'desire'
  than a reality currently.
