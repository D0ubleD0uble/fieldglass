pub struct Grib1Message {
    pub message_index: i32,
    pub byte_offset: i32,
    pub total_length: i32,
}

pub struct Grib1Reader {
    pub file_path: String,
    pub message_count: i32,
}

pub fn open(file_path: String) -> Grib1Reader {
    todo!("Implement open")
}

pub fn message_count(reader: Grib1Reader) -> i32 {
    todo!("Implement message_count")
}

pub fn read_message(reader: Grib1Reader, index: i32) -> Grib1Message {
    todo!("Implement read_message")
}

pub fn decode_field(reader: Grib1Reader, index: i32) -> Vec<f64> {
    todo!("Implement decode_field")
}
