use fieldglass_core::FieldglassError;

pub struct IndicatorSection {
    /// Total length of the GRIB message in bytes (IS + all sections + ES).
    pub total_length: u32,
    /// GRIB edition number (1 for GRIB1).
    pub edition: u8,
}

/// Parse the 8-byte Indicator Section starting at `bytes[0]`.
pub fn parse_indicator(bytes: &[u8]) -> Result<IndicatorSection, FieldglassError> {
    if bytes.len() < 8 {
        return Err(FieldglassError::Parse(format!(
            "IS requires 8 bytes, got {}",
            bytes.len()
        )));
    }
    if &bytes[0..4] != b"GRIB" {
        return Err(FieldglassError::InvalidMagic);
    }
    let total_length = u32::from_be_bytes([0, bytes[4], bytes[5], bytes[6]]);
    let edition = bytes[7];
    Ok(IndicatorSection { total_length, edition })
}
