---
name: Bug report
about: A file that doesn't render correctly, an error message, a crash, a wrong value
title: "[bug] "
labels: bug
---

## What happened

<!-- One sentence: what did you expect, and what did you see instead? -->

## The file involved

<!--
The most useful thing you can send is a small file that shows the problem.
If you can share one (a few hundred KB is plenty for most bugs), please attach
it or link to a public copy. For a Zarr store, a zip of the store folder, or
the store's URL, works. If you can't share it, please tell me:
-->

- Format and edition: <!-- e.g. GRIB1, GRIB2, NetCDF classic, NetCDF-4, Zarr v2, Zarr v3 -->
- Originating center or model: <!-- e.g. NCEP GFS, ECMWF IFS, CMC -->
- Approximate size:
- Where it came from:

## Steps to reproduce

1.
2.
3.

## Where you ran it

<!-- Fill in the part for whichever way you use Fieldglass, and delete the rest. -->

VS Code extension:

- Fieldglass version: <!-- from the Extensions view, or `code --list-extensions --show-versions` -->
- VS Code version: <!-- Help → About -->

Rust crates:

- Crate and version: <!-- e.g. fieldglass 0.6.0, fieldglass-grib2 0.6.0 -->
- Features turned on or off:
- Rust version: <!-- `rustc --version` -->

Browser package:

- `@fieldglass/wasm` version, and the default or `/full` entry:
- Browser and version:

For all of them:

- OS and architecture: <!-- e.g. macOS 14.5 arm64, Windows 11 x64, Ubuntu 24.04 x64 -->

## Logs and screenshots

<!--
- The text of any error message, including its `code` if it has one.
- In VS Code: the output of Help → Toggle Developer Tools → Console while the
  file is open, and a screenshot of the metadata table if a value looks wrong.
- In a browser: the console output from the page and its Worker.
-->
