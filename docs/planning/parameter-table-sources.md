# Parameter and code table sources

Today `grib2/src/tables.rs::lookup_parameter` carries 44 curated triples and
`grib1/src/tables.rs` about 375, plus the generated `tables_ecmwf.rs`. The
full WMO master set is about 1,430 parameters, before local tables.

Each source below becomes a generator script under `tools/` that regenerates
Rust tables from a pinned upstream (the existing `gen_ecmwf_tables.py`
pattern). Every generator is an independent, reviewable PR.

| Source | Covers | Size | License |
|---|---|---|---|
| [wmo-im/GRIB2](https://github.com/wmo-im/GRIB2) release CSVs (pin a tag; v37 current) | Full WMO master tables: all 60 Table 4.2 discipline/category files, full 4.5, 4.10, and the rest of the code tables; `Status` column marks deprecated entries | ~1.4k params + code tables | MIT |
| [wmo-im/CCT](https://github.com/wmo-im/CCT) `C11.csv` / `C12.csv` | Originating centres (326) and sub-centres (218); replaces the curated centre subset | ~550 rows | MIT |
| eccodes `definitions/grib2/localConcepts/{ecmf,edzw,…}` | ECMWF (~3.4k) and DWD/ICON (~3.5k) local parameters with shortName abbreviations; also eswi, cnmc and other European centres | ~7k+ entries | Apache-2.0 |
| wgrib2 `src/gribtables/ncep/gribtable.dat` | NCEP WMO+local parameters with abbreviations (the NCO web pages have no machine-readable form). Same family adds MRMS, NDFD, KMA, BOM cheaply | 1,883 NCEP + extras | public domain |
| NCO ON388 web pages (scraper) | NCEP GRIB1 Table 2 versions 128–141 (no alternative source) | ~600–800 | public domain |
| JMA technical PDFs | Small hand-curated table (JMA mostly uses master params + local *templates*) | small | n/a |
| [CF standard-name table](https://cfconventions.org/Data/cf-standard-names/current/src/cf-standard-name-table.xml) (v94) | NetCDF: `standard_name` → canonical units + one-line description, stripped (not the 4.4 MB XML) | ~5.1k names | open |

## Resolution policy

Matches eccodes and netCDF-java practice: build from the latest WMO tables
("latest wins"; entries are only added or deprecated, never renumbered);
parameter/category/discipline codes ≥ 192 resolve against the originating
centre's local table first; keep deprecated entries displayable. Regenerate on
the twice-yearly WMO fast-track rhythm (see
[`standards-watch-list.md`](standards-watch-list.md)).

Keep generated files out of patch coverage in `codecov.yml`, and regenerate
via the script rather than editing output by hand.
