# Parameter and code table sources

Today `grib2/src/tables.rs::lookup_parameter` carries 44 curated triples and
`grib1/src/tables.rs` about 375, plus the generated `tables_ecmwf.rs`. The
full WMO master set is about 1,430 parameters, before local tables.

Each source below becomes a generator script under `tools/` that regenerates
Rust tables from a pinned upstream (the existing `gen_ecmwf_tables.py`
pattern). Every generator is an independent, reviewable PR.

| Source | Covers | Size | License |
|---|---|---|---|
| ~~[wmo-im/GRIB2](https://github.com/wmo-im/GRIB2) release CSVs~~ **Done (#415)**, pinned at **v37**: `tools/gen_wmo_grib2_tables.py` | Table 4.2 (all 60 discipline/category files), 4.5, 4.4. 4.10 and the remaining code tables are still to do | 1,387 params + 87 surfaces + 12 time units | MIT |
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

## What #415 turned up

Two things worth carrying into the remaining generators:

- **WMO publishes no short names.** The CSVs carry name and units only, so a
  generated master entry has an empty abbreviation and the curated table in
  `tables.rs` supplies the short name for the triples it covers. Restoring
  short names across the board is what the centre-local generators (#424-#426,
  and wgrib2 for NCEP) are for.
- **Cross-check the generated table against a second transcription.** eccodes
  ships the same Code Table 4.2, so `tools/gen_eccodes_parameter_snapshot.py`
  snapshots it and `tests/wmo_parameter_tables.rs` compares the two. That
  comparison immediately found three curated entries carrying GRIB1 ON388
  codes on GRIB2 triples — `(0,3,9)`, `(2,0,5)`, `(10,1,2)` — each naming the
  wrong quantity, one of them labelling a current component as sea-surface
  temperature. A generator is only as trustworthy as its oracle; build the
  oracle from a different source than the generator.
- **Units need normalising for display, not in the table.** WMO is not
  self-consistent (`m/s` and `m s-1` both appear), so #432 typesets them at the
  display seam, keeping the generated file byte-identical to its pinned tag.
  Build that as an allow-list of unit symbols rather than a structural rule:
  the real table contains `CCITT IA5` (a character encoding), `m2/3 s-1` (a
  *fractional* exponent), and `Code table 4.253` (a cross-reference), each of
  which an obvious "letter then digit means exponent" rule corrupts. Rewrite a
  string only when every token is a known unit, and leave it alone otherwise —
  a missed normalisation is always defensible, a corrupted one is not.
- **`Status` has upstream typos.** `Operationaal`, `Oprational`, `Operation`
  and `operational` all appear in v37 beside `Operational`. Any filter on that
  column has to fold case and tolerate them, or it silently drops entries.
