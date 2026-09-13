//! GMT colour palette tables (`.cpt`): the palette format GMT, cpt-city and
//! the Scientific Colour Maps and cmocean distributions all ship.
//!
//! A CPT is a list of *slices*, each running from one value to the next and
//! from one colour to another:
//!
//! ```text
//! # COLOR_MODEL = RGB
//! 0    10/0/121    0.5  40/0/150
//! 0.5  40/0/150    1    255/255/255
//! B    black
//! F    white
//! N    128/128/128
//! ```
//!
//! A slice whose two colours are equal is a flat band, so a *discrete* table is
//! the same format as a continuous one. `B`, `F` and `N` give the colours below
//! the first slice, above the last, and for missing values.
//!
//! [`parse_cpt`] reads one into a [`ColorTable`], and
//! [`ColorTable::to_colormap`] compiles that to the 256-entry lookup table
//! every colormap paints through. The table spans the slices' own values, low
//! to high; the display range still comes from the field, so the file's own
//! values say *where along the ramp* each colour sits, not which data values
//! get it.
//!
//! What is accepted, following GMT's own CPT documentation:
//!
//! - colours as `r/g/b`, as three separate `r g b` fields, as `#rrggbb`, as a
//!   single grey level, or by X11 name (`white`, `ghost white`, `GhostWhite`),
//!   which is the set GMT reads;
//! - the `L`, `U` and `B` annotation flag at the end of a slice, and a
//!   `;label` after it, both of which are ignored;
//! - `# COLOR_MODEL = RGB`, and `# RANGE = low/high`, which is kept but not
//!   applied.
//!
//! What is refused, with the line it was found on: other colour models (HSV,
//! CMYK), transparency (`r/g/b@50`), pattern fills, categorical tables (one key
//! and one colour per line), and slices that run backwards or leave a gap.

use std::fmt;

use crate::color_names::COLOR_NAMES;
use crate::colormap::{Colormap, ColormapKind};

/// One slice: a linear run from `rgb0` at `z0` to `rgb1` at `z1`.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Slice {
    z0: f64,
    rgb0: [f64; 3],
    z1: f64,
    rgb1: [f64; 3],
}

/// A parsed colour palette table. Build one with [`parse_cpt`].
#[derive(Debug, Clone, PartialEq)]
pub struct ColorTable {
    /// Contiguous and ascending: each slice starts where the last one ended.
    slices: Vec<Slice>,
    background: Option<[u8; 3]>,
    foreground: Option<[u8; 3]>,
    nan: Option<[u8; 3]>,
    range: Option<(f64, f64)>,
}

/// Why a CPT could not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CptError {
    line: Option<usize>,
    message: String,
}

impl CptError {
    fn at(line: usize, message: impl Into<String>) -> Self {
        Self {
            line: Some(line),
            message: message.into(),
        }
    }

    /// The 1-based line the problem is on, when it is on one.
    pub fn line(&self) -> Option<usize> {
        self.line
    }

    /// What is wrong, without the line number.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for CptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(line) => write!(f, "line {line}: {}", self.message),
            None => f.write_str(&self.message),
        }
    }
}

impl std::error::Error for CptError {}

/// Read a CPT. See the [module docs](self) for what is accepted.
pub fn parse_cpt(text: &str) -> Result<ColorTable, CptError> {
    let mut table = ColorTable {
        slices: Vec::new(),
        background: None,
        foreground: None,
        nan: None,
        range: None,
    };
    // The line each slice came from, for the contiguity error.
    let mut slice_lines = Vec::new();

    for (index, raw) in text.lines().enumerate() {
        let line = index + 1;
        let content = raw.trim();
        if content.is_empty() {
            continue;
        }
        if let Some(comment) = content.strip_prefix('#') {
            read_comment(comment, line, &mut table)?;
            continue;
        }
        // A `;label` annotates a slice for a colour bar; it says nothing about
        // colour.
        let content = content.split(';').next().unwrap_or_default();
        let mut fields: Vec<&str> = content.split_whitespace().collect();
        let slot = match fields.first().copied() {
            Some("B") => Some(&mut table.background),
            Some("F") => Some(&mut table.foreground),
            Some("N") => Some(&mut table.nan),
            _ => None,
        };
        if let Some(slot) = slot {
            *slot = Some(to_bytes(parse_color(&fields[1..], line)?));
            continue;
        }
        if matches!(fields.last().copied(), Some("L" | "U" | "B")) {
            fields.pop();
        }
        table.slices.push(parse_slice(&fields, line)?);
        slice_lines.push(line);
    }

    let (Some(first), Some(last)) = (table.slices.first(), table.slices.last()) else {
        return Err(CptError {
            line: None,
            message: "the file holds no colour slices".into(),
        });
    };
    // Written values are rounded to a few decimals, so a boundary one slice ends
    // on and the next starts on is compared with a little room — far less than
    // any slice is wide.
    let tolerance = (last.z1 - first.z0).abs() * 1e-9;
    for (pair, lines) in table.slices.windows(2).zip(slice_lines.windows(2)) {
        if (pair[1].z0 - pair[0].z1).abs() > tolerance {
            let what = if pair[1].z0 > pair[0].z1 {
                "leaves a gap after"
            } else {
                "overlaps"
            };
            return Err(CptError::at(
                lines[1],
                format!(
                    "this slice starts at {} but the one on line {} ends at {}; a slice that {what} \
                     the previous one is not supported",
                    pair[1].z0, lines[0], pair[0].z1
                ),
            ));
        }
    }
    Ok(table)
}

/// The comments GMT gives meaning to. Every other comment is ignored.
fn read_comment(comment: &str, line: usize, table: &mut ColorTable) -> Result<(), CptError> {
    let Some((key, value)) = comment.split_once('=') else {
        return Ok(());
    };
    let value = value.trim();
    match key.trim() {
        "COLOR_MODEL" => {
            let model = value.trim_start_matches('+');
            if !model.eq_ignore_ascii_case("rgb") {
                return Err(CptError::at(
                    line,
                    format!("colour model {value:?} is not supported; only RGB tables are"),
                ));
            }
        }
        "RANGE" => {
            let bounds = value
                .split_once('/')
                .and_then(|(low, high)| Some((number(low)?, number(high)?)));
            let Some(bounds) = bounds else {
                return Err(CptError::at(
                    line,
                    format!("RANGE {value:?} is not two numbers as low/high"),
                ));
            };
            table.range = Some(bounds);
        }
        _ => {}
    }
    Ok(())
}

/// `z0 colour z1 colour`, where each colour is one field or three.
fn parse_slice(fields: &[&str], line: usize) -> Result<Slice, CptError> {
    let (c0, z1, c1): (&[&str], &str, &[&str]) = match fields.len() {
        4 => (&fields[1..2], fields[2], &fields[3..4]),
        8 => (&fields[1..4], fields[4], &fields[5..8]),
        // One colour is a single field and the other is three; which is which
        // shows in the second field.
        6 if number(fields[1]).is_none() => (&fields[1..2], fields[2], &fields[3..6]),
        6 => (&fields[1..4], fields[4], &fields[5..6]),
        2 | 3 => {
            return Err(CptError::at(
                line,
                "this is a categorical table (one key and one colour per line), which is not \
                 supported; import a continuous or banded table",
            ));
        }
        n => {
            return Err(CptError::at(
                line,
                format!("expected `z0 colour z1 colour`, found {n} fields"),
            ));
        }
    };
    let z = |field: &str| {
        number(field).ok_or_else(|| CptError::at(line, format!("{field:?} is not a number")))
    };
    let slice = Slice {
        z0: z(fields[0])?,
        rgb0: parse_color(c0, line)?,
        z1: z(z1)?,
        rgb1: parse_color(c1, line)?,
    };
    if slice.z1 <= slice.z0 {
        return Err(CptError::at(
            line,
            format!(
                "this slice runs from {} down to {}; slices must run from low to high",
                slice.z0, slice.z1
            ),
        ));
    }
    Ok(slice)
}

/// One colour, from its one or three fields, as channel values in `0..=255`.
fn parse_color(fields: &[&str], line: usize) -> Result<[f64; 3], CptError> {
    let channels = |parts: &[&str]| -> Result<[f64; 3], CptError> {
        let mut rgb = [0.0; 3];
        for (slot, part) in rgb.iter_mut().zip(parts) {
            *slot = number(part)
                .filter(|c| (0.0..=255.0).contains(c))
                .ok_or_else(|| {
                    CptError::at(
                        line,
                        format!("{part:?} is not a colour channel from 0 to 255"),
                    )
                })?;
        }
        Ok(rgb)
    };
    match fields {
        [r, g, b] => channels(&[r, g, b]),
        [field] => parse_color_field(field, line, channels),
        _ => Err(CptError::at(
            line,
            format!("expected a colour, found {:?}", fields.join(" ")),
        )),
    }
}

fn parse_color_field(
    field: &str,
    line: usize,
    channels: impl Fn(&[&str]) -> Result<[f64; 3], CptError>,
) -> Result<[f64; 3], CptError> {
    if field.contains('@') {
        return Err(CptError::at(
            line,
            format!("{field:?} sets a transparency, which is not supported"),
        ));
    }
    if let Some(hex) = field.strip_prefix('#') {
        let valid = hex.len() == 6 && hex.bytes().all(|b| b.is_ascii_hexdigit());
        if !valid {
            return Err(CptError::at(
                line,
                format!("{field:?} is not a #rrggbb colour"),
            ));
        }
        let channel = |at: usize| f64::from(u8::from_str_radix(&hex[at..at + 2], 16).unwrap_or(0));
        return Ok([channel(0), channel(2), channel(4)]);
    }
    let slashes: Vec<&str> = field.split('/').collect();
    if slashes.len() == 3 {
        return channels(&slashes);
    }
    let dashes: Vec<&str> = field.split('-').collect();
    if dashes.len() == 3 && dashes.iter().all(|d| number(d).is_some()) {
        return Err(CptError::at(
            line,
            format!("{field:?} is an h-s-v colour, and only RGB tables are supported"),
        ));
    }
    if number(field).is_some() {
        let grey = channels(&[field, field, field])?;
        return Ok(grey);
    }
    if field.starts_with(['p', 'P']) && field[1..].starts_with(|c: char| c.is_ascii_digit()) {
        return Err(CptError::at(
            line,
            format!("{field:?} is a pattern fill, which is not supported"),
        ));
    }
    let folded = field.to_ascii_lowercase();
    COLOR_NAMES
        .binary_search_by(|(name, _)| (*name).cmp(folded.as_str()))
        .map(|at| COLOR_NAMES[at].1.map(f64::from))
        .map_err(|_| CptError::at(line, format!("{field:?} is not a colour name GMT knows")))
}

/// A finite number, or `None`.
fn number(field: &str) -> Option<f64> {
    field.trim().parse::<f64>().ok().filter(|v| v.is_finite())
}

fn to_bytes(rgb: [f64; 3]) -> [u8; 3] {
    rgb.map(|c| c.round().clamp(0.0, 255.0) as u8)
}

impl ColorTable {
    /// The value the first slice starts at and the one the last slice ends at.
    pub fn z_range(&self) -> (f64, f64) {
        // `parse_cpt` refuses a table with no slices, so both ends exist.
        let first = self.slices.first().map_or(0.0, |s| s.z0);
        let last = self.slices.last().map_or(0.0, |s| s.z1);
        (first, last)
    }

    /// How many slices the table holds.
    pub fn slice_count(&self) -> usize {
        self.slices.len()
    }

    /// The `B` colour, for values below the first slice.
    pub fn background(&self) -> Option<[u8; 3]> {
        self.background
    }

    /// The `F` colour, for values above the last slice.
    pub fn foreground(&self) -> Option<[u8; 3]> {
        self.foreground
    }

    /// The `N` colour, for missing values.
    pub fn nan(&self) -> Option<[u8; 3]> {
        self.nan
    }

    /// The `# RANGE = low/high` the file declares, if any. Kept, not applied:
    /// the display range comes from the field.
    pub fn range(&self) -> Option<(f64, f64)> {
        self.range
    }

    /// The colour at `z`, interpolated within its slice. A value on a boundary
    /// takes the slice that starts there, as GMT's `z0 <= z < z1` does; a value
    /// outside the table takes the nearest end.
    pub fn color_at(&self, z: f64) -> [u8; 3] {
        let (low, high) = self.z_range();
        let z = if z.is_nan() { low } else { z.clamp(low, high) };
        // The last slice starting at or below `z`.
        let at = self.slices.partition_point(|s| s.z0 <= z).saturating_sub(1);
        let slice = self.slices[at];
        let t = ((z - slice.z0) / (slice.z1 - slice.z0)).clamp(0.0, 1.0);
        to_bytes(std::array::from_fn(|ch| {
            slice.rgb0[ch] + (slice.rgb1[ch] - slice.rgb0[ch]) * t
        }))
    }

    /// The 256-entry RGB lookup table, low → high: entry `i` is the colour
    /// `i / 255` of the way across [`z_range`](Self::z_range).
    ///
    /// That is where the painter reads entry `i` from as well — it rounds a
    /// value's ramp position to the nearest of 256 — so each entry is the
    /// table's colour at the centre of the values that land on it.
    pub fn lut(&self) -> [u8; 256 * 3] {
        let (low, high) = self.z_range();
        let mut lut = [0u8; 256 * 3];
        for i in 0..256 {
            // The last entry is the top of the table exactly, not whatever a
            // rounded `low + span` comes to.
            let z = if i == 255 {
                high
            } else {
                low + (high - low) * i as f64 / 255.0
            };
            lut[i * 3..i * 3 + 3].copy_from_slice(&self.color_at(z));
        }
        lut
    }

    /// Compile to a [`Colormap`] under `name` and `label`, to paint with.
    pub fn to_colormap(&self, name: impl Into<String>, label: impl Into<String>) -> Colormap {
        Colormap::from_lut(name, label, ColormapKind::Sequential, &self.lut())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> ColorTable {
        parse_cpt(text).unwrap_or_else(|e| panic!("{e}"))
    }

    fn refusal(text: &str) -> CptError {
        parse_cpt(text).expect_err("should be refused")
    }

    #[test]
    fn every_colour_spelling_reads_the_same_colour() {
        for spelling in [
            "0 255/0/0 1 255/0/0",
            "0 255 0 0 1 255 0 0",
            "0 #ff0000 1 #FF0000",
            "0 red 1 Red",
            "0 255/0/0 1 255 0 0",
            "0 255 0 0 1 red",
        ] {
            let table = parse(spelling);
            assert_eq!(table.color_at(0.5), [255, 0, 0], "{spelling}");
        }
    }

    #[test]
    fn a_single_number_is_a_grey_level_and_names_fold_spaces_and_case() {
        assert_eq!(parse("0 128 1 128").color_at(0.0), [128, 128, 128]);
        // X11's `green` is full green, not CSS's half.
        assert_eq!(parse("0 green 1 green").color_at(0.0), [0, 255, 0]);
        assert_eq!(
            parse("0 GhostWhite 1 ghostwhite").color_at(0.0),
            [248, 248, 255]
        );
    }

    #[test]
    fn a_slice_interpolates_linearly_and_a_boundary_takes_the_slice_starting_there() {
        let table = parse("0 0/0/0 10 100/200/250\n10 0/0/0 20 0/0/0\n");
        assert_eq!(table.color_at(0.0), [0, 0, 0]);
        assert_eq!(table.color_at(5.0), [50, 100, 125]);
        // Just below 10 is nearly the first slice's end; 10 itself is the second
        // slice's start.
        assert_eq!(table.color_at(9.999), [100, 200, 250]);
        assert_eq!(table.color_at(10.0), [0, 0, 0]);
        // Outside the table clamps to the nearest end.
        assert_eq!(table.color_at(-5.0), [0, 0, 0]);
        assert_eq!(table.z_range(), (0.0, 20.0));
    }

    #[test]
    fn a_banded_table_keeps_its_steps_in_the_lookup_table() {
        // Two flat bands, half the range each.
        let table = parse("0 blue 1 blue\n1 red 2 red\n");
        let lut = table.lut();
        // Entry 127 sits at 127/255 < 0.5 of the way across: blue. Entry 128 is
        // past the halfway boundary: red. No blended entry between them.
        assert_eq!(&lut[127 * 3..128 * 3], &[0, 0, 255]);
        assert_eq!(&lut[128 * 3..129 * 3], &[255, 0, 0]);
        let colormap = table.to_colormap("bands", "Bands");
        assert_eq!(
            colormap.lut(false),
            lut,
            "the colormap paints the same table"
        );
    }

    #[test]
    fn background_foreground_nan_range_and_annotations_are_read() {
        let table = parse(
            "# COLOR_MODEL = +rgb\n# RANGE = -2/3.5\n\
             0 black 1 white L ;low\n\
             1 white 2 black U\n\
             B 1/2/3\nF #0a0b0c\nN gray\n",
        );
        assert_eq!(table.slice_count(), 2);
        assert_eq!(table.background(), Some([1, 2, 3]));
        assert_eq!(table.foreground(), Some([10, 11, 12]));
        assert_eq!(table.nan(), Some([190, 190, 190]));
        assert_eq!(table.range(), Some((-2.0, 3.5)));
        // Other comments are ignored, including ones with an `=`.
        parse("# Note: hinge = 0\n0 black 1 white\n");
    }

    #[test]
    fn unsupported_constructs_are_refused_on_their_line() {
        for (text, line, needle) in [
            ("# COLOR_MODEL = HSV\n0 0-1-1 1 360-1-1\n", 1, "only RGB"),
            ("0 red 1 blue\n1 255/0/0@50 2 red\n", 2, "transparency"),
            ("0 0-1-1 1 360-1-1\n", 1, "h-s-v"),
            ("0 p7 1 p7\n", 1, "pattern fill"),
            ("# a categorical table\n0 green\n1 blue\n", 2, "categorical"),
            ("0 red 1 blue\n2 red 3 blue\n", 2, "gap"),
            ("0 red 2 blue\n1 red 3 blue\n", 2, "overlaps"),
            ("1 red 0 blue\n", 1, "low to high"),
            ("0 chartreuse-ish 1 red\n", 1, "colour name"),
            ("0 300/0/0 1 red\n", 1, "0 to 255"),
            ("0 #ff00 1 red\n", 1, "#rrggbb"),
            ("zero red 1 blue\n", 1, "not a number"),
            ("0 red 1 blue 2\n", 1, "found 5 fields"),
            ("# RANGE = low\n0 red 1 blue\n", 1, "RANGE"),
            ("0 nan 1 red\n", 1, "colour name"),
        ] {
            let err = refusal(text);
            assert_eq!(err.line(), Some(line), "{text:?}: {err}");
            assert!(err.message().contains(needle), "{text:?}: {err}");
        }
        let empty = refusal("# only comments\nB black\n");
        assert_eq!(empty.line(), None);
        assert!(empty.to_string().contains("no colour slices"));
    }

    #[test]
    fn a_nan_or_infinite_value_never_reaches_a_slice() {
        // `inf` parses as a float; a table must not be built on it.
        let err = refusal("0 red inf blue\n");
        assert!(err.message().contains("not a number"), "{err}");
        // And looking a NaN up answers the low end rather than panicking.
        assert_eq!(parse("0 red 1 blue").color_at(f64::NAN), [255, 0, 0]);
    }
}
