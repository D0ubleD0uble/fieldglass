//! WMO GRIB2 master code tables — generated, do not edit by hand.
//!
//! Generated from the `wmo-im/GRIB2` release **v37** CSVs by
//! `tools/gen_wmo_grib2_tables.py`. Those tables are published by the World
//! Meteorological Organization under the MIT licence; the parameter
//! definitions themselves are factual data from WMO Manual on Codes (FM 92
//! GRIB), Code Tables 4.2, 4.4, and 4.5.
//!
//! 1387 parameters across 60 discipline/category tables,
//! 87 fixed surface types, 12 time-range units.
//!
//! Deprecated entries are kept and marked: a file in the wild may still use
//! one, and naming it is better than reporting an unknown parameter. Entries
//! WMO marks reserved, reserved-for-local-use, or missing are omitted, so the
//! caller can fall back cleanly instead of showing a placeholder as a name.
//!
//! Regenerate:
//!
//! ```text
//! python3 tools/gen_wmo_grib2_tables.py && cargo fmt
//! ```

/// WMO Code Table 4.2 — parameter by discipline, category, and number.
///
/// Returns `(name, units)`; WMO publishes no short name, so the
/// abbreviation is the caller's to supply (see `tables::lookup_parameter`).
pub(crate) fn parameter(
    discipline: u8,
    category: u8,
    number: u8,
) -> Option<(&'static str, &'static str)> {
    match (discipline, category) {
        (0, 0) => discipline_0_category_0(number),
        (0, 1) => discipline_0_category_1(number),
        (0, 2) => discipline_0_category_2(number),
        (0, 3) => discipline_0_category_3(number),
        (0, 4) => discipline_0_category_4(number),
        (0, 5) => discipline_0_category_5(number),
        (0, 6) => discipline_0_category_6(number),
        (0, 7) => discipline_0_category_7(number),
        (0, 13) => discipline_0_category_13(number),
        (0, 14) => discipline_0_category_14(number),
        (0, 15) => discipline_0_category_15(number),
        (0, 16) => discipline_0_category_16(number),
        (0, 17) => discipline_0_category_17(number),
        (0, 18) => discipline_0_category_18(number),
        (0, 19) => discipline_0_category_19(number),
        (0, 20) => discipline_0_category_20(number),
        (0, 21) => discipline_0_category_21(number),
        (0, 22) => discipline_0_category_22(number),
        (0, 190) => discipline_0_category_190(number),
        (0, 191) => discipline_0_category_191(number),
        (1, 0) => discipline_1_category_0(number),
        (1, 1) => discipline_1_category_1(number),
        (1, 2) => discipline_1_category_2(number),
        (2, 0) => discipline_2_category_0(number),
        (2, 3) => discipline_2_category_3(number),
        (2, 4) => discipline_2_category_4(number),
        (2, 5) => discipline_2_category_5(number),
        (2, 6) => discipline_2_category_6(number),
        (2, 7) => discipline_2_category_7(number),
        (3, 0) => discipline_3_category_0(number),
        (3, 1) => discipline_3_category_1(number),
        (3, 2) => discipline_3_category_2(number),
        (3, 3) => discipline_3_category_3(number),
        (3, 4) => discipline_3_category_4(number),
        (3, 5) => discipline_3_category_5(number),
        (3, 6) => discipline_3_category_6(number),
        (4, 0) => discipline_4_category_0(number),
        (4, 1) => discipline_4_category_1(number),
        (4, 2) => discipline_4_category_2(number),
        (4, 3) => discipline_4_category_3(number),
        (4, 4) => discipline_4_category_4(number),
        (4, 5) => discipline_4_category_5(number),
        (4, 6) => discipline_4_category_6(number),
        (4, 7) => discipline_4_category_7(number),
        (4, 8) => discipline_4_category_8(number),
        (4, 9) => discipline_4_category_9(number),
        (4, 10) => discipline_4_category_10(number),
        (10, 0) => discipline_10_category_0(number),
        (10, 1) => discipline_10_category_1(number),
        (10, 2) => discipline_10_category_2(number),
        (10, 3) => discipline_10_category_3(number),
        (10, 4) => discipline_10_category_4(number),
        (10, 191) => discipline_10_category_191(number),
        (20, 0) => discipline_20_category_0(number),
        (20, 1) => discipline_20_category_1(number),
        (20, 2) => discipline_20_category_2(number),
        (20, 3) => discipline_20_category_3(number),
        (20, 4) => discipline_20_category_4(number),
        (20, 5) => discipline_20_category_5(number),
        (191, 0) => discipline_191_category_0(number),
        _ => None,
    }
}

/// Discipline 0, parameter category 0.
fn discipline_0_category_0(number: u8) -> Option<(&'static str, &'static str)> {
    Some(match number {
        0 => ("Temperature", "K"),
        1 => ("Virtual temperature", "K"),
        2 => ("Potential temperature", "K"),
        3 => (
            "Pseudo-adiabatic potential temperature or equivalent potential temperature",
            "K",
        ),
        4 => ("Maximum temperature", "K"), // deprecated
        5 => ("Minimum temperature", "K"), // deprecated
        6 => ("Dewpoint temperature", "K"),
        7 => ("Dewpoint depression (or deficit)", "K"),
        8 => ("Lapse rate", "K/m"),
        9 => ("Temperature anomaly", "K"),
        10 => ("Latent heat net flux", "W m-2"),
        11 => ("Sensible heat net flux", "W m-2"),
        12 => ("Heat index", "K"),
        13 => ("Wind chill factor", "K"),
        14 => ("Minimum dewpoint depression", "K"), // deprecated
        15 => ("Virtual potential temperature", "K"),
        16 => ("Snow phase change heat flux", "W m-2"),
        17 => ("Skin temperature", "K"),
        18 => ("Snow temperature (top of snow)", "K"),
        19 => ("Turbulent transfer coefficient for heat", "Numeric"),
        20 => ("Turbulent diffusion coefficient for heat", "m2/s"),
        21 => ("Apparent temperature", "K"),
        22 => ("Temperature tendency due to short-wave radiation", "K s-1"),
        23 => ("Temperature tendency due to long-wave radiation", "K s-1"),
        24 => (
            "Temperature tendency due to short-wave radiation, clear sky",
            "K s-1",
        ),
        25 => (
            "Temperature tendency due to long-wave radiation, clear sky",
            "K s-1",
        ),
        26 => ("Temperature tendency due to parameterization", "K s-1"),
        27 => ("Wet-bulb temperature", "K"),
        28 => ("Unbalanced component of temperature", "K"),
        29 => ("Temperature advection", "K s-1"),
        30 => ("Latent heat net flux due to evaporation", "W m-2"),
        31 => ("Latent heat net flux due to sublimation", "W m-2"),
        32 => ("Wet-bulb potential temperature", "K"),
        _ => return None,
    })
}

/// Discipline 0, parameter category 1.
fn discipline_0_category_1(number: u8) -> Option<(&'static str, &'static str)> {
    Some(match number {
        0 => ("Specific humidity", "kg/kg"),
        1 => ("Relative humidity", "%"),
        2 => ("Humidity mixing ratio", "kg/kg"),
        3 => ("Precipitable water", "kg m-2"),
        4 => ("Vapour pressure", "Pa"),
        5 => ("Saturation deficit", "Pa"),
        6 => ("Evaporation", "kg m-2"),
        7 => ("Precipitation rate", "kg m-2 s-1"), // deprecated
        8 => ("Total precipitation", "kg m-2"),    // deprecated
        9 => ("Large-scale precipitation (non-convective)", "kg m-2"), // deprecated
        10 => ("Convective precipitation", "kg m-2"), // deprecated
        11 => ("Snow depth", "m"),
        12 => ("Snowfall rate water equivalent", "kg m-2 s-1"), // deprecated
        13 => ("Water equivalent of accumulated snow depth", "kg m-2"), // deprecated
        14 => ("Convective snow", "kg m-2"),                    // deprecated
        15 => ("Large-scale snow", "kg m-2"),                   // deprecated
        16 => ("Snow melt", "kg m-2"),
        17 => ("Snow age", "d"),
        18 => ("Absolute humidity", "kg m-3"),
        19 => ("Precipitation type", "(Code table 4.201)"),
        20 => ("Integrated liquid water", "kg m-2"),
        21 => ("Condensate", "kg/kg"),
        22 => ("Cloud mixing ratio", "kg/kg"),
        23 => ("Ice water mixing ratio", "kg/kg"),
        24 => ("Rain mixing ratio", "kg/kg"),
        25 => ("Snow mixing ratio", "kg/kg"),
        26 => ("Horizontal moisture convergence", "kg kg-1 s-1"),
        27 => ("Maximum relative humidity", "%"), // deprecated
        28 => ("Maximum absolute humidity", "kg m-3"), // deprecated
        29 => ("Total snowfall", "m"),            // deprecated
        30 => ("Precipitable water category", "(Code table 4.202)"),
        31 => ("Hail", "m"),
        32 => ("Graupel (snow pellets)", "kg/kg"),
        33 => ("Categorical rain", "(Code table 4.222)"),
        34 => ("Categorical freezing rain", "(Code table 4.222)"),
        35 => ("Categorical ice pellets", "(Code table 4.222)"),
        36 => ("Categorical snow", "(Code table 4.222)"),
        37 => ("Convective precipitation rate", "kg m-2 s-1"),
        38 => ("Horizontal moisture divergence", "kg kg-1 s-1"),
        39 => ("Per cent frozen precipitation", "%"),
        40 => ("Potential evaporation", "kg m-2"),
        41 => ("Potential evaporation rate", "W m-2"),
        42 => ("Snow cover", "%"),
        43 => ("Rain fraction of total cloud water", "Proportion"),
        44 => ("Rime factor", "Numeric"),
        45 => ("Total column integrated rain", "kg m-2"),
        46 => ("Total column integrated snow", "kg m-2"),
        47 => ("Large scale water precipitation (non-convective)", "kg m-2"), // deprecated
        48 => ("Convective water precipitation", "kg m-2"),                   // deprecated
        49 => ("Total water precipitation", "kg m-2"),                        // deprecated
        50 => ("Total snow precipitation", "kg m-2"),                         // deprecated
        51 => (
            "Total column water (Vertically integrated total water (vapour + cloud water/ice))",
            "kg m-2",
        ),
        52 => ("Total precipitation rate", "kg m-2 s-1"),
        53 => ("Total snowfall rate water equivalent", "kg m-2 s-1"),
        54 => ("Large scale precipitation rate", "kg m-2 s-1"),
        55 => ("Convective snowfall rate water equivalent", "kg m-2 s-1"),
        56 => ("Large scale snowfall rate water equivalent", "kg m-2 s-1"),
        57 => ("Total snowfall rate", "m/s"),
        58 => ("Convective snowfall rate", "m/s"),
        59 => ("Large scale snowfall rate", "m/s"),
        60 => ("Snow depth water equivalent", "kg m-2"),
        61 => ("Snow density", "kg m-3"),
        62 => ("Snow evaporation", "kg m-2"),
        64 => ("Total column integrated water vapour", "kg m-2"),
        65 => ("Rain precipitation rate", "kg m-2 s-1"),
        66 => ("Snow precipitation rate", "kg m-2 s-1"),
        67 => ("Freezing rain precipitation rate", "kg m-2 s-1"),
        68 => ("Ice pellets precipitation rate", "kg m-2 s-1"),
        69 => ("Total column integrated cloud water", "kg m-2"),
        70 => ("Total column integrated cloud ice", "kg m-2"),
        71 => ("Hail mixing ratio", "kg/kg"),
        72 => ("Total column integrated hail", "kg m-2"),
        73 => ("Hail precipitation rate", "kg m-2 s-1"),
        74 => ("Total column integrated graupel", "kg m-2"),
        75 => ("Graupel (snow pellets) precipitation rate", "kg m-2 s-1"),
        76 => ("Convective rain rate", "kg m-2 s-1"),
        77 => ("Large scale rain rate", "kg m-2 s-1"),
        78 => (
            "Total column integrated water (all components including precipitation)",
            "kg m-2",
        ),
        79 => ("Evaporation rate", "kg m-2 s-1"),
        80 => ("Total condensate", "kg/kg"),
        81 => ("Total column-integrated condensate", "kg m-2"),
        82 => ("Cloud ice mixing-ratio", "kg/kg"),
        83 => ("Specific cloud liquid water content", "kg/kg"),
        84 => ("Specific cloud ice water content", "kg/kg"),
        85 => ("Specific rainwater content", "kg/kg"),
        86 => ("Specific snow water content", "kg/kg"),
        87 => ("Stratiform precipitation rate", "kg m-2 s-1"),
        88 => ("Categorical convective precipitation", "(Code table 4.222)"),
        90 => ("Total kinematic moisture flux", "kg kg-1 m s-1"),
        91 => (
            "u-component (zonal) kinematic moisture flux",
            "kg kg-1 m s-1",
        ),
        92 => (
            "v-component (meridional) kinematic moisture flux",
            "kg kg-1 m s-1",
        ),
        93 => ("Relative humidity with respect to water", "%"),
        94 => ("Relative humidity with respect to ice", "%"),
        95 => ("Freezing or frozen precipitation rate", "kg m-2 s-1"),
        96 => ("Mass density of rain", "kg m-3"),
        97 => ("Mass density of snow", "kg m-3"),
        98 => ("Mass density of graupel", "kg m-3"),
        99 => ("Mass density of hail", "kg m-3"),
        100 => ("Specific number concentration of rain", "kg-1"),
        101 => ("Specific number concentration of snow", "kg-1"),
        102 => ("Specific number concentration of graupel", "kg-1"),
        103 => ("Specific number concentration of hail", "kg-1"),
        104 => ("Number density of rain", "m-3"),
        105 => ("Number density of snow", "m-3"),
        106 => ("Number density of graupel", "m-3"),
        107 => ("Number density of hail", "m-3"),
        108 => (
            "Specific humidity tendency due to parameterization",
            "kg kg-1 s-1",
        ),
        109 => (
            "Mass density of liquid water coating on hail expressed as mass of liquid water per unit volume of air",
            "kg m-3",
        ),
        110 => (
            "Specific mass of liquid water coating on hail expressed as mass of liquid water per unit mass of moist air",
            "kg kg-1",
        ),
        111 => (
            "Mass mixing ratio of liquid water coating on hail expressed as mass of liquid water per unit mass of dry air",
            "kg kg-1",
        ),
        112 => (
            "Mass density of liquid water coating on graupel expressed as mass of liquid water per unit volume of air",
            "kg m-3",
        ),
        113 => (
            "Specific mass of liquid water coating on graupel expressed as mass of liquid water per unit mass of moist air",
            "kg kg-1",
        ),
        114 => (
            "Mass mixing ratio of liquid water coating on graupel expressed as mass of liquid water per unit mass of dry air",
            "kg kg-1",
        ),
        115 => (
            "Mass density of liquid water coating on snow expressed as mass of liquid water per unit volume of air",
            "kg m-3",
        ),
        116 => (
            "Specific mass of liquid water coating on snow expressed as mass of liquid water per unit mass of moist air",
            "kg kg-1",
        ),
        117 => (
            "Mass mixing ratio of liquid water coating on snow expressed as mass of liquid water per unit mass of dry air",
            "kg kg-1",
        ),
        118 => ("Unbalanced component of specific humidity", "kg kg-1"),
        119 => (
            "Unbalanced component of specific cloud liquid water content",
            "kg kg-1",
        ),
        120 => (
            "Unbalanced component of specific cloud ice water content",
            "kg kg-1",
        ),
        121 => ("Fraction of snow cover", "Proportion"),
        122 => ("Precipitation intensity index", "(Code table 4.247)"),
        123 => ("Dominant precipitation type", "(Code table 4.201)"),
        124 => ("Presence of showers", "(Code table 4.222)"),
        125 => ("Presence of blowing snow", "(Code table 4.222)"),
        126 => ("Presence of blizzard", "(Code table 4.222)"),
        127 => (
            "Ice pellets (non-water equivalent) precipitation rate",
            "m/s",
        ),
        128 => ("Total solid precipitation rate", "kg m-2 s-1"),
        129 => ("Effective radius of cloud water", "m"),
        130 => ("Effective radius of rain", "m"),
        131 => ("Effective radius of cloud ice", "m"),
        132 => ("Effective radius of snow", "m"),
        133 => ("Effective radius of graupel", "m"),
        134 => ("Effective radius of hail", "m"),
        135 => ("Effective radius of subgrid liquid clouds", "m"),
        136 => ("Effective radius of subgrid ice clouds", "m"),
        137 => ("Effective aspect ratio of rain", ""),
        138 => ("Effective aspect ratio of cloud ice", ""),
        139 => ("Effective aspect ratio of snow", ""),
        140 => ("Effective aspect ratio of graupel", ""),
        141 => ("Effective aspect ratio of hail", ""),
        142 => ("Effective aspect ratio of subgrid ice clouds", ""),
        143 => ("Potential evaporation rate", "kg m-2 s-1"),
        144 => ("Specific rain water content (convective)", "kg kg-1"),
        145 => ("Specific snow water content (convective)", "kg kg-1"),
        146 => ("Cloud ice precipitation rate", "kg m-2 s-1"),
        147 => ("Character of precipitation", "(Code table 4.249)"),
        148 => ("Snow evaporation rate", "kg m-2 s-1"),
        149 => ("Cloud water mixing ratio", "kg kg-1"),
        150 => (
            "Column integrated eastward water vapour mass flux",
            "kg m-1 s-1",
        ),
        151 => (
            "Column integrated northward water vapour mass flux",
            "kg m-1 s-1",
        ),
        152 => (
            "Column integrated eastward cloud liquid water mass flux",
            "kg m-1 s-1",
        ),
        153 => (
            "Column integrated northward cloud liquid water mass flux",
            "kg m-1 s-1",
        ),
        154 => (
            "Column integrated eastward cloud ice mass flux",
            "kg m-1 s-1",
        ),
        155 => (
            "Column integrated northward cloud ice mass flux",
            "kg m-1 s-1",
        ),
        156 => ("Column integrated eastward rain mass flux", "kg m-1 s-1"),
        157 => ("Column integrated northward rain mass flux", "kg m-1 s-1"),
        158 => ("Column integrated eastward snow mass flux", "kg m-1 s-1"),
        159 => ("Column integrated northward snow mass flux", "kg m-1 s-1"),
        160 => (
            "Column integrated divergence of water vapour mass flux",
            "kg m-2 s-1",
        ),
        161 => (
            "Column integrated divergence of cloud liquid water mass flux",
            "kg m-2 s-1",
        ),
        162 => (
            "Column integrated divergence of cloud ice mass flux",
            "kg m-2 s-1",
        ),
        163 => (
            "Column integrated divergence of rain mass flux",
            "kg m-2 s-1",
        ),
        164 => (
            "Column integrated divergence of snow mass flux",
            "kg m-2 s-1",
        ),
        165 => (
            "Column integrated divergence of total water mass flux",
            "kg m-2 s-1",
        ),
        166 => ("Column integrated water vapour flux", "kg m-2 s-1"),
        167 => ("Total column supercooled liquid water", "kg m-2"),
        168 => (
            "Saturation specific humidity with respect to water",
            "kg m-3",
        ),
        169 => (
            "Total column integrated saturation specific humidity with respect to water",
            "kg m-2",
        ),
        170 => ("Mean mass diameter of hail", "m"),
        171 => ("Estimated maximum diameter of hail", "m"),
        _ => return None,
    })
}

/// Discipline 0, parameter category 2.
fn discipline_0_category_2(number: u8) -> Option<(&'static str, &'static str)> {
    Some(match number {
        0 => ("Wind direction (from which blowing)", "degree true"),
        1 => ("Wind speed", "m/s"),
        2 => ("u-component of wind", "m/s"),
        3 => ("v-component of wind", "m/s"),
        4 => ("Stream function", "m2/s"),
        5 => ("Velocity potential", "m2/s"),
        6 => ("Montgomery stream function", "m2 s-2"),
        7 => ("Sigma coordinate vertical velocity", "/s"),
        8 => ("Vertical velocity (pressure)", "Pa/s"),
        9 => ("Vertical velocity (geometric)", "m/s"),
        10 => ("Absolute vorticity", "/s"),
        11 => ("Absolute divergence", "/s"),
        12 => ("Relative vorticity", "/s"),
        13 => ("Relative divergence", "/s"),
        14 => ("Potential vorticity", "K m2 kg-1 s-1"),
        15 => ("Vertical u-component shear", "/s"),
        16 => ("Vertical v-component shear", "/s"),
        17 => ("Momentum flux, u-component", "N m-2"),
        18 => ("Momentum flux, v-component", "N m-2"),
        19 => ("Wind mixing energy", "J"),
        20 => ("Boundary layer dissipation", "W m-2"),
        21 => ("Maximum wind speed", "m/s"), // deprecated
        22 => ("Wind speed (gust)", "m/s"),
        23 => ("u-component of wind (gust)", "m/s"),
        24 => ("v-component of wind (gust)", "m/s"),
        25 => ("Vertical speed shear", "/s"),
        26 => ("Horizontal momentum flux", "N m-2"),
        27 => ("u-component storm motion", "m/s"),
        28 => ("v-component storm motion", "m/s"),
        29 => ("Drag coefficient", "Numeric"),
        30 => ("Frictional velocity", "m/s"),
        31 => ("Turbulent diffusion coefficient for momentum", "m2/s"),
        32 => ("Eta coordinate vertical velocity", "/s"),
        33 => ("Wind fetch", "m"),
        34 => ("Normal wind component", "m/s"),
        35 => ("Tangential wind component", "m/s"),
        36 => (
            "Amplitude function for Rossby wave envelope for meridional wind",
            "m/s",
        ),
        37 => ("Northward turbulent surface stress", "N m-2 s"), // deprecated
        38 => ("Eastward turbulent surface stress", "N m-2 s"),  // deprecated
        39 => ("Eastward wind tendency due to parameterization", "m s-2"),
        40 => ("Northward wind tendency due to parameterization", "m s-2"),
        41 => ("u-component of geostrophic wind", "m s-1"),
        42 => ("v-component of geostrophic wind", "m s-1"),
        43 => ("Geostrophic wind direction", "degree true"),
        44 => ("Geostrophic wind speed", "m s-1"),
        45 => ("Unbalanced component of divergence", "s-1"),
        46 => ("Vorticity advection", "s-2"),
        47 => ("Surface roughness for heat", "m"),
        48 => ("Surface roughness for moisture", "m"),
        49 => ("Wind stress", "N m-2"),
        50 => ("Eastward wind stress", "N m-2"),
        51 => ("Northward wind stress", "N m-2"),
        52 => ("u-component of wind stress", "N m-2"),
        53 => ("v-component of wind stress", "N m-2"),
        54 => (
            "Natural logarithm of surface roughness length for heat",
            "Numeric",
        ),
        55 => (
            "Natural logarithm of surface roughness length for moisture",
            "Numeric",
        ),
        56 => ("u-component of neutral wind", "m s-1"),
        57 => ("v-component of neutral wind", "m s-1"),
        58 => ("Magnitude of turbulent surface stress", "N m-2"),
        59 => ("Vertical divergence", "s-1"),
        60 => ("Drag thermal coefficient", "Numeric"),
        61 => ("Drag evaporation coefficient", "Numeric"),
        62 => ("Eastward turbulent surface stress", "N m-2"),
        63 => ("Northward turbulent surface stress", "N m-2"),
        64 => (
            "Eastward turbulent surface stress due to orographic form drag",
            "N m-2",
        ),
        65 => (
            "Northward turbulent surface stress due to orographic form drag",
            "N m-2",
        ),
        66 => (
            "Eastward turbulent surface stress due to surface roughness",
            "N m-2",
        ),
        67 => (
            "Northward turbulent surface stress due to surface roughness",
            "N m-2",
        ),
        68 => ("Convective gust", "m s-1"),
        69 => ("Turbulent gust", "m s-1"),
        70 => ("Wind speed threshold for wind erosion", "m s-1"),
        _ => return None,
    })
}

/// Discipline 0, parameter category 3.
fn discipline_0_category_3(number: u8) -> Option<(&'static str, &'static str)> {
    Some(match number {
        0 => ("Pressure", "Pa"),
        1 => ("Pressure reduced to MSL", "Pa"),
        2 => ("Pressure tendency", "Pa/s"),
        3 => ("ICAO Standard Atmosphere Reference Height", "m"),
        4 => ("Geopotential", "m2 s-2"),
        5 => ("Geopotential height", "gpm"),
        6 => ("Geometric height", "m"),
        7 => ("Standard deviation of height", "m"),
        8 => ("Pressure anomaly", "Pa"),
        9 => ("Geopotential height anomaly", "gpm"),
        10 => ("Density", "kg m-3"),
        11 => ("Altimeter setting", "Pa"),
        12 => ("Thickness", "m"),
        13 => ("Pressure altitude", "m"),
        14 => ("Density altitude", "m"),
        15 => ("5-wave geopotential height", "gpm"),
        16 => ("Zonal flux of gravity wave stress", "N m-2"),
        17 => ("Meridional flux of gravity wave stress", "N m-2"),
        18 => ("Planetary boundary layer height", "m"),
        19 => ("5-wave geopotential height anomaly", "gpm"),
        20 => ("Standard deviation of subgrid-scale orography", "m"),
        21 => ("Angle of subgrid-scale orography", "rad"),
        22 => ("Slope of subgrid-scale orography", "Numeric"),
        23 => ("Gravity wave dissipation", "W m-2"),
        24 => ("Anisotropy of subgrid-scale orography", "Numeric"),
        25 => ("Natural logarithm of pressure in Pa", "Numeric"),
        26 => ("Exner pressure", "Numeric"),
        27 => ("Updraught mass flux", "kg m-2 s-1"),
        28 => ("Downdraught mass flux", "kg m-2 s-1"),
        29 => ("Updraught detrainment rate", "kg m-3 s-1"),
        30 => ("Downdraught detrainment rate", "kg m-3 s-1"),
        31 => ("Unbalanced component of logarithm of surface pressure", ""),
        32 => ("Saturation water vapour pressure", "Pa"),
        33 => ("Geometric altitude above mean sea level", "m"),
        34 => ("Geometric height above ground level", "m"),
        35 => (
            "Column integrated divergence of total mass flux",
            "kg m-2 s-1",
        ),
        36 => ("Column integrated eastward total mass flux", "kg m-1 s-1"),
        37 => ("Column integrated northward total mass flux", "kg m-1 s-1"),
        38 => ("Standard deviation of filtered subgrid orography", "m"),
        39 => ("Column integrated mass of atmosphere", "kg m-2"),
        40 => ("Column integrated eastward geopotential flux", "W m-1"),
        41 => ("Column integrated northward geopotential flux", "W m-1"),
        42 => (
            "Column integrated divergence of water geopotential flux",
            "W m-2",
        ),
        43 => ("Column integrated divergence of geopotential flux", "W m-2"),
        44 => ("Height of zero-degree wet-bulb temperature", "m"),
        45 => ("Height of one-degree wet-bulb temperature", "m"),
        46 => ("Pressure departure from hydrostatic state", "Pa"),
        _ => return None,
    })
}

/// Discipline 0, parameter category 4.
fn discipline_0_category_4(number: u8) -> Option<(&'static str, &'static str)> {
    Some(match number {
        0 => ("Net short-wave radiation flux (surface)", "W m-2"), // deprecated
        1 => ("Net short-wave radiation flux (top of atmosphere)", "W m-2"), // deprecated
        2 => ("Short-wave radiation flux", "W m-2"),               // deprecated
        3 => ("Global radiation flux", "W m-2"),
        4 => ("Brightness temperature", "K"),
        5 => ("Radiance (with respect to wave number)", "W m-1 sr-1"),
        6 => ("Radiance (with respect to wavelength)", "W m-3 sr-1"),
        7 => ("Downward short-wave radiation flux", "W m-2"),
        8 => ("Upward short-wave radiation flux", "W m-2"),
        9 => ("Net short wave radiation flux", "W m-2"),
        10 => ("Photosynthetically active radiation", "W m-2"),
        11 => ("Net short-wave radiation flux, clear sky", "W m-2"),
        12 => ("Downward UV radiation", "W m-2"),
        13 => ("Direct short-wave radiation flux", "W m-2"),
        14 => ("Diffuse short-wave radiation flux", "W m-2"),
        15 => (
            "Upward UV radiation emitted/reflected from the Earth's surface",
            "W m-2",
        ),
        50 => ("UV index (under clear sky)", "Numeric"),
        51 => ("UV index", "Numeric"),
        52 => ("Downward short-wave radiation flux, clear sky", "W m-2"),
        53 => ("Upward short-wave radiation flux, clear sky", "W m-2"),
        54 => ("Direct normal short-wave radiation flux", "W m-2"),
        55 => ("UV visible albedo for diffuse radiation", "%"),
        56 => ("UV visible albedo for direct radiation", "%"),
        57 => (
            "UV visible albedo for direct radiation, geometric component",
            "%",
        ),
        58 => (
            "UV visible albedo for direct radiation, isotropic component",
            "%",
        ),
        59 => (
            "UV visible albedo for direct radiation, volumetric component",
            "%",
        ),
        60 => (
            "Photosynthetically active radiation flux, clear sky",
            "W m-2",
        ),
        61 => ("Direct short-wave radiation flux, clear sky", "W m-2"),
        62 => (
            "Direct normal short-wave radiation flux, clear sky",
            "W m-2",
        ),
        63 => ("Diffuse short-wave radiation flux, clear sky", "W m-2"),
        _ => return None,
    })
}

/// Discipline 0, parameter category 5.
fn discipline_0_category_5(number: u8) -> Option<(&'static str, &'static str)> {
    Some(match number {
        0 => ("Net long-wave radiation flux (surface)", "W m-2"), // deprecated
        1 => ("Net long-wave radiation flux (top of atmosphere)", "W m-2"), // deprecated
        2 => ("Long-wave radiation flux", "W m-2"),               // deprecated
        3 => ("Downward long-wave radiation flux", "W m-2"),
        4 => ("Upward long-wave radiation flux", "W m-2"),
        5 => ("Net long-wave radiation flux", "W m-2"),
        6 => ("Net long-wave radiation flux, clear sky", "W m-2"),
        7 => ("Brightness temperature", "K"),
        8 => ("Downward long-wave radiation flux, clear sky", "W m-2"),
        9 => ("Near IR albedo for diffuse radiation", "%"),
        10 => ("Near IR albedo for direct radiation", "%"),
        11 => (
            "Near IR albedo for direct radiation, geometric component",
            "%",
        ),
        12 => (
            "Near IR albedo for direct radiation, isotropic component",
            "%",
        ),
        13 => (
            "Near IR albedo for direct radiation, volumetric component",
            "%",
        ),
        _ => return None,
    })
}

/// Discipline 0, parameter category 6.
fn discipline_0_category_6(number: u8) -> Option<(&'static str, &'static str)> {
    Some(match number {
        0 => ("Cloud ice", "kg m-2"),
        1 => ("Total cloud cover", "%"),
        2 => ("Convective cloud cover", "%"),
        3 => ("Low cloud cover", "%"),
        4 => ("Medium cloud cover", "%"),
        5 => ("High cloud cover", "%"),
        6 => ("Cloud water", "kg m-2"),
        7 => ("Cloud amount", "%"),
        8 => ("Cloud type", "(Code table 4.203)"),
        9 => ("Thunderstorm maximum tops", "m"),
        10 => ("Thunderstorm coverage", "(Code table 4.204)"),
        11 => ("Cloud base", "m"),
        12 => ("Cloud top", "m"),
        13 => ("Ceiling", "m"),
        14 => ("Non-convective cloud cover", "%"),
        15 => ("Cloud work function", "J/kg"),
        16 => ("Convective cloud efficiency", "Proportion"),
        17 => ("Total condensate", "kg/kg"), // deprecated
        18 => ("Total column-integrated cloud water", "kg m-2"), // deprecated
        19 => ("Total column-integrated cloud ice", "kg m-2"), // deprecated
        20 => ("Total column-integrated condensate", "kg m-2"), // deprecated
        21 => ("Ice fraction of total condensate", "Proportion"),
        22 => ("Cloud cover", "%"),
        23 => ("Cloud ice mixing ratio", "kg/kg"), // deprecated
        24 => ("Sunshine", "Numeric"),
        25 => ("Horizontal extent of cumulonimbus (CB)", "%"),
        26 => ("Height of convective cloud base", "m"),
        27 => ("Height of convective cloud top", "m"),
        28 => ("Number of cloud droplets per unit mass of air", "/kg"),
        29 => ("Number of cloud ice particles per unit mass of air", "/kg"),
        30 => ("Number density of cloud droplets", "m-3"),
        31 => ("Number density of cloud ice particles", "m-3"),
        32 => ("Fraction of cloud cover", "Numeric"),
        33 => ("Sunshine duration", "s"),
        34 => ("Surface long-wave effective total cloudiness", "Numeric"),
        35 => ("Surface short-wave effective total cloudiness", "Numeric"),
        36 => ("Fraction of stratiform precipitation cover", "Proportion"),
        37 => ("Fraction of convective precipitation cover", "Proportion"),
        38 => ("Mass density of cloud droplets", "kg m-3"),
        39 => ("Mass density of cloud ice", "kg m-3"),
        40 => ("Mass density of convective cloud water droplets", "kg m-3"),
        47 => ("Volume fraction of cloud water droplets", "Numeric"),
        48 => ("Volume fraction of cloud ice particles", "Numeric"),
        49 => ("Volume fraction of cloud (ice and/or water)", "Numeric"),
        50 => ("Fog", "%"),
        51 => ("Sunshine duration fraction", "Proportion"),
        _ => return None,
    })
}

/// Discipline 0, parameter category 7.
fn discipline_0_category_7(number: u8) -> Option<(&'static str, &'static str)> {
    Some(match number {
        0 => ("Parcel lifted index (to 500 hPa)", "K"),
        1 => ("Best lifted index (to 500 hPa)", "K"),
        2 => ("K index", "K"),
        3 => ("KO index", "K"),
        4 => ("Total totals index", "K"),
        5 => ("Sweat index", "Numeric"),
        6 => ("Convective available potential energy", "J/kg"),
        7 => ("Convective inhibition", "J/kg"),
        8 => ("Storm relative helicity", "J/kg"),
        9 => ("Energy helicity index", "Numeric"),
        10 => ("Surface lifted index", "K"),
        11 => ("Best (4-layer) lifted index", "K"),
        12 => ("Richardson number", "Numeric"),
        13 => ("Showalter index", "K"),
        15 => ("Updraught helicity", "m2 s-2"),
        16 => ("Bulk Richardson number", "Numeric"),
        17 => ("Gradient Richardson number", "Numeric"),
        18 => ("Flux Richardson number", "Numeric"),
        19 => ("Convective available potential energy - shear", "m2 s-2"),
        20 => ("Thunderstorm intensity index", "(Code table 4.246)"),
        21 => ("Storm severity index", "Numeric"),
        22 => ("Reciprocal Obukhov length", "m-1"),
        23 => ("Storm relative helicity – right moving storm", "J/kg"),
        24 => ("Storm relative helicity – left moving storm", "J/kg"),
        25 => ("Effective storm relative helicity – mean flow", "J/kg"),
        26 => (
            "Effective storm relative helicity – right moving storm",
            "J/kg",
        ),
        27 => (
            "Effective storm relative helicity – left moving storm",
            "J/kg",
        ),
        _ => return None,
    })
}

/// Discipline 0, parameter category 13.
fn discipline_0_category_13(number: u8) -> Option<(&'static str, &'static str)> {
    Some(match number {
        0 => ("Aerosol type", "(Code table 4.205)"),
        _ => return None,
    })
}

/// Discipline 0, parameter category 14.
fn discipline_0_category_14(number: u8) -> Option<(&'static str, &'static str)> {
    Some(match number {
        0 => ("Total ozone", "DU"),
        1 => ("Ozone mixing ratio", "kg/kg"),
        2 => ("Total column integrated ozone", "DU"),
        _ => return None,
    })
}

/// Discipline 0, parameter category 15.
fn discipline_0_category_15(number: u8) -> Option<(&'static str, &'static str)> {
    Some(match number {
        0 => ("Base spectrum width", "m/s"),
        1 => ("Base reflectivity", "dB"),
        2 => ("Base radial velocity", "m/s"),
        3 => ("Vertically integrated liquid water (VIL)", "kg m-2"),
        4 => ("Layer-maximum base reflectivity", "dB"),
        5 => ("Precipitation", "kg m-2"), // deprecated
        6 => ("Radar spectra (1)", ""),
        7 => ("Radar spectra (2)", ""),
        8 => ("Radar spectra (3)", ""),
        9 => ("Reflectivity of cloud droplets", "dB"),
        10 => ("Reflectivity of cloud ice", "dB"),
        11 => ("Reflectivity of snow", "dB"),
        12 => ("Reflectivity of rain", "dB"),
        13 => ("Reflectivity of graupel", "dB"),
        14 => ("Reflectivity of hail", "dB"),
        15 => ("Hybrid scan reflectivity", "dB"),
        16 => ("Hybrid scan reflectivity height", "m"),
        17 => ("Precipitation rate", "kg m-2 s-1"),
        18 => ("Radar data quality index", "proportion"),
        19 => ("Radar data quality flag", "Code Table 4.106"),
        20 => ("Layer-maximum precipitation rate", "kg m-2 s-1"),
        _ => return None,
    })
}

/// Discipline 0, parameter category 16.
fn discipline_0_category_16(number: u8) -> Option<(&'static str, &'static str)> {
    Some(match number {
        0 => ("Equivalent radar reflectivity factor for rain", "mm6 m-3"),
        1 => ("Equivalent radar reflectivity factor for snow", "mm6 m-3"),
        2 => (
            "Equivalent radar reflectivity factor for parameterized convection",
            "mm6 m-3",
        ),
        3 => ("Echo top", "m"),
        4 => ("Reflectivity", "dB"),
        5 => ("Composite reflectivity", "dB"),
        6 => ("Precipitation rate", "kg m-2 s-1"),
        7 => ("Layer-maximum precipitation rate", "kg m-2 s-1"),
        8 => ("Layer-maximum reflectivity", "dB"),
        _ => return None,
    })
}

/// Discipline 0, parameter category 17.
fn discipline_0_category_17(number: u8) -> Option<(&'static str, &'static str)> {
    Some(match number {
        0 => ("Lightning strike density", "m-2 s-1"),
        1 => ("Lightning potential index (LPI)", "J kg-1"),
        2 => ("Cloud-to-ground lightning flash density", "km-2 day-1"),
        3 => ("Cloud-to-cloud lightning flash density", "km-2 day-1"),
        4 => ("Total lightning flash density", "km-2 day-1"),
        5 => ("Subgrid-scale lightning potential index", "J kg-1"),
        _ => return None,
    })
}

/// Discipline 0, parameter category 18.
fn discipline_0_category_18(number: u8) -> Option<(&'static str, &'static str)> {
    Some(match number {
        0 => ("Air activity concentration of caesium 137", "Bq m-3"),
        1 => ("Air activity concentration of iodine 131", "Bq m-3"),
        2 => (
            "Air activity concentration of radioactive pollutant",
            "Bq m-3",
        ),
        3 => ("Ground deposition activity of caesium 137", "Bq m-2"),
        4 => ("Ground deposition activity of iodine 131", "Bq m-2"),
        5 => (
            "Ground deposition activity of radioactive pollutant",
            "Bq m-2",
        ),
        6 => (
            "Time-integrated air activity concentration of caesium pollutant",
            "Bq s m-3",
        ),
        7 => (
            "Time-integrated air activity concentration of iodine pollutant",
            "Bq s m-3",
        ),
        8 => (
            "Time-integrated air activity concentration of radioactive pollutant",
            "Bq s m-3",
        ),
        10 => ("Air activity concentration", "Bq m-3"),
        11 => ("Wet deposition activity", "Bq m-2"),
        12 => ("Dry deposition activity", "Bq m-2"),
        13 => ("Total deposition activity (wet + dry)", "Bq m-2"),
        14 => ("Specific activity concentration", "Bq kg-1"),
        15 => ("Maximum of air activity concentration in layer", "Bq m-3"),
        16 => ("Height of maximum air activity concentration", "m"),
        17 => ("Column-integrated air activity concentration", "Bq m-2"),
        18 => (
            "Column-averaged air activity concentration in layer",
            "Bq m-3",
        ),
        19 => ("Deposition activity arrival", "s"),
        20 => ("Deposition activity ended", "s"),
        21 => ("Cloud activity arrival", "s"),
        22 => ("Cloud activity ended", "s"),
        23 => ("Effective dose rate", "nSv h-1"),
        24 => ("Thyroid dose rate (adult)", "nSv h-1"),
        25 => ("Gamma dose rate (adult)", "nSv h-1"),
        26 => ("Activity emission", "Bq s-1"),
        _ => return None,
    })
}

/// Discipline 0, parameter category 19.
fn discipline_0_category_19(number: u8) -> Option<(&'static str, &'static str)> {
    Some(match number {
        0 => ("Visibility", "m"),
        1 => ("Albedo", "%"),
        2 => ("Thunderstorm probability", "%"),
        3 => ("Mixed layer depth", "m"),
        4 => ("Volcanic ash", "(Code table 4.206)"),
        5 => ("Icing top", "m"),
        6 => ("Icing base", "m"),
        7 => ("Icing", "(Code table 4.207)"),
        8 => ("Turbulence top", "m"),
        9 => ("Turbulence base", "m"),
        10 => ("Turbulence", "(Code table 4.208)"),
        11 => ("Turbulent kinetic energy", "J/kg"),
        12 => ("Planetary boundary-layer regime", "(Code table 4.209)"),
        13 => ("Contrail intensity", "(Code table 4.210)"),
        14 => ("Contrail engine type", "(Code table 4.211)"),
        15 => ("Contrail top", "m"),
        16 => ("Contrail base", "m"),
        17 => ("Maximum snow albedo", "%"), // deprecated
        18 => ("Snow free albedo", "%"),
        19 => ("Snow albedo", "%"),
        20 => ("Icing", "%"),
        21 => ("In-cloud turbulence", "%"),
        22 => ("Clear air turbulence (CAT)", "%"),
        23 => ("Supercooled large droplet probability", "%"),
        24 => ("Convective turbulent kinetic energy", "J/kg"),
        25 => ("Weather", "(Code table 4.225)"),
        26 => ("Convective outlook", "(Code table 4.224)"),
        27 => ("Icing scenario", "(Code table 4.227)"),
        28 => (
            "Mountain wave turbulence (eddy dissipation rate)",
            "m2/3 s-1",
        ),
        29 => ("Clear air turbulence (CAT)", "m2/3 s-1"),
        30 => ("Eddy dissipation parameter", "m2/3 s-1"),
        31 => ("Maximum of eddy dissipation parameter in layer", "m2/3 s-1"),
        32 => ("Highest freezing level", "m"),
        33 => ("Visibility through liquid fog", "m"),
        34 => ("Visibility through ice fog", "m"),
        35 => ("Visibility through blowing snow", "m"),
        36 => ("Presence of snow squalls", "(Code table 4.222)"),
        37 => ("Icing severity", "(Code table 4.228)"),
        38 => ("Sky transparency index", "(Code table 4.214)"),
        39 => ("Seeing index", "(Code table 4.214)"),
        40 => ("Snow level", "m"),
        41 => ("Duct base height", "m"),
        42 => ("Trapping layer base height", "m"),
        43 => ("Trapping layer top height", "m"),
        44 => (
            "Mean vertical gradient of refractivity inside trapping layer",
            "m-1",
        ),
        45 => (
            "Minimum vertical gradient of refractivity inside trapping layer",
            "m-1",
        ),
        46 => ("Net radiation flux", "W m-2"),
        47 => ("Global irradiance on tilted surfaces", "W m-2"),
        48 => ("Top of persistent contrails", "m"),
        49 => ("Base of persistent contrails", "m"),
        50 => (
            "Convectively-induced turbulence (CIT) (eddy dissipation rate)",
            "m2/3 s-1",
        ),
        51 => ("Visibility through precipitation", "m"),
        52 => ("Hail kinetic energy flux", "J m-2 s-1"),
        _ => return None,
    })
}

/// Discipline 0, parameter category 20.
fn discipline_0_category_20(number: u8) -> Option<(&'static str, &'static str)> {
    Some(match number {
        0 => ("Mass density (concentration)", "kg m-3"),
        1 => ("Column-integrated mass density", "kg m-2"),
        2 => ("Mass mixing ratio (mass fraction in air)", "kg/kg"),
        3 => ("Atmosphere emission mass flux", "kg m-2 s-1"),
        4 => ("Atmosphere net production mass flux", "kg m-2 s-1"),
        5 => (
            "Atmosphere net production and emission mass flux",
            "kg m-2 s-1",
        ),
        6 => ("Surface dry deposition mass flux", "kg m-2 s-1"),
        7 => ("Surface wet deposition mass flux", "kg m-2 s-1"),
        8 => ("Atmosphere re-emission mass flux", "kg m-2 s-1"),
        9 => (
            "Wet deposition by large-scale precipitation mass flux",
            "kg m-2 s-1",
        ),
        10 => (
            "Wet deposition by convective precipitation mass flux",
            "kg m-2 s-1",
        ),
        11 => ("Sedimentation mass flux", "kg m-2 s-1"),
        12 => ("Dry deposition mass flux", "kg m-2 s-1"),
        13 => ("Transfer from hydrophobic to hydrophilic", "kg kg-1 s-1"),
        14 => (
            "Transfer from SO2 (sulphur dioxide) to SO4 (sulphate)",
            "kg kg-1 s-1",
        ),
        15 => ("Dry deposition velocity", "m/s"),
        16 => ("Mass mixing ratio with respect to dry air", "kg/kg"),
        17 => ("Mass mixing ratio with respect to wet air", "kg/kg"),
        18 => ("Potential of hydrogen (pH)", "pH"),
        19 => (
            "Loss rate due to reaction with hydroxyl radical (OH)",
            "kg kg-1 s-1",
        ),
        20 => ("Photolysis rate", "s-1"),
        21 => ("Emisssion potential", "kg m-2 s-1"),
        50 => ("Amount in atmosphere", "mol"),
        51 => ("Concentration in air", "mol m-3"),
        52 => ("Volume mixing ratio (fraction in air)", "mol/mol"),
        53 => (
            "Chemical gross production rate of concentration",
            "mol m-3 s-1",
        ),
        54 => (
            "Chemical gross destruction rate of concentration",
            "mol m-3 s-1",
        ),
        55 => ("Surface flux", "mol m-2 s-1"),
        56 => ("Changes of amount in atmosphere", "mol/s"),
        57 => ("Total yearly average burden of the atmosphere", "mol"),
        58 => ("Total yearly averaged atmospheric loss", "mol/s"),
        59 => ("Aerosol number concentration", "m-3"),
        60 => ("Aerosol specific number concentration", "kg-1"),
        61 => ("Maximum of mass density in layer", "kg m-3"),
        62 => ("Height of maximum mass density", "m"),
        63 => ("Column-averaged mass density in layer", "kg m-3"),
        64 => ("Mole fraction with respect to dry air", "mol/mol"),
        65 => ("Mole fraction with respect to wet air", "mol/mol"),
        66 => (
            "Column-integrated in-cloud scavenging rate by precipitation",
            "kg m-2 s-1",
        ),
        67 => (
            "Column-integrated below-cloud scavenging rate by precipitation",
            "kg m-2 s-1",
        ),
        68 => (
            "Column-integrated release rate from evaporating precipitation",
            "kg m-2 s-1",
        ),
        69 => (
            "Column-integrated in-cloud scavenging rate by large-scale precipitation",
            "kg m-2 s-1",
        ),
        70 => (
            "Column-integrated below-cloud scavenging rate by large-scale precipitation",
            "kg m-2 s-1",
        ),
        71 => (
            "Column-integrated release rate from evaporating large-scale precipitation",
            "kg m-2 s-1",
        ),
        72 => (
            "Column-integrated in-cloud scavenging rate by convective precipitation",
            "kg m-2 s-1",
        ),
        73 => (
            "Column-integrated below-cloud scavenging rate by convective precipitation",
            "kg m-2 s-1",
        ),
        74 => (
            "Column-integrated release rate from evaporating convective precipitation",
            "kg m-2 s-1",
        ),
        75 => ("Wildfire flux", "kg m-2 s-1"),
        76 => ("Emission rate", "kg kg-1 s-1"),
        77 => ("Surface emission flux", "kg m-2 s-1"),
        78 => ("Column integrated eastward mass flux", "kg m-1 s-1"),
        79 => ("Column integrated northward mass flux", "kg m-1 s-1"),
        80 => ("Column integrated divergence of mass flux", "kg m-2 s-1"),
        81 => ("Column integrated net source", "kg m-2 s-1"),
        82 => ("Sink mass flux", "kg m-2 s-1"),
        83 => ("Source mass flux", "kg m-2 s-1"),
        84 => ("Volume-mean total column mixing ratio", "mol mol-1"),
        100 => ("Surface area density (aerosol)", "m-1"),
        101 => ("Vertical visual range", "m"),
        102 => ("Aerosol optical thickness", "Numeric"),
        103 => ("Single scattering albedo", "Numeric"),
        104 => ("Asymmetry factor", "Numeric"),
        105 => ("Aerosol extinction coefficient", "m-1"),
        106 => ("Aerosol absorption coefficient", "m-1"),
        107 => ("Aerosol lidar backscatter from satellite", "m-1 sr-1"),
        108 => ("Aerosol lidar backscatter from the ground", "m-1 sr-1"),
        109 => ("Aerosol lidar extinction from satellite", "m-1"),
        110 => ("Aerosol lidar extinction from the ground", "m-1"),
        111 => ("Angstrom exponent", "Numeric"),
        112 => ("Absorption aerosol optical thickness", "Numeric"),
        113 => ("Aerosol backscatter coefficient", "m-1 sr-1"),
        _ => return None,
    })
}

/// Discipline 0, parameter category 21.
fn discipline_0_category_21(number: u8) -> Option<(&'static str, &'static str)> {
    Some(match number {
        0 => ("Column integrated potential + internal energy", "J m-2"),
        1 => ("Column integrated kinetic energy", "J m-2"),
        2 => ("Column integrated total energy", "J m-2"),
        3 => ("Column integrated enthalpy", "J m-2"),
        4 => ("Column integrated water enthalpy", "J m-2"),
        5 => ("Column integrated eastward enthalpy flux", "W m-1"),
        6 => ("Column integrated northward enthalpy flux", "W m-1"),
        7 => ("Column integrated eastward potential energy flux", "W m-1"),
        8 => ("Column integrated northward potential energy flux", "W m-1"),
        9 => ("Column integrated eastward kinetic energy flux", "W m-1"),
        10 => ("Column integrated northward kinetic energy flux", "W m-1"),
        11 => ("Column integrated eastward total energy flux", "W m-1"),
        12 => ("Column integrated northward total energy flux", "W m-1"),
        13 => ("Divergence of column integrated enthalpy flux", "W m-2"),
        14 => (
            "Divergence of column integrated potential energy flux",
            "W m-2",
        ),
        15 => (
            "Divergence of column integrated water potential energy flux",
            "W m-2",
        ),
        16 => (
            "Divergence of column integrated kinetic energy flux",
            "W m-2",
        ),
        17 => ("Divergence of column integrated total energy flux", "W m-2"),
        18 => (
            "Divergence of column integrated water enthalpy flux",
            "W m-2",
        ),
        19 => ("Column integrated eastward heat flux", "W m-1"),
        20 => ("Column integrated northward heat flux", "W m-1"),
        21 => (
            "Column integrated potential+internal+latent energy",
            "J m-2",
        ),
        22 => ("Eady growth rate", "day-1"),
        _ => return None,
    })
}

/// Discipline 0, parameter category 22.
fn discipline_0_category_22(number: u8) -> Option<(&'static str, &'static str)> {
    Some(match number {
        0 => ("Standard Precipitation Index (SPI)", "dimensionless"),
        1 => (
            "Standardized Precipitation Evapotranspiration Index (SPEI)",
            "dimensionless",
        ),
        2 => ("Standardized Streamflow Index (SSFI)", "dimensionless"),
        3 => (
            "Standardized Reservoir Supply Index (SRSI)",
            "dimensionless",
        ),
        4 => ("Standardized Water-level Index (SWI)", "dimensionless"),
        5 => (
            "Standardized Snowmelt and Rain Index (SMRI)",
            "dimensionless",
        ),
        6 => ("Streamflow Drought Index (SDI)", "dimensionless"),
        _ => return None,
    })
}

/// Discipline 0, parameter category 190.
fn discipline_0_category_190(number: u8) -> Option<(&'static str, &'static str)> {
    Some(match number {
        0 => ("Arbitrary text string", "CCITT IA5"),
        _ => return None,
    })
}

/// Discipline 0, parameter category 191.
fn discipline_0_category_191(number: u8) -> Option<(&'static str, &'static str)> {
    Some(match number {
        0 => (
            "Seconds prior to initial reference time (defined in Section 1)",
            "s",
        ),
        1 => ("Geographical latitude", "deg N"),
        2 => ("Geographical longitude", "deg E"),
        3 => ("Days since last observation", "d"),
        4 => ("Tropical cyclone density track", "Numeric"),
        5 => ("Hurricane track in spatiotemporal vicinity", "boolean"),
        6 => ("Tropical storm track in spatiotemporal vicinity", "boolean"),
        7 => (
            "Tropical depression track in spatiotemporal vicinity",
            "boolean",
        ),
        _ => return None,
    })
}

/// Discipline 1, parameter category 0.
fn discipline_1_category_0(number: u8) -> Option<(&'static str, &'static str)> {
    Some(match number {
        0 => (
            "Flash flood guidance (Encoded as an accumulation over a floating subinterval of time between the reference time and valid time)",
            "kg m-2",
        ),
        1 => (
            "Flash flood runoff (Encoded as an accumulation over a floating subinterval of time)",
            "kg m-2",
        ),
        2 => ("Remotely-sensed snow cover", "(Code table 4.215)"),
        3 => ("Elevation of snow-covered terrain", "(Code table 4.216)"),
        4 => ("Snow water equivalent per cent of normal", "%"),
        5 => ("Baseflow-groundwater runoff", "kg m-2"),
        6 => ("Storm surface runoff", "kg m-2"),
        7 => ("Discharge from rivers or streams", "m3/s"),
        8 => ("Groundwater upper storage", "kg m-2"),
        9 => ("Groundwater lower storage", "kg m-2"),
        10 => ("Side flow into river channel", "m3 s-1 m-1"),
        11 => ("River storage of water", "m3"),
        12 => ("Floodplain storage of water", "m3"),
        13 => ("Water on soil surface", "kg m-2"),
        14 => ("Upstream accumulated precipitation", "kg m-2"),
        15 => ("Upstream accumulated snow melt", "kg m-2"),
        16 => ("Percolation rate", "kg m-2 s-1"),
        17 => ("River outflow of water", "m3 s-1"),
        18 => ("Floodplain outflow of water", "m3 s-1"),
        19 => ("Floodpath outflow of water", "m3 s-1"),
        20 => ("Water on surface", "kg m-2"),
        21 => ("Water surface elevation", "m"),
        22 => ("Groundwater return flow rate", "m3 s-1"),
        23 => ("River and floodplain storage", "m3"),
        24 => ("Depth averaged river velocity", "m s-1"),
        _ => return None,
    })
}

/// Discipline 1, parameter category 1.
fn discipline_1_category_1(number: u8) -> Option<(&'static str, &'static str)> {
    Some(match number {
        0 => (
            "Conditional per cent precipitation amount fractile for an overall period (Encoded as an accumulation)",
            "kg m-2",
        ),
        1 => (
            "Per cent precipitation in a sub-period of an overall period (Encoded as per cent accumulation over the sub-period)",
            "%",
        ),
        2 => ("Probability of 0.01 inch of precipitation (POP)", "%"),
        _ => return None,
    })
}

/// Discipline 1, parameter category 2.
fn discipline_1_category_2(number: u8) -> Option<(&'static str, &'static str)> {
    Some(match number {
        0 => ("Water depth", "m"),
        1 => ("Water temperature", "K"),
        2 => ("Water fraction", "Proportion"),
        3 => ("Sediment thickness", "m"),
        4 => ("Sediment temperature", "K"),
        5 => ("Ice thickness", "m"),
        6 => ("Ice temperature", "K"),
        7 => ("Ice cover", "Proportion"),
        8 => ("Land cover (0 = water, 1 = land)", "Proportion"),
        9 => ("Shape factor with respect to salinity profile", ""),
        10 => (
            "Shape factor with respect to temperature profile in thermocline",
            "",
        ),
        11 => (
            "Attenuation coefficient of water with respect to solar radiation",
            "m-1",
        ),
        12 => ("Salinity", "kg/kg"),
        13 => ("Cross-sectional area of flow in channel", "m2"),
        14 => ("Snow temperature", "K"),
        15 => ("Lake depth", "m"),
        16 => ("River depth", "m"),
        17 => ("Floodplain depth", "m"),
        18 => ("Floodplain flooded fraction", "proportion"),
        19 => ("Floodplain flooded area", "m2"),
        20 => ("River fraction", "proportion"),
        21 => ("River area", "m2"),
        22 => (
            "Fraction of river coverage plus river related flooding",
            "proportion",
        ),
        23 => ("Area of river coverage plus river related flooding", "m2"),
        _ => return None,
    })
}

/// Discipline 2, parameter category 0.
fn discipline_2_category_0(number: u8) -> Option<(&'static str, &'static str)> {
    Some(match number {
        0 => ("Land cover (0 = sea, 1 = land)", "Proportion"),
        1 => ("Surface roughness", "m"),
        2 => ("Soil temperature", "K"),           // deprecated
        3 => ("Soil moisture content", "kg m-2"), // deprecated
        4 => ("Vegetation", "%"),
        5 => ("Water runoff", "kg m-2"),
        6 => ("Evapotranspiration", "kg-2 s-1"),
        7 => ("Model terrain height", "m"),
        8 => ("Land use", "(Code table 4.212)"),
        9 => ("Volumetric soil moisture content", "Proportion"), // deprecated
        10 => ("Ground heat flux", "W m-2"),                     // deprecated
        11 => ("Moisture availability", "%"),
        12 => ("Exchange coefficient", "kg m-2 s-1"),
        13 => ("Plant canopy surface water", "kg m-2"),
        14 => ("Blackadar’s mixing length scale", "m"),
        15 => ("Canopy conductance", "m/s"),
        16 => ("Minimal stomatal resistance", "s/m"),
        17 => ("Wilting point", "Proportion"), // deprecated
        18 => ("Solar parameter in canopy conductance", "Proportion"),
        19 => ("Temperature parameter in canopy", "Proportion"),
        20 => ("Humidity parameter in canopy conductance", "Proportion"),
        21 => (
            "Soil moisture parameter in canopy conductance",
            "Proportion",
        ),
        22 => ("Soil moisture", "kg m-3"), // deprecated
        23 => ("Column-integrated soil water", "kg m-2"), // deprecated
        24 => ("Heat flux", "W m-2"),
        25 => ("Volumetric soil moisture", "m3 m-3"),
        26 => ("Wilting point", "kg m-3"),
        27 => ("Volumetric wilting point", "m3 m-3"),
        28 => ("Leaf area index", "Numeric"),
        29 => ("Evergreen forest cover", "Proportion"),
        30 => ("Deciduous forest cover", "Proportion"),
        31 => ("Normalized differential vegetation index (NDVI)", "Numeric"),
        32 => ("Root depth of vegetation", "m"),
        33 => ("Water runoff and drainage", "kg m-2"),
        34 => ("Surface water runoff", "kg m-2"),
        35 => ("Tile class", "(Code table 4.243)"),
        36 => ("Tile fraction", "Proportion"),
        37 => ("Tile percentage", "%"),
        38 => ("Soil volumetric ice content (water equivalent)", "m3 m-3"),
        39 => ("Evapotranspiration rate", "kg m-2 s-1"),
        40 => ("Potential evapotranspiration rate", "kg m-2 s-1"),
        41 => ("Snow melt rate", "kg m-2 s-1"),
        42 => ("Water runoff and drainage rate", "kg m-2 s-1"),
        43 => ("Drainage direction", "(Code table 4.250)"),
        44 => ("Upstream area", "m2"),
        45 => ("Wetland cover", "Proportion"),
        46 => ("Wetland type", "(Code table 4.239)"),
        47 => ("Irrigation cover", "Proportion"),
        48 => ("C4 crop cover", "Proportion"),
        49 => ("C4 grass cover", "Proportion"),
        50 => ("Skin reservoir content", "kg m-2"),
        51 => ("Surface runoff rate", "kg m-2 s-1"),
        52 => ("Subsurface runoff rate", "kg m-2 s-1"),
        53 => ("Low-vegetation cover", "Proportion"),
        54 => ("High-vegetation cover", "Proportion"),
        55 => ("Leaf area index, low-vegetation", "m2 m-2"),
        56 => ("Leaf area index, high-vegetation", "m2 m-2"),
        57 => ("Type of low-vegetation", "Code table 4.234"),
        58 => ("Type of high-vegetation", "Code table 4.234"),
        59 => ("Net ecosystem exchange flux", "kg m-2 s-1"),
        60 => ("Gross primary production flux", "kg m-2 s-1"),
        61 => ("Ecosystem respiration flux", "kg m-2 s-1"),
        62 => ("Emissivity", "Proportion"),
        63 => ("Canopy air temperature", "K"),
        _ => return None,
    })
}

/// Discipline 2, parameter category 3.
fn discipline_2_category_3(number: u8) -> Option<(&'static str, &'static str)> {
    Some(match number {
        0 => ("Soil type", "(Code table 4.213)"),
        1 => ("Upper layer soil temperature", "K"), // deprecated
        2 => ("Upper layer soil moisture", "kg m-3"), // deprecated
        3 => ("Lower layer soil moisture", "kg m-3"), // deprecated
        4 => ("Bottom layer soil temperature", "K"), // deprecated
        5 => ("Liquid volumetric soil moisture (non-frozen)", "Proportion"), // deprecated
        6 => ("Number of soil layers in root zone", "Numeric"),
        7 => ("Transpiration stress-onset (soil moisture)", "Proportion"), // deprecated
        8 => ("Direct evaporation cease (soil moisture)", "Proportion"),   // deprecated
        9 => ("Soil porosity", "Proportion"),                              // deprecated
        10 => ("Liquid volumetric soil moisture (non-frozen)", "m3 m-3"),
        11 => (
            "Volumetric transpiration stress-onset (soil moisture)",
            "m3 m-3",
        ),
        12 => ("Transpiration stress-onset (soil moisture)", "kg m-3"),
        13 => (
            "Volumetric direct evaporation cease (soil moisture)",
            "m3 m-3",
        ),
        14 => ("Direct evaporation cease (soil moisture)", "kg m-3"),
        15 => ("Soil porosity", "m3 m-3"),
        16 => ("Volumetric saturation of soil moisture", "m3 m-3"),
        17 => ("Saturation of soil moisture", "kg m-3"),
        18 => ("Soil temperature", "K"),
        19 => ("Soil moisture", "kg m-3"),
        20 => ("Column-integrated soil moisture", "kg m-2"),
        21 => ("Soil ice", "kg m-3"),
        22 => ("Column-integrated soil ice", "kg m-2"),
        23 => ("Liquid water in snow pack", "kg m-2"),
        24 => ("Frost index", "K day-1"),
        25 => ("Snow depth at elevation bands", "kg m-2"),
        26 => ("Soil heat flux", "W m-2"),
        27 => ("Soil depth", "m"),
        28 => ("Snow temperature", "K"),
        29 => ("Ice temperature", "K"),
        30 => ("Soil wetness index", "Numeric"),
        _ => return None,
    })
}

/// Discipline 2, parameter category 4.
fn discipline_2_category_4(number: u8) -> Option<(&'static str, &'static str)> {
    Some(match number {
        0 => ("Fire outlook", "(Code table 4.224)"),
        1 => ("Fire outlook due to dry thunderstorm", "(Code table 4.224)"),
        2 => ("Haines index", "Numeric"),
        3 => ("Fire burned area", "%"),
        4 => ("Fosberg index", "Numeric"),
        5 => (
            "Forest Fire Weather Index (as defined by the Canadian Forest Service)",
            "Numeric",
        ),
        6 => (
            "Fine Fuel Moisture Code (as defined by the Canadian Forest Service)",
            "Numeric",
        ),
        7 => (
            "Duff Moisture Code (as defined by the Canadian Forest Service)",
            "Numeric",
        ),
        8 => (
            "Drought Code (as defined by the Canadian Forest Service)",
            "Numeric",
        ),
        9 => (
            "Initial Fire Spread Index (as defined by the Canadian Forest Service)",
            "Numeric",
        ),
        10 => (
            "Fire Buildup Index (as defined by the Canadian Forest Service)",
            "Numeric",
        ),
        11 => (
            "Fire Daily Severity Rating (as defined by the Canadian Forest Service)",
            "Numeric",
        ),
        12 => ("Keetch-Byram drought index", "Numeric"),
        13 => (
            "Drought factor (as defined by the Australian forest service )",
            "Numeric",
        ),
        14 => (
            "Rate of spread (as defined by the Australian forest service )",
            "m/s",
        ),
        15 => (
            "Fire danger index (as defined by the Australian forest service )",
            "Numeric",
        ),
        16 => (
            "Spread component (as defined by the US Forest Service National Fire Danger Rating System)",
            "Numeric",
        ),
        17 => (
            "Burning index (as defined by the US Forest Service National Fire Danger Rating System)",
            "Numeric",
        ),
        18 => (
            "Ignition component (as defined by the US Forest Service National Fire Danger Rating System)",
            "%",
        ),
        19 => (
            "Energy release component (as defined by the US Forest Service National Fire Danger Rating System)",
            "Joule/m2",
        ),
        20 => ("Burning area", "%"),
        21 => ("Burnable area", "%"),
        22 => ("Unburnable area", "%"),
        23 => ("Fuel load", "kg m-2"),
        24 => ("Combustion completeness", "%"),
        25 => ("Fuel moisture content", "kg kg-1"),
        26 => (
            "Wildfire potential (as defined by the US NOAA Global Systems Laboratory)",
            "Numeric",
        ),
        27 => ("Live leaf fuel load", "kg m-2"),
        28 => ("Live wood fuel load", "kg m-2"),
        29 => ("Dead leaf fuel load", "kg m-2"),
        30 => ("Dead wood fuel load", "kg m-2"),
        31 => ("Live fuel moisture content", "kg kg-1"),
        32 => ("Fine dead leaf moisture content", "kg kg-1"),
        33 => ("Dense dead leaf moisture content", "kg kg-1"),
        34 => ("Fine dead wood moisture content", "kg kg-1"),
        35 => ("Dense dead wood moisture content", "kg kg-1"),
        36 => ("Fire radiative power", "W"),
        37 => ("Live fuel moisture content in low vegetation", "kg kg-1"),
        38 => ("Live fuel moisture content in high vegetation", "kg kg-1"),
        39 => ("Mean height of maximum injection", "m"),
        40 => ("Injection height", "m"),
        41 => ("Plume bottom height", "m"),
        42 => ("Plume top height", "m"),
        43 => ("Probability of fire detection", "%"),
        44 => ("Probability of ignition from lightning", "%"),
        _ => return None,
    })
}

/// Discipline 2, parameter category 5.
fn discipline_2_category_5(number: u8) -> Option<(&'static str, &'static str)> {
    Some(match number {
        0 => ("Glacier cover", "Proportion"),
        1 => ("Glacier temperature", "K"),
        _ => return None,
    })
}

/// Discipline 2, parameter category 6.
fn discipline_2_category_6(number: u8) -> Option<(&'static str, &'static str)> {
    Some(match number {
        0 => ("Urban cover", "Proportion"),
        1 => ("Road cover", "Proportion"),
        2 => ("Building cover", "Proportion"),
        3 => ("Building height", "m"),
        4 => ("Vertical-to-horizontal area fraction", "m2 m-2"),
        5 => ("Standard deviation of building height", "m"),
        6 => ("Distance downward from roof surface", "m"),
        7 => ("Distance inward from outer wall surface", "m"),
        8 => ("Distance downward from road surface", "m"),
        _ => return None,
    })
}

/// Discipline 2, parameter category 7.
fn discipline_2_category_7(number: u8) -> Option<(&'static str, &'static str)> {
    Some(match number {
        0 => ("Heat net flux", "W m-2"),
        1 => ("Latent heat net flux", "W m-2"),
        2 => ("Sensible heat net flux", "W m-2"),
        _ => return None,
    })
}

/// Discipline 3, parameter category 0.
fn discipline_3_category_0(number: u8) -> Option<(&'static str, &'static str)> {
    Some(match number {
        0 => ("Scaled radiance", "Numeric"),
        1 => ("Scaled albedo", "Numeric"),
        2 => ("Scaled brightness temperature", "Numeric"),
        3 => ("Scaled precipitable water", "Numeric"),
        4 => ("Scaled lifted index", "Numeric"),
        5 => ("Scaled cloud top pressure", "Numeric"),
        6 => ("Scaled skin temperature", "Numeric"),
        7 => ("Cloud mask", "(Code table 4.217)"),
        8 => ("Pixel scene type", "(Code table 4.218)"),
        9 => ("Fire detection indicator", "(Code table 4.223)"),
        _ => return None,
    })
}

/// Discipline 3, parameter category 1.
fn discipline_3_category_1(number: u8) -> Option<(&'static str, &'static str)> {
    Some(match number {
        0 => ("Estimated precipitation", "kg m-2"),
        1 => ("Instantaneous rain rate", "kg m-2 s-1"),
        2 => ("Cloud top height", "m"),
        3 => ("Cloud top height quality indicator", "(Code table 4.219)"),
        4 => ("Estimated u-component of wind", "m/s"),
        5 => ("Estimated v-component of wind", "m/s"),
        6 => ("Number of pixel used", "Numeric"),
        7 => ("Solar zenith angle", "deg"),
        8 => ("Relative azimuth angle", "deg"),
        9 => ("Reflectance in 0.6 micron channel", "%"),
        10 => ("Reflectance in 0.8 micron channel", "%"),
        11 => ("Reflectance in 1.6 micron channel", "%"),
        12 => ("Reflectance in 3.9 micron channel", "%"),
        13 => ("Atmospheric divergence", "/s"),
        14 => ("Cloudy brightness temperature", "K"),
        15 => ("Clear-sky brightness temperature", "K"),
        16 => (
            "Cloudy radiance (with respect to wave number)",
            "W m-1 sr-1",
        ),
        17 => (
            "Clear-sky radiance (with respect to wave number)",
            "W m-1 sr-1",
        ),
        19 => ("Wind speed", "m/s"),
        20 => ("Aerosol optical thickness at 0.635 μm", ""),
        21 => ("Aerosol optical thickness at 0.810 μm", ""),
        22 => ("Aerosol optical thickness at 1.640 μm", ""),
        23 => ("Angstrom coefficient", ""),
        24 => ("Cosine of the solar zenith angle", "Numeric"),
        27 => ("Bidirectional reflectance factor", "Numeric"),
        28 => ("Brightness temperature", "K"),
        29 => ("Scaled radiance", "Numeric"),
        30 => ("Reflectance in 0.4 micron channel", "%"),
        31 => ("Cloudy reflectance", "%"),
        32 => ("Clear reflectance", "%"),
        98 => (
            "Correlation coefficient between MPE rain-rates for the co-located IR data and the microwave data rain-rates",
            "Numeric",
        ),
        99 => (
            "Standard deviation between MPE rain-rates for the co-located IR data and the microwave data rain-rates",
            "kg m-2 s-1",
        ),
        _ => return None,
    })
}

/// Discipline 3, parameter category 2.
fn discipline_3_category_2(number: u8) -> Option<(&'static str, &'static str)> {
    Some(match number {
        0 => ("Clear sky probability", "%"),
        1 => ("Cloud top temperature", "K"),
        2 => ("Cloud top pressure", "Pa"),
        3 => ("Cloud type", "(Code table 4.218)"),
        4 => ("Cloud phase", "(Code table 4.218)"),
        5 => ("Cloud optical depth", "Numeric"),
        6 => ("Cloud particle effective radius", "m"),
        7 => ("Cloud liquid water path", "kg m-2"),
        8 => ("Cloud ice water path", "kg m-2"),
        9 => ("Cloud albedo", "Numeric"),
        10 => ("Cloud emissivity", "Numeric"),
        11 => ("Effective absorption optical depth ratio", "Numeric"),
        30 => ("Measurement cost", "Numeric"),
        31 => ("Upper layer cloud optical depth", "Numeric"), // deprecated
        32 => ("Upper layer cloud top pressure", "Pa"),       // deprecated
        33 => ("Upper layer cloud effective radius", "m"),    // deprecated
        34 => ("Error in upper layer cloud optical depth", "Numeric"), // deprecated
        35 => ("Error in upper layer cloud top pressure", "Pa"), // deprecated
        36 => ("Error in upper layer cloud effective radius", "m"), // deprecated
        37 => ("Lower layer cloud optical depth", "Numeric"), // deprecated
        38 => ("Lower layer cloud top pressure", "Pa"),       // deprecated
        39 => ("Error in lower layer cloud optical depth", "Numeric"), // deprecated
        40 => ("Error in lower layer cloud top pressure", "Pa"), // deprecated
        _ => return None,
    })
}

/// Discipline 3, parameter category 3.
fn discipline_3_category_3(number: u8) -> Option<(&'static str, &'static str)> {
    Some(match number {
        0 => (
            "Probability of encountering marginal visual flight rule conditions",
            "%",
        ),
        1 => (
            "Probability of encountering low instrument flight rule conditions",
            "%",
        ),
        2 => (
            "Probability of encountering instrument flight rule conditions",
            "%",
        ),
        _ => return None,
    })
}

/// Discipline 3, parameter category 4.
fn discipline_3_category_4(number: u8) -> Option<(&'static str, &'static str)> {
    Some(match number {
        0 => ("Volcanic ash probability", "%"),
        1 => ("Volcanic ash cloud top temperature", "K"),
        2 => ("Volcanic ash cloud top pressure", "Pa"),
        3 => ("Volcanic ash cloud top height", "m"),
        4 => ("Volcanic ash cloud emissivity", "Numeric"),
        5 => (
            "Volcanic ash effective absorption optical depth ratio",
            "Numeric",
        ),
        6 => ("Volcanic ash cloud optical depth", "Numeric"),
        7 => ("Volcanic ash column density", "kg m-2"),
        8 => ("Volcanic ash particle effective radius", "m"),
        _ => return None,
    })
}

/// Discipline 3, parameter category 5.
fn discipline_3_category_5(number: u8) -> Option<(&'static str, &'static str)> {
    Some(match number {
        0 => ("Interface sea-surface temperature", "K"),
        1 => ("Skin sea-surface temperature", "K"),
        2 => ("Sub-skin sea-surface temperature", "K"),
        3 => ("Foundation sea-surface temperature", "K"),
        4 => (
            "Estimated bias between sea-surface temperature and standard",
            "K",
        ),
        5 => (
            "Estimated standard deviation between sea surface temperature and standard",
            "K",
        ),
        _ => return None,
    })
}

/// Discipline 3, parameter category 6.
fn discipline_3_category_6(number: u8) -> Option<(&'static str, &'static str)> {
    Some(match number {
        0 => ("Global solar irradiance", "W m-2"),
        1 => ("Global solar exposure", "J m-2"),
        2 => ("Direct solar irradiance", "W m-2"),
        3 => ("Direct solar exposure", "J m-2"),
        4 => ("Diffuse solar irradiance", "W m-2"),
        5 => ("Diffuse solar exposure", "J m-2"),
        _ => return None,
    })
}

/// Discipline 4, parameter category 0.
fn discipline_4_category_0(number: u8) -> Option<(&'static str, &'static str)> {
    Some(match number {
        0 => ("Temperature", "K"),
        1 => ("Electron temperature", "K"),
        2 => ("Proton temperature", "K"),
        3 => ("Ion temperature", "K"),
        4 => ("Parallel temperature", "K"),
        5 => ("Perpendicular temperature", "K"),
        _ => return None,
    })
}

/// Discipline 4, parameter category 1.
fn discipline_4_category_1(number: u8) -> Option<(&'static str, &'static str)> {
    Some(match number {
        0 => ("Velocity magnitude (speed)", "m s-1"),
        1 => (
            "1st vector component of velocity (coordinate system dependent)",
            "m s-1",
        ),
        2 => (
            "2nd vector component of velocity (coordinate system dependent)",
            "m s-1",
        ),
        3 => (
            "3rd vector component of velocity (coordinate system dependent)",
            "m s-1",
        ),
        _ => return None,
    })
}

/// Discipline 4, parameter category 2.
fn discipline_4_category_2(number: u8) -> Option<(&'static str, &'static str)> {
    Some(match number {
        0 => ("Particle number density", "m-3"),
        1 => ("Electron density", "m-3"),
        2 => ("Proton density", "m-3"),
        3 => ("Ion density", "m-3"),
        4 => ("Vertical total electron content", "TECU"),
        5 => ("HF absorption frequency", "Hz"),
        6 => ("HF absorption", "dB"),
        7 => ("Spread F", "m"),
        8 => ("h'F", "m"),
        9 => ("Critical frequency", "Hz"),
        10 => ("Maximal usable frequency (MUF)", "Hz"),
        11 => ("Peak height (hm)", "m"),
        12 => ("Peak density (Nm)", "m-3"),
        13 => ("Equivalent slab thickness (tau)", "km"),
        _ => return None,
    })
}

/// Discipline 4, parameter category 3.
fn discipline_4_category_3(number: u8) -> Option<(&'static str, &'static str)> {
    Some(match number {
        0 => ("Magnetic field magnitude", "T"),
        1 => ("1st vector component of magnetic field", "T"),
        2 => ("2nd vector component of magnetic field", "T"),
        3 => ("3rd vector component of magnetic field", "T"),
        4 => ("Electric field magnitude", "V m-1"),
        5 => ("1st vector component of electric field", "V m-1"),
        6 => ("2nd vector component of electric field", "V m-1"),
        7 => ("3rd vector component of electric field", "V m-1"),
        _ => return None,
    })
}

/// Discipline 4, parameter category 4.
fn discipline_4_category_4(number: u8) -> Option<(&'static str, &'static str)> {
    Some(match number {
        0 => ("Proton flux (differential)", "(m2 s sr eV)-1"),
        1 => ("Proton flux (integral)", "(m2 s sr )-1"),
        2 => ("Electron flux (differential)", "(m2 s sr eV)-1"),
        3 => ("Electron flux (integral)", "(m2 s sr)-1"),
        4 => ("Heavy ion flux (differential)", "(m2 s sr eV/nuc)-1"),
        5 => ("Heavy ion flux (integral)", "(m2 s sr)-1"),
        6 => ("Cosmic ray neutron flux", "h-1"),
        _ => return None,
    })
}

/// Discipline 4, parameter category 5.
fn discipline_4_category_5(number: u8) -> Option<(&'static str, &'static str)> {
    Some(match number {
        0 => ("Amplitude", "dB"),
        1 => ("Phase", "rad"),
        2 => ("Frequency", "Hz"),
        3 => ("Wavelength", "m"),
        _ => return None,
    })
}

/// Discipline 4, parameter category 6.
fn discipline_4_category_6(number: u8) -> Option<(&'static str, &'static str)> {
    Some(match number {
        0 => ("Integrated solar irradiance", "W m-2"),
        1 => ("Solar X-ray flux (XRS long)", "W m-2"),
        2 => ("Solar X-ray flux (XRS short)", "W m-2"),
        3 => ("Solar EUV irradiance", "W m-2"),
        4 => ("Solar spectral irradiance", "W m-2 nm-1"),
        5 => ("F10.7", "W m-2 Hz-1"),
        6 => ("Solar radio emissions", "W m-2 Hz-1"),
        _ => return None,
    })
}

/// Discipline 4, parameter category 7.
fn discipline_4_category_7(number: u8) -> Option<(&'static str, &'static str)> {
    Some(match number {
        0 => ("Limb intensity", "J m-2 s-1"),
        1 => ("Disk intensity", "J m-2 s-1"),
        2 => ("Disk intensity day", "J m-2 s-1"),
        3 => ("Disk intensity night", "J m-2 s-1"),
        _ => return None,
    })
}

/// Discipline 4, parameter category 8.
fn discipline_4_category_8(number: u8) -> Option<(&'static str, &'static str)> {
    Some(match number {
        0 => ("X-ray radiance", "W sr-1 m-2"),
        1 => ("EUV radiance", "W sr-1 m-2"),
        2 => ("H-alpha radiance", "W sr-1 m-2"),
        3 => ("White light radiance", "W sr-1 m-2"),
        4 => ("CaII-K radiance", "W sr-1 m-2"),
        5 => ("White light coronagraph radiance", "W sr-1 m-2"),
        6 => ("Heliospheric radiance", "W sr-1 m-2"),
        7 => ("Thematic mask", "Numeric"),
        8 => ("Solar induced chlorophyll fluorescence", "W m-2 sr-1 m-1"),
        _ => return None,
    })
}

/// Discipline 4, parameter category 9.
fn discipline_4_category_9(number: u8) -> Option<(&'static str, &'static str)> {
    Some(match number {
        0 => ("Pedersen conductivity", "S m-1"),
        1 => ("Hall conductivity", "S m-1"),
        2 => ("Parallel conductivity", "S m-1"),
        _ => return None,
    })
}

/// Discipline 4, parameter category 10.
fn discipline_4_category_10(number: u8) -> Option<(&'static str, &'static str)> {
    Some(match number {
        0 => ("Scintillation index (sigma phi)", "rad"),
        1 => ("Scintillation index S4", "Numeric"),
        2 => ("Rate of change of TEC index (ROTI)", "TECU/min"),
        3 => (
            "Disturbance ionosphere index spatial gradient (DIXSG)",
            "Numeric",
        ),
        4 => ("Along arc TEC rate (AATR)", "TECU/min"),
        5 => ("Kp", "Numeric"),
        6 => ("Equatorial disturbance storm time index (Dst)", "nT"),
        7 => ("Auroral electrojet (AE)", "nT"),
        _ => return None,
    })
}

/// Discipline 10, parameter category 0.
fn discipline_10_category_0(number: u8) -> Option<(&'static str, &'static str)> {
    Some(match number {
        0 => ("Wave spectra (1)", ""),
        1 => ("Wave spectra (2)", ""),
        2 => ("Wave spectra (3)", ""),
        3 => ("Significant height of combined wind waves and swell", "m"),
        4 => ("Direction of wind waves", "degree true"),
        5 => ("Significant height of wind waves", "m"),
        6 => ("Mean period of wind waves", "s"),
        7 => ("Direction of swell waves", "degree true"),
        8 => ("Significant height of swell waves", "m"),
        9 => ("Mean period of swell waves", "s"),
        10 => ("Primary wave direction", "degree true"),
        11 => ("Primary wave mean period", "s"),
        12 => ("Secondary wave direction", "degree true"),
        13 => ("Secondary wave mean period", "s"),
        14 => (
            "Mean direction of combined wind waves and swell",
            "degree true",
        ),
        15 => ("Mean period of combined wind waves and swell", "s"),
        16 => ("Coefficient of drag with waves", ""),
        17 => ("Friction velocity", "m/s"),
        18 => ("Wave stress", "N m-2"),
        19 => ("Normalized wave stress", ""),
        20 => ("Mean square slope of waves", ""),
        21 => ("u-component surface Stokes drift", "m/s"),
        22 => ("v-component surface Stokes drift", "m/s"),
        23 => ("Period of maximum individual wave height", "s"),
        24 => ("Maximum individual wave height", "m"),
        25 => ("Inverse mean wave frequency", "s"),
        26 => ("Inverse mean frequency of wind waves", "s"),
        27 => ("Inverse mean frequency of total swell", "s"),
        28 => ("Mean zero-crossing wave period", "s"),
        29 => ("Mean zero-crossing period of wind waves", "s"),
        30 => ("Mean zero-crossing period of total swell", "s"),
        31 => ("Wave directional width", ""),
        32 => ("Directional width of wind waves", ""),
        33 => ("Directional width of total swell", ""),
        34 => ("Peak wave period", "s"),
        35 => ("Peak period of wind waves", "s"),
        36 => ("Peak period of total swell", "s"),
        37 => ("Altimeter wave height", "m"),
        38 => ("Altimeter corrected wave height", "m"),
        39 => ("Altimeter range relative correction", ""),
        40 => ("10-metre neutral wind speed over waves", "m/s"),
        41 => ("10-metre wind direction over waves", "deg"),
        42 => ("Wave energy spectrum", "m2 s rad-1"),
        43 => ("Kurtosis of the sea-surface elevation due to waves", ""),
        44 => ("Benjamin-Feir index", ""),
        45 => ("Spectral peakedness factor", "/s"),
        46 => ("Peak wave direction", "deg"),
        47 => ("Significant wave height of first swell partition", "m"),
        48 => ("Significant wave height of second swell partition", "m"),
        49 => ("Significant wave height of third swell partition", "m"),
        50 => ("Mean wave period of first swell partition", "s"),
        51 => ("Mean wave period of second swell partition", "s"),
        52 => ("Mean wave period of third swell partition", "s"),
        53 => ("Mean wave direction of first swell partition", "deg"),
        54 => ("Mean wave direction of second swell partition", "deg"),
        55 => ("Mean wave direction of third swell partition", "deg"),
        56 => ("Wave directional width of first swell partition", ""),
        57 => ("Wave directional width of second swell partition", ""),
        58 => ("Wave directional width of third swell partition", ""),
        59 => ("Wave frequency width of first swell partition", ""),
        60 => ("Wave frequency width of second swell partition", ""),
        61 => ("Wave frequency width of third swell partition", ""),
        62 => ("Wave frequency width", ""),
        63 => ("Frequency width of wind waves", ""),
        64 => ("Frequency width of total swell", ""),
        65 => ("Peak wave period of first swell partition", "s"),
        66 => ("Peak wave period of second swell partition", "s"),
        67 => ("Peak wave period of third swell partition", "s"),
        68 => (
            "Peak wave direction of first swell partition",
            "degree true",
        ),
        69 => (
            "Peak wave direction of second swell partition",
            "degree true",
        ),
        70 => (
            "Peak wave direction of third swell partition",
            "degree true",
        ),
        71 => ("Peak direction of wind waves", "degree true"),
        72 => ("Peak direction of total swell", "degree true"),
        73 => ("Whitecap fraction", "fraction"),
        74 => ("Mean direction of total swell", "degree"),
        75 => ("Mean direction of wind waves", "degree"),
        76 => ("Charnock", "Numeric"),
        77 => ("Wave Spectral Skewness", "Numeric"),
        78 => ("Wave energy flux magnitude", "W m-1"),
        79 => ("Wave energy flux mean direction", "degree true"),
        80 => ("Ratio of wave angular and frequency width", "Numeric"),
        81 => ("Free convective velocity over the oceans", "m s-1"),
        82 => ("Air density over the oceans", "kg m-3"),
        83 => ("Normalized energy flux into waves", "Numeric"),
        84 => ("Normalized stress into ocean", "Numeric"),
        85 => ("Normalized energy flux into ocean", "Numeric"),
        86 => (
            "Surface elevation variance due to waves (over all frequencies and directions)",
            "m2 s rad-1",
        ),
        87 => ("Wave induced mean sea level correction", "m"),
        88 => ("Spectral width index", "Numeric"),
        89 => ("Number of events in freak waves statistics", "Numeric"),
        90 => ("u-component of surface momentum flux into ocean", "N m-2"),
        91 => ("v-component of surface momentum flux into ocean", "N m-2"),
        92 => ("Wave turbulent energy flux into ocean", "W m-2"),
        93 => ("Envelop maximum individual wave height", "m"),
        94 => ("Time domain maximum individual crest height", "m"),
        95 => ("Time domain maximum individual wave height", "m"),
        96 => ("Space time maximum individual crest height", "m"),
        97 => ("Space time maximum individual wave height", "m"),
        98 => ("Goda peakedness factor", "Numeric"),
        99 => ("Benjamin-Feir index 2D (BFI2D)", "Numeric"),
        100 => ("Crest-trough correlation", "Numeric"),
        101 => (
            "X component of the wave radiative stress to sea-ice",
            "N m-2",
        ),
        102 => (
            "Y component of the wave radiative stress to sea-ice",
            "N m-2",
        ),
        103 => ("u-component of atmospheric surface momentum flux", "N m-2"),
        104 => ("v-component of atmospheric surface momentum flux", "N m-2"),
        _ => return None,
    })
}

/// Discipline 10, parameter category 1.
fn discipline_10_category_1(number: u8) -> Option<(&'static str, &'static str)> {
    Some(match number {
        0 => ("Current direction", "degree true"),
        1 => ("Current speed", "m/s"),
        2 => ("u-component of current", "m/s"),
        3 => ("v-component of current", "m/s"),
        4 => ("Rip current occurrence probability", "%"),
        5 => ("Eastward current", "m s-1"),
        6 => ("Northward current", "m s-1"),
        _ => return None,
    })
}

/// Discipline 10, parameter category 2.
fn discipline_10_category_2(number: u8) -> Option<(&'static str, &'static str)> {
    Some(match number {
        0 => ("Ice cover", "Proportion"),
        1 => ("Ice thickness", "m"),
        2 => ("Direction of ice drift", "degree true"),
        3 => ("Speed of ice drift", "m/s"),
        4 => ("u-component of ice drift", "m/s"),
        5 => ("v-component of ice drift", "m/s"),
        6 => ("Ice growth rate", "m/s"),
        7 => ("Ice divergence", "/s"),
        8 => ("Ice temperature", "K"),
        9 => ("Module of ice internal pressure", "Pa m"),
        10 => (
            "Zonal vector component of vertically integrated ice internal pressure",
            "Pa m",
        ),
        11 => (
            "Meridional vector component of vertically integrated ice internal pressure",
            "Pa m",
        ),
        12 => ("Compressive ice strength", "N/m"),
        13 => ("Snow temperature (over sea ice)", "K"),
        14 => ("Albedo", "Numeric"),
        15 => ("Sea ice volume per unit area", "m3 m-2"),
        16 => ("Snow volume over sea ice per unit area", "m3 m-2"),
        17 => ("Sea ice heat content", "J m-2"),
        18 => ("Snow over sea ice heat content", "J m-2"),
        19 => ("Ice freeboard thickness", "m"),
        20 => ("Ice melt pond fraction", "fraction"),
        21 => ("Ice melt pond depth", "m"),
        22 => ("Ice melt pond volume per unit area", "m3 m-2"),
        23 => ("Sea ice fraction tendency due to parameterization", "s-1"),
        24 => ("x-component of ice drift", "m s-1"),
        25 => ("y-component of ice drift", "m s-1"),
        26 => ("Sea ice salinity", "g kg-1"),
        27 => ("Freezing/melting potential", "W m-2"),
        28 => ("Melt onset date", "Numeric"),
        29 => ("Freeze onset date", "Numeric"),
        30 => ("Sea-ice breakup memory", "Numeric"),
        31 => ("Downward short-wave radiation flux", "W m-2"),
        _ => return None,
    })
}

/// Discipline 10, parameter category 3.
fn discipline_10_category_3(number: u8) -> Option<(&'static str, &'static str)> {
    Some(match number {
        0 => ("Water temperature", "K"),
        1 => ("Deviation of sea level from mean", "m"),
        2 => ("Heat exchange coefficient", ""),
        3 => ("Practical salinity", "Numeric"),
        4 => ("Downward heat flux", "W m-2"),
        5 => ("Eastward surface stress", "N m-2"),
        6 => ("Northward surface stress", "N m-2"),
        7 => ("x-component surface stress", "N m-2"),
        8 => ("y-component surface stress", "N m-2"),
        9 => ("Thermosteric change in sea surface height", "m"),
        10 => ("Halosteric change in sea surface height", "m"),
        11 => ("Steric change in sea surface height", "m"),
        12 => ("Sea salt flux", "kg m-2 s-1"),
        13 => ("Net upward water flux", "kg m-2 s-1"),
        14 => ("Eastward surface water velocity", "m s-1"),
        15 => ("Northward surface water velocity", "m s-1"),
        16 => ("x-component of surface water velocity", "m s-1"),
        17 => ("y-component of surface water velocity", "m s-1"),
        18 => ("Heat flux correction", "W m-2"),
        19 => (
            "Sea surface height tendency due to parameterization",
            "m s-1",
        ),
        20 => (
            "Deviation of sea level from mean with inverse barometer correction",
            "m",
        ),
        21 => ("Salinity", "kg kg-1"),
        22 => ("Downward short-wave radiation flux", "W m-2"),
        _ => return None,
    })
}

/// Discipline 10, parameter category 4.
fn discipline_10_category_4(number: u8) -> Option<(&'static str, &'static str)> {
    Some(match number {
        0 => ("Main thermocline depth", "m"),
        1 => ("Main thermocline anomaly", "m"),
        2 => ("Transient thermocline depth", "m"),
        3 => ("Salinity", "kg/kg"),
        4 => ("Ocean vertical heat diffusivity", "m2/s"),
        5 => ("Ocean vertical salt diffusivity", "m2/s"),
        6 => ("Ocean vertical momentum diffusivity", "m2/s"),
        7 => ("Bathymetry", "m"),
        11 => ("Shape factor with respect to salinity profile", ""),
        12 => (
            "Shape factor with respect to temperature profile in thermocline",
            "",
        ),
        13 => (
            "Attenuation coefficient of water with respect to solar radiation",
            "/m",
        ),
        14 => ("Water depth", "m"),
        15 => ("Water temperature", "K"),
        16 => ("Water density (rho)", "kg m-3"),
        17 => ("Water density anomaly (sigma)", "kg m-3"), // deprecated
        18 => ("Water potential temperature (theta)", "K"),
        19 => ("Water potential density (rho theta)", "kg m-3"),
        20 => ("Water potential density anomaly (sigma theta)", "kg m-3"),
        21 => ("Practical salinity", "Numeric"),
        22 => ("Water column-integrated heat content", "J m-2"),
        23 => ("Eastward water velocity", "m s-1"),
        24 => ("Northward water velocity", "m s-1"),
        25 => ("x-component water velocity", "m s-1"),
        26 => ("y-component water velocity", "m s-1"),
        27 => ("Upward water velocity", "m s-1"),
        28 => ("Vertical eddy diffusivity", "m2 s-1"),
        29 => ("Bottom pressure equivalent height", "m"),
        30 => ("Fresh water flux into sea water from rivers", "kg m-2 s-1"),
        31 => ("Fresh water flux correction", "kg m-2 s-1"),
        32 => ("Virtual salt flux into sea water", "g kg-1 m-2 s-1"),
        33 => ("Virtual salt flux correction", "g kg-1 m-2 s-1"),
        34 => (
            "Seawater temperature tendency due to Newtonian relaxation",
            "K s-1",
        ),
        35 => (
            "Seawater salinity tendency due to Newtonian relaxation",
            "g kg-1 s-1",
        ),
        36 => (
            "Seawater temperature tendency due to parameterization",
            "K s-1",
        ),
        37 => (
            "Seawater salinity tendency due to parameterization",
            "g kg-1 s-1",
        ),
        38 => (
            "Eastward sea water velocity tendency due to parameterization",
            "m s-2",
        ),
        39 => (
            "Northward sea water velocity tendency due to parameterization",
            "m s-2",
        ),
        40 => (
            "Seawater temperature tendency due to direct bias correction",
            "K s-1",
        ),
        41 => (
            "Seawater salinity tendency due to direct bias correction",
            "g kg-1 s-1",
        ),
        42 => ("Seawater meridional volume transport", "m3 m-2 s-1"),
        43 => ("Seawater zonal volume transport", "m3 m-2 s-1"),
        44 => (
            "Seawater column integrated meridional volume transport",
            "m3 m-1 s-1",
        ),
        45 => (
            "Seawater column integrated zonal volume transport",
            "m3 m-1 s-1",
        ),
        46 => ("Seawater meridional mass transport", "kg m-2 s-1"),
        47 => ("Seawater zonal mass transport", "kg m-2 s-1"),
        48 => (
            "Seawater column integrated meridional mass transport",
            "kg m-1 s-1",
        ),
        49 => (
            "Seawater column integrated zonal mass transport",
            "kg m-1 s-1",
        ),
        50 => ("Seawater column integrated practical salinity", "g kg-1 m"),
        51 => ("Seawater column integrated salinity", "kg kg-1 m"),
        52 => ("Mixed layer depth", "m"),
        53 => ("Normal component of water velocity", "m s-1"),
        54 => ("Tangential component of water velocity", "m s-1"),
        55 => ("Sea water upward volume transport", "m3 m-2 s-1"),
        56 => ("Sea water upward mass transport", "kg m-2 s-1"),
        57 => ("Sea water age since surface contact", "s"),
        58 => ("Sea water downward short-wave radiation flux", "W m-2"),
        _ => return None,
    })
}

/// Discipline 10, parameter category 191.
fn discipline_10_category_191(number: u8) -> Option<(&'static str, &'static str)> {
    Some(match number {
        0 => (
            "Seconds prior to initial reference time (defined in Section 1)",
            "s",
        ),
        1 => ("Meridional overturning stream function", "m3/s"),
        3 => ("Days since last observation", "d"),
        4 => ("Barotropic stream function", "m3 s-1"),
        _ => return None,
    })
}

/// Discipline 20, parameter category 0.
fn discipline_20_category_0(number: u8) -> Option<(&'static str, &'static str)> {
    Some(match number {
        0 => ("Universal thermal climate index", "K"),
        1 => ("Mean radiant temperature", "K"),
        2 => ("Wet-bulb globe temperature", "K"),
        3 => ("Globe temperature", "K"),
        4 => ("Humidex", "K"),
        5 => ("Effective temperature", "K"),
        6 => ("Normal effective temperature", "K"),
        7 => ("Standard effective temperature", "K"),
        8 => ("Physiological equivalent temperature", "K"),
        9 => ("UV biologically effective dose", "W m-2"),
        10 => ("UV biologically effective dose, clear-sky", "W m-2"),
        11 => ("Excess heat factor", "K2"),
        12 => ("Excess cold factor", "K2"),
        _ => return None,
    })
}

/// Discipline 20, parameter category 1.
fn discipline_20_category_1(number: u8) -> Option<(&'static str, &'static str)> {
    Some(match number {
        0 => ("Malaria cases", "Fraction"),
        1 => ("Malaria circumsporozoite protein rate", "Fraction"),
        2 => (
            "Plasmodium falciparum entomological inoculation rate",
            "Bites per day per person",
        ),
        3 => (
            "Human bite rate by anopheles vectors",
            "Bites per day per person",
        ),
        4 => ("Malaria immunity", "Fraction"),
        5 => ("Falciparum parasite rates", "Fraction"),
        6 => (
            "Detectable falciparum parasite ratio (after day 10)",
            "Fraction",
        ),
        7 => ("Anopheles vector to host ratio", "Fraction"),
        8 => ("Anopheles vector number", "Number m-2"),
        9 => (
            "Fraction of malarial vector reproductive habitat",
            "Fraction",
        ),
        _ => return None,
    })
}

/// Discipline 20, parameter category 2.
fn discipline_20_category_2(number: u8) -> Option<(&'static str, &'static str)> {
    Some(match number {
        0 => ("Population density", "Person m-2"),
        _ => return None,
    })
}

/// Discipline 20, parameter category 3.
fn discipline_20_category_3(number: u8) -> Option<(&'static str, &'static str)> {
    Some(match number {
        0 => ("Renewable power capacity", "W"),
        1 => ("Renewable power production rate", "W"),
        2 => ("Wind power capacity", "W"),
        3 => ("Wind power production rate", "W"),
        4 => ("Solar photovoltaic (PV) power capacity", "W"),
        5 => ("Solar photovoltaic (PV) power production rate", "W"),
        6 => ("Solar non-photovoltaic (PV) power capacity", "W"),
        7 => ("Solar non-photovoltaic (PV) power production rate", "W"),
        8 => ("Concentrated solar power (CSP) power capacity", "W"),
        9 => ("Concentrated solar power (CSP) power production rate", "W"),
        _ => return None,
    })
}

/// Discipline 20, parameter category 4.
fn discipline_20_category_4(number: u8) -> Option<(&'static str, &'static str)> {
    Some(match number {
        1 => ("Downburst", "Code table 4.253"),
        2 => ("Lightning (electrical storm)", "Code table 4.253"),
        3 => ("Thunderstorm", "Code table 4.253"),
        4 => ("Coastal flood", "Code table 4.253"),
        5 => ("Estuarine (coastal) flood", "Code table 4.253"),
        6 => ("Flash flood", "Code table 4.253"),
        7 => ("Fluvial (riverine) flood", "Code table 4.253"),
        8 => ("Groundwater flood", "Code table 4.253"),
        9 => ("Ice-jam flood including debris", "Code table 4.253"),
        10 => ("Ponding (drainage) flood", "Code table 4.253"),
        11 => ("Snowmelt flood", "Code table 4.253"),
        12 => ("Surface water flooding", "Code table 4.253"),
        13 => ("Glacial lake outburst flood", "Code table 4.253"),
        14 => ("Black carbon (brown clouds)", "Code table 4.253"),
        15 => ("Dust storm or sandstorm", "Code table 4.253"),
        16 => ("Fog", "Code table 4.253"),
        17 => ("Haze", "Code table 4.253"),
        18 => ("Polluted air", "Code table 4.253"),
        19 => ("Sand haze", "Code table 4.253"),
        20 => ("Smoke", "Code table 4.253"),
        21 => ("Ocean acidification", "Code table 4.253"),
        22 => ("Rogue wave", "Code table 4.253"),
        23 => ("Sea water intrusion", "Code table 4.253"),
        24 => ("Sea ice (ice bergs)", "Code table 4.253"),
        25 => ("Ice flow", "Code table 4.253"),
        26 => ("Seiche", "Code table 4.253"),
        27 => ("Storm surge", "Code table 4.253"),
        28 => ("Storm tides", "Code table 4.253"),
        29 => ("Tsunami", "Code table 4.253"),
        30 => (
            "Depression or cyclone (low pressure area) (pressure-related)",
            "Code table 4.253",
        ),
        31 => (
            "Extra-tropical cyclone (pressure-related)",
            "Code table 4.253",
        ),
        32 => (
            "Sub-tropical cyclone (pressure-related)",
            "Code table 4.253",
        ),
        33 => ("Acid rain", "Code table 4.253"),
        34 => ("Blizzard", "Code table 4.253"),
        35 => ("Drought", "Code table 4.253"),
        36 => ("Hail", "Code table 4.253"),
        37 => ("Ice storm", "Code table 4.253"),
        38 => ("Snow", "Code table 4.253"),
        39 => ("Snowstorm", "Code table 4.253"),
        40 => ("Cold wave", "Code table 4.253"),
        41 => ("Dzud", "Code table 4.253"),
        42 => ("Freeze", "Code table 4.253"),
        43 => ("Frost (hoar frost)", "Code table 4.253"),
        44 => ("Freezing rain (supercooled rain)", "Code table 4.253"),
        45 => ("Glaze", "Code table 4.253"),
        46 => ("Ground frost", "Code table 4.253"),
        47 => ("Heatwave", "Code table 4.253"),
        48 => ("Icing (including ice)", "Code table 4.253"),
        49 => ("Thaw", "Code table 4.253"),
        50 => ("Avalanche", "Code table 4.253"),
        51 => ("Mud flow", "Code table 4.253"),
        52 => ("Rock slide", "Code table 4.253"),
        53 => ("Derecho", "Code table 4.253"),
        54 => ("Gale (strong gale)", "Code table 4.253"),
        55 => ("Squall", "Code table 4.253"),
        56 => ("Subtropical storm", "Code table 4.253"),
        57 => (
            "Tropical cyclone (cyclonic wind, rain [storm] surge) (wind-related)",
            "Code table 4.253",
        ),
        58 => ("Tropical storm (wind-related)", "Code table 4.253"),
        59 => ("Tornado", "Code table 4.253"),
        60 => ("Wind", "Code table 4.253"),
        _ => return None,
    })
}

/// Discipline 20, parameter category 5.
fn discipline_20_category_5(number: u8) -> Option<(&'static str, &'static str)> {
    Some(match number {
        1 => ("Household air pollution", "Code table 4.253"),
        2 => ("Air pollution (point source)", "Code table 4.253"),
        3 => ("Ambient (outdoor) air pollution", "Code table 4.253"),
        4 => ("Land degradation", "Code table 4.253"),
        5 => ("Soil degradation", "Code table 4.253"),
        6 => ("Runoff / nonpoint source pollution", "Code table 4.253"),
        7 => ("Salinity", "Code table 4.253"),
        8 => ("Biodiversity loss", "Code table 4.253"),
        9 => ("Deforestation", "Code table 4.253"),
        10 => ("Forest declines and diebacks", "Code table 4.253"),
        11 => ("Forest disturbances", "Code table 4.253"),
        12 => ("Forest invasive species", "Code table 4.253"),
        13 => ("Wildfires", "Code table 4.253"),
        14 => ("Desertification", "Code table 4.253"),
        15 => ("Loss of mangroves", "Code table 4.253"),
        16 => ("Wetland loss/degradation", "Code table 4.253"),
        17 => ("Coral bleaching", "Code table 4.253"),
        18 => ("Compressive soils", "Code table 4.253"),
        19 => ("Soil erosion", "Code table 4.253"),
        20 => ("Coastal erosion and shoreline change", "Code table 4.253"),
        21 => ("Permafrost loss", "Code table 4.253"),
        22 => ("Sand mining", "Code table 4.253"),
        23 => ("Sea level rise", "Code table 4.253"),
        24 => ("Eutrophication", "Code table 4.253"),
        _ => return None,
    })
}

/// Discipline 191, parameter category 0.
fn discipline_191_category_0(number: u8) -> Option<(&'static str, &'static str)> {
    Some(match number {
        0 => (
            "Stochastically Perturbed Parametrization Tendency (SPPT)",
            "Numeric",
        ),
        1 => (
            "Stochastically Perturbed Parameterizations (SPP)",
            "Numeric",
        ),
        2 => ("Stochastic Kinetic Energy Backscatter (SKEB)", "Numeric"),
        3 => ("Stochastic Trigger of Convection (STC)", "Numeric"),
        4 => ("Stochastic boundary-layer Humidity (SHUM)", "Numeric"),
        5 => ("Stochastic Total Tendency Perturbations (STTP)", "Numeric"),
        _ => return None,
    })
}

/// WMO Code Table 4.5 — fixed surface types, with units where WMO gives them.
pub(crate) fn fixed_surface(value: u8) -> Option<&'static str> {
    Some(match value {
        1 => "Ground or water surface",
        2 => "Cloud base level",
        3 => "Level of cloud tops",
        4 => "Level of 0 °C isotherm",
        5 => "Level of adiabatic condensation lifted from the surface",
        6 => "Maximum wind level",
        7 => "Tropopause",
        8 => "Nominal top of the atmosphere",
        9 => "Sea bottom",
        10 => "Entire atmosphere",
        11 => "Cumulonimbus (CB) base (m)",
        12 => "Cumulonimbus (CB) top (m)",
        13 => {
            "Lowest level where vertically integrated cloud cover exceeds the specified percentage (cloud base for a given percentage cloud cover) (%)"
        }
        14 => "Level of free convection (LFC)",
        15 => "Convective condensation level (CCL)",
        16 => "Level of neutral buoyancy or equilibrium level (LNB)",
        17 => "Departure level of the most unstable parcel of air (MUDL)",
        18 => "Departure level of a mixed layer parcel of air with specified layer depth (Pa)",
        19 => "Lowest level where cloud cover exceeds the specified percentage (%)",
        20 => "Isothermal level (K)",
        21 => {
            "Lowest level where mass density exceeds the specified value (base for a given threshold of mass density) (kg m-3)"
        }
        22 => {
            "Highest level where mass density exceeds the specified value (top for a given threshold of mass density) (kg m-3)"
        }
        23 => {
            "Lowest level where air concentration exceeds the specified value (base for a given threshold of air concentration) (Bq m-3)"
        }
        24 => {
            "Highest level where air concentration exceeds the specified value (top for a given threshold of air concentration) (Bq m-3)"
        }
        25 => {
            "Highest level where radar reflectivity exceeds the specified value (echo top for a given threshold of reflectivity) (dBZ)"
        }
        26 => "Convective cloud layer base (m)",
        27 => "Convective cloud layer top (m)",
        28 => "Effective inflow layer base",
        29 => "Effective inflow layer top",
        30 => "Specified radius from the centre of the Sun (m)",
        31 => "Solar photosphere",
        32 => "Ionospheric D-region level",
        33 => "Ionospheric E-region level",
        34 => "Ionospheric F1-region level",
        35 => "Ionospheric F2-region level",
        36 => "Stratopause",
        37 => "Hygropause",
        100 => "Isobaric surface (Pa)",
        101 => "Mean sea level",
        102 => "Specific altitude above mean sea level (m)",
        103 => "Specified height level above ground (m)",
        104 => "Sigma level (\"sigma\" value)",
        105 => "Hybrid level",
        106 => "Depth below land surface (m)",
        107 => "Isentropic (theta) level (K)",
        108 => "Level at specified pressure difference from ground to level (Pa)",
        109 => "Potential vorticity surface (K m2 kg-1 s-1)",
        111 => "Eta level",
        113 => "Logarithmic hybrid level",
        114 => "Snow level",
        115 => "Sigma height level",
        117 => "Mixed layer depth (m)",
        118 => "Hybrid height level",
        119 => "Hybrid pressure level",
        150 => "Generalized vertical height coordinate",
        151 => "Soil level",
        152 => "Sea-ice level",
        160 => "Depth below sea level (m)",
        161 => "Depth below water surface (m)",
        162 => "Lake or river bottom",
        163 => "Bottom of sediment layer",
        164 => "Bottom of thermally active sediment layer",
        165 => "Bottom of sediment layer penetrated by thermal wave",
        166 => "Mixing layer",
        167 => "Bottom of root zone",
        168 => "Ocean model level",
        169 => {
            "Ocean level defined by water density (sigma-theta) difference from near-surface to level (kg m-3)"
        }
        170 => {
            "Ocean level defined by water potential temperature difference from near-surface to level (K)"
        }
        171 => {
            "Ocean level defined by vertical eddy diffusivity difference from near-surface to level (m2 s-1)"
        }
        172 => {
            "Ocean level defined by water density (rho) difference from near-surface to level (m)"
        }
        173 => "Top of snow over sea ice on sea, lake or river",
        174 => "Top surface of ice on sea, lake or river",
        175 => "Top surface of ice, under snow cover, on sea, lake or river",
        176 => "Bottom surface (underside) ice on sea, lake or river",
        177 => "Deep soil (of indefinite depth)",
        179 => "Top surface of glacier ice and inland ice",
        180 => "Deep inland or glacier ice (of indefinite depth)",
        181 => "Grid tile land fraction as a model surface",
        182 => "Grid tile water fraction as a model surface",
        183 => "Grid tile ice fraction on sea, lake or river as a model surface",
        184 => "Grid tile glacier ice and inland ice fraction as a model surface",
        185 => "Roof level",
        186 => "Wall level",
        187 => "Road level",
        188 => "Melt pond top surface",
        189 => "Melt pond bottom surface",
        191 => "Abstract level with no vertical localization",
        _ => return None,
    })
}

/// WMO Code Table 4.4 — indicator of unit of time range.
pub(crate) fn time_range_unit(value: u8) -> Option<&'static str> {
    Some(match value {
        0 => "Minute",
        1 => "Hour",
        2 => "Day",
        3 => "Month",
        4 => "Year",
        5 => "Decade (10 years)",
        6 => "Normal (30 years)",
        7 => "Century (100 years)",
        10 => "3 hours",
        11 => "6 hours",
        12 => "12 hours",
        13 => "Second",
        _ => return None,
    })
}
