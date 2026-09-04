/// A human-readable parameter (e.g. "Temperature", "Wind Speed")
#[derive(Debug)]
pub struct Parameter {
    /// Human-readable name, or `"Unknown"` when no table resolved the id.
    pub name: String,
    /// The table's short name, e.g. `"2t"`. Empty when unresolved.
    pub abbreviation: String,
    /// Units as the table states them; empty when dimensionless or unresolved.
    pub units: String,
    /// The parameter's numeric id in its own format's table.
    pub id: i32,
}

/// A vertical level descriptor
#[derive(Debug)]
pub struct Level {
    /// The surface type, named — `"isobaricInhPa"`, `"heightAboveGround"`.
    pub level_type: String,
    /// The level's value on that surface.
    pub value: f64,
    /// Units of `value`, e.g. `"hPa"`, `"m"`.
    pub units: String,
}

/// Geographic grid geometry
#[derive(Debug)]
pub struct GridDefinition {
    /// The grid family, named — `"latlon"`, `"lambert"`, `"polar_stereo"`, …
    pub grid_type: String,
    /// Points along a row.
    pub ni: i32,
    /// Rows.
    pub nj: i32,
    /// Latitude of the first scanned point, degrees.
    pub lat_first: f64,
    /// Longitude of the first scanned point, degrees.
    pub lon_first: f64,
    /// Latitude of the last scanned point, degrees.
    pub lat_last: f64,
    /// Longitude of the last scanned point, degrees.
    pub lon_last: f64,
    /// i-direction increment in degrees.
    pub di: f64,
    /// j-direction increment in degrees.
    pub dj: f64,
}

/// All metadata for a single data message, format-agnostic.
/// raw_fields carries format-specific extras without polluting the struct.
#[derive(Debug)]
pub struct Metadata {
    /// What the field is.
    pub parameter: Parameter,
    /// Where in the vertical it sits.
    pub level: Level,
    /// Reference (analysis) time, rendered.
    pub reference_time: String,
    /// Forecast lead in whole hours from `reference_time`.
    pub forecast_hours: i32,
    /// Originating centre, named.
    pub originating_centre: String,
    /// The grid, or `None` for a message that has none (a spectral field).
    pub grid: Option<GridDefinition>,
}
