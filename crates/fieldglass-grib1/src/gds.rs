pub struct GridDescription {
    pub grid_type: i32,
    pub ni: i32,
    pub nj: i32,
    pub lat_first: f64,
    pub lon_first: f64,
    pub lat_last: f64,
    pub lon_last: f64,
    pub di: f64,
    pub dj: f64,
}

pub fn parse_grid_description(bytes: Vec<u8>) -> GridDescription {
    todo!("Implement parse_grid_description")
}
