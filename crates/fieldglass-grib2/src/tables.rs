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

/// Look up an originating/generating centre (WMO Common Code Table C-1).
///
/// Subset of the full WMO list — the centres most commonly seen in publicly
/// distributed GRIB2 products. Codes in the C-1 table are 16-bit on the
/// wire even though the WMO assignments below 256 are stable across
/// editions. Returns `None` for unknown codes; callers should fall back to
/// rendering the numeric id.
pub fn lookup_centre(centre: u16) -> Option<&'static str> {
    let name = match centre {
        7 => "US National Weather Service - NCEP",
        8 => "US NWS Telecommunications Gateway",
        9 => "US National Weather Service - Other",
        34 => "Tokyo (RSMC) - JMA",
        38 => "Beijing (RSMC) - CMA",
        40 => "Seoul - KMA",
        46 => "INPE",
        54 => "Montreal (RSMC) - CMC",
        58 => "Fleet Numerical Meteorology and Oceanography Center",
        59 => "NOAA Forecast Systems Laboratory",
        60 => "NCAR",
        74 => "UK Met Office - Exeter (RSMC)",
        78 => "Offenbach (RSMC) - DWD",
        80 => "Rome (RSMC)",
        82 => "Norrköping - SMHI",
        85 => "Toulouse (RSMC) - Météo-France",
        86 => "Helsinki - FMI",
        88 => "Oslo - MET Norway",
        94 => "Copenhagen - DMI",
        97 => "European Space Agency (ESA)",
        98 => "European Centre for Medium-Range Weather Forecasts (ECMWF)",
        173 => "NASA",
        _ => return None,
    };
    Some(name)
}

/// Significance of reference time (WMO Code Table 1.2).
pub fn lookup_reference_time_significance(value: u8) -> &'static str {
    match value {
        0 => "Analysis",
        1 => "Start of forecast",
        2 => "Verifying time of forecast",
        3 => "Observation time",
        255 => "Missing",
        _ => "Unknown",
    }
}

/// Production status of processed data (WMO Code Table 1.3).
pub fn lookup_production_status(value: u8) -> &'static str {
    match value {
        0 => "Operational products",
        1 => "Operational test products",
        2 => "Research products",
        3 => "Re-analysis products",
        4 => "TIGGE",
        5 => "TIGGE test",
        6 => "S2S operational products",
        7 => "S2S test products",
        8 => "UERRA",
        9 => "UERRA test",
        10 => "Climate Data Record",
        11 => "Climate projections",
        12 => "Climate Forecast System Reanalysis",
        13 => "Climate Forecast System Reforecasts",
        255 => "Missing",
        _ => "Unknown",
    }
}

/// Grid definition template number (WMO Code Table 3.1) — short label.
pub fn lookup_grid_template(template: u16) -> &'static str {
    match template {
        0 => "Latitude/longitude",
        1 => "Rotated latitude/longitude",
        2 => "Stretched latitude/longitude",
        3 => "Stretched and rotated latitude/longitude",
        10 => "Mercator",
        20 => "Polar stereographic",
        30 => "Lambert conformal",
        31 => "Albers equal area",
        40 => "Gaussian latitude/longitude",
        41 => "Rotated Gaussian latitude/longitude",
        50 => "Spherical harmonic coefficients",
        90 => "Space view perspective",
        100 => "Triangular grid (icosahedral)",
        110 => "Equatorial azimuthal equidistant",
        120 => "Azimuth-range projection",
        140 => "Lambert azimuthal equal area",
        _ => "Unknown grid template",
    }
}

/// Shape of the reference Earth (WMO Code Table 3.2).
pub fn lookup_earth_shape(shape: u8) -> &'static str {
    match shape {
        0 => "Spherical (radius 6 367 470.0 m)",
        1 => "Spherical (custom radius)",
        2 => "Oblate spheroid (IAU 1965)",
        3 => "Oblate spheroid (custom axes)",
        4 => "Oblate spheroid (IAG-GRS80)",
        5 => "Oblate spheroid (WGS84)",
        6 => "Spherical (radius 6 371 229.0 m)",
        7 => "Oblate spheroid (custom axes, m)",
        8 => "Spherical (radius 6 371 200.0 m, derived)",
        9 => "Oblate spheroid (OSGB 1936 / Airy)",
        _ => "Unknown earth shape",
    }
}

/// Type of processed data (WMO Code Table 1.4).
pub fn lookup_data_type(value: u8) -> &'static str {
    match value {
        0 => "Analysis products",
        1 => "Forecast products",
        2 => "Analysis and forecast products",
        3 => "Control forecast products",
        4 => "Perturbed forecast products",
        5 => "Control and perturbed forecast products",
        6 => "Processed satellite observations",
        7 => "Processed radar observations",
        8 => "Event probability",
        192..=254 => "Reserved for local use",
        255 => "Missing",
        _ => "Unknown",
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

    #[test]
    fn known_centres() {
        assert_eq!(
            lookup_centre(98),
            Some("European Centre for Medium-Range Weather Forecasts (ECMWF)")
        );
        assert_eq!(lookup_centre(7), Some("US National Weather Service - NCEP"));
    }

    #[test]
    fn unknown_centre_returns_none() {
        assert_eq!(lookup_centre(0xFFFE), None);
    }

    #[test]
    fn reference_time_significance_table() {
        assert_eq!(lookup_reference_time_significance(0), "Analysis");
        assert_eq!(lookup_reference_time_significance(1), "Start of forecast");
        assert_eq!(lookup_reference_time_significance(255), "Missing");
        assert_eq!(lookup_reference_time_significance(99), "Unknown");
    }

    #[test]
    fn production_status_table() {
        assert_eq!(lookup_production_status(0), "Operational products");
        assert_eq!(lookup_production_status(3), "Re-analysis products");
        assert_eq!(lookup_production_status(255), "Missing");
        assert_eq!(lookup_production_status(99), "Unknown");
    }

    #[test]
    fn data_type_table() {
        assert_eq!(lookup_data_type(1), "Forecast products");
        assert_eq!(lookup_data_type(2), "Analysis and forecast products");
        assert_eq!(lookup_data_type(200), "Reserved for local use");
        assert_eq!(lookup_data_type(255), "Missing");
        assert_eq!(lookup_data_type(99), "Unknown");
    }

    /// Pin every centre arm we curated. Each arm is a constant mapping with
    /// no logic — the value of this test is catching accidental edits to the
    /// centre IDs (e.g. swapping 78 and 80 during a refactor).
    #[test]
    fn centre_lookup_pins_curated_ids() {
        for (id, expected) in [
            (7u16, "US National Weather Service - NCEP"),
            (8, "US NWS Telecommunications Gateway"),
            (9, "US National Weather Service - Other"),
            (34, "Tokyo (RSMC) - JMA"),
            (38, "Beijing (RSMC) - CMA"),
            (40, "Seoul - KMA"),
            (46, "INPE"),
            (54, "Montreal (RSMC) - CMC"),
            (58, "Fleet Numerical Meteorology and Oceanography Center"),
            (59, "NOAA Forecast Systems Laboratory"),
            (60, "NCAR"),
            (74, "UK Met Office - Exeter (RSMC)"),
            (78, "Offenbach (RSMC) - DWD"),
            (80, "Rome (RSMC)"),
            (82, "Norrköping - SMHI"),
            (85, "Toulouse (RSMC) - Météo-France"),
            (86, "Helsinki - FMI"),
            (88, "Oslo - MET Norway"),
            (94, "Copenhagen - DMI"),
            (97, "European Space Agency (ESA)"),
            (
                98,
                "European Centre for Medium-Range Weather Forecasts (ECMWF)",
            ),
            (173, "NASA"),
        ] {
            assert_eq!(lookup_centre(id), Some(expected), "centre {id}");
        }
    }

    #[test]
    fn discipline_lookup_pins_all_arms() {
        for (id, expected) in [
            (0u8, "Meteorological products"),
            (1, "Hydrological products"),
            (2, "Land surface products"),
            (3, "Satellite remote sensing products"),
            (4, "Space weather products"),
            (10, "Oceanographic products"),
            (20, "Health and socioeconomic impacts"),
        ] {
            assert_eq!(lookup_discipline(id), expected, "discipline {id}");
        }
    }

    #[test]
    fn reference_time_significance_pins_all_arms() {
        for (id, expected) in [
            (0u8, "Analysis"),
            (1, "Start of forecast"),
            (2, "Verifying time of forecast"),
            (3, "Observation time"),
        ] {
            assert_eq!(lookup_reference_time_significance(id), expected);
        }
    }

    #[test]
    fn production_status_pins_all_arms() {
        for (id, expected) in [
            (0u8, "Operational products"),
            (1, "Operational test products"),
            (2, "Research products"),
            (3, "Re-analysis products"),
            (4, "TIGGE"),
            (5, "TIGGE test"),
            (6, "S2S operational products"),
            (7, "S2S test products"),
            (8, "UERRA"),
            (9, "UERRA test"),
            (10, "Climate Data Record"),
            (11, "Climate projections"),
            (12, "Climate Forecast System Reanalysis"),
            (13, "Climate Forecast System Reforecasts"),
        ] {
            assert_eq!(lookup_production_status(id), expected, "status {id}");
        }
    }

    #[test]
    fn data_type_pins_all_arms() {
        for (id, expected) in [
            (0u8, "Analysis products"),
            (1, "Forecast products"),
            (2, "Analysis and forecast products"),
            (3, "Control forecast products"),
            (4, "Perturbed forecast products"),
            (5, "Control and perturbed forecast products"),
            (6, "Processed satellite observations"),
            (7, "Processed radar observations"),
            (8, "Event probability"),
        ] {
            assert_eq!(lookup_data_type(id), expected, "data_type {id}");
        }
    }
}
