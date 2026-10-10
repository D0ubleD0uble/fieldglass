# HEALPix coordinate goldens: provenance

`healpix_n{2,4}_{ring,nested}.grib2.coords.json` hold the pixel centres, in
pixel order, of four GRIB2 §3.150 (HEALPix) fixtures in `fieldglass-grib2`:
Nside 2 and 4, each in RING and NESTED ordering. `tests/healpix.rs` checks
every pixel `fieldglass-core` places against them. They are the test's only
input from outside this crate, so they live here, beside it, and the published
crate carries them (#926).

Both the GRIB2 files and these goldens are written by
`tools/build_grib2_healpix_fixtures.py` in the Fieldglass repository. eccodes
ships no HEALPix sample, so each GRIB2 file starts from eccodes' stock `GRIB2`
sample with `gridType` switched to `healpix` and `Nside`, `ordering` and
`longitudeOfFirstGridPoint` (45°) set.

The centres come from eccodes, and each file names the version that produced
it in its `oracle` field:

- **RING** from the pinned eccodes 2.34.1 command-line tool,
  `grib_get_data -L "%.9f %.9f"`.
- **NESTED** from the `eccodes` PyPI wheel (2.48) geoiterator, because
  2.34.1's geoiterator refuses NESTED ordering ("Only ring ordering is
  supported").

eccodes is released under the Apache 2.0 license. The files are generated;
do not edit them by hand.
