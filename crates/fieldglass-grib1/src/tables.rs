pub struct ParameterEntry {
    pub id: i32,
    pub table_version: i32,
    pub name: String,
    pub abbreviation: String,
    pub units: String,
}

pub fn lookup_parameter(id: i32, table_version: i32) -> ParameterEntry {
    todo!("Implement lookup_parameter")
}

pub fn lookup_level_name(level_type: i32) -> String {
    todo!("Implement lookup_level_name")
}

pub fn lookup_centre_name(centre_id: i32) -> String {
    todo!("Implement lookup_centre_name")
}
