//! The notices the third-party data tables in Fieldglass ask a redistributor
//! to carry (#1563).
//!
//! The colormaps, the X11 color names, the WMO code tables and the tables
//! generated from ecCodes' definitions are other people's data, compiled into
//! the npm package's `.wasm`, the VS Code extension's Node addon, and the source
//! of the crates.io crates that hold the tables. A consumer of any of them never
//! sees this repository's README, so each package has a `NOTICE` file of its
//! own, and this list is what those files must contain. Apache-2.0 section 4(d)
//! asks for a NOTICE in a source redistribution as much as a binary one, and
//! BSD and MIT notices travel with copies in any form (#1621).
//!
//! The list sits beside the colormap and color-name tables, the ones most
//! likely to grow, so it is the one place a new table gets its terms recorded.
//! It is held from two sides:
//!
//! - this crate's tests hold it to the colormap registry: every colormap is
//!   listed exactly once, as is the X11 table;
//! - `crates/fieldglass/tests/third_party_notices.rs` includes this file and
//!   holds both `NOTICE` files to it: every credit is in each package it
//!   names, up to line wrapping, and every license text byte for byte. That
//!   test lives in the umbrella, whose `tests/` is not published, because it
//!   reads files outside this crate.
//!
//! The file is self-contained (it names nothing else in the crate) so that
//! test can include it with `#[path]`.
//!
//! The wording of the colormap and color-name credits is the README's
//! (`README.md`, "Third-party data"), with Markdown autolinks written as plain
//! URLs.

/// One source of bundled data and what its terms ask a redistributor to carry.
pub(crate) struct DataNotice {
    /// The tables it covers: colormap wire names, [`X11_COLOR_NAMES`], or a
    /// generated table's path under `crates/`.
    pub(crate) tables: &'static [&'static str],
    /// The `NOTICE` files ([`NPM`], [`EXTENSION`], and the crates.io crates
    /// [`CORE`], [`GRIB1`], [`GRIB2`] and [`UMBRELLA`]) of the packages that
    /// carry these tables, compiled in or as source. Empty for data that owes
    /// nobody a notice.
    pub(crate) packages: &'static [&'static str],
    /// The credit line, compared up to line wrapping, or `None` for data that
    /// owes nobody one.
    pub(crate) credit: Option<&'static str>,
    /// License or notice text the terms require to travel with every copy,
    /// compared byte for byte. The Apache License text is not here: the npm
    /// package ships `LICENSE-APACHE`, and the extension's `LICENSE.md` links
    /// it.
    pub(crate) terms: Option<&'static str>,
}

/// The npm package `@fieldglass/wasm`. `pack.sh` copies this file into it.
pub(crate) const NPM: &str = "crates/fieldglass-wasm/npm/NOTICE";

/// The VS Code extension. `vsce` packages this file from `extension/`.
pub(crate) const EXTENSION: &str = "extension/NOTICE";

/// The crates.io crates that ship third-party data in their source. Each has
/// a `NOTICE` at its root, which `cargo package` includes because no `include`
/// list leaves it out.
pub(crate) const CORE: &str = "crates/fieldglass-core/NOTICE";
pub(crate) const GRIB1: &str = "crates/fieldglass-grib1/NOTICE";
pub(crate) const GRIB2: &str = "crates/fieldglass-grib2/NOTICE";
pub(crate) const UMBRELLA: &str = "crates/fieldglass/NOTICE";

/// The id [`DATA_NOTICES`] gives the X11 color-name table the CPT parser reads.
pub(crate) const X11_COLOR_NAMES: &str = "x11-color-names";

/// Every source of third-party data compiled into Fieldglass.
pub(crate) const DATA_NOTICES: &[DataNotice] = &[
    // ColorBrewer: Apache-2.0 (matplotlib's `LICENSE/LICENSE_COLORBREWER`).
    // The older ColorBrewer terms, v1.1, ask for this sentence in end-user
    // documentation, with the URL as http://colorbrewer.org/. The README has
    // always carried it, with the URL as https.
    DataNotice {
        tables: &["rdbu", "brbg"],
        packages: &[NPM, EXTENSION, CORE],
        credit: Some(
            "This product includes color specifications and designs developed by \
             Cynthia Brewer (https://colorbrewer.org/). The red–blue and brown–teal \
             diverging maps are from ColorBrewer, © Cynthia Brewer, Mark Harrower \
             and The Pennsylvania State University, licensed under the Apache \
             License, Version 2.0.",
        ),
        terms: None,
    },
    // turbo: "Copyright 2019 Google LLC. SPDX-License-Identifier: Apache-2.0",
    // the header of Anton Mikhailov's published table.
    DataNotice {
        tables: &["turbo"],
        packages: &[NPM, EXTENSION, CORE],
        credit: Some(
            "The turbo colormap is © 2019 Google LLC (Anton Mikhailov), licensed \
             under the Apache License, Version 2.0.",
        ),
        terms: None,
    },
    // CC0 asks for nothing. The credit is a courtesy the README extends, and is
    // carried so the packages say what the README says.
    DataNotice {
        tables: &["viridis", "plasma", "cividis"],
        packages: &[NPM, EXTENSION, CORE],
        credit: Some(
            "Viridis, plasma and cividis are from matplotlib and are released into \
             the public domain (CC0) by their authors: Nathaniel J. Smith, Stéfan \
             van der Walt and Eric Firing (viridis, plasma), and Jamie R. Nuñez, \
             Christopher R. Anderton and Ryan S. Renslow (cividis).",
        ),
        terms: None,
    },
    // Kenneth Moreland's "Smooth Cool Warm", which matplotlib ships as
    // `coolwarm`. His color-advice page (kennethmoreland.com/color-advice/)
    // says it "was first featured in ParaView and is covered by its BSD
    // license", whose terms ask that a binary reproduce the copyright notice,
    // conditions and disclaimer in its documentation.
    DataNotice {
        tables: &["coolwarm"],
        packages: &[NPM, EXTENSION, CORE],
        credit: Some(
            "The cool–warm diverging map is Kenneth Moreland's Smooth Cool Warm, \
             which was first featured in ParaView and is covered by ParaView's BSD \
             license, © 2000 Kitware Inc. That license is below.",
        ),
        terms: Some(PARAVIEW_LICENSE),
    },
    // A black-to-white ramp defined in `tools/gen_colormaps.py`.
    DataNotice {
        tables: &["grayscale"],
        packages: &[],
        credit: None,
        terms: None,
    },
    // X.Org `app/rgb`, release rgb-1.1.1 (`tools/gen_color_names.py`). Its
    // `COPYING` asks that the copyright and permission notice "be included in
    // all copies or substantial portions of the Software", so the text itself
    // travels, not a pointer to it. It is the whole file, both notices in it.
    // The `.wasm` does not link these names today, since it does not parse
    // color tables. Its NOTICE carries them so it stays right when it does.
    DataNotice {
        tables: &[X11_COLOR_NAMES],
        packages: &[NPM, EXTENSION, CORE],
        credit: Some(
            "The color names an imported color table may use are the X11 color \
             database (`rgb.txt` from X.Org `app/rgb`), © 1985, 1989, 1998 The Open \
             Group, used under the X11 permission notice in that repository's \
             `COPYING`.",
        ),
        terms: Some(X11_RGB_COPYING),
    },
    // `wmo-im/GRIB2` v37 and `wmo-im/CCT` v2026-06-01, whose `LICENSE.md` files
    // are the same MIT License, byte for byte. MIT asks that its notice "be
    // included in all copies or substantial portions of the Software".
    DataNotice {
        tables: &[
            "fieldglass-grib2/src/tables_wmo.rs",
            "fieldglass-grib2/src/tables_cct.rs",
            "fieldglass-grib2/src/tables_constituent.rs",
            "fieldglass-grib1/src/tables_cct.rs",
            "fieldglass-core/src/center_tables.rs",
        ],
        packages: &[NPM, EXTENSION, CORE, GRIB1, GRIB2],
        credit: Some(
            "The GRIB2 parameter, surface and time-range names, and the names of \
             originating centers, sub-centers and chemical constituents, are \
             generated from the World Meteorological Organization's code tables \
             (https://github.com/wmo-im/GRIB2 and https://github.com/wmo-im/CCT), published \
             under the MIT License. Their LICENSE.md is below.",
        ),
        terms: Some(WMO_MIT_LICENSE),
    },
    // ecCodes 2.34.1 definitions, Apache-2.0. The generated headers call these
    // factual data. The notice is carried anyway in case they are a derivative
    // work, which is what Apache-2.0 section 4(d) asks of one. ecCodes' NOTICE
    // is the same at 2.34.1, 2.45.0 and 2.48.0.
    DataNotice {
        tables: &[
            "fieldglass-grib2/src/tables_ecmf.rs",
            "fieldglass-grib2/src/tables_edzw.rs",
            "fieldglass-grib1/src/tables_local.rs",
        ],
        packages: &[NPM, EXTENSION, GRIB1, GRIB2],
        credit: Some(
            "The ECMWF and DWD local GRIB2 parameter names and the ECMWF local GRIB1 \
             parameter tables are generated from the definitions shipped with \
             ecCodes (https://github.com/ecmwf/eccodes), © ECMWF, licensed under the \
             Apache License, Version 2.0. ecCodes' NOTICE is below.",
        ),
        terms: Some(ECCODES_NOTICE),
    },
    // The fetch planner's MARS short names, from ecCodes 2.34.1, 2.45.0 and
    // 2.48.0. Behind the umbrella's `fetchplan` feature, which only the npm
    // package's `full/` entry builds. The extension's addon does not. The
    // umbrella crate ships the file in source. The credit starts lowercase so
    // each NOTICE can lead it with its own "In the ..." clause.
    DataNotice {
        tables: &["fieldglass/src/fetchplan/mars_short_names.rs"],
        packages: &[NPM, UMBRELLA],
        credit: Some(
            "the MARS short names the fetch planner checks are generated from \
             ecCodes' definitions (https://github.com/ecmwf/eccodes), © ECMWF, \
             licensed under the Apache License, Version 2.0. ecCodes' NOTICE is below.",
        ),
        terms: Some(ECCODES_NOTICE),
    },
    // wgrib2 v3.8.0's NCEP table, a U.S. federal government work in the public
    // domain (see the generated headers).
    DataNotice {
        tables: &[
            "fieldglass-grib2/src/tables_ncep.rs",
            "fieldglass-grib2/src/tables_wmo_short.rs",
        ],
        packages: &[],
        credit: None,
        terms: None,
    },
];

/// `Copyright.txt` from ParaView (gitlab.kitware.com/paraview/paraview) at tag
/// `v6.2.0`, verbatim. It was `License_v1.2.txt` in older releases.
const PARAVIEW_LICENSE: &str = "\
Copyright (c) 2000 Kitware Inc.

Redistribution and use in source and binary forms, with or without modification,
are permitted provided that the following conditions are met:

* Redistributions of source code must retain the above copyright notice, this list
  of conditions and the following disclaimer.

* Redistributions in binary form must reproduce the above copyright notice, this
  list of conditions and the following disclaimer in the documentation and/or
  other materials provided with the distribution.

* Neither the name of Kitware nor the names of any contributors may be used to
  endorse or promote products derived from this software without specific prior
  written permission.

THIS SOFTWARE IS PROVIDED BY THE COPYRIGHT HOLDERS AND CONTRIBUTORS \"AS IS\" AND
ANY EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED
WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE
DISCLAIMED. IN NO EVENT SHALL THE AUTHORS OR CONTRIBUTORS BE LIABLE FOR ANY
DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES
(INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR SERVICES;
LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER CAUSED AND ON
ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT LIABILITY, OR TORT
(INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY OUT OF THE USE OF THIS
SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH DAMAGE.
";

/// `COPYING` from X.Org `app/rgb` at tag `rgb-1.1.1`, verbatim.
const X11_RGB_COPYING: &str = "\
Copyright 1985, 1989, 1998  The Open Group

Permission to use, copy, modify, distribute, and sell this software and its
documentation for any purpose is hereby granted without fee, provided that
the above copyright notice appear in all copies and that both that
copyright notice and this permission notice appear in supporting
documentation.

The above copyright notice and this permission notice shall be included
in all copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED \"AS IS\", WITHOUT WARRANTY OF ANY KIND, EXPRESS
OR IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF
MERCHANTABILITY, FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.
IN NO EVENT SHALL THE OPEN GROUP BE LIABLE FOR ANY CLAIM, DAMAGES OR
OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE,
ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR
OTHER DEALINGS IN THE SOFTWARE.

Except as contained in this notice, the name of The Open Group shall
not be used in advertising or otherwise to promote the sale, use or
other dealings in this Software without prior written authorization
from The Open Group.

Copyright (c) 1994, 2008, Oracle and/or its affiliates.

Permission is hereby granted, free of charge, to any person obtaining a
copy of this software and associated documentation files (the \"Software\"),
to deal in the Software without restriction, including without limitation
the rights to use, copy, modify, merge, publish, distribute, sublicense,
and/or sell copies of the Software, and to permit persons to whom the
Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice (including the next
paragraph) shall be included in all copies or substantial portions of the
Software.

THE SOFTWARE IS PROVIDED \"AS IS\", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT.  IN NO EVENT SHALL
THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING
FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
";

/// `LICENSE.md` from `wmo-im/GRIB2` at tag `v37`, verbatim. `wmo-im/CCT` at tag
/// `v2026-06-01` has the same file.
const WMO_MIT_LICENSE: &str = "\
# The MIT License (MIT)

Copyright &copy; 2020-2024

* * *

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the \"Software\"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED \"AS IS\", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
";

/// `NOTICE` from `ecmwf/eccodes` at tag `2.34.1`, verbatim but for the blank
/// line that ends the file.
const ECCODES_NOTICE: &str = "\
ECMWF ecCodes
(C) Copyright 2005- ECMWF.

This product includes software developed at ECMWF (http://www.ecmwf.int).

Parts of the definitions provided by WMO (https://public.wmo.int/en)

Microsoft Visual Studio support provided with the help of
Finnish Meteorological Institute (FMI).

Adaptive Entropy Coding (CCSDS) provided by
The Max-Planck-Institute for Meteorology and Deutsches Klimarechenzentrum GmbH.

IBM POWER optimisations were developed at
The Max-Planck-Institute for Meteorology.

Decoding double array optimisations provided by
Norwegian Meteorological Institute.

OpenMP API Support provided by Eric Millin.

Fortran source code for grib_typeSizes.f90 from Robert Pincus
Cooperative Institue for Meteorological Satellite Studies
University of Wisconsin
";
