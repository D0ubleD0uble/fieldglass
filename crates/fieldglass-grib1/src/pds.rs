pub struct ProductDefinition {
    pub table_version: i32,
    pub originating_centre: i32,
    pub generating_process: i32,
    pub parameter_id: i32,
    pub level_type: i32,
    pub level_value_1: i32,
    pub level_value_2: i32,
    pub reference_year: i32,
    pub reference_month: i32,
    pub reference_day: i32,
    pub reference_hour: i32,
    pub reference_minute: i32,
    pub time_unit: i32,
    pub p1: i32,
    pub p2: i32,
    pub time_range: i32,
    pub has_gds: bool,
    pub has_bms: bool,
}

pub fn parse_product_definition(bytes: Vec<u8>) -> ProductDefinition {
    todo!("Implement parse_product_definition")
}
