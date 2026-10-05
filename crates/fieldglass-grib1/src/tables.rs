/// Parameter entry from a GRIB1 Table 2 (WMO ON388 international table, or a
/// centre-local table such as ECMWF 128).
///
/// `Copy` and `Eq` because it is three `&'static str`s out of a static table:
/// cheap to pass by value, and two entries naming the same parameter with the
/// same abbreviation and units *are* the same entry. Deriving them is what lets
/// a caller assert `lookup_parameter(..) == None` rather than reach for a field
/// (#556's residual shape, for the one type this change needed it on).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParameterEntry {
    /// The parameter's human-readable name, e.g. `"Temperature"`.
    pub name: &'static str,
    /// The table's short name, e.g. `"t"`.
    pub abbreviation: &'static str,
    /// The parameter's units as the table states them; empty when it is
    /// dimensionless.
    pub units: &'static str,
}

/// WMO originating-centre code for ECMWF (Common Code Table C-1).
const CENTRE_ECMWF: u8 = 98;

/// The first `table_version` that names a centre-*local* Table 2.
///
/// ON388 fixes versions 1-127 by international agreement and reserves 128-254
/// for a centre to redefine the whole id space as it likes. 255 is the missing
/// value, and falls on the local side of the line deliberately: a message that
/// declines to name its table has not named the international one.
const FIRST_LOCAL_TABLE_VERSION: u8 = 128;

/// The display name for a parameter [`lookup_parameter`] answered `None` for.
///
/// The stack-wide contract for an unresolved parameter is the codes that went
/// unresolved rather than a bare `"Unknown"`, because they are the only thing
/// that tells a user *which* table is missing. Stated once on
/// `fieldglass::api::Field::parameter` (#633).
///
/// The three codes are the ones a lookup takes, outermost first: the centre
/// scopes the table version, which scopes the id. The centre is included even
/// at an international `table_version`, where it does not affect the lookup,
/// because a fixed three-number shape is what makes the string reportable —
/// and because the centre is what tells a maintainer whether a *local* table
/// was expected to cover the id.
///
/// A function rather than a `format!` at each display seam so the umbrella and
/// the napi binding cannot drift apart again, which is how they came to render
/// the same message three different ways.
///
/// ```
/// # use fieldglass_grib1::tables::unresolved_parameter;
/// assert_eq!(unresolved_parameter(98, 128, 210), "Parameter 98/128/210");
/// ```
#[must_use]
pub fn unresolved_parameter(centre: u8, table_version: u8, id: u8) -> String {
    format!("Parameter {centre}/{table_version}/{id}")
}

/// Look up a GRIB1 parameter by id, `table_version` (PDS octet 4), originating
/// `centre` (PDS octet 5) and `sub_centre` (PDS octet 26).
///
/// Versions 1-127 resolve against WMO ON388 Table 2. Versions 128-254 name a
/// *centre-local* table that redefines the whole id space, and so resolve
/// against that centre's table exclusively: an id its table leaves undefined,
/// and every id when this crate ships no table for the centre at all, is
/// unresolved. The WMO table is not a stand-in for a local one — falling back
/// to it would label a DWD or NCEP field with an unrelated name that the
/// message never referenced (#547).
///
/// **Whose local table.** The originating centre's, except that a message from
/// another centre whose sub-centre is ECMWF reads ECMWF's tables: ECMWF
/// produces fields for its member states under their own centre codes. That is
/// eccodes' rule (`grib1/section.1.def`, `centreForTable2`), and
/// `tests/local_tables.rs` holds this function to eccodes' decode of it.
///
/// The local tables carried are every ECMWF one eccodes ships (see
/// `tools/gen_grib1_local_tables.py` for why other centres' are not).
///
/// Ids 128-254 of the WMO branch are ON388's own NCEP-local extension, which
/// the document publishes as part of Table 2; they apply at the international
/// versions, where the id space is otherwise unassigned above 127. eccodes
/// answers nothing there, because its WMO table stops at 127; ON388 is the
/// table US producers write against, so this keeps answering (#601).
///
/// Unrecognised ids return `None`; callers render [`unresolved_parameter`] as
/// the fallback. `None` rather than a sentinel entry named `"Unknown"` so that
/// "no table resolved this" is a state the type system carries, not a name a
/// display seam has to recognise by its English text (#633).
pub fn lookup_parameter(
    id: u8,
    table_version: u8,
    centre: u8,
    sub_centre: u8,
) -> Option<ParameterEntry> {
    if table_version >= FIRST_LOCAL_TABLE_VERSION {
        let tables_of = if centre != CENTRE_ECMWF && sub_centre == CENTRE_ECMWF {
            CENTRE_ECMWF
        } else {
            centre
        };
        return crate::tables_local::lookup(tables_of, table_version, id);
    }
    let (name, abbreviation, units) = match id {
        1 => ("Pressure", "PRES", "Pa"),
        2 => ("Pressure reduced to MSL", "PRMSL", "Pa"),
        3 => ("Pressure tendency", "PTEND", "Pa/s"),
        4 => ("Potential vorticity", "PVORT", "K m2 kg-1 s-1"),
        5 => ("ICAO Standard Atmosphere Reference Height", "ICAHT", "m"),
        6 => ("Geopotential", "GP", "m2/s2"),
        7 => ("Geopotential height", "HGT", "gpm"),
        8 => ("Geometric height", "DIST", "m"),
        9 => ("Standard deviation of height", "HSTDV", "m"),
        10 => ("Total ozone", "TOZNE", "Dobson"),
        11 => ("Temperature", "TMP", "K"),
        12 => ("Virtual temperature", "VTMP", "K"),
        13 => ("Potential temperature", "POT", "K"),
        14 => ("Equivalent potential temperature", "EPOT", "K"),
        15 => ("Maximum temperature", "TMAX", "K"),
        16 => ("Minimum temperature", "TMIN", "K"),
        17 => ("Dew point temperature", "DPT", "K"),
        18 => ("Dew point depression", "DEPR", "K"),
        19 => ("Lapse rate", "LAPR", "K/m"),
        20 => ("Visibility", "VIS", "m"),
        21 => ("Radar Spectra (1)", "RDSP1", "-"),
        22 => ("Radar Spectra (2)", "RDSP2", "-"),
        23 => ("Radar Spectra (3)", "RDSP3", "-"),
        24 => ("Parcel lifted index (to 500 hPa)", "PLI", "K"),
        25 => ("Temperature anomaly", "TMPA", "K"),
        26 => ("Pressure anomaly", "PRESA", "Pa"),
        27 => ("Geopotential height anomaly", "GPA", "gpm"),
        28 => ("Wave Spectra (1)", "WVSP1", ""),
        29 => ("Wave Spectra (2)", "WVSP2", ""),
        30 => ("Wave Spectra (3)", "WVSP3", ""),
        31 => ("Wind direction", "WDIR", "deg true"),
        32 => ("Wind speed", "WIND", "m/s"),
        33 => ("u-component of wind", "UGRD", "m/s"),
        34 => ("v-component of wind", "VGRD", "m/s"),
        35 => ("Stream function", "STRM", "m2/s"),
        36 => ("Velocity potential", "VPOT", "m2/s"),
        37 => ("Montgomery stream function", "MNTSF", "m2/s2"),
        38 => ("Sigma coordinate vertical velocity", "SGCVV", "/s"),
        39 => ("Vertical velocity (pressure)", "VVEL", "Pa/s"),
        40 => ("Vertical velocity (geometric)", "DZDT", "m/s"),
        41 => ("Absolute vorticity", "ABSV", "/s"),
        42 => ("Absolute divergence", "ABSD", "/s"),
        43 => ("Relative vorticity", "RELV", "/s"),
        44 => ("Relative divergence", "RELD", "/s"),
        45 => ("Vertical u-component shear", "VUCSH", "/s"),
        46 => ("Vertical v-component shear", "VVCSH", "/s"),
        47 => ("Direction of current", "DIRC", "deg true"),
        48 => ("Speed of current", "SPC", "m/s"),
        49 => ("u-component of current", "UOGRD", "m/s"),
        50 => ("v-component of current", "VOGRD", "m/s"),
        51 => ("Specific humidity", "SPFH", "kg/kg"),
        52 => ("Relative humidity", "RH", "%"),
        53 => ("Humidity mixing ratio", "MIXR", "kg/kg"),
        54 => ("Precipitable water", "PWAT", "kg/m2"),
        55 => ("Vapor pressure", "VAPP", "Pa"),
        56 => ("Saturation deficit", "SATD", "Pa"),
        57 => ("Evaporation", "EVP", "kg/m2"),
        58 => ("Cloud Ice", "CICE", "kg/m2"),
        59 => ("Precipitation rate", "PRATE", "kg/m2/s"),
        60 => ("Thunderstorm probability", "TSTM", "%"),
        61 => ("Total precipitation", "APCP", "kg/m2"),
        62 => ("Large scale precipitation", "NCPCP", "kg/m2"),
        63 => ("Convective precipitation", "ACPCP", "kg/m2"),
        64 => ("Snowfall rate water equivalent", "SRWEQ", "kg/m2/s"),
        65 => ("Water equiv. of accum. snow depth", "WEASD", "kg/m2"),
        66 => ("Snow depth", "SNOD", "m"),
        67 => ("Mixed layer depth", "MIXHT", "m"),
        68 => ("Transient thermocline depth", "TTHDP", "m"),
        69 => ("Main thermocline depth", "MTHD", "m"),
        70 => ("Main thermocline anomaly", "MTHA", "m"),
        71 => ("Total cloud cover", "TCDC", "%"),
        72 => ("Convective cloud cover", "CDCON", "%"),
        73 => ("Low cloud cover", "LCDC", "%"),
        74 => ("Medium cloud cover", "MCDC", "%"),
        75 => ("High cloud cover", "HCDC", "%"),
        76 => ("Cloud water", "CWAT", "kg/m2"),
        77 => ("Best lifted index (to 500 hPa)", "BLI", "K"),
        78 => ("Convective snow", "SNOC", "kg/m2"),
        79 => ("Large scale snow", "SNOL", "kg/m2"),
        80 => ("Water Temperature", "WTMP", "K"),
        81 => ("Land cover (land=1, sea=0)", "LAND", "proportion"),
        82 => ("Deviation of sea level from mean", "DSLM", "m"),
        83 => ("Surface roughness", "SFCR", "m"),
        84 => ("Albedo", "ALBDO", "%"),
        85 => ("Soil temperature", "TSOIL", "K"),
        86 => ("Soil moisture content", "SOILM", "kg/m2"),
        87 => ("Vegetation", "VEG", "%"),
        88 => ("Salinity", "SALTY", "kg/kg"),
        89 => ("Density", "DEN", "kg/m3"),
        90 => ("Water runoff", "WATR", "kg/m2"),
        91 => ("Ice cover (ice=1, no ice=0)", "ICEC", "proportion"),
        92 => ("Ice thickness", "ICETK", "m"),
        93 => ("Direction of ice drift", "DICED", "deg true"),
        94 => ("Speed of ice drift", "SICED", "m/s"),
        95 => ("u-component of ice drift", "UICE", "m/s"),
        96 => ("v-component of ice drift", "VICE", "m/s"),
        97 => ("Ice growth rate", "ICEG", "m/s"),
        98 => ("Ice divergence", "ICED2", "m/s"),
        99 => ("Snow melt", "SNOM", "kg/m2"),
        100 => (
            "Significant height of combined wind waves and swell",
            "HTSGW",
            "m",
        ),
        101 => ("Direction of wind waves", "WVDIR", "deg true"),
        102 => ("Significant height of wind waves", "WVHGT", "m"),
        103 => ("Mean period of wind waves", "WVPER", "s"),
        104 => ("Direction of swell waves", "SWDIR", "deg true"),
        105 => ("Significant height of swell waves", "SWELL", "m"),
        106 => ("Mean period of swell waves", "SWPER", "s"),
        107 => ("Primary wave direction", "DIRPW", "deg true"),
        108 => ("Primary wave mean period", "PERPW", "s"),
        109 => ("Secondary wave direction", "DIRSW", "deg true"),
        110 => ("Secondary wave mean period", "PERSW", "s"),
        111 => ("Net short-wave radiation flux (surface)", "NSWRS", "W/m2"),
        112 => ("Net long wave radiation flux (surface)", "NLWRS", "W/m2"),
        113 => (
            "Net short-wave radiation flux (top of atmosphere)",
            "NSWRT",
            "W/m2",
        ),
        114 => (
            "Net long wave radiation flux (top of atmosphere)",
            "NLWRT",
            "W/m2",
        ),
        115 => ("Long wave radiation flux", "LWAVR", "W/m2"),
        116 => ("Short wave radiation flux", "SWAVR", "W/m2"),
        117 => ("Global radiation flux", "GRAD", "W/m2"),
        118 => ("Brightness temperature", "BRTMP", "K"),
        119 => ("Radiance (wave number)", "LWRAD", "W/m/sr"),
        120 => ("Radiance (wave length)", "SWRAD", "W/m3/sr"),
        121 => ("Latent heat net flux", "LHTFL", "W/m2"),
        122 => ("Sensible heat net flux", "SHTFL", "W/m2"),
        123 => ("Boundary layer dissipation", "BLYDP", "W/m2"),
        124 => ("Momentum flux, u component", "UFLX", "N/m2"),
        125 => ("Momentum flux, v component", "VFLX", "N/m2"),
        126 => ("Wind mixing energy", "WMIXE", "J"),
        127 => ("Image data", "IMGD", "-"),
        128 => (
            "Mean Sea Level Pressure (Standard Atmosphere Reduction)",
            "MSLSA",
            "Pa",
        ),
        129 => (
            "Mean Sea Level Pressure (MAPS System Reduction)",
            "MSLMA",
            "Pa",
        ),
        130 => (
            "Mean Sea Level Pressure (NAM Model Reduction)",
            "MSLET",
            "Pa",
        ),
        131 => ("Surface lifted index", "LFTX", "K"),
        132 => ("Best (4 layer) lifted index", "4LFTX", "K"),
        133 => ("K index", "KX", "K"),
        134 => ("Sweat index", "SX", "K"),
        135 => ("Horizontal moisture divergence", "MCONV", "kg/kg/s"),
        136 => ("Vertical speed shear", "VWSH", "1/s"),
        137 => (
            "3-hr pressure tendency Std. Atmos. Reduction",
            "TSLSA",
            "Pa/s",
        ),
        138 => ("Brunt-Vaisala frequency (squared)", "BVF2", "1/s2"),
        139 => ("Potential vorticity (density weighted)", "PVMW", "1/s/m"),
        140 => ("Categorical rain", "CRAIN", "non-dim"),
        141 => ("Categorical freezing rain", "CFRZR", "non-dim"),
        142 => ("Categorical ice pellets", "CICEP", "non-dim"),
        143 => ("Categorical snow", "CSNOW", "non-dim"),
        144 => ("Volumetric soil moisture content", "SOILW", "fraction"),
        145 => ("Potential evaporation rate", "PEVPR", "W/m2"),
        146 => ("Cloud work function", "CWORK", "J/kg"),
        147 => ("Zonal flux of gravity wave stress", "UGWD", "N/m2"),
        148 => ("Meridional flux of gravity wave stress", "VGWD", "N/m2"),
        149 => ("Potential vorticity", "PVORT2", "m2/s/kg"),
        150 => (
            "Covariance between meridional and zonal wind",
            "COVMZ",
            "m2/s2",
        ),
        151 => (
            "Covariance between temperature and zonal wind",
            "COVTZ",
            "K*m/s",
        ),
        152 => (
            "Covariance between temperature and meridional wind",
            "COVTM",
            "K*m/s",
        ),
        153 => ("Cloud Mixing Ratio", "CLWMR", "kg/kg"),
        154 => ("Ozone mixing ratio", "O3MR", "kg/kg"),
        155 => ("Ground Heat Flux", "GFLUX", "W/m2"),
        156 => ("Convective inhibition", "CIN", "J/kg"),
        157 => ("Convective Available Potential Energy", "CAPE", "J/kg"),
        158 => ("Turbulent Kinetic Energy", "TKE", "J/kg"),
        159 => ("Condensation pressure of lifted parcel", "CONDP", "Pa"),
        160 => ("Clear Sky Upward Solar Flux", "CSUSF", "W/m2"),
        161 => ("Clear Sky Downward Solar Flux", "CSDSF", "W/m2"),
        162 => ("Clear Sky upward long wave flux", "CSULF", "W/m2"),
        163 => ("Clear Sky downward long wave flux", "CSDLF", "W/m2"),
        164 => ("Cloud forcing net solar flux", "CFNSF", "W/m2"),
        165 => ("Cloud forcing net long wave flux", "CFNLF", "W/m2"),
        166 => ("Visible beam downward solar flux", "VBDSF", "W/m2"),
        167 => ("Visible diffuse downward solar flux", "VDDSF", "W/m2"),
        168 => ("Near IR beam downward solar flux", "NBDSF", "W/m2"),
        169 => ("Near IR diffuse downward solar flux", "NDDSF", "W/m2"),
        170 => ("Rain water mixing ratio", "RWMR", "kg/kg"),
        171 => ("Snow mixing ratio", "SNMR", "kg/kg"),
        172 => ("Momentum flux", "MFLX", "N/m2"),
        173 => ("Mass point model surface", "LMH", "non-dim"),
        174 => ("Velocity point model surface", "LMV", "non-dim"),
        175 => ("Model layer number (from bottom up)", "MLYNO", "non-dim"),
        176 => ("Latitude", "NLAT", "deg"),
        177 => ("East longitude", "ELON", "deg"),
        178 => ("Ice mixing ratio", "ICMR", "kg/kg"),
        179 => ("Graupel mixing ratio", "GRMR", "kg/kg"),
        180 => ("Surface wind gust", "GUST", "m/s"),
        181 => ("x-gradient of log pressure", "LPSX", "1/m"),
        182 => ("y-gradient of log pressure", "LPSY", "1/m"),
        183 => ("x-gradient of height", "HGTX", "m/m"),
        184 => ("y-gradient of height", "HGTY", "m/m"),
        185 => ("Turbulence Potential Forecast Index", "TPFI", "non-dim"),
        186 => ("Total Icing Potential Diagnostic", "TIPD", "non-dim"),
        187 => ("Lightning", "LTNG", "non-dim"),
        188 => ("Rate of water dropping from canopy to ground", "RDRIP", "-"),
        189 => ("Virtual potential temperature", "VPTMP", "K"),
        190 => ("Storm relative helicity", "HLCY", "m2/s2"),
        191 => ("Probability from ensemble", "PROB", "numeric"),
        192 => (
            "Probability from ensemble normalized w.r.t. climate",
            "PROBN",
            "numeric",
        ),
        193 => ("Probability of precipitation", "POP", "%"),
        194 => ("Percent of frozen precipitation", "CPOFP", "%"),
        195 => ("Probability of freezing precipitation", "CPOZP", "%"),
        196 => ("u-component of storm motion", "USTM", "m/s"),
        197 => ("v-component of storm motion", "VSTM", "m/s"),
        198 => ("Number concentration for ice particles", "NCIP", "-"),
        199 => ("Direct evaporation from bare soil", "EVBS", "W/m2"),
        200 => ("Canopy water evaporation", "EVCW", "W/m2"),
        201 => ("Ice-free water surface", "ICWAT", "%"),
        202 => ("Convective weather detection index", "CWDI", "non-dim"),
        203 => ("VAFTAD", "VAFTD", "log10(kg/m3)"),
        204 => ("Downward short wave rad. flux", "DSWRF", "W/m2"),
        205 => ("Downward long wave rad. flux", "DLWRF", "W/m2"),
        206 => ("Ultra violet index", "UVI", "W/m2"),
        207 => ("Moisture availability", "MSTAV", "%"),
        208 => ("Exchange coefficient", "SFEXC", "(kg/m3)(m/s)"),
        209 => ("No. of mixed layers next to surface", "MIXLY", "integer"),
        210 => ("Transpiration", "TRANS", "W/m2"),
        211 => ("Upward short wave rad. flux", "USWRF", "W/m2"),
        212 => ("Upward long wave rad. flux", "ULWRF", "W/m2"),
        213 => ("Amount of non-convective cloud", "CDLYR", "%"),
        214 => ("Convective Precipitation rate", "CPRAT", "kg/m2/s"),
        215 => ("Temperature tendency by all physics", "TTDIA", "K/s"),
        216 => ("Temperature tendency by all radiation", "TTRAD", "K/s"),
        217 => (
            "Temperature tendency by non-radiation physics",
            "TTPHY",
            "K/s",
        ),
        218 => ("Precipitation index", "PREIX", "fraction"),
        219 => ("Std. dev. of IR T over 1x1 deg area", "TSD1D", "K"),
        220 => ("Natural log of surface pressure", "NLGSP", "ln(kPa)"),
        221 => ("Planetary boundary layer height", "HPBL", "m"),
        222 => ("5-wave geopotential height", "5WAVH", "gpm"),
        223 => ("Plant canopy surface water", "CNWAT", "kg/m2"),
        224 => ("Soil type (as in Zobler)", "SOTYP", "integer"),
        225 => ("Vegetation type (as in SiB)", "VGTYP", "integer"),
        226 => ("Blackadar's mixing length scale", "BMIXL", "m"),
        227 => ("Asymptotic mixing length scale", "AMIXL", "m"),
        228 => ("Potential evaporation", "PEVAP", "kg/m2"),
        229 => ("Snow phase-change heat flux", "SNOHF", "W/m2"),
        230 => ("5-wave geopotential height anomaly", "5WAVA", "gpm"),
        231 => ("Convective cloud mass flux", "MFLUX", "Pa/s"),
        232 => ("Downward total radiation flux", "DTRF", "W/m2"),
        233 => ("Upward total radiation flux", "UTRF", "W/m2"),
        234 => ("Baseflow-groundwater runoff", "BGRUN", "kg/m2"),
        235 => ("Storm surface runoff", "SSRUN", "kg/m2"),
        236 => (
            "Supercooled Large Droplet Icing Potential Diagnostic",
            "SIPD",
            "numeric",
        ),
        237 => ("Total ozone", "O3TOT", "kg/m2"),
        238 => ("Snow cover", "SNOWC", "%"),
        239 => ("Snow temperature", "SNOT", "K"),
        240 => (
            "Covariance between temperature and vertical wind",
            "COVTW",
            "K*m/s",
        ),
        241 => ("Large scale condensate heat rate", "LRGHR", "K/s"),
        242 => ("Deep convective heating rate", "CNVHR", "K/s"),
        243 => ("Deep convective moistening rate", "CNVMR", "kg/kg/s"),
        244 => ("Shallow convective heating rate", "SHAHR", "K/s"),
        245 => ("Shallow convective moistening rate", "SHAMR", "kg/kg/s"),
        246 => ("Vertical diffusion heating rate", "VDFHR", "K/s"),
        247 => ("Vertical diffusion zonal acceleration", "VDFUA", "m/s2"),
        248 => (
            "Vertical diffusion meridional acceleration",
            "VDFVA",
            "m/s2",
        ),
        249 => ("Vertical diffusion moistening rate", "VDFMR", "kg/kg/s"),
        250 => ("Solar radiative heating rate", "SWHR", "K/s"),
        251 => ("Long wave radiative heating rate", "LWHR", "K/s"),
        252 => ("Drag coefficient", "CD", "non-dim"),
        253 => ("Friction velocity", "FRICV", "m/s"),
        254 => ("Richardson number", "RI", "non-dim"),
        _ => return None,
    };
    Some(ParameterEntry {
        name,
        abbreviation,
        units,
    })
}

/// WMO originating-centre code for NCEP (Common Code Table C-1).
const CENTRE_NCEP: u8 = 7;

/// WMO originating-centre code for JMA (Common Code Table C-1).
const CENTRE_JMA: u8 = 34;

/// A level type from Code Table 3: its name, and how PDS octets 11 and 12 read
/// for it.
///
/// One row carries both because the table does: a row's contents column is
/// what says whether a level has a value, and in which unit. Keeping the two in
/// separate lists is how they drifted (#869).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct LevelType {
    /// The level's name, without its unit.
    pub(crate) name: &'static str,
    /// What octets 11 and 12 hold.
    pub(crate) value: LevelValue,
}

/// What PDS octets 11 and 12 hold for a level type.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum LevelValue {
    /// Nothing: a surface, a whole column or a cloud level. The octets are
    /// ignored, whatever they hold.
    None,
    /// One 16-bit value, `octets / divisor`, shown to `decimals` places.
    Single {
        unit: Option<&'static str>,
        divisor: u32,
        decimals: usize,
    },
    /// Two one-octet bounds, top (octet 11) then bottom (octet 12).
    Layer {
        unit: Option<&'static str>,
        top: Bound,
        bottom: Bound,
    },
}

/// How one octet of a layer reads.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Bound {
    /// The octet as it stands.
    Raw,
    /// The octet times ten: kPa shown in hPa.
    Tenfold,
    /// The octet in hundredths.
    Hundredths,
    /// 475 K minus the octet.
    Below475,
    /// 1100 hPa minus the octet.
    Below1100,
    /// 1.1 minus the octet in thousandths.
    BelowSigma1_1,
}

impl Bound {
    pub(crate) fn show(self, octet: u8) -> String {
        let v = i32::from(octet);
        match self {
            Bound::Raw => v.to_string(),
            Bound::Tenfold => (v * 10).to_string(),
            Bound::Hundredths => format!("{:.2}", f64::from(v) / 100.0),
            Bound::Below475 => (475 - v).to_string(),
            Bound::Below1100 => (1100 - v).to_string(),
            Bound::BelowSigma1_1 => format!("{:.3}", 1.1 - f64::from(v) * 0.001),
        }
    }
}

const fn none(name: &'static str) -> LevelType {
    LevelType {
        name,
        value: LevelValue::None,
    }
}

const fn single(name: &'static str, unit: Option<&'static str>) -> LevelType {
    scaled(name, unit, 1, 0)
}

const fn scaled(
    name: &'static str,
    unit: Option<&'static str>,
    divisor: u32,
    decimals: usize,
) -> LevelType {
    LevelType {
        name,
        value: LevelValue::Single {
            unit,
            divisor,
            decimals,
        },
    }
}

const fn layer(
    name: &'static str,
    unit: Option<&'static str>,
    top: Bound,
    bottom: Bound,
) -> LevelType {
    LevelType {
        name,
        value: LevelValue::Layer { unit, top, bottom },
    }
}

/// Code Table 3 as it applies to a message from `centre`.
///
/// Which table that is follows eccodes (`grib1/section.1.def` reads
/// `grib1/local/<centre>/3.table` before the master table), with one addition:
/// eccodes ships no NCEP table, so NCEP's local levels come from its Office
/// Note 388, Table 3. The sub-centre plays no part, in eccodes or here.
///
/// * NCEP (7): ON388, which redefines 210 and adds 126 and 204-254.
/// * ECMWF (98): the master table plus 211 and 212.
/// * JMA (34): the master table plus 211-213 (JRA-55's land levels).
/// * Every other centre: the master table, WMO's codes plus 210, an ECMWF
///   extension eccodes applies to all centres.
///
/// `tests/code_tables.rs` holds every row to eccodes' decode and to ON388.
pub(crate) fn level_type(code: u8, centre: u8) -> Option<LevelType> {
    let local = match centre {
        CENTRE_NCEP => ncep_level_type(code),
        CENTRE_ECMWF => ecmwf_level_type(code),
        CENTRE_JMA => jma_level_type(code),
        _ => None,
    };
    local.or_else(|| wmo_level_type(code))
}

/// The WMO codes of Code Table 3, plus 210 as eccodes' master table carries it.
fn wmo_level_type(code: u8) -> Option<LevelType> {
    use Bound::*;
    Some(match code {
        1 => none("Ground or water surface"),
        2 => none("Cloud base level"),
        3 => none("Cloud top level"),
        4 => none("0°C isotherm level"),
        5 => none("Adiabatic condensation level"),
        6 => none("Maximum wind speed level"),
        7 => none("Tropopause level"),
        8 => none("Nominal top of atmosphere"),
        9 => none("Sea bottom"),
        20 => scaled("Isothermal level", Some("K"), 100, 2),
        100 => single("Isobaric level", Some("hPa")),
        101 => layer("Layer between two isobaric levels", Some("kPa"), Raw, Raw),
        102 => none("Mean sea level"),
        103 => single("Specified altitude above MSL", Some("m")),
        104 => layer(
            "Layer between two altitudes above MSL",
            Some("hm"),
            Raw,
            Raw,
        ),
        105 => single("Specified height above ground", Some("m")),
        106 => layer(
            "Layer between two heights above ground",
            Some("hm"),
            Raw,
            Raw,
        ),
        107 => scaled("Sigma level", Some("σ"), 10_000, 4),
        108 => layer(
            "Layer between two sigma levels",
            Some("σ"),
            Hundredths,
            Hundredths,
        ),
        109 => single("Hybrid level", None),
        110 => layer("Layer between two hybrid levels", None, Raw, Raw),
        111 => single("Depth below land surface", Some("cm")),
        112 => layer(
            "Layer between two depths below land surface",
            Some("cm"),
            Raw,
            Raw,
        ),
        113 => single("Isentropic (theta) level", Some("K")),
        114 => layer(
            "Layer between two isentropic levels",
            Some("K"),
            Below475,
            Below475,
        ),
        115 => single(
            "Level at specified pressure difference from ground",
            Some("hPa"),
        ),
        116 => layer(
            "Layer between two pressure difference levels",
            Some("hPa"),
            Raw,
            Raw,
        ),
        117 => scaled("Potential vorticity surface", Some("PVU"), 1_000, 3),
        119 => scaled("Eta level", None, 10_000, 4),
        120 => layer("Layer between two eta levels", None, Hundredths, Hundredths),
        121 => layer(
            "Layer between two isobaric surfaces (high precision)",
            Some("hPa"),
            Below1100,
            Below1100,
        ),
        125 => single("Specified height above ground (high precision)", Some("cm")),
        128 => layer(
            "Layer between two sigma levels (high precision)",
            Some("σ"),
            BelowSigma1_1,
            BelowSigma1_1,
        ),
        // WMO and eccodes give the top in kPa and the bottom as 1100 hPa
        // minus it; ON388 says the top is in hPa too. One octet of hPa cannot
        // reach the lower troposphere, which is the point of the mixed form,
        // so this follows WMO and shows both bounds in hPa.
        141 => layer(
            "Layer between two isobaric surfaces (mixed precision)",
            Some("hPa"),
            Tenfold,
            Below1100,
        ),
        160 => single("Depth below sea level", Some("m")),
        200 => none("Entire atmosphere"),
        201 => none("Entire ocean"),
        210 => single("Isobaric surface", Some("Pa")),
        255 => none("Missing"),
        _ => return None,
    })
}

/// ECMWF's additions, from eccodes' `grib1/local/ecmf/3.table`.
fn ecmwf_level_type(code: u8) -> Option<LevelType> {
    Some(match code {
        211 => none("Ocean wave level"),
        212 => none("Ocean mixed layer"),
        _ => return None,
    })
}

/// JMA's additions for JRA-55, from eccodes' `grib1/local/rjtd/3.table`.
fn jma_level_type(code: u8) -> Option<LevelType> {
    Some(match code {
        211 => none("Entire soil"),
        212 => none("Bottom of land surface model"),
        213 => single("Underground layer number of land surface model", None),
        _ => return None,
    })
}

/// NCEP's local levels, from ON388 Table 3 and 3a. ON388 gives a contents
/// column only for codes 100-201; a special level below carries a value only
/// where its meaning says so (235, 236 and 241).
fn ncep_level_type(code: u8) -> Option<LevelType> {
    Some(match code {
        126 => single("Isobaric level", Some("Pa")),
        204 => none("Highest tropospheric freezing level"),
        206 => none("Grid scale cloud bottom level"),
        207 => none("Grid scale cloud top level"),
        209 => none("Boundary layer cloud bottom level"),
        210 => none("Boundary layer cloud top level"),
        211 => none("Boundary layer cloud layer"),
        212 => none("Low cloud bottom level"),
        213 => none("Low cloud top level"),
        214 => none("Low cloud layer"),
        215 => none("Cloud ceiling"),
        216 => none("Cumulonimbus base"),
        217 => none("Cumulonimbus top"),
        220 => none("Planetary boundary layer"),
        222 => none("Middle cloud bottom level"),
        223 => none("Middle cloud top level"),
        224 => none("Middle cloud layer"),
        232 => none("High cloud bottom level"),
        233 => none("High cloud top level"),
        234 => none("High cloud layer"),
        235 => scaled("Ocean isotherm level", Some("°C"), 10, 1),
        236 => layer(
            "Layer between two depths below ocean surface",
            Some("dam"),
            Bound::Raw,
            Bound::Raw,
        ),
        237 => none("Bottom of ocean mixed layer"),
        238 => none("Bottom of ocean isothermal layer"),
        239 => none("Layer: ocean surface to 26°C isothermal level"),
        240 => none("Ocean mixed layer"),
        241 => single("Ordered sequence of data", None),
        242 => none("Convective cloud bottom level"),
        243 => none("Convective cloud top level"),
        244 => none("Convective cloud layer"),
        245 => none("Lowest level of the wet bulb zero"),
        246 => none("Maximum equivalent potential temperature level"),
        247 => none("Equilibrium level"),
        248 => none("Shallow convective cloud bottom level"),
        249 => none("Shallow convective cloud top level"),
        251 => none("Deep convective cloud bottom level"),
        252 => none("Deep convective cloud top level"),
        253 => none("Lowest bottom level of supercooled liquid water layer"),
        254 => none("Highest top level of supercooled liquid water layer"),
        _ => return None,
    })
}

/// The name of a level type in Code Table 3, as it applies to a message from
/// `centre`.
///
/// The table is WMO's for every centre, plus the centre's own local levels
/// where it has them: NCEP's from its Office Note 388 (which also gives 210
/// its own meaning), and ECMWF's and JMA's from eccodes. A local code from
/// another centre is not named.
///
/// `None` for a code that table does not name; a caller that shows the answer
/// keeps the code instead (#774).
pub fn lookup_level_type(code: u8, centre: u8) -> Option<&'static str> {
    level_type(code, centre).map(|t| t.name)
}

/// Unit of time (WMO ON388 Table 4).
///
/// **Not** GRIB2's Code Table 4.4, despite the overlap: GRIB1 code 13 is
/// 15 minutes where GRIB2 code 13 is a second, GRIB1 adds 14 for 30 minutes,
/// and GRIB1 spells a second as 254 — a code GRIB2 does not define at all.
/// Reading one table for the other edition silently misreports the lead time,
/// so the two editions keep separate tables on purpose.
///
/// `None` for a code the table does not name; a caller that shows the answer
/// keeps the code instead (#774).
pub fn lookup_time_unit(value: u8) -> Option<&'static str> {
    Some(match value {
        0 => "minute",
        1 => "hour",
        2 => "day",
        3 => "month",
        4 => "year",
        5 => "decade",
        6 => "normal (30 years)",
        7 => "century",
        10 => "3 hours",
        11 => "6 hours",
        12 => "12 hours",
        13 => "15 minutes",
        14 => "30 minutes",
        254 => "second",
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ecmwf_table_128_resolves_common_era5_params() {
        // Centre 98, table 128 — the fields ERA5 / IFS users actually open.
        for (id, name, abbr, units) in [
            (167u8, "2 metre temperature", "2t", "K"),
            (165, "10 metre U wind component", "10u", "m s**-1"),
            (166, "10 metre V wind component", "10v", "m s**-1"),
            (151, "Mean sea level pressure", "msl", "Pa"),
        ] {
            let p = lookup_parameter(id, 128, CENTRE_ECMWF, 0).expect("id {id} resolves");
            assert_eq!(
                (p.name, p.abbreviation, p.units),
                (name, abbr, units),
                "id {id}"
            );
        }
    }

    #[test]
    fn ecmwf_table_129_resolves_gradient_table() {
        let p = lookup_parameter(129, 129, CENTRE_ECMWF, 0).expect("129/129 resolves");
        assert_eq!(
            (p.name, p.abbreviation, p.units),
            ("Geopotential gradient", "zgrd", "m**2 s**-2")
        );
    }

    #[test]
    fn ecmwf_local_table_unknown_id_does_not_resolve_to_wmo() {
        // id 61 is undefined in ECMWF table 128 (it's "Total precipitation" in
        // the WMO table). An ECMWF local table must not leak the WMO meaning.
        assert_eq!(lookup_parameter(61, 128, CENTRE_ECMWF, 0), None);
    }

    #[test]
    fn unshipped_local_table_does_not_resolve_to_wmo() {
        // Centre 7 (NCEP) table 129 is an NCEP-local table this crate does not
        // ship. Answering from the WMO table would label id 11 "Temperature" —
        // a name out of a table the message never referenced (#547).
        assert_eq!(lookup_parameter(11, 129, 7, 0), None);
    }

    #[test]
    fn ecmwf_ids_do_not_leak_to_another_centre() {
        // 167 is ECMWF's 2 metre temperature. A centre-7 message declaring its
        // own table 128 means something else by 167, and we do not know what.
        assert_eq!(lookup_parameter(167, 128, 7, 0), None);
        assert_eq!(
            lookup_parameter(167, 128, CENTRE_ECMWF, 0)
                .expect("ECMWF resolves its own id")
                .name,
            "2 metre temperature"
        );
    }

    #[test]
    fn ecmwf_local_version_eccodes_does_not_ship_does_not_resolve() {
        // Every ECMWF local table eccodes ships is carried, from 128 to 235,
        // but not every number in that range is a table: 134 is not one. It must
        // not quietly become the WMO table, which would name id 11 "Temperature".
        assert_eq!(lookup_parameter(11, 134, CENTRE_ECMWF, 0), None);
        // A shipped one beyond 129 does resolve.
        assert_eq!(
            lookup_parameter(2, 210, CENTRE_ECMWF, 0).map(|p| p.abbreviation),
            Some("aermr02")
        );
    }

    #[test]
    fn a_sub_centre_of_ecmwf_reads_ecmwf_tables_only_at_local_versions() {
        // Rome (80), produced by ECMWF: ECMWF's 2 metre temperature.
        assert_eq!(
            lookup_parameter(167, 128, 80, CENTRE_ECMWF).map(|p| p.abbreviation),
            Some("2t")
        );
        // Rome on its own has no table 128 here.
        assert_eq!(lookup_parameter(167, 128, 80, 0), None);
        // At an international version the sub-centre changes nothing.
        assert_eq!(
            lookup_parameter(11, 2, 80, CENTRE_ECMWF).map(|p| p.abbreviation),
            Some("TMP")
        );
    }

    #[test]
    fn non_ecmwf_centre_keeps_the_wmo_table_below_128() {
        // The gate keys on the version, not the centre: an international
        // version is the international table for everyone, ECMWF included.
        for centre in [0u8, 7, 54, 78, 85, CENTRE_ECMWF] {
            for version in [1u8, 2, 3] {
                let p = lookup_parameter(11, version, centre, 0)
                    .expect("an international version resolves against WMO");
                assert_eq!(
                    (p.name, p.abbreviation, p.units),
                    ("Temperature", "TMP", "K"),
                    "centre {centre} version {version}"
                );
            }
        }
    }

    #[test]
    fn every_id_of_an_unshipped_local_table_does_not_resolve() {
        // The whole id space, not just the ids that happen to collide with a
        // WMO entry: a local table redefines all of it, so there is nothing
        // left for the WMO table to answer.
        for centre in [0u8, 7, 54, 78, 85, 255] {
            for version in [FIRST_LOCAL_TABLE_VERSION, 129, 200, 254, 255] {
                for id in 0..=255u8 {
                    assert_eq!(
                        lookup_parameter(id, version, centre, 0),
                        None,
                        "centre {centre} version {version} id {id}"
                    );
                }
            }
        }
    }

    #[test]
    fn ecmwf_centre_with_international_version_uses_wmo() {
        // Centre 98 but table_version 1 is the international table, not a local
        // one — id 33 is WMO u-component of wind, not the ECMWF id-33 entry.
        let p = lookup_parameter(33, 1, CENTRE_ECMWF, 0).expect("WMO id 33 resolves");
        assert_eq!(
            (p.name, p.abbreviation, p.units),
            ("u-component of wind", "UGRD", "m/s")
        );
    }
}
