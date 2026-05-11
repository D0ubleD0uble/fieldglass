//! WMO GRIB2 lookup tables.
//!
//! These are the single source of truth for human-readable names of GRIB2
//! coded values. Extend the tables here rather than hardcoding strings at
//! the napi or TypeScript layer.

/// Look up the human-readable name for a GRIB2 discipline (WMO Code Table 0.0).
///
/// Covers all currently-defined disciplines as of the WMO Manual on Codes
/// Vol I.2 (FM 92 GRIB Edition 2). Returns `"Unknown discipline"` for codes
/// that fall outside the table or land in reserved ranges.
pub fn lookup_discipline(discipline: u8) -> &'static str {
    match discipline {
        0 => "Meteorological products",
        1 => "Hydrological products",
        2 => "Land surface products",
        3 => "Satellite remote sensing products",
        4 => "Space weather products",
        10 => "Oceanographic products",
        20 => "Health and socioeconomic impacts",
        255 => "Missing",
        _ => "Unknown discipline",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_disciplines() {
        assert_eq!(lookup_discipline(0), "Meteorological products");
        assert_eq!(lookup_discipline(1), "Hydrological products");
        assert_eq!(lookup_discipline(2), "Land surface products");
        assert_eq!(lookup_discipline(3), "Satellite remote sensing products");
        assert_eq!(lookup_discipline(4), "Space weather products");
        assert_eq!(lookup_discipline(10), "Oceanographic products");
    }

    #[test]
    fn unknown_falls_back() {
        assert_eq!(lookup_discipline(99), "Unknown discipline");
    }

    #[test]
    fn missing_sentinel() {
        assert_eq!(lookup_discipline(255), "Missing");
    }
}
