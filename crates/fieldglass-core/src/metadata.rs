/// A human-readable parameter (e.g. "Temperature", "Wind Speed")
pub struct Parameter {
    pub name: String,
    pub abbreviation: String,
    pub units: String,
    pub id: i32,
}

/// A vertical level descriptor
pub struct Level {
    pub level_type: String,
    pub value: f64,
    pub units: String,
}

/// Geographic grid geometry
pub struct GridDefinition {
    pub grid_type: String,
    pub ni: i32,
    pub nj: i32,
    pub lat_first: f64,
    pub lon_first: f64,
    pub lat_last: f64,
    pub lon_last: f64,
    pub di: f64,
    pub dj: f64,
}

/// All metadata for a single data message, format-agnostic.
/// raw_fields carries format-specific extras without polluting the struct.
pub struct Metadata {
    pub parameter: Parameter,
    pub level: Level,
    pub reference_time: String,
    pub forecast_hours: i32,
    pub originating_centre: String,
    pub grid: Option<GridDefinition>,
}
