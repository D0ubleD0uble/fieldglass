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
