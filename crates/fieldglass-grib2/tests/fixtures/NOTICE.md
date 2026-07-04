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
