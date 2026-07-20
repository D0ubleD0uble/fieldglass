# Test fixture provenance

## `rotated_latlon_surface.grib2`

Copied verbatim from the eccodes distribution's encoding samples
(`samples/rotated_ll_sfc_grib2.tmpl`). Single message, GDS template **3.1**
(rotated latitude/longitude) on a 16×31 grid with a rotated southern pole,
centre 98 (ECMWF). A constant 2-m temperature field (DRS 5.0, bitsPerValue 0).
eccodes and its samples are released under the Apache 2.0 license.

## `polar_stereographic_surface.grib2`

Copied verbatim from the eccodes distribution's encoding samples
(`samples/polar_stereographic_sfc_grib2.tmpl`). Single message, GDS template
**3.20** (polar stereographic) on a 16×31 grid, centre 98 (ECMWF). A constant
field (DRS 5.0, bitsPerValue 0). eccodes and its samples are released under
the Apache 2.0 license.

## `reduced_gaussian_pressure_level.grib2`

Sourced from the public ECMWF eccodes test data corpus
(<https://get.ecmwf.int/test-data/eccodes/data/>). Single message, GDS
template **3.40** (Gaussian latitude/longitude — reduced variant), centre 98
(ECMWF), reference time 2008-02-06T12:00:00Z. eccodes is released under the
Apache 2.0 license; the test data is bundled with the eccodes distribution.

## `gfs_c255_latlon.grib2`

Sourced from the public ECMWF eccodes test data corpus as `gfs.c255.grib2`
(<https://get.ecmwf.int/test-data/eccodes/data/gfs.c255.grib2>). Single
message of NCEP GFS output, GDS template **3.0** (regular latitude/longitude),
0.5° global grid (10512 points). NOAA NCEP-produced GRIB2 data is U.S.
government work and in the public domain; the eccodes corpus redistribution
is under Apache 2.0.

## `regular_latlon_surface.grib2`

Sourced verbatim from the public ECMWF eccodes test data corpus
(<https://sites.ecmwf.int/repository/eccodes/test-data/data/regular_latlon_surface.grib2>).
Single message of 2-metre temperature on a coarse 16×31 regular lat/lon
grid, GDS template **3.0**, PDS template **4.0**, **DRS template 5.0
(simple packing)**, R ≈ 270 K. Used by the §5–§7 decode tests as a
small, fully simple-packed end-to-end fixture (gfs_c255 uses complex
packing 5.3, eta_lambert / reduced_gaussian use 5.0 but at larger
grids). eccodes is released under the Apache 2.0 license.

## `eta_lambert_msg0.grib2`

First GRIB2 message extracted from `eta.grb` in the
[`pygrib` sample-data corpus](https://github.com/jswhit/pygrib/tree/master/sampledata)
(`https://raw.githubusercontent.com/jswhit/pygrib/master/sampledata/eta.grb`).
NOAA Eta-model output (NAM predecessor), GDS template **3.30** (Lambert
Conformal), 12-km CONUS grid. NOAA Eta is U.S. government work in the public
domain; pygrib redistributes the file under its 3-Clause BSD license. Only
the first message is retained to keep the fixture small (10 KB vs. 920 KB
for the original multi-message file).

## `hrrr_complex_spd_lambert.grib2`

Real NOAA/NCEP HRRR surface-temperature message, fetched from the public
`noaa-hrrr-bdp-pds` AWS Open Data bucket
(`hrrr.<date>/conus/hrrr.t00z.wrfsfcf00.grib2`, the `TMP:surface` field
extracted by byte range). GDS template **3.30** (Lambert Conformal), a 1799×1059
3-km CONUS grid (1,905,141 points), DRS template **5.3**
(`grid_complex_spatial_differencing`) with `orderOfSpatialDifferencing = 2`.
This is the packing/grid combination NCEP ships for HRRR and NAM today — both
moved off JPEG 2000 to complex packing — so it is the real counterpart to the
re-encoded 5.3 fixtures and the first real order-2 case on a Lambert grid. NOAA
NCEP output is U.S. government work in the public domain.
`hrrr_complex_spd_lambert_expected.json` is its eccodes 2.34.1 value+§5 oracle.

## `ecmwf_ccsds_latlon.grib2`

Real ECMWF open-data IFS 2-metre-temperature message, fetched from the public
ECMWF open-data endpoint (<https://data.ecmwf.int/forecasts/>, the `2t` step-0
field extracted by byte range from the `.index` sidecar). GDS template **3.0**
(regular latitude/longitude), a 1440×721 0.25° global grid (1,038,240 points),
DRS template **5.42** (`grid_ccsds` / libaec). This is the packing ECMWF ships
for all gridded open data (IFS cycle 48r1 onward), so it confirms the pure-Rust
AEC decoder handles a real ECMWF codestream rather than only re-encoded
fixtures. ECMWF real-time open data is published under CC-BY-4.0 (attribution:
"Generated using Copernicus/ECMWF open data").
`ecmwf_ccsds_latlon_expected.json` is its eccodes 2.34.1 value+§5 oracle.

## `ieee32_regular_latlon.grib2` / `ieee64_regular_latlon.grib2` (+ `ieee64_regular_latlon_expected.json`)

`regular_latlon_surface.grib2` re-encoded by eccodes 2.34.1 into the IEEE
floating-point packing (DRS template **5.4**, `grid_ieee`), at both precisions:

```
grib_set -s packingType=grid_ieee,precision=1 regular_latlon_surface.grib2 ieee32_regular_latlon.grib2
grib_set -s packingType=grid_ieee,precision=2 regular_latlon_surface.grib2 ieee64_regular_latlon.grib2
```

Template 5.4 stores each value verbatim as a big-endian IEEE float (precision
1 → 32-bit, 2 → 64-bit) with no reference/binary/decimal-scale transform.
Because the source field was already quantised by simple packing to values
that are f32-exact, the 32-bit and 64-bit fixtures decode to byte-identical
fields — both are kept so the test exercises the f32 and f64 read paths.
`ieee64_regular_latlon_expected.json` is the `grib_get_data` oracle (count,
min/max/mean, anchored samples); decode tolerance is recorded in the file.
eccodes returns `GRIB_NOT_IMPLEMENTED` for precision 3 (128-bit), and so do
we. See eccodes `grib2/template.5.4.def` + `grib_accessor_class_data_raw_packing`.

## §5 packing fixtures and eccodes oracles

These cover the §5 Data Representation templates exercised by the GRIB2 decode
work. 5.2 (`grid_complex`) decodes today and `complex_regular_latlon.grib2` is
its decode regression fixture (`tests/decode_complex.rs`); the rest are staged
for their decode implementations and are listed as ❌ in the
[GRIB2 packing modes](../../../../../README.md#grib2-packing-modes) table. Each
was produced by re-encoding an already-bundled fixture with eccodes 2.34.1
`grib_set`, so reproduction is deterministic and no new upstream file is added
(the source files' provenance/licences are documented above). Each fixture
ships two oracles, both generated with eccodes 2.34.1:

- `<name>.grib2.eccodes.ref.json` — curated §0–§6 metadata snapshot from
  `tools/regenerate-eccodes-snapshots.py` (the shared `eccodes_reference.rs`
  harness; metadata + §5 template number cross-check green).
- `<name>_expected.json` — the **decode target**: `grib_get_data` value stats
  (count, missing, min/max/mean, anchored samples, absolute tolerance) plus a
  `section5` block capturing every §5 packing parameter eccodes reports
  (group counts/widths/lengths, spatial-differencing order, codec flags, …).
  This is the ground truth the decoders validate against without needing
  eccodes in the sandbox.

| Fixture | DRS template | `packingType` | Issue |
|---|---|---|---|
| `complex_regular_latlon.grib2` | 5.2 | `grid_complex` | decodes today (#107); decode regression fixture |
| `complex_spd2_regular_latlon.grib2` | 5.3 (2nd-order) | `grid_complex_spatial_differencing` | #109 |
| `gfs_c255_latlon.grib2` (existing) | 5.3 (1st-order) | `grid_complex_spatial_differencing` | #109 |
| `jpeg2000_regular_latlon.grib2` | 5.40 | `grid_jpeg` | #116 |
| `png_eta_lambert.grib2` | 5.41 | `grid_png` | #118 |
| `ccsds_regular_latlon.grib2` | 5.42 (16-bit) | `grid_ccsds` | #117 |
| `ccsds_regular_latlon_8bit.grib2` | 5.42 (8-bit) | `grid_ccsds` | #117 |
| `ccsds_regular_latlon_24bit.grib2` | 5.42 (24-bit) | `grid_ccsds` | #117 |

```
# 5.2 complex (group-split, no differencing)
grib_set -s packingType=grid_complex regular_latlon_surface.grib2 complex_regular_latlon.grib2
# 5.3 complex + 2nd-order spatial differencing (order-1 is the real gfs_c255 below)
grib_set -s packingType=grid_complex_spatial_differencing,orderOfSpatialDifferencing=2 \
  regular_latlon_surface.grib2 complex_spd2_regular_latlon.grib2
# 5.40 JPEG 2000 (lossless, typeOfCompressionUsed=0)
grib_set -s packingType=grid_jpeg regular_latlon_surface.grib2 jpeg2000_regular_latlon.grib2
# 5.42 CCSDS / AEC (libaec)
grib_set -s packingType=grid_ccsds regular_latlon_surface.grib2 ccsds_regular_latlon.grib2
# 5.42 CCSDS at 8- and 24-bit sample widths — derived from the 16-bit fixture.
# `-r` forces a real repack (without it, grib_set only relabels §5 and leaves
# the 16-bit §7 stream in place, producing an inconsistent message).
grib_set -r -s bitsPerValue=8  ccsds_regular_latlon.grib2 ccsds_regular_latlon_8bit.grib2
grib_set -r -s bitsPerValue=24 ccsds_regular_latlon.grib2 ccsds_regular_latlon_24bit.grib2
# 5.41 PNG — see note below on the source choice
grib_set -s packingType=grid_png eta_lambert_msg0.grib2 png_eta_lambert.grib2
```

Notes on the choices:

- **1st- vs 2nd-order differencing (#109).** The already-committed
  `gfs_c255_latlon.grib2` is real NCEP GFS data packed as 5.3 with
  `orderOfSpatialDifferencing = 1`; `gfs_c255_latlon_expected.json` is its new
  value+§5 oracle. `complex_spd2_regular_latlon.grib2` supplies the 2nd-order
  case, so the decoder is exercised against both orders.
- **CCSDS bit widths (#117).** The same field is packed at 8, 16, and 24 bits
  so the three fixtures exercise all three AEC option-ID-length code paths
  (`id_len` 3 / 4 / 5). 24-bit is the wide-sample / multi-byte path ECMWF uses
  for many operational fields (ECMWF moved all gridded GRIB2 output to CCSDS in
  IFS cycle 48r1), so it is the most important to pin. All three decode
  byte-for-byte against the eccodes oracle.
- **PNG source (#118).** eccodes' libpng *writer* rejects the small
  `regular_latlon_surface` field at its native bit depth, and forcing a low
  `bitsPerValue` there clips the value range. `eta_lambert_msg0.grib2` PNG-packs
  at its native 13-bit depth with a full-fidelity round-trip (decoded range
  matches the simple-packed source exactly), so it's the PNG fixture. Grid type
  is irrelevant to the §5/§7 PNG decode path (decode is decoupled from grid
  geometry), so a Lambert grid is fine here.
- **Operational corpus.** The real HRRR (5.3 / Lambert) and ECMWF open-data
  (5.42 / lat-lon) messages documented above are the named-model counterparts to
  these re-encoded fixtures, added under #123 so the "renders real-world files"
  claim is pinned per model, not just per packing. Remaining real-model coverage
  (MRMS, and models still on JPEG 2000) is tracked there and exercised via the
  manual sample corpus (`samples/`, `tools/fetch_samples.sh`) rather than
  committed fixtures.

See eccodes `grib2/template.5.{2,3,40,41,42}.def` and the matching
`grib_accessor_class_data_*` packing classes.

## Complex-packing missing-value / row-by-row fixtures (#217)

Four derived fixtures pin the 5.2 / 5.3 inline missing-value management and
row-by-row group-splitting decode paths. All are deterministic re-encodes or
single-byte patches of already-bundled fixtures (source provenance above), and
each ships the same two eccodes 2.34.1 oracles as the §5 fixtures above.

| Fixture | DRS template | Envelope | Issue |
|---|---|---|---|
| `complex_mvm1_regular_latlon.grib2` | 5.2 | missing-value management 1 (primary) | #217 |
| `complex_spd2_mvm1_regular_latlon.grib2` | 5.3 (2nd-order) | missing-value management 1 | #217 |
| `complex_mvm2_regular_latlon.grib2` | 5.2 | missing-value management 2 (primary + secondary) | #217 |
| `complex_rowbyrow_regular_latlon.grib2` | 5.2 | group-splitting method 0 (row by row) | #217 |

The two management-1 fixtures re-encode `regular_latlon_surface.grib2` with
`grib_filter`, setting 46 of the 496 values to the default `missingValue`
(9999): index 5, the run 40–79, and 130, 131, 260, 388, 495 — a long run (so
the encoder emits whole-missing groups) plus scattered singles (so missing
points are embedded inside normal-width groups):

```
# rules (values elided; the 9999 entries are at the indexes listed above)
set packingType="grid_complex";                       # or grid_complex_spatial_differencing
# set orderOfSpatialDifferencing=2;                   # 5.3 fixture only
set values={ ... };
write "complex_mvm1_regular_latlon.grib2";
```

eccodes' `grid_complex` encoder detects the missing entries and sets
`missingValueManagementUsed = 1` itself (`DataG22OrderPacking::pack`); it never
emits management 2 or row-by-row splitting, so those two fixtures are
single-byte §5 patches — the sanctioned hand-built-fixture / decode-as-oracle
strategy for envelopes eccodes can decode but not encode:

- `complex_mvm2_regular_latlon.grib2`: `complex_mvm1_regular_latlon.grib2`
  with §5 octet 23 (`missingValueManagementUsed`) patched 1 → 2. eccodes'
  *decode* of the patched message is the oracle: two extra points whose packed
  increment equals the secondary sentinel (`2^width − 2`) become missing
  (48 total), confirming the fixture really exercises the secondary path.
- `complex_rowbyrow_regular_latlon.grib2`: `complex_regular_latlon.grib2`
  with §5 octet 22 (`groupSplittingMethodUsed`) patched 1 → 0. The §7 group
  structure is self-describing, so eccodes decodes the patched message
  identically to the original (verified byte-for-byte via `grib_get_data`);
  the fixture pins that our decoder accepts method 0 rather than erroring.

## Complex-packing NG == 0 constant-field fixtures (#222)

Two derived fixtures pin the `numberOfGroupsOfDataValues == 0` constant-field
decode (eccodes ECC-2095): every point equals the §5 reference value verbatim,
with no `2^E · 10^-D` transform and nothing read from §7.

| Fixture | DRS template | Envelope | Issue |
|---|---|---|---|
| `complex_ng0_regular_latlon.grib2` | 5.2 | NG = 0 (constant field) | #222 |
| `complex_spd2_ng0_regular_latlon.grib2` | 5.3 (2nd-order) | NG = 0 (constant field) | #222 |

Each is a byte patch of the matching committed fixture
(`complex_regular_latlon.grib2` / `complex_spd2_regular_latlon.grib2`; source
provenance above): §5 octets 32–35 (`numberOfGroupsOfDataValues`) zeroed, §7
truncated to its bare 5-octet header (no group blocks — for 5.3 not even the
spatial-differencing extra descriptors), and the §7 length and §0
`totalLength` recomputed to match. The patch is scripted:
`tools/build_grib2_ng0_fixtures.py` rebuilds both fixtures deterministically
from the committed sources.

**Oracle version caveat:** ECC-2095 shipped in eccodes **2.42.0**, so the
otherwise-pinned 2.34.1 cannot be the value oracle here — it predates the fix
and mis-decodes NG == 0 (for the 5.3 fixture it reads past the truncated §7
and returns garbage without erroring). The `<fixture>_expected.json` value
oracles were therefore decoded with eccodes **2.47.3**
(`codes_get_values`, via the `eccodes` PyPI wheel): all 496 points equal the
reference value 270.466796875 exactly, for both fixtures. The
`.eccodes.ref.json` metadata snapshots remain 2.34.1
(`tools/regenerate-eccodes-snapshots.py`), which reads §0–§6 only and is
unaffected.

## Run-length packing fixtures (#301)

Two hand-built fixtures pin the run-length packing decode (DRS template
**5.200**, `grid_run_length`): runs of quantised level indices resolved through
a level → value table, level 0 marking missing.

| Fixture | DRS template | Exercises | Issue |
|---|---|---|---|
| `runlength_regular_latlon.grib2` | 5.200 (8-bit) | multi-digit base-`range` run, level-0 missing | #301 |
| `runlength_4bit_regular_latlon.grib2` | 5.200 (4-bit) | sub-byte codes, base-10 multi-digit runs, single-point runs, negative decimal scale (raw byte 129 → −1) | #301 |

eccodes 2.34.1 (the pin) **decodes** `grid_run_length` but cannot be coaxed
into **encoding** it from the CLI — its run-length encoder only accepts values
that already fall exactly on a preset level table, which `grib_set` has no way
to establish. So these are hand-built by `tools/build_grib2_runlength_fixtures.py`
(the sanctioned hand-built-fixture / decode-as-oracle path): §0–§4 are reused
verbatim from `regular_latlon_surface.grib2`, and §5/§6/§7/§8 are synthesised —
a template-5.200 §5, a no-bitmap §6 (run-length encodes missing as level 0, so
no §6 bitmap is needed), and a run-length §7. §7 streams are kept a whole number
of bytes, because a real encoder writes `floor(bits/8)` bytes and a partial
trailing code would otherwise be invented on decode.

The `<fixture>_expected.json` value oracles are eccodes **2.34.1**
`grib_get_data` / `grib_get` (count, missing count, min/max/mean over present
points, anchored samples, and the §5 run-length parameters); the builder
asserts eccodes decodes exactly the field it encoded before writing them. The
`.eccodes.ref.json` metadata snapshots are 2.34.1 as usual.

## Log-preprocessing fixtures (#305)

Two fixtures pin the log-preprocessing decode (DRS template **5.61**,
`grid_simple_log_preprocessing`): simple packing of the log-transformed field,
decoded as simple unpacking then `Y = exp(X) − B` with `B` the §5
`preProcessingParameter`.

| Fixture | DRS template | Exercises | Issue |
|---|---|---|---|
| `log_regular_latlon.grib2` | 5.61 | `B = 0` branch (`Y = exp(X)`) | #305 |
| `log_negative_regular_latlon.grib2` | 5.61 | `B ≠ 0` branch (`Y = exp(X) − B`) | #305 |

eccodes 2.34.1 (the pin) both **encodes and decodes** this template, so — like
the CCSDS fixtures — both are eccodes re-encodings of
`regular_latlon_surface.grib2`, built by
`tools/build_grib2_log_preprocessing_fixtures.py`, with eccodes' decode as the
value oracle. `log_regular_latlon.grib2` re-packs the field directly
(`grib_set -r -s packingType=grid_simple_log_preprocessing`); the field is
all-positive, so the encoder chooses `preProcessingParameter = 0`.
`log_negative_regular_latlon.grib2` first shifts the field by −300 K
(`grib_set -s offsetValuesBy=-300`) so it holds non-positive values, which
drives the encoder to a non-zero `preProcessingParameter` and exercises the
subtraction branch.

The `<fixture>_expected.json` value oracles are eccodes 2.34.1 `grib_get_data` /
`grib_get`; because the decode reconstructs values through `exp()`, their
tolerance is `0.01` rather than the linear packings' `0.001`. The
`.eccodes.ref.json` metadata snapshots are 2.34.1 as usual.

The WMO note flags 5.61 as experimental ("not validated … bilateral tests
only"), and there is no known operational producer; a second independent oracle
(e.g. wgrib2) would strengthen the cross-check but was not available in this
environment. The decode is a small, deterministic transform over the
already-validated simple-packing path, and both the eccodes decode and the
closed-form `exp`/`exp − B` arithmetic (see the `ds.rs` unit tests) agree.

## Pre-standard local-template fixtures (#307)

Two local-use (49152+) data-representation templates whose §5/§7 are identical
to a registered image packing, so they decode through the same codec:

| Fixture | DRS template | Decodes via | Oracle | Issue |
|---|---|---|---|---|
| `jpeg2000_local_40000.grib2` | 5.40000 | JPEG 2000 (5.40) | eccodes `grid_jpeg` | #307 |
| `png_local_40010.grib2` | 5.40010 | PNG (5.41) | 5.41 decode of the same §7 | #307 |

Each is the matching committed image fixture with only its §5
data-representation-template number relabelled (octets 10–11), by
`tools/build_grib2_local_template_fixtures.py`; the §7 codestream is untouched.
`jpeg2000_local_40000.grib2` comes from `jpeg2000_regular_latlon.grib2`
(40 → 40000) and `png_local_40010.grib2` from `png_eta_lambert.grib2`
(41 → 40010).

`template.5.40000.def` is literally `include template.5.40.def`, so eccodes
decodes 5.40000 as `grid_jpeg` and is the value oracle (and the
`.eccodes.ref.json` metadata snapshot). **eccodes has no `template.5.40010.def`
and cannot decode that file at all** (it errors with "No final 7777"), which is
the whole point — it is a genuinely exceed-eccodes decode. Its value oracle is
therefore eccodes' decode of the *original* `png_eta_lambert.grib2` (5.41),
whose §7 is byte-identical, and it is excluded from
`tools/regenerate-eccodes-snapshots.py` (see `ECCODES_UNDECODABLE`) since no
eccodes snapshot can be produced.

The ECMWF second-order local templates 5.50001 / 5.50002 (`grid_second_order`)
in the same issue are a separate, larger codec; see the section below.

## Second-order packing fixtures (#307)

Three fixtures pin the second-order (general-extended) packing decode — the
GRIB1 `grid_second_order` codec carried into GRIB2 across templates **5.50002**
(`grid_second_order`) and **5.50001** (`grid_second_order_no_boustrophedonic`).
Unlike run-length or log-preprocessing, eccodes 2.34.1 **can** CLI-encode this
packing, so the fixtures are repacked from `regular_latlon_surface.grib2` by
`tools/build_grib2_second_order_fixtures.py` rather than hand-built:

| Fixture | DRS template | `packingType` | Exercises | Issue |
|---|---|---|---|---|
| `second_order_regular_latlon.grib2` | 5.50002 | `grid_second_order` | common case, `boustrophedonicOrdering = 0`, orderOfSPD = 2 | #307 |
| `second_order_no_boust_regular_latlon.grib2` | 5.50001 | `grid_second_order_SPD2` | no `secondOrderFlags` octet | #307 |
| `second_order_boust_regular_latlon.grib2` | 5.50002 | `grid_second_order` | `boustrophedonicOrdering = 1` (alternating-row) | #307 |

```sh
# 5.50002 grid_second_order (the common case, boustrophedonicOrdering=0)
grib_set -r -s packingType=grid_second_order \
  regular_latlon_surface.grib2 second_order_regular_latlon.grib2
# 5.50001 grid_second_order_no_boustrophedonic (no secondOrderFlags octet)
grib_set -r -s packingType=grid_second_order_no_boustrophedonic \
  regular_latlon_surface.grib2 second_order_no_boust_regular_latlon.grib2
# 5.50002 with boustrophedonicOrdering=1 — flip the flag WITHOUT -r so eccodes
# reverses the odd rows on decode (a pure metadata flip, not a repack).
grib_set -s secondOrderFlags=128 \
  second_order_regular_latlon.grib2 second_order_boust_regular_latlon.grib2
```

Each `<name>_expected.json` carries the **full** eccodes `grib_get_data` decode
(all 496 values, in scan order) plus the §5 parameters and summary stats, so
`tests/decode_second_order.rs` asserts value-for-value agreement. The
boustrophedonic fixture's oracle is eccodes' own decode of the flag-flipped
message: `grib_set` without `-r` only sets the `secondOrderFlags` byte and
leaves the §7 stream in place, so eccodes applies `data_apply_boustrophedonic`
(reverse odd rows) on decode — the exact path Fieldglass must match.

## Spectral (spherical-harmonic) fixture (#302)

`spectral_simple_t63.grib2` pins the spherical-harmonic decode (§3 template
3.50 + §5 template 5.50, `spectral_simple`). It is eccodes 2.34.1's own
`sh_sfc_grib2.tmpl` sample (a T63 surface field, `J = K = M = 63`, 4160 stored
values = `(63+1)·(63+2)`) re-encoded to `spectral_simple` with
`grib_set -r -s packingType=spectral_simple`; ECMWF ships the sample under the
Apache License 2.0. `bitsPerValue = 16`; the real `(0,0)` coefficient is stored
out of band in §5 (`realPartOf00`) and the rest are simple-packed in §7.

`spectral_simple_t63.eccodes.ref.txt` is the 4160 coefficients as eccodes
decodes them, one per line. A spectral message has no grid, so `grib_get_data`
prints a bare `Value` column (no latitude/longitude) — this is a coefficient
oracle, not a gridded one. Regenerate with:

```sh
grib_get_data spectral_simple_t63.grib2 | tail -n +2 \
  > spectral_simple_t63.eccodes.ref.txt
```

The `.eccodes.ref.json` metadata snapshot is 2.34.1 as usual; it exercises
§0–§4 parsing for a message that carries no `Ni`/`Nj` and no earth shape.

`spectral_complex_t63.grib2` pins the complex spectral decode (§5 template
5.51). It is eccodes 2.34.1's own `sh_pl_grib2.tmpl` sample (a T63
pressure-level field, `J = K = M = 63`, sub-truncation `KS = 20`,
`bitsPerValue = 16`, Laplacian `P = 0.722`, 4160 stored values), copied
verbatim; ECMWF ships it under the Apache License 2.0. Its coefficient oracle
`spectral_complex_t63.eccodes.ref.txt` is `grib_get_data | tail -n +2` as above,
and the `.eccodes.ref.json` snapshot is 2.34.1. The §7 has two parts — an
unpacked IEEE-float sub-truncation and a Laplacian-rescaled simple-packed
remainder — so this fixture exercises the whole `decode_spectral_complex` path.

## Bi-Fourier packing fixtures (#304)

`bifourier_ellipse_keepaxes.grib2`, `bifourier_diamond_no_axes.grib2`,
`bifourier_rectangle_keepaxes.grib2`, and `bifourier_ellipse_ieee32.grib2` pin
the bi-Fourier spectral decode (§3 template 3.63 `lambert_bf` + §5 template
5.53 `bifourier_complex`) — the ACCORD/ALADIN/AROME limited-area form. They are
**round-trips**: bi-Fourier has no public data and the eccodes CLI cannot set a
coefficient array, so each is built by
`tools/build_grib2_bifourier_fixtures.py` from eccodes' own
`lambert_bf_grib2.tmpl` sample (Apache License 2.0) with a chosen truncation
geometry and a synthetic coefficient array.

Encoder/oracle version split (as for the second-order fixtures, see #307):

- **Encoded** with the eccodes **Python wheel** (libeccodes **2.48.0**), because
  only the array-set API (`codes_set_values`) can install the coefficient
  array; the CLI has no equivalent.
- **Value oracle** `<name>.eccodes.ref.txt` is `grib_get_data <f> | tail -n +2`
  from the **pinned CLI eccodes 2.34.1**, which decodes the wheel-encoded bytes.
  Bi-Fourier's only known decode fix (ECC-1207, empty non-packed truncation) was
  fixed in 2.21.0, before the 2.34.1 pin, so the pin is a valid value oracle.

The four cover the three truncation shapes (ellipse, diamond, rectangle),
`biFourierPackingModeForAxes` both set and clear, and both unpacked-subset float
precisions (`unpackedSubsetPrecision` 2 = IEEE 64-bit, the sample default, and
1 = IEEE 32-bit). Regenerate all four with
`python3 tools/build_grib2_bifourier_fixtures.py`.

## Matrix-of-values fixture (#306)

`matrix_simple_regular_latlon.grib2` pins the flat form of the matrix-of-values
decode (§5 template 5.1, `grid_simple_matrix`, `matrixBitmapsPresent = 0`). It is
eccodes' `GRIB2.tmpl` sample re-encoded with `packingType = grid_simple_matrix`,
`NR = 2`, `NC = 3`, `bitsPerValue = 12`, and a synthetic value array. Template
5.1 is experimental and the CLI cannot set a values array, so it is encoded with
the eccodes **Python wheel** (2.48.0); the value oracle
`matrix_simple_regular_latlon.eccodes.ref.txt` is the value column of
`grib_get_data` from the pinned **CLI 2.34.1**. In this flat form eccodes stores
one simple-packed value per grid point (`numberOfCodedValues = numberOfDataPoints`,
the `NR`/`NC` metadata not expanding the data), so the pinned CLI is a full value
oracle. Regenerate with `python3 tools/build_grib2_matrix_fixtures.py`.

## Spectral-render oracle (#303)

`spectral_render_t63.oracle.txt` is the inverse-spherical-harmonic-transform
oracle for the T63 field: the grid values of `spectral_simple_t63.grib2`
synthesized onto a fixed 5° regular lat/lon grid (37 lats × 72 lons). eccodes
**cannot** synthesize a grid from spectral coefficients (its geoiterator returns
"not yet implemented" and re-packing to `grid_simple` fails), so — unlike every
other fixture here — there is no eccodes oracle. Instead the field is computed
directly from ECMWF's definitive spectral definition (the fully-normalized
associated-Legendre recurrence and the real-field reconstruction) by
`tools/build_grib2_spectral_render_oracle.py`, taking the committed coefficient
oracle `spectral_simple_t63.eccodes.ref.txt` as input. The formula was
cross-validated three ways during development: exact analytic single-coefficient
cases (`P̄_0^0 = 1`, `P̄_1^0 = √3·μ`, `P̄_2^0 = √5·(3μ²−1)/2`), the fields those
produce, and an independent pyshtools synthesis that reproduces this field to
~5·10⁻⁸ once the ECMWF complex coefficients are mapped to pyshtools real
coefficients (`m > 0` carries a `√2` complex→real factor, imaginary part negated)
and scaled by `√(4π)`. Regenerate with
`python3 tools/build_grib2_spectral_render_oracle.py` (needs numpy).
