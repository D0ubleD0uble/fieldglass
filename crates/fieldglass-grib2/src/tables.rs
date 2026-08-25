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
        10 => "Copernicus regional reanalysis",
        11 => "Copernicus regional reanalysis test",
        12 => "Destination Earth",
        13 => "Destination Earth test",
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
        12 => "Transverse Mercator",
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
        other => crate::tables_wmo::time_range_unit(other).unwrap_or("Unknown time-range unit"),
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
        other => crate::tables_wmo::fixed_surface(other).unwrap_or("Unknown fixed surface"),
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
        12 => "Return period",
        13 => "Median",
        192..=254 => "Reserved for local use",
        255 => "Missing",
        _ => "Unknown statistical process",
    }
}

/// Who wrote the message and which of their tables to read, for resolving
/// centre-local parameters (#439).
///
/// These are the three §1 fields eccodes keys a local concept on. `centre` and
/// `sub_centre` because a centre may delegate parameter space to one of its
/// sub-centres, and `local_tables_version` because a centre may revise its own
/// table without WMO involvement — ECMWF gates 132 of its entries on it, across
/// two versions, so dropping it would make those unresolvable.
/// `master_tables_version` is deliberately absent: WMO only ever adds or
/// deprecates entries, never renumbers them, so "latest wins" and the version
/// does not select between meanings.
///
/// A struct rather than positional arguments for two reasons. `centre` and
/// `sub_centre` are both `u16`, so as a pair they are silently swappable at
/// every call site. And the centres that plug into the local registry do not
/// all resolve on the same keys — DWD needs §4 context on top of the triple —
/// so this is the one place that grows a field, rather than every signature
/// that threads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Originator {
    /// §1 octets 6-7, WMO Common Code Table C-11.
    pub centre: u16,
    /// §1 octets 8-9, WMO Common Code Table C-12. 0 means "no sub-centre".
    pub sub_centre: u16,
    /// §1 octet 11. The centre's own table revision; 0 or 255 mean "not used".
    pub local_tables_version: u8,
}

impl Originator {
    pub fn new(centre: u16, sub_centre: u16, local_tables_version: u8) -> Self {
        Self {
            centre,
            sub_centre,
            local_tables_version,
        }
    }
}

/// The codes WMO reserves for local use, in every one of discipline, category
/// and parameter number.
///
/// 255 is deliberately outside it: WMO assigns 255 to "missing" in each of the
/// three tables, so a file setting it is declining to say, not pointing at a
/// centre's own definition. Routing it to a local table would invite a centre
/// entry to put a confident name on an absent value.
const LOCAL_USE: std::ops::RangeInclusive<u8> = 192..=254;

/// Whether a triple names local code space, and so should be offered to the
/// originating centre's table before the WMO set.
///
/// Any one of the three being local is enough — a local discipline makes the
/// whole triple the centre's to define, and so does a local category under a
/// standard discipline.
fn is_local_use(discipline: u8, category: u8, number: u8) -> bool {
    LOCAL_USE.contains(&discipline) || LOCAL_USE.contains(&category) || LOCAL_USE.contains(&number)
}

/// Look up a GRIB2 parameter by `(discipline, category, number)` and return
/// `(short_name, long_name, units)`.
///
/// Resolution follows `docs/planning/parameter-table-sources.md`, which is
/// eccodes' and netCDF-java's rule: a triple in local code space is offered to
/// the originating centre's table first, and everything else — plus anything
/// the centre does not define — resolves against the curated subset and then
/// the generated WMO master table.
///
/// The two ranges do not in fact overlap: the master table stops at 191 in all
/// three dimensions, so a local entry can never shadow a WMO one. That is
/// asserted in `tests/local_parameter_dispatch.rs` rather than relied on
/// silently, because it is a property of the upstream data and not of this
/// code.
///
/// Unrecognised triples return `None`; callers should render the numeric
/// triple as a fallback.
pub fn lookup_parameter(
    originator: Originator,
    discipline: u8,
    category: u8,
    number: u8,
) -> Option<(&'static str, &'static str, &'static str)> {
    resolve_parameter(
        originator,
        discipline,
        category,
        number,
        crate::tables_local::lookup,
    )
}

/// The resolution policy, with the centre-local table injected.
///
/// Split out from [`lookup_parameter`] so the policy can be exercised against
/// a stub table while the real registry is still empty (#439 lands the seam
/// before #424-#426 land any data). Testing the policy through a stub is the
/// only way to prove the ordering now; once a real centre table exists this
/// keeps working unchanged.
fn resolve_parameter(
    originator: Originator,
    discipline: u8,
    category: u8,
    number: u8,
    local: impl Fn(Originator, u8, u8, u8) -> Option<(&'static str, &'static str, &'static str)>,
) -> Option<(&'static str, &'static str, &'static str)> {
    if is_local_use(discipline, category, number)
        && let Some(entry) = local(originator, discipline, category, number)
    {
        return Some(entry);
    }
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
        // CLMR, not the ON388 GRIB1 form CLWMR (parameter 153) this once
        // carried: NCO Code Table 4.2-0-1 number 22 is CLMR (#469).
        (0, 1, 22) => ("CLMR", "Cloud mixing ratio", "kg kg⁻¹"),

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

        // Discipline 10 — Oceanographic
        // HTSGW. This once read WVHGT, which is NCO Code Table 4.2-10-0
        // number *5*, "Significant height of wind waves" — a different
        // parameter, so the entry paired one parameter's abbreviation with
        // another's name (#469; same class as the three #415 corrected).
        (10, 0, 3) => ("HTSGW", "Significant height of combined wind+swell", "m"),

        // Everything else falls through to the generated WMO master table.
        // WMO publishes no short names, so the abbreviation comes from a
        // second generated table (wgrib2's centre-0 rows, #469) and is only
        // ever consulted for a triple WMO has already named — 1230 of the
        // 1387 master parameters. The remaining 157 keep an empty
        // abbreviation, which the display seam already handles.
        _ => {
            let (name, units) = crate::tables_wmo::parameter(discipline, category, number)?;
            let abbreviation =
                crate::tables_wmo_short::short_name(discipline, category, number).unwrap_or("");
            (abbreviation, name, units)
        }
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

    /// A stand-in centre table, so the resolution *order* can be proven while
    /// the real registry is still empty (#439 lands the seam ahead of
    /// #424-#426). Answers for one centre only, so "the centre is consulted"
    /// and "the right centre is consulted" are separable.
    fn stub_local(
        originator: Originator,
        discipline: u8,
        category: u8,
        number: u8,
    ) -> Option<(&'static str, &'static str, &'static str)> {
        match (originator.centre, discipline, category, number) {
            // A genuinely local triple, which is the case the seam exists for.
            (7, 0, 192, 1) => Some(("LOCAL", "A centre-local parameter", "K")),
            // A standard triple the master set also defines. A centre must not
            // be able to take this one over — asserted below.
            (7, 0, 0, 0) => Some(("SHADOW", "Must never be reached", "K")),
            _ => None,
        }
    }

    /// The acceptance criterion for #439: a local code prefers the centre's
    /// entry, and a standard code still reaches the master set.
    #[test]
    fn a_local_code_prefers_the_centre_table() {
        let ncep = Originator::new(7, 0, 0);
        assert_eq!(
            resolve_parameter(ncep, 0, 192, 1, stub_local),
            Some(("LOCAL", "A centre-local parameter", "K")),
        );
        // Another centre gets nothing from NCEP's table.
        assert_eq!(
            resolve_parameter(Originator::new(98, 0, 0), 0, 192, 1, stub_local),
            None
        );
        // And with no local table at all, the same triple is unresolved.
        assert_eq!(resolve_parameter(ncep, 0, 192, 1, |_, _, _, _| None), None);
    }

    /// The other half: a standard code is never offered to the centre, so a
    /// local table cannot shadow a WMO parameter even if it tries.
    #[test]
    fn a_standard_code_never_reaches_the_centre_table() {
        let ncep = Originator::new(7, 0, 0);
        assert_eq!(
            resolve_parameter(ncep, 0, 0, 0, stub_local),
            Some(("TMP", "Temperature", "K")),
            "0/0/0 is master space; the stub's SHADOW entry must not win"
        );
    }

    /// A local code the centre does *not* define falls through to the master
    /// chain rather than stopping at the centre — which is what "try local
    /// first, then fall back" means, and is only observable when the local
    /// table is non-empty but incomplete.
    #[test]
    fn an_undefined_local_code_falls_through() {
        let ncep = Originator::new(7, 0, 0);
        // The master set defines nothing at 192+, so this is None — but it is
        // None by falling through, not by the centre answering.
        assert_eq!(resolve_parameter(ncep, 0, 192, 99, stub_local), None);
        // Same triple shape below 192 still resolves, proving the fall-through
        // path is live rather than short-circuited.
        assert_eq!(
            resolve_parameter(ncep, 0, 1, 8, stub_local),
            Some(("APCP", "Total precipitation", "kg m⁻²")),
        );
    }

    /// Any one of the three components being in local space is enough.
    #[test]
    fn local_use_is_any_of_the_three_components() {
        assert!(is_local_use(192, 0, 0));
        assert!(is_local_use(0, 192, 0));
        assert!(is_local_use(0, 0, 192));
        assert!(is_local_use(0, 0, 254));
        assert!(!is_local_use(191, 191, 191));
        // 255 is "missing" in each table, not local use.
        assert!(!is_local_use(255, 0, 0));
        assert!(!is_local_use(0, 255, 0));
        assert!(!is_local_use(0, 0, 255));
    }

    /// The local table version reaches the table too. ECMWF gates 132 of its
    /// entries on it across two versions, so a centre really can mean different
    /// parameters by one triple depending on which revision wrote the file.
    #[test]
    fn the_local_table_version_reaches_the_local_table() {
        fn by_version(
            originator: Originator,
            _: u8,
            _: u8,
            _: u8,
        ) -> Option<(&'static str, &'static str, &'static str)> {
            match originator.local_tables_version {
                1 => Some(("V1", "As table version 1 defines it", "K")),
                228 => Some(("V228", "As table version 228 defines it", "m")),
                _ => None,
            }
        }
        let at =
            |version| resolve_parameter(Originator::new(98, 0, version), 0, 192, 0, by_version);
        assert_eq!(at(1), Some(("V1", "As table version 1 defines it", "K")));
        assert_eq!(
            at(228),
            Some(("V228", "As table version 228 defines it", "m"))
        );
        assert_eq!(
            at(0),
            None,
            "a file declaring no local table gets no local entry"
        );
    }

    /// A sub-centre reaches the table too — some centres delegate parameter
    /// space to one, so it has to be part of the key rather than dropped.
    #[test]
    fn the_sub_centre_reaches_the_local_table() {
        fn by_sub_centre(
            originator: Originator,
            _: u8,
            _: u8,
            _: u8,
        ) -> Option<(&'static str, &'static str, &'static str)> {
            match originator.sub_centre {
                4 => Some(("EMC", "Environmental Modeling Center", "")),
                _ => None,
            }
        }
        assert_eq!(
            resolve_parameter(Originator::new(7, 4, 0), 0, 192, 0, by_sub_centre),
            Some(("EMC", "Environmental Modeling Center", "")),
        );
        assert_eq!(
            resolve_parameter(Originator::new(7, 0, 0), 0, 192, 0, by_sub_centre),
            None
        );
    }

    #[test]
    fn missing_sentinel() {
        assert_eq!(lookup_discipline(255), "Missing");
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
            (10, "Copernicus regional reanalysis"),
            (11, "Copernicus regional reanalysis test"),
            (12, "Destination Earth"),
            (13, "Destination Earth test"),
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
        assert_eq!(
            lookup_parameter(Originator::default(), 0, 0, 0),
            Some(("TMP", "Temperature", "K"))
        );
        assert_eq!(
            lookup_parameter(Originator::default(), 0, 1, 8),
            Some(("APCP", "Total precipitation", "kg m⁻²"))
        );
        assert_eq!(
            lookup_parameter(Originator::default(), 0, 2, 2),
            Some(("UGRD", "U-component of wind", "m s⁻¹"))
        );
        assert_eq!(
            lookup_parameter(Originator::default(), 0, 3, 5),
            Some(("HGT", "Geopotential height", "gpm"))
        );
        // Sea-surface temperature is 10/3/0 "Water temperature", from the
        // generated WMO table — it was long carried at 10/1/2, which is the
        // u-component of current (see `curated_grib1_transcriptions_corrected`).
        assert_eq!(
            lookup_parameter(Originator::default(), 10, 3, 0),
            Some(("WTMP", "Water temperature", "K"))
        );
    }

    /// Walk every `(discipline, category, number)` there is.
    ///
    /// The two properties below are about the *whole* join between WMO's
    /// parameter table and wgrib2's short names, not about any triple a person
    /// would think to pick, so they are asserted exhaustively. The public-seam
    /// half of #469 — coverage counts and spot checks against NCO — lives in
    /// `tests/wmo_master_short_names.rs`; these two are here because they need
    /// `tables_wmo_short` in view, which is private to the crate.
    fn every_triple() -> impl Iterator<Item = (u8, u8, u8)> {
        (0..=255u8).flat_map(|d| (0..=255u8).flat_map(move |c| (0..=255u8).map(move |n| (d, c, n))))
    }

    /// A short name may only ever name a parameter WMO has already defined.
    ///
    /// This is what makes joining a second upstream safe: `resolve_parameter`
    /// consults the short-name table only after `tables_wmo::parameter` has
    /// answered, so a centre-0 row for a triple WMO does not carry is dead
    /// rather than dangerous. Measured at wgrib2 v3.8.0 against WMO v37 there
    /// are none; if a future bump on either pin adds one, this says so instead
    /// of the arm sitting there unreachable and unnoticed.
    #[test]
    fn no_short_name_invents_a_parameter() {
        let orphans: Vec<_> = every_triple()
            .filter(|&(d, c, n)| {
                crate::tables_wmo_short::short_name(d, c, n).is_some()
                    && crate::tables_wmo::parameter(d, c, n).is_none()
            })
            .collect();
        assert!(
            orphans.is_empty(),
            "wgrib2 centre-0 rows with no WMO master entry: {orphans:?}"
        );
    }

    /// Every abbreviation the user sees equals the generated one.
    ///
    /// #469 asked whether the curated arms should win over wgrib2. The answer
    /// is to remove the question: measuring found 39 of the 41 already agreed,
    /// and the two that did not were both wrong — `CLWMR` at 0/1/22 and
    /// `WVHGT` at 10/0/3 were ON388 GRIB1 abbreviations transcribed onto GRIB2
    /// triples, the failure `curated_grib1_transcriptions_corrected` records
    /// three earlier instances of. With those corrected there is nothing left
    /// to arbitrate, so no precedence rule exists to get wrong. This keeps it
    /// that way: a hand-written arm that disagrees with the generated table
    /// fails here rather than quietly shadowing it.
    #[test]
    fn curated_abbreviations_never_shadow_the_generated_table() {
        let disagreements: Vec<_> = every_triple()
            .filter_map(|(d, c, n)| {
                let generated = crate::tables_wmo_short::short_name(d, c, n)?;
                let (shown, _, _) = lookup_parameter(Originator::default(), d, c, n)?;
                (shown != generated).then_some(((d, c, n), shown, generated))
            })
            .collect();
        assert!(
            disagreements.is_empty(),
            "shown abbreviation differs from the generated table: {disagreements:?}"
        );
    }

    /// Three curated entries named the wrong parameter: their triples were
    /// GRIB1 ON388 codes transcribed onto GRIB2 discipline/category/number.
    /// Both WMO Code Table 4.2 (v37) and eccodes disagreed with all three, so
    /// they are gone and the generated master table now answers instead —
    /// with an abbreviation of its own since #469, which is why these read
    /// three columns rather than two.
    ///
    /// Pinned here rather than left to the bulk table so a future edit that
    /// reintroduces a hand-written arm for one of these triples fails loudly.
    #[test]
    fn curated_grib1_transcriptions_corrected() {
        // Was "Density" (GRIB1 ON388 code 89). Density is 0/3/10.
        assert_eq!(
            lookup_parameter(Originator::default(), 0, 3, 9),
            Some(("GPA", "Geopotential height anomaly", "gpm"))
        );
        assert_eq!(
            lookup_parameter(Originator::default(), 0, 3, 10),
            Some(("DEN", "Density", "kg m-3"))
        );

        // Was "Soil moisture content" (GRIB1 ON388 code 86). It is 2/0/3.
        assert_eq!(
            lookup_parameter(Originator::default(), 2, 0, 5),
            Some(("WATR", "Water runoff", "kg m-2"))
        );
        assert_eq!(
            lookup_parameter(Originator::default(), 2, 0, 3),
            Some(("SOILM", "Soil moisture content", "kg m-2"))
        );

        // Was "Sea surface temperature". 10/1/2 is a current component.
        assert_eq!(
            lookup_parameter(Originator::default(), 10, 1, 2),
            Some(("UOGRD", "u-component of current", "m/s"))
        );
    }

    #[test]
    fn parameter_lookup_misses_return_none() {
        assert_eq!(lookup_parameter(Originator::default(), 0, 0, 250), None);
        assert_eq!(lookup_parameter(Originator::default(), 255, 0, 0), None);
    }

    // -----------------------------------------------------------------------
    // Pin-every-arm coverage for the §4 lookup tables. Matches the precedent
    // set by `centre_lookup_pins_curated_ids` — these tests have no logic, so
    // their entire value is catching accidental edits to the WMO IDs during
    // a refactor (e.g. swapping arms 11 and 12 of stat-process when adding
    // a new code).
    // -----------------------------------------------------------------------

    #[test]
    fn generating_process_type_pins_all_arms() {
        for (id, expected) in [
            (0u8, "Analysis"),
            (1, "Initialization"),
            (2, "Forecast"),
            (3, "Bias-corrected forecast"),
            (4, "Ensemble forecast"),
            (5, "Probability forecast"),
            (6, "Forecast error"),
            (7, "Analysis error"),
            (8, "Observation"),
            (9, "Climatological"),
            (10, "Probability-weighted forecast"),
            (11, "Bias-corrected ensemble forecast"),
            (12, "Post-processed analysis"),
            (13, "Post-processed forecast"),
            (14, "Nowcast"),
            (15, "Hindcast"),
            (16, "Physical retrieval"),
            (17, "Regression analysis"),
            (18, "Difference between two forecasts"),
        ] {
            assert_eq!(
                lookup_generating_process_type(id),
                expected,
                "generating process {id}"
            );
        }
    }

    #[test]
    fn time_range_unit_pins_all_arms() {
        for (id, expected) in [
            (0u8, "Minute"),
            (1, "Hour"),
            (2, "Day"),
            (3, "Month"),
            (4, "Year"),
            (5, "Decade (10 years)"),
            (6, "Normal (30 years)"),
            (7, "Century"),
            (10, "3 hours"),
            (11, "6 hours"),
            (12, "12 hours"),
            (13, "Second"),
        ] {
            assert_eq!(lookup_time_range_unit(id), expected, "time-range unit {id}");
        }
    }

    #[test]
    fn fixed_surface_pins_all_arms() {
        for (id, expected) in [
            (1u8, "Ground or water surface"),
            (2, "Cloud base level"),
            (3, "Cloud top level"),
            (4, "Level of 0°C isotherm"),
            (5, "Level of adiabatic condensation lifted from the surface"),
            (6, "Maximum wind level"),
            (7, "Tropopause"),
            (8, "Nominal top of the atmosphere"),
            (9, "Sea bottom"),
            (20, "Isothermal level (K)"),
            (100, "Isobaric surface (Pa)"),
            (101, "Mean sea level"),
            (102, "Specific altitude above mean sea level (m)"),
            (103, "Specified height above ground (m)"),
            (104, "Sigma level"),
            (105, "Hybrid level"),
            (106, "Depth below land surface (m)"),
            (107, "Isentropic (theta) level (K)"),
            (
                108,
                "Level at specified pressure difference from ground (Pa)",
            ),
            (109, "Potential vorticity surface (10⁻⁶ K m² kg⁻¹ s⁻¹)"),
            (117, "Mixed-layer depth"),
            (160, "Depth below sea level (m)"),
            (200, "Entire atmosphere as a single layer"),
            (201, "Entire ocean as a single layer"),
        ] {
            assert_eq!(lookup_fixed_surface(id), expected, "fixed surface {id}");
        }
    }

    #[test]
    fn ensemble_type_pins_all_arms() {
        for (id, expected) in [
            (0u8, "Unperturbed high-resolution control forecast"),
            (1, "Unperturbed low-resolution control forecast"),
            (2, "Negatively perturbed forecast"),
            (3, "Positively perturbed forecast"),
            (4, "Multi-model forecast"),
        ] {
            assert_eq!(lookup_ensemble_type(id), expected, "ensemble type {id}");
        }
    }

    #[test]
    fn statistical_process_pins_all_arms() {
        for (id, expected) in [
            (0u8, "Average"),
            (1, "Accumulation"),
            (2, "Maximum"),
            (3, "Minimum"),
            (4, "Difference (end minus start)"),
            (5, "Root mean square"),
            (6, "Standard deviation"),
            (7, "Covariance"),
            (8, "Difference (start minus end)"),
            (9, "Ratio"),
            (10, "Standardized anomaly"),
            (11, "Summation"),
            (12, "Return period"),
            (13, "Median"),
        ] {
            assert_eq!(
                lookup_statistical_process(id),
                expected,
                "stat process {id}"
            );
        }
    }

    #[test]
    fn parameter_pins_all_curated_triples() {
        for ((d, c, n), expected) in [
            // Discipline 0 — Meteorological
            ((0u8, 0u8, 0u8), ("TMP", "Temperature", "K")),
            ((0, 0, 1), ("VTMP", "Virtual temperature", "K")),
            ((0, 0, 2), ("POT", "Potential temperature", "K")),
            (
                (0, 0, 3),
                ("EPOT", "Pseudo-adiabatic potential temperature", "K"),
            ),
            ((0, 0, 4), ("TMAX", "Maximum temperature", "K")),
            ((0, 0, 5), ("TMIN", "Minimum temperature", "K")),
            ((0, 0, 6), ("DPT", "Dew-point temperature", "K")),
            ((0, 0, 7), ("DEPR", "Dew-point depression", "K")),
            ((0, 0, 8), ("LAPR", "Lapse rate", "K m⁻¹")),
            ((0, 0, 17), ("SKINT", "Skin temperature", "K")),
            ((0, 1, 0), ("SPFH", "Specific humidity", "kg kg⁻¹")),
            ((0, 1, 1), ("RH", "Relative humidity", "%")),
            ((0, 1, 2), ("MIXR", "Humidity mixing ratio", "kg kg⁻¹")),
            ((0, 1, 3), ("PWAT", "Precipitable water", "kg m⁻²")),
            ((0, 1, 7), ("PRATE", "Precipitation rate", "kg m⁻² s⁻¹")),
            ((0, 1, 8), ("APCP", "Total precipitation", "kg m⁻²")),
            (
                (0, 1, 9),
                ("NCPCP", "Large-scale precipitation (non-conv.)", "kg m⁻²"),
            ),
            ((0, 1, 10), ("ACPCP", "Convective precipitation", "kg m⁻²")),
            ((0, 1, 11), ("SNOD", "Snow depth", "m")),
            (
                (0, 1, 13),
                (
                    "WEASD",
                    "Water equivalent of accumulated snow depth",
                    "kg m⁻²",
                ),
            ),
            ((0, 1, 22), ("CLMR", "Cloud mixing ratio", "kg kg⁻¹")),
            (
                (0, 2, 0),
                ("WDIR", "Wind direction (from which blowing)", "° true"),
            ),
            ((0, 2, 1), ("WIND", "Wind speed", "m s⁻¹")),
            ((0, 2, 2), ("UGRD", "U-component of wind", "m s⁻¹")),
            ((0, 2, 3), ("VGRD", "V-component of wind", "m s⁻¹")),
            (
                (0, 2, 8),
                ("VVEL", "Vertical velocity (pressure)", "Pa s⁻¹"),
            ),
            (
                (0, 2, 9),
                ("DZDT", "Vertical velocity (geometric)", "m s⁻¹"),
            ),
            ((0, 2, 10), ("ABSV", "Absolute vorticity", "s⁻¹")),
            ((0, 3, 0), ("PRES", "Pressure", "Pa")),
            ((0, 3, 1), ("PRMSL", "Pressure reduced to MSL", "Pa")),
            ((0, 3, 2), ("PTEND", "Pressure tendency", "Pa s⁻¹")),
            ((0, 3, 5), ("HGT", "Geopotential height", "gpm")),
            ((0, 3, 6), ("DIST", "Geometric height", "m")),
            ((0, 6, 1), ("TCDC", "Total cloud cover", "%")),
            ((0, 6, 3), ("LCDC", "Low cloud cover", "%")),
            ((0, 6, 4), ("MCDC", "Medium cloud cover", "%")),
            ((0, 6, 5), ("HCDC", "High cloud cover", "%")),
            (
                (0, 7, 6),
                ("CAPE", "Convective available potential energy", "J kg⁻¹"),
            ),
            ((0, 7, 7), ("CIN", "Convective inhibition", "J kg⁻¹")),
            // Discipline 2 — Land surface
            (
                (2, 0, 0),
                ("LAND", "Land cover (0=sea, 1=land)", "proportion"),
            ),
            // Discipline 10 — Oceanographic
            (
                (10, 0, 3),
                ("HTSGW", "Significant height of combined wind+swell", "m"),
            ),
        ] {
            assert_eq!(
                lookup_parameter(Originator::default(), d, c, n),
                Some(expected),
                "parameter ({d}/{c}/{n})"
            );
        }
    }
}
