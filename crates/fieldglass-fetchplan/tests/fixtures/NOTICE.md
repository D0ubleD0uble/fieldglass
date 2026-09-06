# Test fixture provenance

Every file here is a **real, unmodified manifest sidecar** fetched from a public
endpoint on 2026-09-06, for the 2026-09-04 00Z cycle. They are text, a few
kilobytes each, and they are committed verbatim — not trimmed, not
regenerated — because the whole point of the corpus is that the parser is held
to what the producers actually emit rather than to what this repo believes they
emit. Three of the shapes the parser handles (`n.m` sub-messages, the numeric
`var discipline=…` fallback name, probability qualifiers) were discovered by
reading these files, not by reading a specification.

No credentials are needed for any of these endpoints. Nothing here is a GRIB
message; a sidecar carries offsets and labels, not data.

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
