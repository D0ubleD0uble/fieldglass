//! CSV serialization of a decoded field.
//!
//! Two layouts, matching the export choices a viewer offers:
//!
//! - [`field_to_csv_matrix`] — a 2-D matrix of values, `nj` rows each of `ni`
//!   comma-separated cells, in grid scan order. No coordinates.
//! - [`field_to_csv_long`] — a long `lat,lon,value` table, one row per grid
//!   point, with the geographic coordinates from a caller-supplied forward map.
//!
//! A missing point (`None`) renders as an **empty value cell** in both layouts,
//! so the mask survives a round trip. Values and coordinates are formatted with
//! Rust's shortest round-trippable `f64` representation, so parsing a cell back
//! with `str::parse::<f64>()` recovers the exact stored bit pattern.

/// Format one value cell: the shortest round-trippable form, or empty for a
/// missing point.
fn cell(value: Option<f64>) -> String {
    match value {
        Some(v) => v.to_string(),
        None => String::new(),
    }
}

/// Serialize `values` (grid scan order, `i` fastest) as a 2-D matrix: `nj`
/// rows, each `ni` comma-separated cells. A missing point is an empty cell.
///
/// `values` must hold exactly `ni · nj` entries; extra entries are ignored and
/// a short slice stops early (the caller owns the shape).
pub fn field_to_csv_matrix(values: &[Option<f64>], ni: usize, nj: usize) -> String {
    let mut out = String::new();
    for j in 0..nj {
        for i in 0..ni {
            if i > 0 {
                out.push(',');
            }
            if let Some(v) = values.get(j * ni + i) {
                out.push_str(&cell(*v));
            }
        }
        out.push('\n');
    }
    out
}

/// Serialize `values` as a long `lat,lon,value` table with a header row. `coords`
/// maps a grid index `(i, j)` to `(lat, lon)` in degrees; a point whose
/// coordinate is `None` (a malformed or unlocatable cell) is skipped, so every
/// emitted row carries a real location. A missing *value* still emits its row
/// with an empty value cell.
pub fn field_to_csv_long(
    values: &[Option<f64>],
    ni: usize,
    nj: usize,
    coords: impl Fn(usize, usize) -> Option<(f64, f64)>,
) -> String {
    let mut out = String::from("lat,lon,value\n");
    for j in 0..nj {
        for i in 0..ni {
            let Some((lat, lon)) = coords(i, j) else {
                continue;
            };
            let value = values.get(j * ni + i).copied().flatten();
            out.push_str(&lat.to_string());
            out.push(',');
            out.push_str(&lon.to_string());
            out.push(',');
            out.push_str(&cell(value));
            out.push('\n');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_cell(s: &str) -> Option<f64> {
        if s.is_empty() {
            None
        } else {
            Some(s.parse().expect("cell parses"))
        }
    }

    #[test]
    fn matrix_round_trips_including_missing() {
        // 3×2 grid with a missing hole at (i=1, j=0).
        let values = vec![
            Some(1.5),
            None,
            Some(3.25),
            Some(-4.0),
            Some(5.0),
            Some(6.125),
        ];
        let csv = field_to_csv_matrix(&values, 3, 2);
        assert_eq!(csv, "1.5,,3.25\n-4,5,6.125\n");

        // Round-trip: parse back cell by cell and compare to the input.
        let parsed: Vec<Option<f64>> = csv
            .lines()
            .flat_map(|line| line.split(',').map(parse_cell))
            .collect();
        assert_eq!(parsed, values);
    }

    #[test]
    fn long_round_trips_including_missing() {
        // 2×2 grid; (i=0, j=1) is missing.
        let values = vec![Some(10.0), Some(20.0), None, Some(40.0)];
        // A trivial forward map: lat = j, lon = i.
        let csv = field_to_csv_long(&values, 2, 2, |i, j| Some((j as f64, i as f64)));

        let mut lines = csv.lines();
        assert_eq!(lines.next(), Some("lat,lon,value"));
        let rows: Vec<(f64, f64, Option<f64>)> = lines
            .map(|line| {
                let mut c = line.split(',');
                let lat = c.next().unwrap().parse().unwrap();
                let lon = c.next().unwrap().parse().unwrap();
                let value = parse_cell(c.next().unwrap());
                (lat, lon, value)
            })
            .collect();
        assert_eq!(
            rows,
            vec![
                (0.0, 0.0, Some(10.0)),
                (0.0, 1.0, Some(20.0)),
                (1.0, 0.0, None),
                (1.0, 1.0, Some(40.0)),
            ],
        );
    }

    #[test]
    fn long_skips_unlocatable_points() {
        // A point whose coordinate map returns None is dropped entirely.
        let values = vec![Some(1.0), Some(2.0)];
        let csv = field_to_csv_long(
            &values,
            2,
            1,
            |i, _| {
                if i == 0 { Some((0.0, 0.0)) } else { None }
            },
        );
        assert_eq!(csv, "lat,lon,value\n0,0,1\n");
    }

    #[test]
    fn matrix_empty_grid_is_empty() {
        assert_eq!(field_to_csv_matrix(&[], 0, 0), "");
    }
}
