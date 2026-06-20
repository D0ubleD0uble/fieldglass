# Test fixture provenance

## `cmc_wind_300_2010052400_p012.grib`

Single-message GRIB1 file from the Canadian Meteorological Centre regional
model (wind speed at 300 hPa, polar-stereographic 60 km grid, 2010-05-24
00Z + 12 h). Originally distributed with the [pygrib sample data
set](https://github.com/jswhit/pygrib/tree/master/sampledata) (MIT-licensed,
J. Whitaker).

## `ecmwf_lfpw_msg0.grib1`

First message extracted from a 64-message ECMWF GRIB1 file
(`ecmwf_lfpw.grib1`) — geopotential at 50 hPa, 240 × 121 lat-long grid,
2006-12-10 18Z + 24 h, encoded with `grid_second_order` (SPD order 2,
boustrophedonic, general-extended). Used to pin the complex-packing
variant detection and (in a follow-up) as the decode oracle for the
second-order packing implementation.

The file was sourced from another open-source application's test corpus
and is believed to be redistributable. If you are the rights-holder and
this is in error, please [open an issue](https://github.com/D0ubleD0uble/fieldglass/issues)
and we will replace it with a synthesised equivalent.

## `ecmwf_lfpw_msg0_expected.json`

Decoder oracle: counts, min/max/mean, and 12 anchored sample values dumped
from the fixture above by `grib_get_data` (eccodes 2.34.1) on
2026-05-09. Tolerance for value comparison is recorded in the file
itself.

## `ecmwf_spd3_msg0.grib1` + `ecmwf_spd3_msg0_expected.json`

`ecmwf_lfpw_msg0.grib1` re-encoded by eccodes 2.34.1 into the third-order
spatial-differencing variant (`grid_second_order_SPD3`):

```
grib_set -r -s boustrophedonicOrdering=0,packingType=grid_second_order in.grib1 tmp
# orderOfSPD is read-only; the SPD3 packingType encodes order 3 directly:
grib_set -r -s boustrophedonicOrdering=0,packingType=grid_second_order_SPD3 in.grib1 ecmwf_spd3_msg0.grib1
```

The decoded field is identical to the SPD-2 fixture (same values, re-packed
at a different SPD order); the `.json` is its `grib_get_data` oracle. This
exercises the `orderOfSPD = 3` read path (three SPD seeds + bias). Boustrophedonic
ordering is turned off because eccodes' encoder mis-counts points when
re-packing this boustrophedonic source. eccodes 2.34.1 refuses to *encode* the
`no_SPD` / `SPD1` orders ("array too small") and the `row_by_row` /
`constant_width` / `general_grib1` layouts ("not implemented") — these are
**encoder** limitations; eccodes still *decodes* all of them, which is what the
hand-built fixtures below rely on.

## `ecmwf_spd3_boust_msg0.grib1` + `ecmwf_spd3_boust_msg0_expected.json`

`ecmwf_spd3_msg0.grib1` with the boustrophedonic flag bit set on in place —
BDS extended-flag octet (byte 13 of section 4), bit 6 (`0x04`), toggled `0x1b →
0x1f`; no other byte changed:

```python
b[offsetSection4 + 13] |= 0x04
```

eccodes can't *encode* a boustrophedonic SPD-3 file (ECC-1402: the encoder
miscounts `numericValues` by `orderOfSPD − 2` when re-packing — observed as
`numberOfPoints=29040 != sizeOf(numericValues)=29041`), but it *decodes* the
byte-edited file without complaint and applies the odd-row reversal. The `.json`
is its `grib_get_data` oracle. This is the only fixture exercising the
boustrophedonic row-reversal **combined with** the order-3 inverse: 14 400
values (the 60 odd rows × 240 columns) differ from the non-boustrophedonic
fixture, and each odd row is the exact reverse of its non-boustrophedonic
counterpart (e.g. `boust[240]` == `nonboust[479]`). The decoded field is no
longer the meteorological original; the bytes are what matter.

## `hand_second_order_no_SPD.grib1` / `hand_second_order_SPD1.grib1` (+ `_expected.json`)

Hand-assembled general-extended second-order BDS streams for the two SPD orders
eccodes won't encode, spliced onto the real IS/PDS/GDS of
`ecmwf_spd3_msg0.grib1` (240×121 lat-long, no bitmap) with the IS total-length
and BDS section-length octets recomputed. Built to the wire layout in eccodes'
`grib1/data.grid_second_order_no_SPD.def` / `data.grid_second_order_SPD1.def`
(identical to `grid_second_order.def` except the SPD block, read only
`if (orderOfSPD)`), cross-checked against `crates/fieldglass-grib1/src/packing/second_order.rs`.

- **no_SPD** (`orderOfSPD = 0`): two zero-width groups, `firstOrderValues`
  100 and 200, `groupLengths` 14520 + 14520. Order 0 applies no inverse
  differencing → first 14 520 points are 100, the rest 200.
- **SPD1** (`orderOfSPD = 1`): `widthOfSPD = 8`, seed = 0, bias = 0; one group,
  `groupWidth = 1`, `firstOrderValues = 1`, `groupLength = 29039`, all
  second-order raw values 0 → reconstructed `X[i] = 1`. The order-1 inverse is
  the cumulative sum starting at the seed, so 0 followed by 29 039 ones yields
  the ramp 0, 1, 2, …, 29039.

eccodes 2.34.1 *decodes* both (the encode refusal is encoder-only), so the
`grib_get_data` oracles in the `.json` files are independent of fieldglass; the
fields are also exactly hand-computable. The PDS/GDS metadata is inherited from
the geopotential source and does not describe these synthetic fields — only the
decode path is under test.

## `hand_second_order_{row_by_row,constant_width,general_grib1}.grib1` (+ `_expected.json`)

Hand-assembled BDS streams for the three "classic" (pre-ECMWF-extended) WMO
second-order packings — which eccodes 2.34 refuses to *encode* ("not
implemented") but still *decodes* — spliced onto the same 240×121 IS/PDS/GDS.
Built to the wire layout in eccodes'
`grib1/data.grid_second_order_{row_by_row,constant_width,general_grib1}.def`
and the matching `DataG1SecondOrder*Packing::unpack` reference sources (WMO
No. 306 Vol I.2 FM 92 GRIB1; ECMWF
<https://codes.ecmwf.int/grib/format/grib1/packing/3/>). These have **no SPD**;
the grid is split into groups, each expanded to `firstOrderValues[g] + residual`
(or a run of `firstOrderValues[g]` when the group width is 0).

- **row_by_row** (`secondaryBitmapPresent = 0`, `secondOrderOfDifferentWidth = 1`):
  one group per row (implied bitmap), per-row widths in `groupWidths[121]`. Even
  rows are zero-width (every point = `r*10`), odd rows width-4 (`r*10 + c%16`).
- **constant_width** (`secondaryBitmapPresent = 1`, `secondOrderOfDifferentWidth = 0`):
  an explicit secondary bitmap (1 bit per point marks each group start) and a
  single shared `groupWidth = 4`. 121 groups of 240, `firstOrderValues` g*100,
  residual `n%16` ⇒ `value[n] = (n/240)*100 + n%16`.
- **general_grib1** (`secondaryBitmapPresent = 1`, `secondOrderOfDifferentWidth = 1`):
  variable-length groups delimited by the secondary bitmap, plus per-group
  widths. 120 groups alternating length 200/284 (sum 29040); even groups
  zero-width (`firstOrderValues` g*50), odd groups width-5 (residual = within-group
  offset %32).

All three are pinned to `grib_get_data` oracles and are exactly hand-computable;
again only the decode path is under test, not the inherited geopotential metadata.

## `ieee32_cmc_wind.grib1` / `ieee64_cmc_wind.grib1` (+ `_expected.json`)

`cmc_wind_300_2010052400_p012.grib` re-encoded by eccodes 2.34.1 into the
`grid_ieee` raw IEEE-754 float packing, at both precisions:

```
grib_set -r -s packingType=grid_ieee,precision=1 cmc_wind_300_2010052400_p012.grib ieee32_cmc_wind.grib1
grib_set -r -s packingType=grid_ieee,precision=2 cmc_wind_300_2010052400_p012.grib ieee64_cmc_wind.grib1
```

`grid_ieee` stores values verbatim as big-endian IEEE floats (precision 1 →
32-bit, 2 → 64-bit), no reference/scale transform; the BDS header gains a
`precision` octet (octet 12) and the stream begins at octet 13. The
`_expected.json` files are the `grib_get_data` oracle (counts, min/max/mean, and
anchored samples). The 32-bit oracle carries f32 rounding, so the test tolerance
is `1e-3`. eccodes returns `GRIB_NOT_IMPLEMENTED` for precision 3 (128-bit), and
so do we. See eccodes `grib1/data.grid_ieee.def` + `DataRawPacking`.

## `matrix_simple_cmc_wind.grib1` (+ `_expected.json`)

`cmc_wind_300_2010052400_p012.grib` re-encoded as `packingType=grid_simple_matrix`:

```
grib_set -r -s packingType=grid_simple_matrix cmc_wind_300_2010052400_p012.grib matrix_simple_cmc_wind.grib1
```

eccodes emits the `matrixOfValues = 0` form — a plain simple-packed body behind
the 13-octet matrix sub-header (`octetAtWichPackedDataBegins`, `extendedFlag`,
`NR`, `NC`, `NC1`, `NC2`, the coordinate flags) — so the decoded field equals the
original. `_expected.json` is its `grib_get_data` oracle. See eccodes
`grib1/data.grid_simple_matrix.def`.

## `reduced_gg_n32.grib1`

The eccodes 2.34.1 `reduced_gg_pl_32_grib1.tmpl` sample (an N32 reduced —
quasi-regular — Gaussian GRIB1 grid: 64 rows, per-row `PL` widths 20…128…20,
6114 stored points) with every value set to a non-zero constant so the decode
exercises the reference-value path on a real reduced GDS:

```
grib_set -d 285.5 /usr/share/eccodes/samples/reduced_gg_pl_32_grib1.tmpl reduced_gg_n32.grib1
```

`grib_get_data` is the decode oracle: 6114 points, every value 285.5,
`packingType = grid_simple`, widest row 128. The reduced GDS parse (dims, `PL`
list, bounds) and the row-widening expansion are pinned separately by unit
tests in `src/gds.rs` and `crates/fieldglass-napi/src/lib.rs`; this fixture pins
the reader's native-count (`sum(PL)`) decode integration end to end.

## `hand_matrix_of_values.grib1`

A hand-assembled `grid_simple_matrix` message with `matrixOfValues = 1` — a true
`NR×NC` matrix at every grid point. **eccodes 2.34.1 can neither encode nor
decode this variant**: `grib_set packingType=grid_simple_matrix` only ever
produces the `matrixOfValues = 0` form, and feeding it a real matrix message
makes the `data_g1secondary_bitmap` accessor abort (`m <= secondary_len`
assertion). So there is no `grib_get_data` oracle; the decoder is validated
against eccodes' `grib1/data.grid_simple_matrix.def` + `DataG1SecondaryBitmap` /
`data_apply_bitmap` accessor sources and a hand-computed expectation.

Construction (a small Python builder; IS/PDS/GDS reused from the eccodes
`regular_ll_sfc_grib1.tmpl` sample shrunk to 16×31 = 496 points):

- Section 1 (PDS) `section1Flags` set to `0xC0` (GDS + BMS present).
- A primary BMS marking all 496 grid points present.
- BDS section 4, `grid_simple_matrix`, octet-4 flag `0x10`
  (`additionalFlagPresent`), `bitsPerValue = 8`, `referenceValue = 0`,
  `binaryScaleFactor = 0`, `decimalScaleFactor = 0`.
- `octetAtWichPackedDataBegins` (octets 12–13) = `N` = 496 (number of present
  grid points); `extendedFlag` (octet 14) = `0x0C`
  (`matrixOfValues | secondaryBitmapPresent`); `NR = 1`, `NC = 2` (datum size 2);
  `NC1 = NC2 = 0` (no coordinate coefficients).
- Secondary bitmaps: `N · NR·NC = 992` bits, all set (every matrix cell present).
- Coded values: 992 simple-packed 8-bit integers, value `k = k % 256`.

With nothing masked and `R/E/D = 0`, decoded matrix value at flat index `k`
equals `k % 256` — exactly hand-computable. The bitmap-masked code paths
(absent grid points, clear secondary bits) are covered by unit tests on
`expand_matrix` in `src/packing/matrix.rs`.
