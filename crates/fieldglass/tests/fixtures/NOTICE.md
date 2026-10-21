# Test fixture provenance

Ten real GRIB messages, each the bytes a record of a sidecar in
`crates/fieldglass-fetchplan/tests/fixtures/` points at, so a plan can be
checked against what it fetches. They live here because the umbrella crate
reads them and `fieldglass-fetchplan` does not, which keeps them out of the
fetchplan package (#1132). The sidecars they were taken from stay with
fetchplan, whose `NOTICE.md` carries their provenance. Each section below says
how the message was taken.

## NOAA / NCEP messages (public domain)

Works of the U.S. Government, not subject to copyright protection in the United
States (17 U.S.C. § 105), and redistributed by the NOAA Open Data Dissemination
(NODD) program on AWS Open Data with no restrictions.

### `gfs_0p25_rh_2mb.grib2`

Record 107 of `gfs_0p25.idx` (`RH:2 mb:anl`), fetched on 2026-10-16 from the
object that sidecar describes,
`https://noaa-gfs-bdp-pds.s3.amazonaws.com/gfs.20260904/00/atmos/gfs.t00z.pgrb2.0p25.f000`,
with `Range: bytes=76032983-76033192`: from the record's offset to one byte
before record 108's. Unmodified, 210 bytes, SHA-256
`55e074b163aec826a51805cd1a8414c79c331585f16707c0c4c10882f1e700cd`. The live
sidecar was fetched again the same day and is byte-identical to the committed
one, so the offsets are the ones this message sits at.

It was picked for its size. Relative humidity at 2 hPa is zero everywhere, so
the message is a constant field of 1,038,240 points in complex packing with
spatial differencing (5.3), one of the smallest records in the product; most
are hundreds of kilobytes. eccodes 2.34.1 (`grib_ls`) reads it as `r` on `isobaricInhPa` level 2,
2026-09-04 00Z, minimum and maximum 0. The browser host's planner test
(`crates/fieldglass-wasm/tests/node/fetchplan.mjs`) and the umbrella's
(`crates/fieldglass/tests/fetch_plan_wire.rs`) select this record, build its
`Range` header, verify its envelope on these bytes and decode it (#1081).

### `gefs_chem_0p25_pmtc_sfc.grib2`

Record 22 of `gefs_chem_a2d_0p25.idx` (`PMTC:surface:anl:aerosol=Dust dry:aerosol_size <1e-05:`),
fetched on 2026-10-17 from the object that sidecar describes,
`https://noaa-gefs-pds.s3.amazonaws.com/gefs.20260904/00/chem/pgrb2ap25/gefs.chem.t00z.a2d_0p25.f000.grib2`,
with `Range: bytes=13921007-14137554`: from the record's offset to one byte
before record 23's. Unmodified, 216,548 bytes, SHA-256
`411c4d3228ebe1df466fe4782cd097eda7731e8a82ef2a7799747e15d89ce9b4`. It is the
smallest record in the product; the whole object was read once to find it and
was not kept.

eccodes 2.34.1 (`grib_dump -O`, `grib_get`) reads it as product template 4.48,
discipline 0, category 13 (aerosols), NCEP-local number 192 (`pmtc`,
particulate matter coarse), constituent type 62001, size below 10 × 10⁻⁶ m,
no wavelength interval, first fixed surface 1 (ground or water surface), second
surface missing, 2026-09-04 00Z, step 0, 1,038,240 values, minimum 0.000127789,
maximum 1359.05. Packing is complex with spatial differencing (5.3).

It is the aerosol half of `crates/fieldglass/tests/verify_message_real_pairs.rs`
(#1126): a correct line whose level was refused as a stale sidecar before that
issue, on a template the reader did not parse then. Since #1130 the reader
parses 4.48, and the same file pins what it reads (`message_identity.rs`)
against this `grib_dump -O` output from eccodes 2.34.1:

```text
10        parameterCategory = 13 [Aerosols (grib2/tables/2/4.1.0.table) ]
11        parameterNumber = 192 [Unknown code table entry (grib2/tables/2/4.2.0.13.table) ]
12-13     constituentType = 62001 [Unknown code table entry () ]
14        typeOfSizeInterval = 0 [Below lower limit (grib2/tables/2/4.91.table) ]
15        scaleFactorOfFirstSize = 6
16-19     scaledValueOfFirstSize = 10
20        scaleFactorOfSecondSize = 0
21-24     scaledValueOfSecondSize = 0
25        typeOfWavelengthInterval = 255 [Missing (grib2/tables/2/4.91.table) ]
36        typeOfGeneratingProcess = 2 [Forecast (grib2/tables/2/4.3.table) ]
42        indicatorOfUnitOfTimeRange = 1 [Hour (grib2/tables/2/4.4.table) ]
43-46     forecastTime = 0
47        typeOfFirstFixedSurface = 1 [Ground or water surface (grib2/tables/2/4.5.table , grib2/tables/local/kwbc/1/4.5.table) ]
48        scaleFactorOfFirstFixedSurface = 0
49-52     scaledValueOfFirstFixedSurface = 0
53        typeOfSecondFixedSurface = 255 [Missing (grib2/tables/2/4.5.table , grib2/tables/local/kwbc/1/4.5.table) ]
```

The real-pair tests accept it against record 22 and refuse it against records
21, 23, 24 and 25 of the same cycle (another name, or for 24 the same name and
size of another aerosol type) and against record 26's level.

### `nbm_core_co_apcp_prob.grib2`, `nbm_core_co_apcp.grib2`

Lines 51 and 68 of `nbm_core_co.idx`, fetched on 2026-10-20 from the object
that sidecar describes,
`https://noaa-nbm-grib2-pds.s3.amazonaws.com/blend.20260904/00/core/blend.t00z.core.f001.co.grib2`.
The live sidecar was fetched again the same day and is byte-identical to the
committed one. Each runs from the record's offset to one byte before the next
record's, unmodified:

| File | Line | `Range` | Bytes | SHA-256 |
|---|---|---|---|---|
| `nbm_core_co_apcp_prob.grib2` | 51, `APCP:surface:0-1 hour acc fcst:prob >0.254:prob fcst 255/255` | `bytes=65070481-66012448` | 941,968 | `c9bb065e43611d6204354f18f7e37328f3af94d112848998606539e8b45656d8` |
| `nbm_core_co_apcp.grib2` | 68, `APCP:surface:0-1 hour acc fcst:` | `bytes=80562882-82139117` | 1,576,236 | `2e72ef84e17a1f95e22580b467b6becd6c5e10b5a804ffa41576e389eda9e4ed` |

eccodes 2.34.1 (`grib_ls`, `grib_get`) reads both as `tp` (discipline 0,
category 1, number 8) on the ground (surface type 1), 2026-09-04 00Z, step
range 0-1 accumulated (statistical process 1, one hour), 3,744,965 values. The
first is product template 4.9, probability type 1 (above the upper limit),
upper limit scale factor 3 and scaled value 254, so above 0.254 kg m-2, values
0 to 100; the second is template 4.8, values 0 to 39.116.

They are the probability half of
`crates/fieldglass/tests/verify_message_real_pairs.rs` (#1117): two lines with
the same name, level, window and cycle that only the probability block tells
apart. They are the pair the issue names.

### `gefs_p01_0p50_icetk.grib2`

Record 63 of `gefs_p01_0p50.idx` (`ICETK:surface:anl:ENS=+1`), fetched on
2026-10-21 from the object that sidecar describes,
`https://noaa-gefs-pds.s3.amazonaws.com/gefs.20260904/00/atmos/pgrb2ap5/gep01.t00z.pgrb2a.0p50.f000`,
with `Range: bytes=12095249-12119362`: from the record's offset to one byte
before record 64's. Unmodified, 24,114 bytes, SHA-256
`2067184a3c45a4bcd2cc73a0533451dde68accdd91beed6081833069b2b54d5a`. The live
sidecar was fetched again the same day and is byte-identical to the committed
one. It is the smallest record in the sidecar.

eccodes 2.34.1 (`grib_get`) reads it as product template 4.1, type of
ensemble forecast 3 (positive perturbation), perturbation number 1 of 30,
discipline 10, category 2, number 1 (ice thickness), on the ground (surface
type 1), 2026-09-04 00Z, step 0, 259,920 values, minimum 0, maximum 5, packed
complex with spatial differencing (5.3).

It is the ensemble half of `crates/fieldglass/tests/verify_message_real_pairs.rs`
(#1117): `ENS=+1` against the member block, and the line without it.

## ECMWF messages (CC-BY-4.0)

ECMWF open data is published under the Creative Commons Attribution 4.0
International licence (CC BY 4.0); see
<https://www.ecmwf.int/en/forecasts/datasets/open-data>. Attribution: ECMWF.

### `ecmwf_ifs_0p25_r_50hpa.grib2`, `ecmwf_ifs_0p25_mx2t3.grib2`

Two records of `ecmwf_ifs_0p25_oper.index`, fetched on 2026-10-17 from the
object that index describes. `data.ecmwf.int` keeps only the last few days, so
both came from ECMWF's open-data mirror on AWS, which keeps the archive:
`https://ecmwf-forecasts.s3.amazonaws.com/20260904/00z/ifs/0p25/oper/20260904000000-0h-oper-fc.grib2`.
The mirror's copy of the index
(`…/20260904000000-0h-oper-fc.index`) was fetched the same day and is
byte-identical to the committed one, so the offsets are the ones these
messages sit at. Each is the record's `_offset` for its `_length`, unmodified:

| File | Record | `Range` | Bytes | SHA-256 |
|---|---|---|---|---|
| `ecmwf_ifs_0p25_r_50hpa.grib2` | line 97, `r`, `pl` `50` | `bytes=68405631-68487398` | 81,768 | `db174329abf6972c8240f6518c28888967778943b37dd4a46c94ac491659eeb6` |
| `ecmwf_ifs_0p25_mx2t3.grib2` | line 11, `mx2t3`, `sfc` | `bytes=5038129-5038352` | 224 | `c3d28075fff9505186bae947e95366c461a8308dcd23098f445669935db46597` |

eccodes 2.34.1 (`grib_ls`) reads the first as `r` on `isobaricInhPa` level 50
(first fixed surface 100, scaled value 5000 Pa), minimum 0.462906, maximum
131.463, and the second as `mx2t3` on `heightAboveGround` level 2 (surface 103),
step range 0-3, a constant field of 0. Both are CCSDS-packed (5.42), 2026-09-04
00Z.

They are the ECMWF half of `crates/fieldglass/tests/verify_message_real_pairs.rs`
(#1093). The isobaric one is the smallest `pl` record in the index; the other
is a single-level field that is not on the ground, which is what an ECMWF `sfc`
means and what a reading of `sfc` as WMO surface type 1 refuses.

### `ecmwf_ifs_0p25_mn2t3.grib2`, `ecmwf_ifs_0p25_sot_sol3.grib2`

Two more records of the same index, fetched on 2026-10-20 from the same AWS
mirror object. The mirror's index was fetched again that day and is still
byte-identical to the committed one. Each is the record's `_offset` for its
`_length`, unmodified:

| File | Record | `Range` | Bytes | SHA-256 |
|---|---|---|---|---|
| `ecmwf_ifs_0p25_mn2t3.grib2` | line 13, `mn2t3`, `sfc` | `bytes=5746311-5746534` | 224 | `83a028fc9fd13ba1c68130c321fe67390e45cd411f59b06bd7e65e8f2e228633` |
| `ecmwf_ifs_0p25_sot_sol3.grib2` | line 4, `sot`, `sol` `3` | `bytes=1418371-2043658` | 625,288 | `ab4a6e4b48f4722e767d574baf74b55d61cead50c6729bdd9a3c298c49ce803d` |

eccodes 2.34.1 (`grib_ls`, `grib_get`) reads the first as `mn2t3`: discipline
0, category 0, number 0 like `mx2t3`, product template 4.8 with statistical
process 3 (minimum) over 3 hours where `mx2t3` has 2 (maximum), surface 103 at
2 m, step range 0-3, a constant field of 0. It reads the second as `sot`
(discipline 2, category 3, number 18) on the layer between soil levels 2 and 3
(first and second fixed surface type 151, scaled values 2 and 3), which eccodes
reports as `level` 3 and MARS as `levelist` 3: eccodes aliases a soil layer's
level to its bottom (ECC-854). Minimum 216.183, maximum 325.871. Both are
CCSDS-packed (5.42), 2026-09-04 00Z, tables version 32.

They are the parameter and soil-level half of
`crates/fieldglass/tests/verify_message_real_pairs.rs` (#1116): `mx2t3` against
`mn2t3` bytes, and `sol 1` against `sol 3` bytes.

### `ecmwf_ifs_0p25_vsw_sol1.grib2`

Line 69 of the same index, `vsw` on `sol` `1`, fetched on 2026-10-21 from the
same AWS mirror object, whose index was fetched again that day and is still
byte-identical to the committed one. The record's `_offset` for its `_length`,
`Range: bytes=45216327-45693667`, unmodified, 477,341 bytes, SHA-256
`db4171044a6a6503e9277af41e31600d7c52109ae2e481909e269950d487b2f1`.

eccodes 2.34.1 (`grib_get`) reads it as `vsw` (discipline 2, category 0,
number 25), product template 4.0, on the
layer between soil levels 0 and 1 (first and second fixed surface type 151,
scaled values 0 and 1), `level` 1, 2026-09-04 00Z, step 0, minimum 0, maximum
0.764893, CCSDS-packed (5.42), tables version 32. The top soil layer is coded
as a layer too, so it is numbered by its bottom like `sol 3`.

It is the second soil fixture of
`crates/fieldglass/tests/verify_message_real_pairs.rs` (#1116).

### `ecmwf_ifs_0p25_10u.grib2`, `ecmwf_ifs_0p25_msl.grib2`, `ecmwf_ifs_0p25_str.grib2`

Three more records of `ecmwf_ifs_0p25_oper.index`, fetched on 2026-10-21 from
the same AWS mirror object, whose index was fetched again that day and is
still byte-identical to the committed one. Each is the record's `_offset` for
its `_length`, unmodified:

| File | Record | `Range` | Bytes | SHA-256 |
|---|---|---|---|---|
| `ecmwf_ifs_0p25_10u.grib2` | line 33, `10u`, `sfc` | `bytes=19248396-19990385` | 741,990 | `02101bd301878789754a1d151714503341878c28ef24c4a3838fe82e2cd9a0c7` |
| `ecmwf_ifs_0p25_msl.grib2` | line 3, `msl`, `sfc` | `bytes=897075-1418370` | 521,296 | `38784131f5bebfe95768cea9c38679c17b42e056ef3d82bcc0fa1358bf08840b` |
| `ecmwf_ifs_0p25_str.grib2` | line 99, `str`, `sfc` | `bytes=68948951-68949174` | 224 | `3fb2a60f07f4e1b7f23944feb6bd3f1c15fc67c1c3951c6302a9f32ae7346ef7` |

eccodes 2.34.1 (`grib_ls`) reads them as `10u` (discipline 0, category 2,
number 2, surface 103 at scaled value 10, scale factor 0, minimum -32.696,
maximum 36.7415), `msl` (0.3.0, surface 101 with its value coded missing,
minimum 95438.1, maximum 103810) and `str` (0.5.5, product template 4.8,
statistical process 1, surface 1, a constant field of 0). The first two are
template 4.0. All three are CCSDS-packed (5.42), 2026-09-04 00Z, step 0,
tables version 32. eccodes 2.48.0 (the PyPI wheel) gives the same three names.

They are the shared-codes half of
`crates/fieldglass/tests/verify_message_real_pairs.rs` (#1298): `100u` on the
`10u` bytes, `sp` on `msl`'s and `ttr` on `str`'s, each a neighbouring record
whose codes are the same and whose surface is not.

### `ecmwf_ifs_0p25_tpg100.grib2`

The `tpg100` record of the same run's ensemble probability index, committed as
`crates/fieldglass-fetchplan/tests/fixtures/ecmwf_ifs_0p25_enfo_ep_tpg100.index`,
fetched on 2026-10-21 from
`https://ecmwf-forecasts.s3.amazonaws.com/20260904/00z/ifs/0p25/enfo/20260904000000-240h-enfo-ep.grib2`.
The record's `_offset` for its `_length`, `Range: bytes=385882941-385893887`,
unmodified, 10,947 bytes, SHA-256
`820264e678665d7728723f67fdd7961448f63f41fc1ca05ee5a3adc2a8234625`.

eccodes 2.34.1 (`grib_ls`) reads it as `tpg100`: discipline 0, category 1,
number 52 (as `tp`), product template 4.9, statistical process 1, surface 1,
probability type 3 above a lower limit of scaled value 100 at scale factor 0,
step range 12-36, minimum 0, maximum 99, CCSDS-packed (5.42), 2026-09-04 00Z,
tables version 32. eccodes 2.48.0 gives the same name.

It is the probability half of
`crates/fieldglass/tests/verify_message_real_pairs.rs` (#1298): `tp`, `tpg50`
and `tpg25` against its probability block.

## Built wind fixtures

### `uv_pairs.grib2`, `uv_pairs.grib1`

Not real products. Each is a committed message repeated four times with its
parameter octets rewritten, concatenated into one file, by
`tools/build_conformance_uv_fixtures.py` (rerun it to regenerate; it needs only
the standard library). The grid and packed values are the source message's
and are never read by what they test, which is `vector_pairing` and the
conformance suite's `vectorPairing` cases (#1205): no committed GRIB file holds
a wind component, and the suite names a fixture file rather than building bytes
in memory as `tests/vector_pairing.rs` does.

| File | Source | Messages |
|---|---|---|
| `uv_pairs.grib2` | `gfs_c255_latlon.grib2` (NOAA/NCEP, public domain) | 0 UGRD (0-2-2), 1 VGRD (0-2-3), 2 wind speed (0-2-1), 3 as committed |
| `uv_pairs.grib1` | `j_consecutive_latlon.grib1` (built, see the GRIB1 `NOTICE.md`) | 0 `10u` (165), 1 `10v` (166), 2 `10si` (207), 3 as committed (`2t`, 167) |
