# Fieldglass roadmap

*Adopted 2026-07-19. Picks up where the original phase plan (phases 0–5:
GRIB1/GRIB2/NetCDF parse, decode, and rendering) left off — that plan is now
essentially complete.*

## Strategy

Fieldglass's core value-add is **breadth of file support**: it should open more
of the meteorological data that exists in the wild than any other viewer — not
just what Panoply and eccodes can open. That is the first priority. The second
priority is **complete, human-readable parameter and code tables** across
centres, because a decoded number without a name and units is only half useful.

Two facts from the standards landscape shape the plan:

1. **The GRIB2 packing space is frozen.** Code Table 5.0 registers exactly 13
   data-representation templates; nothing has been added since 5.53, and all
   current WMO activity is §4 product-definition templates and Table 4.2
   parameters. GRIB edition 3 is shelved (repo archived, experimental-use only).
   A complete §5 implementation is therefore a *finishable* goal, and "decodes
   every registered GRIB2 packing, pure Rust, zero build flags" is a durable
   claim no C-stack tool makes (stock eccodes needs JasPer/OpenJPEG and libaec,
   and ships with PNG support off by default).
2. **eccodes' registered-template decode coverage is complete on paper**, so the
   way to exceed it is: (a) real matrix-of-values semantics (5.1 and the GRIB1
   form — eccodes only emits a flat stream), (b) no build-flag conditionals,
   (c) local templates it lacks (DRT 5.40010, NCEP grids 3.204 / 3.32768), and
   (d) *rendering* what today only CLIs can decode — spectral fields, RLE
   radar, native ICON, HEALPix. No viewer in the ecosystem does those.

## Staying current — watch list

Where new templates, packings, and format changes are proposed and announced:

| Channel | What lands there | Cadence |
|---|---|---|
| [wmo-im/GRIB2 issues](https://github.com/wmo-im/GRIB2/issues) + [releases](https://github.com/wmo-im/GRIB2/releases) | New GRIB2 templates/parameters are proposed as issues, batched into `FTyyyy-1/-2` milestones, released as tagged CSVs (MIT). The operational source of truth — codes.wmo.int lags it. | 2 fast-track cycles/yr (May + Nov) |
| [eccodes History of Changes](https://confluence.ecmwf.int/display/ECC/History+of+Changes) | Best single signal for "newly decodable in the reference stack"; also table-version pickup | ~5–6 releases/yr |
| [netcdf-c releases](https://github.com/Unidata/netcdf-c/releases) | New filters (zstd landed in 4.9), NCZarr direction | ~2/yr |
| [HDF5 releases](https://github.com/HDFGroup/hdf5/releases) | Format changes — 2.0.0 (Nov 2025) added the `H5T_COMPLEX` class, unreadable by older readers | ~2/yr |
| [cf-conventions releases](https://github.com/cf-convention/cf-conventions/releases) + [standard-name table](https://cfconventions.org/Data/cf-standard-names/current/src/cf-standard-name-table.xml) | CF conventions (annual, Dec) and standard names (several/yr) | annual / several per yr |
| [DWD definitions bundle](https://opendata.dwd.de/weather/lib/grib/) | DWD's eccodes-definition tarballs, sometimes fresher than upstream eccodes | ad hoc |

A twice-yearly checkpoint after each WMO fast-track publication (May/June and
November) is the natural rhythm for table regeneration and census review.

## Phase 6 — Finish the GRIB2 §5 census ✅ complete (2026-07-20)

**Done.** Every registered §5 Data Representation template (Code Table 5.0) now
decodes, plus the pre-standard local templates — so Fieldglass "decodes every
registered GRIB2 packing, pure Rust, zero build flags," the durable claim no
C-stack tool makes. See the [GRIB2 packing modes](README.md#grib2-packing-modes)
table for the shipped status. The table below records the census plan and the
validation path taken for each; for the three eccodes cannot help with (it
crashes on the true matrix, cannot synthesise spectral grids, ships no 5.40010
definition) correctness was pinned to the definitive spec and independent
implementations.

Registered set (Code Table 5.0): 5.0–5.4, 5.40, 5.41, 5.42, 5.50, 5.51, 5.53,
5.61, 5.200 — all now supported. The census, ordered by wild-data value:

| Template | What / who uses it | Validation path |
|---|---|---|
| **5.200 run-length** | JMA 1-km radar, rain-gauge analysis, nowcasts — real public data ([JMA GPV samples](https://www.data.jma.go.jp/developer/gpv_sample.html)) | eccodes decodes since 2.29.0 (in the 2.34.1 pin) → snapshot oracle; JMA's own [RLE algorithm note](https://www.jmbsc.or.jp/jp/online/joho-sample/Run-Length_Encoding.pdf) |
| **5.50 / 5.51 spectral** | ECMWF IFS spectral fields; decades of MARS/ERA GRIB1+GRIB2 archive | eccodes ships `sh_ml_grib2.tmpl` / `sh_pl_grib{1,2}.tmpl` samples; Laplacian sub-truncation documented in ECMWF's GRIB packing pages; reference impl `DataShPacked.cc` |
| **5.53 bi-Fourier** | ACCORD/ALADIN-family LAM spectral; little public data | eccodes *encodes* it → round-trip fixtures; Météo-France [ALADIN GRIB2 note](https://www.umr-cnrm.fr/aladin/IMG/pdf/grib2.pdf); WMO Tables 5.25/5.26 |
| **5.61 log pre-processing** | Experimental, no known producer | hand-built fixtures; two independent oracles (eccodes *and* wgrib2); decode = simple packing then `Y = exp(X) − B` |
| **5.1 matrix of values** | Experimental; ECMWF historic wave spectra use the GRIB1 form | own decode path (like GRIB1 `matrixOfValues`, per the decode-decoupling exception); hand-built fixture; eccodes flat decode as value oracle. WMO's secondary-bitmap sizing is known-broken for real wave spectra — follow the GRIBEX interpretation |
| Local: **5.40010** pre-standard PNG, 5.40000 pre-standard JPEG 2000, ECMWF 5.50001/5.50002 | pre-standard NCEP / ECMWF GRIB1-style second-order in GRIB2 | 5.40010's payload is identical to 5.41 — trivial; eccodes *fails* on it (no template def), a genuine exceed-eccodes item |

Out of scope: IEEE precision 3 (128-bit) — no known data; eccodes also rejects
it. Keep the clean `UnsupportedSection` error.

**Rendering spectral fields** (inverse spherical-harmonic transform → lat/lon
grid) — the follow-on that turns 5.50/5.51 decode into something no other viewer
offers — **shipped too**: both GRIB1 and GRIB2 spherical-harmonic messages
synthesize back onto a lat/lon grid and render through the normal pipeline
(projection, overlays, contours, probe), via the shared `fieldglass-core::sht`
engine validated against ECMWF's definitive spectral definition. Bi-Fourier
(5.53) rendering — an inverse bi-Fourier transform — remains the one spectral
form that decodes but does not yet render.

## Phase 7 — GRIB2 grid (§3) breadth

Supported today: 3.0, 3.1, 3.10, 3.20, 3.30, 3.40, 3.90. Remaining templates
with operational data, ranked by wild-data volume:

| Template | Data in the wild | Notes |
|---|---|---|
| **3.101 general unstructured (ICON)** | DWD ICON — huge daily open-data volume; GDAL cannot open it at all | **Needs a design decision first**: §3 carries only a grid UUID, no coordinates. Geometry comes out-of-band (ICON grid NetCDF matched by UUID from icon-downloads.mpimet.mpg.de, or CLAT/CLON companion GRIBs on opendata.dwd.de). The seam is additive, not a refactor: core decodes the message and surfaces the geometry *reference*; rendering accepts coordinate arrays as input; resolution policy (cache dir, download, user-pointed companion file) lives in the host adapter. ADR to pin this down |
| **3.150 HEALPix** | DestinE Climate DT (IFS-NEMO, IFS-FESOM, ICON harmonized onto it); newest registered §3 template (2023) | analytic geometry, no external file; eccodes decodes since 2.32.0 |
| **3.12 transverse Mercator** | UK Met Office UKV 1.5 km | false easting/northing + scale factor — genuinely different math from 3.10 |
| **3.140 Lambert azimuthal equal-area** | CEMS/EFAS ≤v4 archive, EUMETSAT OSI SAF sea ice | |
| **NCEP local 3.204 curvilinear, 3.32768/3.32769 rotated Arakawa E/B** | RTOFS ocean; legacy NAM native | eccodes lacks 3.204 and 3.32768 entirely — exceed-eccodes items. 3.204 shares a render path with NetCDF curvilinear grids (phase 9) |
| 3.100 triangular (GME) | archive only (DWD retired GME in 2015) | low priority |

Near-zero public data (defer indefinitely, keep clean errors): 3.2/3.3, 3.4/3.5
variable-resolution, 3.110, 3.120 azimuth-range, 3.31 Albers, 3.61–3.63,
3.1000+ cross-sections. Also carried forward: true ellipsoidal projection
(today oblate spheroids project on the mean radius).

## Phase 8 — GRIB1 completion

- 2-D rendering of spectral messages via the phase-6 inverse transform
  (coefficients already decode).
- Second-order packing with a bitmap masking points (today refused).
- Remaining predefined ON388 Table B grids (pole-staggered 21–26 / 61–64;
  today only 2, 3, 4).
- Deferred with the GRIB2 counterparts: stretched grids, ellipsoidal
  projection.

## Phase 9 — NetCDF / HDF5 frontier

Chunk indexing is already ahead of the field (all five v4 index types plus the
v1 B-tree). The gaps that block real files, ordered by payoff:

- **Filters**: szip — its decode side is exactly the libaec Rice coding we
  already ship in pure Rust for GRIB2 5.42 (`rust-aec`) — common in NASA EOS
  products; zstd (id 32015, netcdf-c ≥ 4.9, DKRZ-recommended); fletcher32
  (checksum); bzip2. This set would *exceed default netcdf-c installs*, which
  often lack the szip/zstd plugins at runtime. Blosc/LZ4: rare in NetCDF,
  defer.
- **Curvilinear (2-D coordinate) grids** — the biggest deferred render item;
  shares the cell-location render path with GRIB2 3.204 and ICON 3.101.
- **String/char data display** — classic `char` variables and HDF5 string
  datasets currently refuse value decode; showing text (station names, time
  labels) in the viewer is table-stakes for ocean/obs files.
- **Paged Fixed/Extensible Array data blocks** (today a clean error).
- **HDF5 2.0 awareness** — detect the new `H5T_COMPLEX` class and report it
  cleanly (files using it are unreadable by *all* older readers, including
  netcdf-c < 4.10).
- **Byte-range read seam** — structure the HDF5 reader around a byte-range
  trait so remote/cloud access (S3, kerchunk/VirtualiZarr-style virtual files)
  falls out nearly free later. Design constraint now, feature later.

Prior art worth reading: pyfive (pure-Python HDF5 reader; the best map of the
sufficient subset). There is no battle-tested pure-Rust HDF5 reader —
fieldglass's is genuinely open ground.

## Phase T — Parameter & code tables (second priority, runs in parallel)

Tedious but high-value, and almost entirely mechanical: generator scripts under
`tools/` regenerating Rust tables from pinned upstream sources (the existing
`gen_ecmwf_tables.py` pattern). All sources below are license-compatible with
attribution headers in the generated files.

| Source | Covers | Size | License |
|---|---|---|---|
| [wmo-im/GRIB2](https://github.com/wmo-im/GRIB2) release CSVs (pin a tag; v37 current) | Full WMO master tables — all 60 Table 4.2 discipline/category files (~1,430 params, vs ~10 curated today), full 4.5, 4.10, and the rest of the code tables; `Status` column marks deprecated entries | ~1.4k params + code tables | MIT |
| [wmo-im/CCT](https://github.com/wmo-im/CCT) `C11.csv` / `C12.csv` | Originating centres (326) + sub-centres (218) — replaces the curated centre subset | ~550 rows | MIT |
| eccodes `definitions/grib2/localConcepts/{ecmf,edzw,…}` | ECMWF (~3.4k) and DWD/ICON (~3.5k) local parameters **with shortName abbreviations**; also eswi, cnmc and other European centres | ~7k+ entries | Apache-2.0 |
| wgrib2 `src/gribtables/ncep/gribtable.dat` | NCEP WMO+local parameters with abbreviations (the NCO web pages have no machine-readable form; wgrib2's table is scraped from them). Same family adds MRMS, NDFD, KMA, BOM cheaply | 1,883 NCEP + extras | public domain |
| NCO ON388 web pages (scraper) | NCEP GRIB1 Table 2 versions 128–141 (no alternative source) | ~600–800 | public domain |
| JMA technical PDFs | small hand-curated table (JMA mostly uses master params + local *templates*, not big local tables) | small | n/a |
| [CF standard-name table](https://cfconventions.org/Data/cf-standard-names/current/src/cf-standard-name-table.xml) (v94) | NetCDF: `standard_name` → canonical units + one-line description, stripped (not the 4.4 MB XML) — names variables that lack `long_name`/`units` | ~5.1k names | open |

Resolution policy (matches eccodes and netCDF-java practice): build from the
latest WMO tables ("latest wins" — entries are only added or deprecated, never
renumbered); parameter/category/discipline codes ≥ 192 resolve against the
originating centre's local table first; keep deprecated entries displayable.
Regenerate on the twice-yearly WMO fast-track rhythm.

## Later / exploratory

- **Additional adapter surfaces** — the format crates are already pure,
  byte-oriented decode engines (`from_bytes`; no filesystem access outside the
  `detect_format` path convenience), so new hosts are thin wrappers in the
  napi mould: **PyO3 bindings** for scientists, a **wasm build** for webview or
  serverless use (all codecs are pure Rust, no threads). The one contract to
  revisit for huge-file and wasm-memory cases is whole-file-in-memory
  `Vec<u8>` — that is the phase-9 byte-range seam, and it is separable.
- **Zarr** (v2/v3; GeoZarr in OGC review 2026) — adoption is real and
  meteorological (ARCO-ERA5, ECMWF Anemoi). Directory-shaped rather than
  file-shaped, so it needs folder detection or ZipStore handling in the
  editor, but no editor-integrated Zarr viewer exists. Medium-term.
- **BUFR** (FM 94) — Panoply doesn't open it; a tree/table inspector would be
  unique. But table management is heavy and it shares nothing with the render
  pipeline: a separate product surface, after Zarr.
- **GRIB3** — shelved by WMO (archived repo, experimental-use only). Just
  detect the edition byte and report a clean "unsupported edition".

## Suggested sequencing

1. ~~**5.200 RLE**~~ — ✅ done. Real public JMA data, in-pin eccodes oracle.
2. ~~**5.50/5.51 spectral decode**, then the **inverse spherical-harmonic
   transform**~~ — ✅ done. Renders GRIB2 *and* GRIB1 spectral — a viewer first.
3. ~~**5.53, 5.61, 5.1 + locals**~~ — ✅ done. **The §5 census is complete**;
   the README carries the "decodes every registered GRIB2 packing" claim.
4. **3.101 ICON** (next) — ADR for out-of-band geometry first, then
   implementation. Highest wild-data payoff remaining in the plan.
5. **3.150 HEALPix**, then 3.12 / 3.140 / NCEP locals.
6. **NetCDF filters** (szip via existing rust-aec, zstd) and **curvilinear
   rendering** (shared with 3.204/ICON).
7. **Phase T table generators** land incrementally alongside all of the above
   — each generator is an independent, reviewable PR.
