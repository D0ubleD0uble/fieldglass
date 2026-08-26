# 0007 — NetCDF units are normalised for notation, not renamed

**Status:** Accepted (2026-08-26). Answers
[#453](https://github.com/D0ubleD0uble/fieldglass/issues/453). Bounds the
notation half of [#249](https://github.com/D0ubleD0uble/fieldglass/issues/249)
(display unit conversion), which remains open and is about values, not spelling.

## Context

Fieldglass typesets the units column at the display seam. GRIB2 got it in #432,
GRIB1 in #441, ECMWF's local table in #424. NetCDF was the one format left
passing its `units` attribute through raw, so the same quantity read `m s⁻¹`
from a GRIB file and `m/s` from a NetCDF one, in the same column of the same
editor.

Wiring the existing `normalize_units` into the NetCDF path is one line. #453
declined to treat it as a wiring change for a reason worth taking seriously:

> Every seam normalised so far reads from a *generated table pinned to a
> standard* … NetCDF units come from the **file author**, not from a spec
> table. Rewriting a user's own metadata for display is a different act.

That is the question this record answers.

## What the corpus actually contains

Measured over every committed `.nc` / `.h5` fixture rather than estimated —
33 distinct `units` strings. Routing them through `normalize_units` as it stood
rewrote **two**: `m/s` and `cm/s`. Everything else passed through, including
`meters`, `kelvin`, `degree_C`, `percent`, `counts`, and every time coordinate.

So the objection #453 raised — that wiring it in would produce a *more* mixed
column than the uniformly-raw one it replaced — was well founded on the code as
it stood. But the reason is not that NetCDF units resist normalisation. It is
that the vocabulary had a hole:

    "W m-2 sr-1 um-1"   →   unchanged

That is GOES ABI radiance. `m-2` and `sr-1` rewrite on their own; the whole
string passed through because `um` was not a recognised symbol and
`is_normalisable` is all-or-nothing. `mm/hr` failed the same way, for `hr`.
Those are exactly the shape the module exists for, and they were failing
silently — indistinguishable, in the output, from a string deliberately left
alone. That is the gap #450 hit on the GRIB side, recurring here.

## Decision

**Yes, NetCDF units go through the same display seam, and the line is notation
versus naming.**

**1. Normalising is legitimate here, and the "author's own metadata" objection
does not survive contact with CF.** CF *requires* the `units` attribute to be
parsable by UDUNITS. The author did not write prose; they picked one
machine-readable spelling among equivalents, in an attribute whose grammar is
fixed by a standard. `m/s` and `m s⁻¹` are the same UDUNITS expression, and
choosing how to render it is the viewer's job — the same judgement already made
for WMO's ASCII.

Nothing downstream reads the string, which was #453's third question. It reaches
three display sites: the metadata table cell, the render panel's title line, and
the probe readout. CSV export headers carry no units (`lat,lon,value`), and no
unit *conversion* exists — #249 is the future one, and it will key on the
attribute the file holds, not on what the table shows.

**2. Notation is restored; names are left exactly as written.**

| | | |
|---|---|---|
| notation | `m/s` → `m s⁻¹`, `m-2` → `m⁻²` | same tokens, different typography |
| transliteration | `um` → `µm` | `u` is what CF writes where the symbol is `µ`, because the attribute is ASCII |
| naming | `meters`, `kelvin`, `degree_C`, `percent` | **untouched** |

The transliteration case belongs with notation, not naming: restoring `µ` from
`u` is the same act as restoring `⁻¹` from `-1`, both undoing a substitution the
encoding forced. Choosing between `meters` and `m`, or `kelvin` and `K`, is a
different act — those are distinct words, and which to use is the author's.

This also answers #453's second question. The allow-list grows by the handful of
symbols the corpus proves are missing (`um`, `hr`, `dbar`). It does **not** grow
a UDUNITS section, and NetCDF does not get its own normaliser. Full UDUNITS
grammar — `10^-6`, `@`, scaled and offset units — is #249's problem if it is
ever anyone's.

**3. A time encoding is refused by rule, not by accident.** `minutes since
1870-01-01 00:00` states an epoch, and HYCOM (RTOFS, in this corpus) writes a
date as `day as %Y%m%d.%f`. Both begin with a real unit token, and both survived
only because `since`, `as` and a date are unrecognised — which stops being true
the moment the vocabulary grows, which is precisely what this record does.
`ENCODING_KEYWORDS` refuses them explicitly.

The second form was found by the corpus sweep, not by anyone thinking of it.

**4. The sweep is the deliverable, not the wiring.**
`crates/fieldglass-netcdf/tests/unit_notation.rs` pins every distinct `units`
string in the corpus against its exact output, and reads the fixture directory
rather than a list so a fixture added later joins without anyone remembering.
A silent passthrough is then a visible snapshot line instead of nothing at all.

## Consequences

- Two strings in the committed corpus change in the units column, and two that
  should always have changed now do: `W m-2 sr-1 um-1` reads `W m⁻² sr⁻¹ µm⁻¹`,
  `mm/hr` reads `mm hr⁻¹`.
- A stray leading space is trimmed as a side effect of tokenising — RTOFS writes
  `" degC"` and `" m"`, which now render without it. Benign, and visible in the
  snapshot rather than hidden.
- `um`, `hr` and `dbar` are recognised for **every** format, not just NetCDF.
  None appears in the WMO tables today (the GRIB snapshots are unchanged), so
  this is latent capability rather than a behaviour change there.
- **What this record does not license:** widening the vocabulary until names
  start being rewritten. A future entry that turns `meters` into `m` is a
  reversal of point 2, not an extension of it, and needs its own argument.

## References

- CF Conventions §3.1 (units): the attribute must be parsable by UDUNITS —
  <https://cfconventions.org/cf-conventions/cf-conventions.html>
- UDUNITS-2 unit grammar and the ASCII `u` for `µ` —
  <https://docs.unidata.ucar.edu/udunits/current/>
- The GRIB-side precedent this follows: #432 (GRIB2), #441 (GRIB1), #424
  (ECMWF), and #450, the silent-passthrough gap the sweeps exist to catch.
