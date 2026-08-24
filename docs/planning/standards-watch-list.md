# Standards watch list

Where new templates, packings, and format changes are proposed and announced.

| Channel | What lands there | Cadence |
|---|---|---|
| [wmo-im/GRIB2 issues](https://github.com/wmo-im/GRIB2/issues) + [releases](https://github.com/wmo-im/GRIB2/releases) | New GRIB2 templates/parameters are proposed as issues, batched into `FTyyyy-1/-2` milestones, released as tagged CSVs (MIT). The operational source of truth; codes.wmo.int lags it. | 2 fast-track cycles/yr (May + Nov) |
| [wmo-im/CCT releases](https://github.com/wmo-im/CCT/releases) | The Common Code Tables: originating centres (C-1 for GRIB1, C-11 for GRIB2) and sub-centres (C-12), as tagged CSVs (MIT). Separate repo and cadence from wmo-im/GRIB2, so a GRIB2 fast-track bump is *not* a centre-table bump. | ~2/yr |
| [eccodes History of Changes](https://confluence.ecmwf.int/display/ECC/History+of+Changes) | Best single signal for "newly decodable in the reference stack"; also table-version pickup | ~5–6 releases/yr |
| [netcdf-c releases](https://github.com/Unidata/netcdf-c/releases) | New filters (zstd landed in 4.9), NCZarr direction | ~2/yr |
| [HDF5 releases](https://github.com/HDFGroup/hdf5/releases) | Format changes; 2.0.0 (Nov 2025) added the `H5T_COMPLEX` class, unreadable by older readers | ~2/yr |
| [cf-conventions releases](https://github.com/cf-convention/cf-conventions/releases) + [standard-name table](https://cfconventions.org/Data/cf-standard-names/current/src/cf-standard-name-table.xml) | CF conventions (annual, Dec) and standard names (several/yr) | annual / several per yr |
| [DWD definitions bundle](https://opendata.dwd.de/weather/lib/grib/) | DWD's eccodes-definition tarballs, sometimes fresher than upstream eccodes | ad hoc |

A twice-yearly checkpoint after each WMO fast-track publication (May/June and
November) is the natural rhythm for table regeneration, census review, and a
roadmap revision.

When bumping `wmo-im/CCT`, read the generated diff rather than accepting it:
`tools/gen_wmo_cct_tables.py` carries sixteen overrides that restore detail WMO
does not publish, and each is pinned to the upstream text it was written
against. An upstream rename fails the generator loudly, which is the intended
prompt to re-review that entry.

The GRIB2 packing space itself is frozen: Code Table 5.0 registers 13 data
representation templates, nothing has been added since 5.53, and all current
WMO activity is §4 product-definition templates and Table 4.2 parameters.
GRIB edition 3 is shelved (repository archived, experimental use only).
