pub struct BinaryDataSection {
    pub scale_factor: f64,
    pub reference_value: f64,
    pub bits_per_value: i32,
    pub grid_point_count: i32,
}

/// Parse BDS header only — do not unpack values
pub fn parse_bds_header(bytes: Vec<u8>) -> BinaryDataSection {
    todo!("Implement parse_bds_header")
}

/// Unpack all grid point values — expensive, call lazily
pub fn decode_values(bytes: Vec<u8>, header: BinaryDataSection) -> Vec<f64> {
    todo!("Implement decode_values")
}
