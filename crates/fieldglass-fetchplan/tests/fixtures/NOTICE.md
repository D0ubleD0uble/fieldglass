# Test fixture provenance

Three kinds of fixture live here, with different provenance.

**The GRIB sidecars in this directory** are **real, unmodified manifest
sidecars** fetched from a public endpoint on 2026-09-06, for the 2026-09-04 00Z
cycle. They are text, a few kilobytes each, and they are committed verbatim —
not trimmed, not regenerated — because the whole point of the corpus is that the
parser is held to what the producers actually emit rather than to what this repo
believes they emit. Three of the shapes the parser handles (`n.m` sub-messages,
the numeric `var discipline=…` fallback name, probability qualifiers) were
discovered by reading these files, not by reading a specification.

No credentials are needed for any of these endpoints. A sidecar carries offsets
and labels, not data.

One sidecar, `gefs_chem_a2d_0p25.idx`, was fetched later, on 2026-10-17, for
the same cycle; its section says so.

**Four GRIB messages** are the bytes a record of a committed sidecar points
at, so a plan can be checked against what it fetches: `gfs_0p25_rh_2mb.grib2`
from `gfs_0p25.idx`, `gefs_chem_0p25_pmtc_sfc.grib2` from
`gefs_chem_a2d_0p25.idx`, and `ecmwf_ifs_0p25_r_50hpa.grib2` and
`ecmwf_ifs_0p25_mx2t3.grib2` from `ecmwf_ifs_0p25_oper.index`. Their sections
below say how each was taken.

**The chunk-addressing fixtures in [`zarr/`](zarr/)** are generated, and the
section at the end of this file says by what and from where.

## NOAA / NCEP sidecars (public domain)

Works of the U.S. Government, not subject to copyright protection in the United
States (17 U.S.C. § 105), and redistributed by the NOAA Open Data Dissemination
(NODD) program on AWS Open Data with no restrictions.

### `gfs_0p25.idx`

`https://noaa-gfs-bdp-pds.s3.amazonaws.com/gfs.20260904/00/atmos/gfs.t00z.pgrb2.0p25.f000.idx`

GFS 0.25° global, analysis. 696 records — the largest sidecar here, and the one
that exercises the range arithmetic at scale. Note that this product has **no**
`n.m` sub-messages, contrary to what one might assume from the issue text; the
sub-message case is covered by `nam_awip12.idx` below, which does.

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

### `hrrr_conus_wrfsfc.idx`

`https://noaa-hrrr-bdp-pds.s3.amazonaws.com/hrrr.20260904/conus/hrrr.t00z.wrfsfcf00.grib2.idx`

HRRR CONUS surface, analysis. 170 records. Carries wgrib2's **numeric fallback
name** on record 3 — `var discipline=0 center=7 local_table=1 parmcat=16
parm=201` — which is the one case where a `.idx` line states WMO codes outright.

### `nam_awip12.idx`

`https://noaa-nam-pds.s3.amazonaws.com/nam.20260904/nam.t00z.awip1200.tm00.grib2.idx`

NAM 12 km CONUS, analysis. 196 records, 20 of them **`n.m` sub-messages** —
`UGRD`/`VGRD` and `USTM`/`VSTM` pairs packed two fields to a message, each pair
sharing one offset. This is the fixture that proves two records can be one
fetch.

### `nbm_core_co.idx`

`https://noaa-nbm-grib2-pds.s3.amazonaws.com/blend.20260904/00/core/blend.t00z.core.f001.co.grib2.idx`

National Blend of Models CONUS core, +1 h. 154 records with **probability
qualifiers** — `prob <304.8`, `prob fcst 3/7`, `probability forecast` — which
are what make one abbreviation and level ambiguous, and so are what the
"ambiguous matches are all returned" rule is tested against.

### `gefs_p01_0p50.idx`

`https://noaa-gefs-pds.s3.amazonaws.com/gefs.20260904/00/atmos/pgrb2ap5/gep01.t00z.pgrb2a.0p50.f000.idx`

GEFS ensemble member 1, 0.5°, analysis. 71 records, every one carrying the
**member qualifier** `ENS=+1`. It is also the one product here whose lines have
**no trailing colon**, which is why the parser drops empty trailing fields
rather than assuming a terminator.

### `gefs_chem_a2d_0p25.idx`

`https://noaa-gefs-pds.s3.amazonaws.com/gefs.20260904/00/chem/pgrb2ap25/gefs.chem.t00z.a2d_0p25.f000.grib2.idx`

GEFS-Aerosols 0.25° two-dimensional fields, analysis. Fetched on 2026-10-17,
unmodified, 3,833 bytes, SHA-256
`acd2463f31ae0b58e578cf0d78f16a2caf2009311b308587b995d374ee5552bd`. 32 records,
each carrying wgrib2's aerosol qualifiers (`aerosol=Dust dry`,
`aerosol_size <1e-05`, `aerosol_wavelength …`). eccodes 2.34.1 (`grib_get -p
productDefinitionTemplateNumber`) reads every one of its messages as product
template 4.48 (optical properties of aerosol), which the reader does not parse.

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
(#1126): a correct line on a template the reader does not parse, whose level
was refused as a stale sidecar before that issue.

## ECMWF sidecar (CC-BY-4.0)

### `ecmwf_ifs_0p25_oper.index`

`https://data.ecmwf.int/forecasts/20260904/00z/ifs/0p25/oper/20260904000000-0h-oper-fc.index`

ECMWF IFS 0.25° operational forecast, step 0. 187 JSON-lines records with
`_offset` **and** `_length`, so every range off this dialect is exact. Carries
the MARS key vocabulary (`class`, `stream`, `type`, `expver`, `domain`,
`levtype`, `levelist`, `step`) the cross-dialect matching is written against.

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

## Zarr and kerchunk fixtures (`zarr/`, generated)

Written by `tools/build_zarr_fixtures.py --plan-only`, from the committed
stores under `crates/fieldglass-zarr/tests/fixtures/`, which that same script
writes with `zarr-python` 3.3.0 and `numcodecs` 0.16.5. Regenerate them with
the script rather than editing them by hand.

There is no public reference document to fetch here, and that is the honest
position rather than a shortcut: a real kerchunk index points into a specific
multi-gigabyte object, so committing one would mean committing either the
archive it addresses or an index whose ranges nothing can check. These are
generated so that the ranges are checkable — the object they address is
committed beside them, 160 bytes of it.

Nothing in this directory is third-party material. The array behind every one
of them is `numpy.arange(24) * 0.5`.

### `v2_zarray.json`, `v3_zarr.json`

The metadata documents of the `raw` and `v3_bytes` stores, copied byte for byte
apart from a trailing newline the repo's end-of-file hook wants, so the two
editions' chunk-grid and chunk-key fields are read from what `zarr-python`
actually writes rather than from prose about the spec.

### `temp.bin`

The four chunk objects of the `zstd` store laid end to end, with seven bytes of
`0xa5` between them. Two deliberate choices:

* **The gaps.** A real reference document points into a file that was never a
  Zarr store — a NetCDF4 or GRIB archive, whose chunks are separated by headers
  the planner never sees. Laid out contiguously, a planner that multiplied a
  chunk index by a length would pass the test; with gaps, the offset the
  document states is the only way to be right. The filler is not zero, so an
  off-by-one range fails to decode rather than yielding plausible numbers.
* **zstd rather than raw.** A raw chunk is little-endian float32, so a seam
  test over one would pass against a reader that never called the codec crate.

### `kerchunk_refs.json`, `kerchunk_templates.json`

Version 1 reference documents over `temp.bin`, carrying the two shapes a `refs`
value takes — an inline string for each metadata document, and a
`[url, offset, length]` triple for each chunk. The second writes the URL once
under `templates` and refers to it as `{{u}}` from every chunk, which is what
kerchunk emits over a single archive; the two must plan identically.

The URL they name (`s3://example-bucket/temp.bin`) does not resolve and is not
meant to. It is opaque to the planner, which hands the string back for a host to
fetch, and the seam test's stand-in host maps it to `temp.bin` beside it.

### `kerchunk_gen.json`

A document with a `gen` block: references generated from jinja2 expressions over
a dimension rather than stated. The planner refuses this by name, and this
fixture is what holds it to refusing rather than quietly planning the `refs` it
can see and omitting everything `gen` would have produced.
