//! Sub-centres, generated from WMO Common Code Table C-12.
//!
//! Do not edit: regenerate with `python3 tools/gen_wmo_cct_tables.py`.
//! Source: `wmo-im/CCT` `v2026-06-01` (MIT), file `C12.csv`.
//!
//! Shared by both GRIB editions: C-12 keys on originating-centre codes, and
//! every centre it names sits below 256, where the C-1 and C-11 assignments
//! agree. 208 pairs across 31 centres.

/// Look up a sub-centre name (WMO Common Code Table C-12).
///
/// Keyed on the **pair**, not on `sub_centre` alone: 51 of the 104 sub-centre
/// codes C-12 defines mean different things under different centres — 4 is
/// NCEP's Environmental Modeling Center and NASA's Goddard Space Flight
/// Center. A flat table would be wrong about half the time.
///
/// `None` for a pair the table does not assign, and always for `sub_centre`
/// 0, which GRIB uses to mean "no sub-centre". WMO does list a name against
/// 0 for one centre (82, Norrköping), but a file setting the field to 0 is
/// declaring the field absent, so 0 is answered `None` under every centre.
pub fn lookup_sub_centre(centre: u16, sub_centre: u16) -> Option<&'static str> {
    if sub_centre == 0 {
        return None;
    }
    let name = match centre {
        2 => match sub_centre {
            201 => "Casey",
            203 => "Davis",
            210 => "Alice Springs",
            211 => "Melbourne Crib Point 1",
            214 => "Darwin",
            217 => "Perth",
            219 => "Townsville",
            232 => "Fiji",
            235 => "Noumea",
            237 => "Papeete",
            250 => "Vladivostock",
            251 => "Guam",
            252 => "Honolulu",
            _ => return None,
        },
        7 => match sub_centre {
            1 => "NCEP Reanalysis Project",
            2 => "NCEP Ensemble Products",
            3 => "NCEP Central Operations",
            4 => "Environmental Modeling Center",
            5 => "Weather Prediction Center",
            6 => "Ocean Prediction Center",
            7 => "Climate Prediction Center",
            8 => "Aviation Weather Center",
            9 => "Storm Prediction Center",
            10 => "National Hurricane Center",
            11 => "NWS Techniques Development Laboratory",
            12 => "NESDIS Office of Research and Applications",
            13 => "Federal Aviation Administration",
            14 => "NWS Meteorological Development Laboratory",
            15 => "North American Regional Reanalysis Project",
            16 => "Space Weather Prediction Center",
            17 => "ESRL Global Systems Division",
            _ => return None,
        },
        34 => match sub_centre {
            207 => "Syowa",
            240 => "Kiyose",
            241 => "Reanalysis project",
            _ => return None,
        },
        39 => match sub_centre {
            225 => "Beijing",
            226 => "Guangzhou",
            228 => "Urumuqi",
            _ => return None,
        },
        40 => match sub_centre {
            243 => "Seoul",
            245 => "Jincheon",
            _ => return None,
        },
        46 => match sub_centre {
            10 => "Cachoeira Paulista (INPE)",
            11 => "Cuiaba (INPE)",
            12 => "Brasilia (SEPIS - INMET)",
            13 => "Fortaleza (FUNCEME)",
            14 => "Natal (Navy Hygrog. Centre)",
            15 => "Manaus (SIVAM)",
            16 => "Natal (INPE)",
            17 => "Boa Vista",
            18 => "SIPAM-Porto Velho-RO",
            19 => "SIPAM-Belem-PA",
            25 => "Sao Paulo University - USP",
            _ => return None,
        },
        69 => match sub_centre {
            204 => "National Institute of Water and Atmospheric Research (NIWA - New Zealand)",
            205 => "Niue",
            206 => "Rarotonga (Cook Islands)",
            207 => "Apia (Samoa)",
            208 => "Tonga",
            209 => "Tuvalu",
            210 => "Kiribati",
            211 => "Tokelau",
            243 => "Kelburn",
            _ => return None,
        },
        72 => match sub_centre {
            249 => "Singapore",
            _ => return None,
        },
        74 => match sub_centre {
            1 => "Shanwick Oceanic Area Control Centre",
            2 => "Fucino",
            3 => "Gatineau",
            4 => "Maspalomas (Spain)",
            5 => "ESA ERS Central Facility",
            6 => "Prince Albert",
            7 => "West Freugh",
            13 => "Tromso",
            21 => "Agenzia Spaziale Italiana (Italy)",
            22 => "Centre National de la Recherche Scientifique (France)",
            23 => "GeoForschungs Zentrum (Germany)",
            24 => "Geodetic Observatory Pecny (Czechia)",
            25 => "Institut d'Estudis Espacials de Catalunya (Spain)",
            26 => "Federal Office of Topography (Switzerland)",
            27 => "Nordic Commission of Geodesy (Norway)",
            28 => "Nordic Commission of Geodesy (Sweden)",
            29 => "Institute Geographique National (France) - Service de geodesie",
            30 => "Bundesamt fuer Kartographie und Geodaesie (Germany)",
            31 => "Institute of Engineering Satellite Surveying and Geodesy (United Kingdom)",
            32 => "Joint Operational Meteorology and Oceanography Centre (JOMOC)",
            33 => "Koninklijk Nederlands Meteorologisch Institut (Netherlands)",
            34 => "Nordic GPS Atmospheric Analysis centre (Sweden)",
            35 => "Instituto Geografico Nacional de Espana (Spain)",
            36 => "Met Eireann (Ireland)",
            37 => "Royal Observatory of Belgium (Belgium)",
            _ => return None,
        },
        78 => match sub_centre {
            10 => "POLARA (Polarimetric Radar Algorithms instance)",
            64 => "Bundeswehr Geoinformation Office (BGIO)",
            110 => "NowCast mobile (Lightning data)",
            221 => {
                "Schleswig-Holstein, Traffic Operations Computing Centre (TOCC) Kiel/Neumuenster"
            }
            222 => "Hamburg, TOCC Hamburg",
            223 => "Niedersachsen, TOCC Hannover",
            224 => "Austria (NMC)",
            225 => "Nordrhein-Westfalen, TOCC Kamen Leverkusen",
            226 => "Hessen, TOCC Ruesselsheim",
            227 => "Rheinland-Pfalz, TOCC Koblenz",
            228 => "Baden-Wuerttemberg, TOCC Ludwigsburg",
            229 => "Bayern, TOCC Freimann",
            230 => "Saarland, TOCC Rohrbach",
            231 => "Bayern, Autobahn directorate Nordbayern",
            232 => "Brandenburg, TOCC Stolpe",
            233 => "Mecklenburg-Vorpommern, TOCC Malchow",
            234 => "Sachsen, TOCC Dresden",
            235 => "Sachsen-Anhalt, TOCC Halle",
            236 => "Thueringen, TOCC Erfurt",
            237 => "EasyWay - Meteotrans",
            254 => "EUMETSAT",
            _ => return None,
        },
        80 => match sub_centre {
            101 => "Albania (NMC)",
            102 => {
                "National Research Council/Institute of Atmospheric Sciences and Climate (CNR-ISAC)"
            }
            _ => return None,
        },
        82 => match sub_centre {
            10 => "Kangerlussuaq - Danish Meteorological Institute (DMI-Greenland)",
            20 => "Oslo - Norwegian Meteorological Institute (NMI-Norway)",
            30 => "Sodankyla - Arctic Research Centre (FMI/ARC-Finland)",
            _ => return None,
        },
        85 => match sub_centre {
            200 => "Institut National de l'Environnement Industriel et des Risques (France)",
            201 => {
                "Rheinisches Institut fuer Umweltforschung an der Universitaet zu Koeln E.V. (Germany)"
            }
            202 => "Institut Francais de Recherche pour l'Exploitation de la Mer",
            203 => "Aarhus University (Denmark)",
            204 => "Institute of Environmental Protection - National Research Institute (Poland)",
            _ => return None,
        },
        89 => match sub_centre {
            1 => "Solar and Ozone Observatory Hradec Kralove",
            _ => return None,
        },
        96 => match sub_centre {
            1 => "Cyprus (NMC)",
            _ => return None,
        },
        110 => match sub_centre {
            229 => "Hong-Kong",
            _ => return None,
        },
        145 => match sub_centre {
            1 => "DBNet station of Cayenne (French Guiana)",
            _ => return None,
        },
        147 => match sub_centre {
            10 => "Cordoba",
            15 => "Ushuaia",
            20 => "Marambio",
            30 => "Santiago de Chile",
            40 => "Punta Arenas",
            50 => "Base Presidente Frei",
            60 => "Cotopaxi",
            _ => return None,
        },
        148 => match sub_centre {
            1 => "Integrated Centre of Aeronautical Meteorology - CIMAER",
            _ => return None,
        },
        160 => match sub_centre {
            1 => "National Climatic Data Center",
            2 => "National Geophysical Data Center",
            3 => "National Oceanographic Data Center",
            4 => "Center for Satellite Applications and Research (STAR)",
            5 => "Joint Polar Satellite System",
            10 => "Tromso (Norway)",
            11 => "McMurdo (Antarctica)",
            _ => return None,
        },
        161 => match sub_centre {
            1 => "Great Lakes Environmental Research Laboratory",
            2 => "Earth System Research Laboratory",
            3 => "Atlantic Oceanographic and Meteorological Laboratory",
            4 => "Pacific Marine Environmental Laboratory",
            5 => "Air Resources Laboratory",
            6 => "Geophysical Fluid Dynamics Laboratory",
            7 => "National Severe Storms Laboratory",
            _ => return None,
        },
        173 => match sub_centre {
            1 => "Ames Research Center",
            2 => "Dryden Flight Research Center",
            3 => "Glenn Research Center",
            4 => "Goddard Space Flight Center",
            5 => "Jet Propulsion Laboratory",
            6 => "Johnson Space Center",
            7 => "Kennedy Space Center",
            8 => "Langley Research Center",
            9 => "Marshall Space Flight Center",
            10 => "Stennis Space Center",
            11 => "Goddard Institute for Space Studies",
            12 => "Independent Verification and Validation Facility",
            13 => "NASA Shared Service Center",
            14 => "Wallops Flight Facility",
            _ => return None,
        },
        176 => match sub_centre {
            10 => "Tromso (Norway)",
            11 => "McMurdo (Antarctica)",
            12 => "Sodankyla (Finland)",
            13 => "Fairbanks (United States)",
            14 => "Barrow (United States)",
            15 => "Rothera (Antarctica)",
            20 => "Honolulu (United States)",
            21 => "Gilmore Creek (United States)",
            22 => "Madison (United States)",
            23 => "Miami (United States)",
            24 => "Mayaguez (Puerto Rico)",
            25 => "Monterey (United States)",
            26 => "Guam",
            27 => "Corvallis (United States)",
            28 => "Hampton (United States)",
            29 => "New York City (United States)",
            _ => return None,
        },
        177 => match sub_centre {
            1 => "Center for Operational Oceanographic Products and Services",
            2 => "Coast Survey Development Laboratory",
            _ => return None,
        },
        183 => match sub_centre {
            1 => "Center for Western Weather and Water Extremes",
            2 => "Global Drifter Program",
            _ => return None,
        },
        191 => match sub_centre {
            1 => "RARS station of Tahiti (French Polynesia)",
            _ => return None,
        },
        204 => match sub_centre {
            101 => "Maupuia",
            102 => "Lauder",
            _ => return None,
        },
        211 => match sub_centre {
            10 => "Saint-Denis (La Reunion)",
            _ => return None,
        },
        227 => match sub_centre {
            1 => "Luxembourg (NMC)",
            _ => return None,
        },
        250 => match sub_centre {
            76 => "Roshydromet (Russian Federation)",
            78 => "Deutscher Wetterdienst (Germany)",
            80 => "Ufficio Generale Spazio Aereo e Meteorologia (Italy)",
            96 => "Hellenic National Meteorological Service (Greece)",
            215 => "MeteoSwiss (Switzerland)",
            220 => "Institute of Meteorology and Water Management (Poland)",
            242 => "National Meteorological Administration (Romania)",
            _ => return None,
        },
        254 => match sub_centre {
            10 => "Tromso (Norway)",
            20 => "Maspalomas (Spain)",
            30 => "Kangerlussuaq (Greenland)",
            40 => "Edmonton (Canada)",
            50 => "Bedford (Canada)",
            60 => "Gander (Canada)",
            70 => "Monterey (United States)",
            80 => "Wallops Island (United States)",
            90 => "Gilmor Creek (United States)",
            100 => "Athens (Greece)",
            120 => "Ewa Beach, Hawaii",
            125 => "Ford Island, Hawaii",
            130 => "Miami, Florida",
            140 => "Lannion (France)",
            150 => "Svalbard (Norway)",
            170 => "Saint-Denis (La Reunion)",
            180 => "Moscow",
            190 => "Muscat",
            200 => "Khabarovsk",
            210 => "Novosibirsk",
            220 => "NOAA Satellite Operations Facility (NSOF)",
            _ => return None,
        },
        _ => return None,
    };
    Some(name)
}
