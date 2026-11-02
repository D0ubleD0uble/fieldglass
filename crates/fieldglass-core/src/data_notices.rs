//! The notices the third-party data tables in this crate ask a redistributor to
//! carry (#1563).
//!
//! The colormap anchors in `colormap_tables` and the X11 color names in
//! `color_names` are other people's data. The colormaps are compiled into the
//! npm package's `.wasm` and the VS Code extension's Node addon; the color
//! names into the addon, which parses color tables, and into the `.wasm` as
//! soon as it does too, so both packages carry their notice. A consumer of
//! either package never sees this repository's README, so each package has a
//! `NOTICE` file of its own, and this list is what those files must contain.
//!
//! The list sits beside the tables so it is the one place a new table gets its
//! terms recorded. The tests below hold it to the colormap registry (every
//! colormap is listed exactly once) and hold both `NOTICE` files to it (every
//! notice is in each, word for word up to line wrapping). A table added
//! without an entry, or a notice dropped from either file, fails `cargo test`.
//!
//! The wording of each `credit` is the README's (`README.md`, "Third-party
//! data"), with its Markdown autolinks written as plain URLs.

/// One source of bundled data and what its terms ask a redistributor to carry.
pub(crate) struct DataNotice {
    /// The tables it covers: colormap wire names, or [`X11_COLOR_NAMES`].
    pub(crate) tables: &'static [&'static str],
    /// The credit line, or `None` for data made here and owing nobody.
    pub(crate) credit: Option<&'static str>,
    /// License text the terms require to travel with every copy, verbatim,
    /// where the package does not already ship it. The Apache License text is
    /// not here: both packages carry `LICENSE-APACHE` or link it already.
    pub(crate) terms: Option<&'static str>,
}

/// The id [`DATA_NOTICES`] gives the X11 color-name table the CPT parser reads.
pub(crate) const X11_COLOR_NAMES: &str = "x11-color-names";

/// Every source of third-party data compiled into this crate.
pub(crate) const DATA_NOTICES: &[DataNotice] = &[
    // ColorBrewer: Apache-2.0 (matplotlib's `LICENSE/LICENSE_COLORBREWER`).
    // The older ColorBrewer terms, v1.1, ask for this exact sentence in
    // end-user documentation, and the README has always carried it.
    DataNotice {
        tables: &["rdbu", "brbg"],
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
        credit: Some(
            "Viridis, plasma and cividis are from matplotlib and are released into \
             the public domain (CC0) by their authors: Nathaniel J. Smith, Stéfan \
             van der Walt and Eric Firing (viridis, plasma), and Jamie R. Nuñez, \
             Christopher R. Anderton and Ryan S. Renslow (cividis).",
        ),
        terms: None,
    },
    DataNotice {
        tables: &["coolwarm"],
        credit: Some(
            "The cool–warm diverging map is by Kenneth Moreland and is released \
             into the public domain (CC0).",
        ),
        terms: None,
    },
    // A black-to-white ramp defined in `tools/gen_colormaps.py`.
    DataNotice {
        tables: &["grayscale"],
        credit: None,
        terms: None,
    },
    // X.Org `app/rgb`, release rgb-1.1.1 (`tools/gen_color_names.py`). Its
    // `COPYING` asks that the copyright and permission notice "be included in
    // all copies or substantial portions of the Software", so the text itself
    // travels, not a pointer to it. It is the whole file, both notices in it.
    DataNotice {
        tables: &[X11_COLOR_NAMES],
        credit: Some(
            "The color names an imported color table may use are the X11 color \
             database (`rgb.txt` from X.Org `app/rgb`), © 1985, 1989, 1998 The Open \
             Group, used under the X11 permission notice in that repository's \
             `COPYING`.",
        ),
        terms: Some(X11_RGB_COPYING),
    },
];

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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{DATA_NOTICES, X11_COLOR_NAMES};
    use crate::color_names::COLOR_NAMES;
    use crate::colormap_tables::COLORMAPS;

    /// The packages that ship these tables, and the `NOTICE` each carries.
    /// `crates/fieldglass-wasm/pack.sh` copies the first into the npm tarball;
    /// `vsce` packages the second from `extension/`.
    const NOTICES: &[(&str, &str)] = &[
        (
            "crates/fieldglass-wasm/npm/NOTICE",
            include_str!("../../fieldglass-wasm/npm/NOTICE"),
        ),
        (
            "extension/NOTICE",
            include_str!("../../../extension/NOTICE"),
        ),
    ];

    /// Collapse every run of whitespace to one space, so a notice wrapped to
    /// a file's line width still matches the list's one-line form.
    fn words(text: &str) -> String {
        text.split_whitespace().collect::<Vec<_>>().join(" ")
    }

    /// Every table the crate compiles in, mapped to how many entries list it.
    fn listed() -> BTreeMap<&'static str, usize> {
        let mut seen = BTreeMap::new();
        for notice in DATA_NOTICES {
            for &table in notice.tables {
                *seen.entry(table).or_insert(0) += 1;
            }
        }
        seen
    }

    #[test]
    fn every_colormap_is_listed_exactly_once() {
        let listed = listed();
        for colormap in COLORMAPS {
            let name: &str = &colormap.name;
            assert_eq!(
                listed.get(name),
                Some(&1),
                "colormap `{name}` must appear in exactly one DATA_NOTICES entry",
            );
        }
    }

    #[test]
    fn the_x11_color_names_are_listed_exactly_once() {
        assert!(!COLOR_NAMES.is_empty());
        assert_eq!(listed().get(X11_COLOR_NAMES), Some(&1));
    }

    #[test]
    fn every_listed_table_exists() {
        for table in listed().keys() {
            assert!(
                *table == X11_COLOR_NAMES || COLORMAPS.iter().any(|c| c.name == *table),
                "DATA_NOTICES lists `{table}`, which is no bundled table",
            );
        }
    }

    #[test]
    fn every_package_notice_carries_every_credit_and_license_text() {
        for (path, text) in NOTICES {
            let text = words(text);
            for notice in DATA_NOTICES {
                for required in [notice.credit, notice.terms].into_iter().flatten() {
                    assert!(
                        text.contains(&words(required)),
                        "{path} is missing the notice for {:?}:\n{required}",
                        notice.tables,
                    );
                }
            }
        }
    }
}
