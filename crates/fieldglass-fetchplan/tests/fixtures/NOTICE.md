# Test fixture provenance

Two kinds of fixture live here, with different provenance.

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

**Four GRIB messages** the sidecars point at (`gfs_0p25_rh_2mb.grib2`,
`gefs_chem_0p25_pmtc_sfc.grib2`, `ecmwf_ifs_0p25_r_50hpa.grib2` and
`ecmwf_ifs_0p25_mx2t3.grib2`) used to live here. Only the umbrella crate reads
them, so they moved to `crates/fieldglass/tests/fixtures/` with their
provenance (#1132), and this crate's package no longer carries them.

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
template 4.48 (optical properties of aerosol), which the reader parses since
#1130.

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

### `ecmwf_ifs_0p25_enfo_ep_tpg100.index`

One line of
`https://ecmwf-forecasts.s3.amazonaws.com/20260904/00z/ifs/0p25/enfo/20260904000000-240h-enfo-ep.index`,
ECMWF's open-data mirror on AWS, fetched on 2026-10-21: line 799 of 1,480,
the `tpg100` record at step `12-36`, unmodified, with its newline. The same
run as `ecmwf_ifs_0p25_oper.index`, from the ensemble's probability product
(`stream` `enfo`, `type` `ep`). The whole index is 327,485 bytes and only this
record is read, so only this line is committed; its `_offset` and `_length`
still address the mirror's object. The message it points at is
`crates/fieldglass/tests/fixtures/ecmwf_ifs_0p25_tpg100.grib2`. SHA-256
`ead9e9233b24c138c95a8a1c123edf71957db94b0e06e58de0d9880aebce0369`.

CC BY 4.0, as above. Attribution: ECMWF.

### `ecmwf_ifs_0p25_waef_ep_swhg8.index`

One line of
`https://ecmwf-forecasts.s3.amazonaws.com/20260904/00z/ifs/0p25/waef/20260904000000-240h-waef-ep.index`,
fetched on 2026-10-21: line 66 of 80, the `swhg8` record at step `12`,
unmodified, with its newline. The same run, from the wave ensemble's
probability product (`stream` `waef`, `type` `ep`). Only this record is read,
so only this line is committed; its `_offset` and `_length` still address the
mirror's object. The message it points at is
`crates/fieldglass/tests/fixtures/ecmwf_ifs_0p25_waef_swhg8.grib2`. SHA-256
`c78ff3b61eb1cff228a66fc531836412d45268619fa0b8e0bfc44fe43eaadae2`.

CC BY 4.0, as above. Attribution: ECMWF.

### `ecmwf_aifs_single_0p25_oper_mcc.index`

One line of
`https://ecmwf-forecasts.s3.amazonaws.com/20260904/00z/aifs-single/0p25/oper/20260904000000-6h-oper-fc.index`,
fetched on 2026-10-21: line 61 of 122, the AIFS-single `mcc` record at step
`6`, `levtype` `sfc`, unmodified, with its newline. Only this record is read,
so only this line is committed; its `_offset` and `_length` still address the
mirror's object. The message it points at is
`crates/fieldglass/tests/fixtures/ecmwf_aifs_single_0p25_mcc.grib2` (#1323).
SHA-256 `69a9d8f1a1be6ded5b255adf8fd014761fc140b733d230bde5ffca751f616208`.

CC BY 4.0, as above. Attribution: ECMWF.

### `ecmwf_aifs_ens_0p25_cf_hcc.index`

One line of
`https://ecmwf-forecasts.s3.amazonaws.com/20260904/00z/aifs-ens/0p25/enfo/20260904000000-6h-enfo-cf.index`,
fetched on 2026-10-21: line 105 of 108, the AIFS-ens control forecast's `hcc`
record at step `6`, `levtype` `sfc`, unmodified, with its newline. Only this
record is read, so only this line is committed. The message it points at is
`crates/fieldglass/tests/fixtures/ecmwf_aifs_ens_0p25_cf_hcc.grib2` (#1323).
SHA-256 `c53302b9962554971de48cb8091f21b2fed25efdca4e757151c23948220bcc78`.

CC BY 4.0, as above. Attribution: ECMWF.

### `ecmwf_ifs_0p25_enfo_ef_pf.index`

Two lines of
`https://ecmwf-forecasts.s3.amazonaws.com/20260904/00z/ifs/0p25/enfo/20260904000000-6h-enfo-ef.index`,
fetched on 2026-11-01: lines 2,031 and 2,950 of 8,500, the IFS ensemble's
perturbed-member (`type` `pf`) records of `asn` member 40 and `ro` member
48 at step `6`, unmodified, each with its newline, in that order. The whole
index is 1,995,780 bytes and only these records are read. The messages they
point at are `crates/fieldglass/tests/fixtures/ecmwf_ifs_0p25_pf_asn_40.grib2`
and `ecmwf_ifs_0p25_pf_ro_48.grib2` (#1304). SHA-256
`08ac49aa3720fb89e2c6fbb0635e005d67770d33e01e345fba0a6a8931392276`.

CC BY 4.0, as above. Attribution: ECMWF.

### `ecmwf_ifs_0p25_enfo_ef_cf.index`

One line of
`https://ecmwf-forecasts.s3.amazonaws.com/20250101/00z/ifs/0p25/enfo/20250101000000-6h-enfo-ef.index`,
fetched on 2026-11-01: line 1,010 of 6,171, the IFS ensemble's control
forecast (`type` `cf`) record of `ro` at step `6`, unmodified, with its
newline. The whole index is 1,442,055 bytes and only this record is read, so
only this line is committed. The message it points at is
`crates/fieldglass/tests/fixtures/ecmwf_ifs_0p25_cf_ro.grib2` (#1552).
SHA-256 `5ed8dfb8057ff037fa01ceb91e98c5572ec0fb3e1e93ec0af0cde0c47952f870`.

CC BY 4.0, as above. Attribution: ECMWF.

### `ecmwf_ifs_0p25_enfo_ep_ws250.index`

Two lines of the same run's
`https://ecmwf-forecasts.s3.amazonaws.com/20260904/00z/ifs/0p25/enfo/20260904000000-240h-enfo-ep.index`
(the object `ecmwf_ifs_0p25_enfo_ep_tpg100.index` is a line of), fetched on
2026-11-01: lines 955 and 966 of 1,480, the ensemble mean (`type` `em`) and
standard deviation (`type` `es`) of `ws` at `pl` `250`, step `228`,
unmodified, each with its newline, in that order. The messages they point at
are `crates/fieldglass/tests/fixtures/ecmwf_ifs_0p25_em_ws250.grib2` and
`ecmwf_ifs_0p25_es_ws250.grib2` (#1304). SHA-256
`63d00b40e95b940ddd14d99a4ca914a92e20c0014653acbda90175fcf63764f3`.

CC BY 4.0, as above. Attribution: ECMWF.

### `ecmwf_ifs_0p25_oper_fc_asn.index`

One line of
`https://ecmwf-forecasts.s3.amazonaws.com/20260904/00z/ifs/0p25/oper/20260904000000-6h-oper-fc.index`,
fetched on 2026-11-01: line 44 of 184, the operational forecast's (`type`
`fc`) record of `asn` at step `6`, unmodified, with its newline. The whole
index is 40,022 bytes and only this record is read, so only this line is
committed. The message it points at is
`crates/fieldglass/tests/fixtures/ecmwf_ifs_0p25_fc_asn.grib2` (#1553).
SHA-256 `a82e7a9d6adb223f310402336ab212bca50e7f67ca98349517a3cc4a29a158a3`.

CC BY 4.0, as above. Attribution: ECMWF.

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
