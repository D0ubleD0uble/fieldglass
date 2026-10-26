# 0017 — GRIB2 product template 4.47 is read as WMO lays it out

**Status:** Accepted (2026-10-21). #1130.

## Context

`fieldglass-grib2` reads the chemistry and aerosol product templates
4.40–4.48 (#1130). Every aerosol template but one puts the aerosol type at §4
octets 12–13, the size range at 14–24 and the type of generating process after
it. Template 4.47 (an ensemble member over a time interval, for aerosol) does
not. WMO's own definition puts the generating process first:

| Octets | WMO 4.47 | 4.46, 4.85, and eccodes' 4.47 |
|---|---|---|
| 12 | type of generating process | aerosol type (12–13) |
| 13–14 | aerosol type | rest of the aerosol type (13), then the size range's interval type (14) |
| 15–25 | size range (type, then two limits) | size range at 14–24, generating process at 25 |
| 26 on | background process, forecast process, cut-off, forecast time, surfaces at 36–47, ensemble block, interval | the same |

The evidence, all read on 2026-10-21:

- **WMO.** `wmo-im/GRIB2` v37, `GRIB2_Template_4_47_ProductDefinitionTemplate_en.csv`:
  octet 12 "Type of generating process", 13–14 "Aerosol type", 15 "Type of
  interval for first and second sizes", 16–25 the two limits. Code Table 4.0
  lists 4.47 as operational. It also lists **4.85**, with the same
  description, laid out as 4.46 is (aerosol type at 12–13, generating process
  at 25).
- **NCEPLIBS-g2c** `src/pdstemplates.c`: `{47, 38, 1, {1, 1, 1, 2, 1, -1, -4, ...}}`,
  three one-octet fields (category, number, generating process) before the
  two-octet aerosol type. NCEP's GRIB2 documentation page for 4.47 agrees.
- **wgrib2** `src/CodeTable.c`: `code_table_4_233_location` returns
  `sec[4] + 12` (octet 13) for 4.47 and `sec[4] + 11` for 4.44–4.46 and 4.48;
  `code_table_4_91_location` returns `sec[4] + 14` for 4.47 and `+ 13` for the
  others; `code_table_4_3_location` returns `sec[4] + 11` (octet 12) for 4.47.
- **eccodes** 2.34.1 (the pin) and 2.45.0 lay 4.47 out as 4.46:
  `template.4.47.def` includes `template.4.aerosol.def` straight after the
  parameter, then `aerosol_size`, then `generating_process`. The 2.48 PyPI
  wheel writes it that way too. eccodes 2.45.0 marks the template deprecated
  (`template_is_deprecated = 1`); WMO does not.
- **A committed case.** `crates/fieldglass-grib2/tests/fixtures/constituent_4_47.grib2`
  is laid out as WMO says. eccodes 2.34.1 `grib_dump -O` reads it as
  `typeOfGeneratingProcess = 1` (the spec's 4), `constituentType = 1266`
  (62009, black carbon dry), `typeOfSizeInterval = 57` (2), and size limits
  2 / 134217728 and 1 / 100663296 (8 / 1 and 6 / 1). The parameter, both
  fixed surfaces, the forecast time, the ensemble block and the interval sit
  at the same octets in both layouts, and eccodes reads them correctly.

## Decision

1. **4.47 is read as WMO defines it**: generating process at octet 12,
   aerosol type at 13–14, size range at 15–25. This follows the project's
   rule that the specification defines correct and reference libraries are
   oracles (conventions, "Correctness comes first"). Two independent readers,
   wgrib2 and g2c, agree with the specification.
2. **The divergence is recorded where it is tested.** `ECCODES_MISREADS` in
   `crates/fieldglass-grib2/tests/eccodes_reference.rs` compares those seven
   keys with the spec's values instead of the snapshot's, and
   `every_listed_misread_still_disagrees` fails if eccodes starts reading them
   as the spec does. The fixture builder asserts eccodes' misreadings, and the
   fixture's `NOTICE.md` records them.
3. **4.44's forecast time is two unsigned octets** (octets 32–33, a 45-octet
   template), as WMO's table and eccodes have it. g2c declares it signed
   (`-2`, sign-magnitude); the two differ only above 32767. A 4.44 of any
   other length is read with a four-octet signed forecast time, the form
   eccodes once wrote and still reads (GRIB-29,
   `template.4.forecast_time_44.def`). WMO lists 4.44 as deprecated, and
   NCEP's page for it (note 2,
   <https://www.nco.ncep.noaa.gov/pmb/docs/grib2/grib2_doc/grib2_temp4-44.shtml>)
   says it is not recommended and 4.48 should be used instead.

## Consequences

- A 4.47 message written through eccodes is laid out the 4.46 way, which is
  what WMO now defines as 4.85. Fieldglass reads a wrong constituent, size
  range and generating process from it (the parameter, surfaces, forecast
  time, member and interval are still right). Its `MessageFingerprint` is
  consistent with itself, so a changed file is still matched to its message;
  `verify_message` compares the wrong constituent and size range with a
  wgrib2 line, which wgrib2 reads from the spec's octets as Fieldglass does.
  No real 4.47 file was found to check against.
- If eccodes adopts the WMO layout, the misread entries fail and are removed.
- Revisit if real 4.47 traffic turns out to be eccodes-written: reading the
  4.46 layout by producer would be a separate, evidence-led decision.
