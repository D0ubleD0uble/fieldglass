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

/// Generating-process type (WMO Code Table 4.3).
pub fn lookup_generating_process_type(value: u8) -> &'static str {
    match value {
        0 => "Analysis",
        1 => "Initialization",
        2 => "Forecast",
        3 => "Bias-corrected forecast",
        4 => "Ensemble forecast",
        5 => "Probability forecast",
        6 => "Forecast error",
        7 => "Analysis error",
        8 => "Observation",
        9 => "Climatological",
        10 => "Probability-weighted forecast",
        11 => "Bias-corrected ensemble forecast",
        12 => "Post-processed analysis",
        13 => "Post-processed forecast",
        14 => "Nowcast",
        15 => "Hindcast",
        16 => "Physical retrieval",
        17 => "Regression analysis",
        18 => "Difference between two forecasts",
        192..=254 => "Reserved for local use",
        255 => "Missing",
        _ => "Unknown generating process",
    }
}

/// Indicator of unit of time range (WMO Code Table 4.4) — short label.
pub fn lookup_time_range_unit(value: u8) -> &'static str {
    match value {
        0 => "Minute",
        1 => "Hour",
        2 => "Day",
        3 => "Month",
        4 => "Year",
        5 => "Decade (10 years)",
        6 => "Normal (30 years)",
        7 => "Century",
        10 => "3 hours",
        11 => "6 hours",
        12 => "12 hours",
        13 => "Second",
        255 => "Missing",
        _ => "Unknown time-range unit",
    }
}

/// Type of fixed surface (WMO Code Table 4.5) — short label covering the
/// surface types commonly emitted by NCEP / ECMWF / DWD. Unrecognised codes
/// fall back to `"Unknown fixed surface"` so callers can render the numeric
/// type with the same shape as other tables.
pub fn lookup_fixed_surface(value: u8) -> &'static str {
    match value {
        1 => "Ground or water surface",
        2 => "Cloud base level",
        3 => "Cloud top level",
        4 => "Level of 0°C isotherm",
        5 => "Level of adiabatic condensation lifted from the surface",
        6 => "Maximum wind level",
        7 => "Tropopause",
        8 => "Nominal top of the atmosphere",
        9 => "Sea bottom",
        20 => "Isothermal level (K)",
        100 => "Isobaric surface (Pa)",
        101 => "Mean sea level",
        102 => "Specific altitude above mean sea level (m)",
        103 => "Specified height above ground (m)",
        104 => "Sigma level",
        105 => "Hybrid level",
        106 => "Depth below land surface (m)",
        107 => "Isentropic (theta) level (K)",
        108 => "Level at specified pressure difference from ground (Pa)",
        109 => "Potential vorticity surface (10⁻⁶ K m² kg⁻¹ s⁻¹)",
        117 => "Mixed-layer depth",
        160 => "Depth below sea level (m)",
        200 => "Entire atmosphere as a single layer",
        201 => "Entire ocean as a single layer",
        // 192..=254 is the local-use range — NCEP uses several codes here
        // (e.g. 242 "Convective cloud bottom level"). We don't try to
        // enumerate centre extensions; surface them as the WMO range label.
        192..=254 => "Reserved for local use",
        255 => "Missing",
        _ => "Unknown fixed surface",
    }
}

/// Type of ensemble forecast (WMO Code Table 4.6).
pub fn lookup_ensemble_type(value: u8) -> &'static str {
    match value {
        0 => "Unperturbed high-resolution control forecast",
        1 => "Unperturbed low-resolution control forecast",
        2 => "Negatively perturbed forecast",
        3 => "Positively perturbed forecast",
        4 => "Multi-model forecast",
        192..=254 => "Reserved for local use",
        255 => "Missing",
        _ => "Unknown ensemble type",
    }
}

/// Statistical process applied to derive a field over a time interval
/// (WMO Code Table 4.10).
pub fn lookup_statistical_process(value: u8) -> &'static str {
    match value {
        0 => "Average",
        1 => "Accumulation",
        2 => "Maximum",
        3 => "Minimum",
        4 => "Difference (end minus start)",
        5 => "Root mean square",
        6 => "Standard deviation",
        7 => "Covariance",
        8 => "Difference (start minus end)",
        9 => "Ratio",
        10 => "Standardized anomaly",
        11 => "Summation",
        12 => "Confidence index",
        13 => "Quality indicator",
        192..=254 => "Reserved for local use",
        255 => "Missing",
        _ => "Unknown statistical process",
    }
}

/// Look up a GRIB2 parameter by `(discipline, category, number)` and return
/// `(short_name, long_name, units)` for the curated subset.
///
/// Covers the parameters routinely emitted by NCEP GFS, ECMWF, and the
/// reanalysis archives — temperature, moisture, momentum, mass, and the
/// common derived radar / land-surface fields. Unrecognised triples return
/// `None`; callers should render the numeric triple as a fallback.
pub fn lookup_parameter(
    discipline: u8,
    category: u8,
    number: u8,
) -> Option<(&'static str, &'static str, &'static str)> {
    let entry = match (discipline, category, number) {
        // Discipline 0 — Meteorological products
        // Category 0: Temperature
        (0, 0, 0) => ("TMP", "Temperature", "K"),
        (0, 0, 1) => ("VTMP", "Virtual temperature", "K"),
        (0, 0, 2) => ("POT", "Potential temperature", "K"),
        (0, 0, 3) => ("EPOT", "Pseudo-adiabatic potential temperature", "K"),
        (0, 0, 4) => ("TMAX", "Maximum temperature", "K"),
        (0, 0, 5) => ("TMIN", "Minimum temperature", "K"),
        (0, 0, 6) => ("DPT", "Dew-point temperature", "K"),
        (0, 0, 7) => ("DEPR", "Dew-point depression", "K"),
        (0, 0, 8) => ("LAPR", "Lapse rate", "K m⁻¹"),
        (0, 0, 17) => ("SKINT", "Skin temperature", "K"),

        // Category 1: Moisture
        (0, 1, 0) => ("SPFH", "Specific humidity", "kg kg⁻¹"),
        (0, 1, 1) => ("RH", "Relative humidity", "%"),
        (0, 1, 2) => ("MIXR", "Humidity mixing ratio", "kg kg⁻¹"),
        (0, 1, 3) => ("PWAT", "Precipitable water", "kg m⁻²"),
        (0, 1, 7) => ("PRATE", "Precipitation rate", "kg m⁻² s⁻¹"),
        (0, 1, 8) => ("APCP", "Total precipitation", "kg m⁻²"),
        (0, 1, 9) => ("NCPCP", "Large-scale precipitation (non-conv.)", "kg m⁻²"),
        (0, 1, 10) => ("ACPCP", "Convective precipitation", "kg m⁻²"),
        (0, 1, 11) => ("SNOD", "Snow depth", "m"),
        (0, 1, 13) => (
            "WEASD",
            "Water equivalent of accumulated snow depth",
            "kg m⁻²",
        ),
        (0, 1, 22) => ("CLWMR", "Cloud mixing ratio", "kg kg⁻¹"),

        // Category 2: Momentum
        (0, 2, 0) => ("WDIR", "Wind direction (from which blowing)", "° true"),
        (0, 2, 1) => ("WIND", "Wind speed", "m s⁻¹"),
        (0, 2, 2) => ("UGRD", "U-component of wind", "m s⁻¹"),
        (0, 2, 3) => ("VGRD", "V-component of wind", "m s⁻¹"),
        (0, 2, 8) => ("VVEL", "Vertical velocity (pressure)", "Pa s⁻¹"),
        (0, 2, 9) => ("DZDT", "Vertical velocity (geometric)", "m s⁻¹"),
        (0, 2, 10) => ("ABSV", "Absolute vorticity", "s⁻¹"),

        // Category 3: Mass
        (0, 3, 0) => ("PRES", "Pressure", "Pa"),
        (0, 3, 1) => ("PRMSL", "Pressure reduced to MSL", "Pa"),
        (0, 3, 2) => ("PTEND", "Pressure tendency", "Pa s⁻¹"),
        (0, 3, 5) => ("HGT", "Geopotential height", "gpm"),
        (0, 3, 6) => ("DIST", "Geometric height", "m"),
        (0, 3, 9) => ("DEN", "Density", "kg m⁻³"),

        // Category 6: Cloud
        (0, 6, 1) => ("TCDC", "Total cloud cover", "%"),
        (0, 6, 3) => ("LCDC", "Low cloud cover", "%"),
        (0, 6, 4) => ("MCDC", "Medium cloud cover", "%"),
        (0, 6, 5) => ("HCDC", "High cloud cover", "%"),

        // Category 7: Thermodynamic stability
        (0, 7, 6) => ("CAPE", "Convective available potential energy", "J kg⁻¹"),
        (0, 7, 7) => ("CIN", "Convective inhibition", "J kg⁻¹"),

        // Discipline 2 — Land surface
        (2, 0, 0) => ("LAND", "Land cover (0=sea, 1=land)", "proportion"),
        (2, 0, 5) => ("SOILM", "Soil moisture content", "kg m⁻²"),

        // Discipline 10 — Oceanographic
        (10, 0, 3) => ("WVHGT", "Significant height of combined wind+swell", "m"),
        (10, 1, 2) => ("SST", "Sea surface temperature", "K"),

        _ => return None,
    };
    Some(entry)
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

    #[test]
    fn generating_process_type_table() {
        assert_eq!(lookup_generating_process_type(0), "Analysis");
        assert_eq!(lookup_generating_process_type(2), "Forecast");
        assert_eq!(lookup_generating_process_type(4), "Ensemble forecast");
        assert_eq!(
            lookup_generating_process_type(200),
            "Reserved for local use"
        );
        assert_eq!(lookup_generating_process_type(255), "Missing");
        assert_eq!(
            lookup_generating_process_type(99),
            "Unknown generating process"
        );
    }

    #[test]
    fn time_range_unit_table() {
        assert_eq!(lookup_time_range_unit(0), "Minute");
        assert_eq!(lookup_time_range_unit(1), "Hour");
        assert_eq!(lookup_time_range_unit(11), "6 hours");
        assert_eq!(lookup_time_range_unit(13), "Second");
        assert_eq!(lookup_time_range_unit(255), "Missing");
        assert_eq!(lookup_time_range_unit(99), "Unknown time-range unit");
    }

    #[test]
    fn fixed_surface_table_covers_common_codes() {
        for (id, expected) in [
            (1u8, "Ground or water surface"),
            (100, "Isobaric surface (Pa)"),
            (101, "Mean sea level"),
            (103, "Specified height above ground (m)"),
            (200, "Entire atmosphere as a single layer"),
            (242, "Reserved for local use"),
            (255, "Missing"),
        ] {
            assert_eq!(lookup_fixed_surface(id), expected, "surface {id}");
        }
        // 190 falls outside both the curated list and the local-use range.
        assert_eq!(lookup_fixed_surface(190), "Unknown fixed surface");
    }

    #[test]
    fn ensemble_type_table() {
        assert_eq!(
            lookup_ensemble_type(0),
            "Unperturbed high-resolution control forecast"
        );
        assert_eq!(lookup_ensemble_type(3), "Positively perturbed forecast");
        assert_eq!(lookup_ensemble_type(200), "Reserved for local use");
        assert_eq!(lookup_ensemble_type(255), "Missing");
        assert_eq!(lookup_ensemble_type(99), "Unknown ensemble type");
    }

    #[test]
    fn statistical_process_table() {
        assert_eq!(lookup_statistical_process(0), "Average");
        assert_eq!(lookup_statistical_process(1), "Accumulation");
        assert_eq!(lookup_statistical_process(2), "Maximum");
        assert_eq!(lookup_statistical_process(11), "Summation");
        assert_eq!(lookup_statistical_process(200), "Reserved for local use");
        assert_eq!(lookup_statistical_process(255), "Missing");
        assert_eq!(
            lookup_statistical_process(99),
            "Unknown statistical process"
        );
    }

    #[test]
    fn parameter_lookup_hits_common_ncep_triples() {
        assert_eq!(lookup_parameter(0, 0, 0), Some(("TMP", "Temperature", "K")));
        assert_eq!(
            lookup_parameter(0, 1, 8),
            Some(("APCP", "Total precipitation", "kg m⁻²"))
        );
        assert_eq!(
            lookup_parameter(0, 2, 2),
            Some(("UGRD", "U-component of wind", "m s⁻¹"))
        );
        assert_eq!(
            lookup_parameter(0, 3, 5),
            Some(("HGT", "Geopotential height", "gpm"))
        );
        assert_eq!(
            lookup_parameter(10, 1, 2),
            Some(("SST", "Sea surface temperature", "K"))
        );
    }

    #[test]
    fn parameter_lookup_misses_return_none() {
        assert_eq!(lookup_parameter(0, 0, 250), None);
        assert_eq!(lookup_parameter(255, 0, 0), None);
    }
}
