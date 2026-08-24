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

The dispatch seam for this landed in #439: `lookup_parameter` takes an
`Originator`, and `tables_local::lookup` is where a centre table plugs in.
ECMWF (#424) and DWD (#425) are in; NCEP (#426) is the remaining one.

The seam keys on the three §1 fields eccodes itself resolves a local concept
on — centre, sub-centre and `localTablesVersion` (ECMWF gates 13 entries on the
last; DWD gates none). `masterTablesVersion` is deliberately not a key: WMO only
adds or deprecates, never renumbers, so latest wins.

What each centre actually contributes, as `tools/gen_localconcepts_tables.py`
measures it at eccodes 2.34.1 — concept blocks in the first two columns,
distinct triples in the rest. The NCEP row is a dry run of the same generator,
kept because it is the number wgrib2 had to beat — and did, 479 to 313, which
is why `tables_ncep.rs` comes from `tools/gen_ncep_tables.py` instead:

| Centre | blocks | outside 192-254 | emitted | §4-keyed | ambiguous | placeholder |
|---|---|---|---|---|---|---|
| ECMWF (#424) | 3,601 | 63 | 2,826 | 48 | 6 | 642 |
| DWD/ICON (#425) | 1,704 | 983 | 213 | 100 | 50 | 254 |
| NCEP via eccodes (not shipped) | 319 | 6 | 313 | 0 | 0 | 0 |

Two corrections to the pre-implementation survey this table replaces. It quoted
3,360 ECMWF and 1,827 DWD entries, with 40 and 1,069 below 192; those came from
counting the concept files a different way, and the generator's numbers are the
ones the shipped tables are built from. And it read DWD's ambiguity as a reason
to defer the centre entirely. In practice the generator's existing skip rules
handle it: the ≥192 rule drops the standard-code majority, the §4 rule drops the
17-way `(0, 0, 0)` collisions, and what is left — 213 triples — is unambiguous
and matches eccodes exactly. DWD needed no decision about §4 context after all;
it needed the same rules applied and the residue measured.

The residue is the point, though. DWD publishes 129 parameter groups for
ICON-D2 and this table names nine of them: `CLCT_MOD`, `CLDEPTH`, `FRESHSNW`,
`Q_SEDIM`, `SDI_2`, `SOILTYP`, `TQC_DIA`, `TQI_DIA`, `TQV_DIA`. The headline
fields — `T_2M`, `TOT_PREC`, `PMSL`, `W_SO` — sit on standard triples the WMO
master set already names correctly, so what the local table adds there is DWD's
abbreviation, not the name, and the ≥192 rule keeps it out. That is names we do
not gain, never names we get wrong; the ECMWF residue (47 blocks below 192) is
the same trade at a hundredth the scale.

A third thing #425 turned up, which #426 should expect: DWD fills whole local
categories with `DUMMY_1` … `DUMMY_508` placeholders, 254 of which land on
otherwise-emittable triples. They are `Experimental product` in another
spelling, and the generator skips them by the same rule.

## Every generator names its encoding

Python's text I/O defaults to the platform locale. On CI and on any UTF-8 or
`C`/`POSIX` machine that is UTF-8 — PEP 540 turns UTF-8 mode on for the latter —
so the default is right and nothing goes wrong. Elsewhere, most obviously
Windows, it is not, and the failure is silent: the pre-#451 GRIB1 ECMWF
generator run with a cp1252 stdout wrote `\x85` where the file should carry
`\xe2\x80\xa6`, producing a Rust source file that is not valid UTF-8 and a
regeneration diff that reads as corruption rather than as an upstream edit.

So every read, every write, and every `subprocess.run(..., text=True)` in
`tools/` states its encoding, whether or not the file has non-ASCII in it today
— `tools/check_generator_encoding.py` is a pre-commit hook that says so. The
subprocess half is the one that is easy to miss: the oracle generators read
parameter names straight out of `grib_get`'s stdout, and `text=True` decodes it
with the locale encoding just as `open()` would.

Generators write their file directly rather than being piped (`python3 tools/gen_X.py`,
not `python3 tools/gen_X.py > out.rs`), because a redirected stdout takes its
encoding from the locale and there is no keyword argument to fix that at the
call site.

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
- **The third and fourth notations are handled (#441).** eccodes writes
  exponents Fortran-style (`kg m**-2`) and ON388 chains solidi (`kg/m2/s`);
  `normalize_units` now reads both, and GRIB1 goes through the same display
  seam as GRIB2. Surveying all four corpora first is what made the second rule
  safe to write, and the shape is worth knowing before #424-#426:

  | Corpus | `**` | solidus | chained solidus |
  |---|---|---|---|
  | WMO GRIB2 master (v37) | — | 16 strings | — |
  | ON388 GRIB1 (`grib1/src/tables.rs`) | — | ~20 strings | 6 strings |
  | eccodes ECMWF (`grib1/src/tables_ecmwf.rs`, and #424) | 61 distinct | — | — |
  | eccodes NCEP (unused) | 28 distinct | — | — |
  | wgrib2 NCEP (`grib2/src/tables_ncep.rs`, #426) | — | 33 strings | 6 strings |
  | eccodes DWD/ICON (#425) | — | — | — |

  So the two ASCII families do not mix: a table is written one way or the
  other. DWD writes neither, using the bare `kg kg-1` form WMO's master table
  also uses, so #425 added no notation and needed no new unit symbols; the four
  strings it did add to the passthrough set (`Pa-3h`, `10-7 s-2`, `Pa(O3)`,
  `Km kg-1 s-1`) are pinned in `grib2/tests/unit_notation.rs`.

  **wgrib2 writes a fifth: the caret**, in 15 of its 62 distinct strings —
  `kg/m^2/s`, `m^2/s^2`, `mm^6/m^3`.
  `normalize_units` reads it as of #426, alongside `**` and the bare form, and
  the change is purely additive — not one previously pinned string moved.
  wgrib2 also writes `*` as an explicit product in 8 strings (`K*m/s`,
  `J/m^2*K`), which is deliberately *not* read: nothing else in any corpus does, and it would collide
  with the `**` operator. Those pass through, as do NCEP's prose dimensionless
  markers (`-`, `non-dim`, `Categorical`, `Integer(0-13)`). Chained solidi are ON388-only, which is what bounds the "everything
  past the first solidus is a denominator" rule to six strings that could be
  checked one by one against the quantity (`kg/m2/s` is precipitation rate,
  `m2/s/kg` potential vorticity).

- **The all-or-nothing rule is what makes the eccodes tables safe.** Their
  `units` field is not always a product of units, and the strings that are not
  would each be corrupted by an eager `**` rule: `10**-6 W m**-2 sr**-1 m**-1`
  leads with a scale factor, `kg m**-3 -1000` carries an additive offset,
  `kg (kg s**-1)**-1` and `log10(kg m**-3)` nest a group. All pass through
  whole today and are pinned in
  `fieldglass-core/src/units.rs::star_star_strings_that_are_not_plain_products_pass_through`.
  #424-#426 should expect them, and should add the symbols their tables
  introduce (`um`, `mm`, `nuc`, …) to the allow-list — the snapshot diff is
  what shows which are missing, since a missing symbol silently degrades to
  passthrough rather than failing.
- **`Status` has upstream typos.** `Operationaal`, `Oprational`, `Operation`
  and `operational` all appear in v37 beside `Operational`. Any filter on that
  column has to fold case and tolerate them, or it silently drops entries.
