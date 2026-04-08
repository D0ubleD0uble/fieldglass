use fieldglass_core::FieldglassError;

pub struct ProductDefinition {
    pub section_len: u32,
    pub table_version: u8,
    pub originating_centre: u8,
    pub generating_process: u8,
    pub grid_number: u8,
    /// True if a Grid Description Section follows.
    pub has_gds: bool,
    /// True if a Bit Map Section follows.
    pub has_bms: bool,
    pub parameter_id: u8,
    pub level_type: u8,
    /// First level value byte (interpretation depends on level_type).
    pub level_value_1: u8,
    /// Second level value byte (interpretation depends on level_type).
    pub level_value_2: u8,
    /// Year within the century (1–100).
    pub reference_year: u8,
    pub reference_month: u8,
    pub reference_day: u8,
    pub reference_hour: u8,
    pub reference_minute: u8,
    /// Time unit indicator (WMO Table 4).
    pub time_unit: u8,
    /// P1: forecast period / start of time range.
    pub p1: u8,
    /// P2: end of time range (used with time_range).
    pub p2: u8,
    /// Time range indicator (WMO Table 5).
    pub time_range: u8,
    /// Century (e.g. 21 for the 2000s).
    pub century: u8,
    pub sub_centre: u8,
    /// Decimal scale factor (signed).
    pub decimal_scale_factor: i16,
}

/// Parse the Product Definition Section starting at `bytes[0]`.
/// `bytes` should begin immediately after the Indicator Section (offset 8 in the message).
pub fn parse_product_definition(bytes: &[u8]) -> Result<ProductDefinition, FieldglassError> {
    if bytes.len() < 28 {
        return Err(FieldglassError::Parse(format!(
            "PDS requires at least 28 bytes, got {}",
            bytes.len()
        )));
    }
    let section_len = u32::from_be_bytes([0, bytes[0], bytes[1], bytes[2]]);
    if section_len < 28 {
        return Err(FieldglassError::Parse(format!(
            "PDS section_len {section_len} is below minimum of 28"
        )));
    }
    if bytes.len() < section_len as usize {
        return Err(FieldglassError::Parse(format!(
            "PDS section_len {section_len} exceeds available bytes {}",
            bytes.len()
        )));
    }

    let flag = bytes[7];
    let has_gds = flag & 0x80 != 0;
    let has_bms = flag & 0x40 != 0;

    let decimal_scale_factor = i16::from_be_bytes([bytes[26], bytes[27]]);

    Ok(ProductDefinition {
        section_len,
        table_version: bytes[3],
        originating_centre: bytes[4],
        generating_process: bytes[5],
        grid_number: bytes[6],
        has_gds,
        has_bms,
        parameter_id: bytes[8],
        level_type: bytes[9],
        level_value_1: bytes[10],
        level_value_2: bytes[11],
        reference_year: bytes[12],
        reference_month: bytes[13],
        reference_day: bytes[14],
        reference_hour: bytes[15],
        reference_minute: bytes[16],
        time_unit: bytes[17],
        p1: bytes[18],
        p2: bytes[19],
        time_range: bytes[20],
        century: bytes[24],
        sub_centre: bytes[25],
        decimal_scale_factor,
    })
}
