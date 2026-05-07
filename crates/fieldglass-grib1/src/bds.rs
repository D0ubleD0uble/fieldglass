use fieldglass_core::FieldglassError;

/// Header of the Binary Data Section. Does not own the packed data.
#[derive(Debug)]
pub struct BdsHeader {
    pub section_len: u32,
    /// True = spherical-harmonic coefficients (unsupported here).
    pub is_spherical_harmonic: bool,
    /// True = complex / second-order packing (unsupported here).
    pub is_complex_packing: bool,
    /// True = integer values; false = floating point.
    pub is_integer_data: bool,
    /// True if extra flag octets follow (complex packing only).
    pub has_extra_flags: bool,
    /// Number of unused bits at the end of the packed data stream.
    pub unused_trailing_bits: u8,
    /// Binary scale factor E (sign-magnitude i16 in the wire format).
    pub binary_scale_factor: i16,
    /// Reference value R, decoded from IBM single-precision float.
    pub reference_value: f64,
    /// Bits per packed value N. Zero means a constant field.
    pub bits_per_value: u8,
}

/// Offset (within the BDS) at which packed data values begin.
pub const BDS_DATA_OFFSET: usize = 11;

/// Parse the 11-byte BDS header. `bytes` should begin at the start of the BDS.
pub fn parse_bds_header(bytes: &[u8]) -> Result<BdsHeader, FieldglassError> {
    if bytes.len() < BDS_DATA_OFFSET {
        return Err(FieldglassError::Parse(format!(
            "BDS header requires {BDS_DATA_OFFSET} bytes, got {}",
            bytes.len()
        )));
    }

    let section_len = read_u24(&bytes[0..3]);
    if (section_len as usize) < BDS_DATA_OFFSET {
        return Err(FieldglassError::Parse(format!(
            "BDS section_len {section_len} below minimum of {BDS_DATA_OFFSET}"
        )));
    }
    if bytes.len() < section_len as usize {
        return Err(FieldglassError::Parse(format!(
            "BDS section_len {section_len} exceeds available bytes {}",
            bytes.len()
        )));
    }

    let flag = bytes[3];
    Ok(BdsHeader {
        section_len,
        is_spherical_harmonic: flag & 0x80 != 0,
        is_complex_packing:    flag & 0x40 != 0,
        is_integer_data:       flag & 0x20 != 0,
        has_extra_flags:       flag & 0x10 != 0,
        unused_trailing_bits:  flag & 0x0F,
        binary_scale_factor:   sign_magnitude_i16(u16::from_be_bytes([bytes[4], bytes[5]])),
        reference_value:       ibm_float_to_f64(u32::from_be_bytes([
            bytes[6], bytes[7], bytes[8], bytes[9],
        ])),
        bits_per_value: bytes[10],
    })
}

/// Decode a simple-packed grid into floating-point values.
///
/// `bds` is the full Binary Data Section starting at its length octets;
/// `header` is the parsed header for `bds`; `decimal_scale` is the PDS
/// `decimal_scale_factor` (D); `bitmap` is the BMS bitmap if one was present;
/// `expected_count` is the total number of grid points (from the GDS).
///
/// Returns one `Option<f64>` per grid point: `None` for points masked out by
/// the bitmap, `Some(value)` otherwise. Complex / spherical-harmonic packing
/// is rejected with `UnsupportedSection`.
pub fn decode_values(
    bds: &[u8],
    header: &BdsHeader,
    decimal_scale: i16,
    bitmap: Option<&[bool]>,
    expected_count: usize,
) -> Result<Vec<Option<f64>>, FieldglassError> {
    if header.is_spherical_harmonic || header.is_complex_packing {
        return Err(FieldglassError::UnsupportedSection);
    }
    if (bds.len() as u32) < header.section_len {
        return Err(FieldglassError::Parse(format!(
            "BDS body shorter than declared section_len {}",
            header.section_len
        )));
    }

    let d_scale = 10f64.powi(-(decimal_scale as i32));
    let r = header.reference_value;
    let two_pow_e = 2f64.powi(header.binary_scale_factor as i32);

    // Constant field: every present grid point equals R / 10^D.
    if header.bits_per_value == 0 {
        let constant = r * d_scale;
        return Ok(materialise_constant(constant, bitmap, expected_count));
    }

    if header.bits_per_value > 32 {
        return Err(FieldglassError::Parse(format!(
            "BDS bits_per_value {} exceeds 32", header.bits_per_value
        )));
    }

    let n = header.bits_per_value;
    let packed_byte_len = header.section_len as usize - BDS_DATA_OFFSET;
    let total_packed_bits = packed_byte_len
        .saturating_mul(8)
        .saturating_sub(header.unused_trailing_bits as usize);
    let stored_count = total_packed_bits / n as usize;

    let present_count = match bitmap {
        Some(b) => b.iter().filter(|p| **p).count(),
        None => expected_count,
    };
    if stored_count < present_count {
        return Err(FieldglassError::Parse(format!(
            "BDS holds {stored_count} values but {present_count} are required"
        )));
    }

    let packed = &bds[BDS_DATA_OFFSET..header.section_len as usize];
    let mut reader = BitReader::new(packed);
    let mut decoded = Vec::with_capacity(present_count);
    for _ in 0..present_count {
        let x = reader.read_bits(n)?;
        decoded.push((r + x as f64 * two_pow_e) * d_scale);
    }

    Ok(interleave_with_bitmap(decoded, bitmap, expected_count))
}

fn materialise_constant(
    value: f64,
    bitmap: Option<&[bool]>,
    expected_count: usize,
) -> Vec<Option<f64>> {
    match bitmap {
        Some(b) => b
            .iter()
            .take(expected_count)
            .map(|present| if *present { Some(value) } else { None })
            .collect(),
        None => vec![Some(value); expected_count],
    }
}

fn interleave_with_bitmap(
    decoded: Vec<f64>,
    bitmap: Option<&[bool]>,
    expected_count: usize,
) -> Vec<Option<f64>> {
    match bitmap {
        None => decoded.into_iter().map(Some).collect(),
        Some(b) => {
            let mut out = Vec::with_capacity(expected_count);
            let mut iter = decoded.into_iter();
            for present in b.iter().take(expected_count) {
                if *present {
                    out.push(iter.next());
                } else {
                    out.push(None);
                }
            }
            out
        }
    }
}

// ---------------------------------------------------------------------------
// Numeric helpers
// ---------------------------------------------------------------------------

/// 16-bit sign-magnitude integer used by GRIB1 for the binary scale factor.
/// High bit is sign, low 15 bits are magnitude.
fn sign_magnitude_i16(raw: u16) -> i16 {
    let magnitude = (raw & 0x7FFF) as i16;
    if raw & 0x8000 != 0 { -magnitude } else { magnitude }
}

/// IBM System/360 single-precision float → f64.
/// Layout: sign(1) | characteristic(7, excess-64) | fraction(24), base 16.
fn ibm_float_to_f64(raw: u32) -> f64 {
    if raw == 0 {
        return 0.0;
    }
    let sign = if raw & 0x8000_0000 != 0 { -1.0 } else { 1.0 };
    let characteristic = ((raw >> 24) & 0x7F) as i32;
    let fraction = (raw & 0x00FF_FFFF) as f64 / (1u32 << 24) as f64;
    sign * fraction * 16f64.powi(characteristic - 64)
}

fn read_u24(b: &[u8]) -> u32 {
    u32::from_be_bytes([0, b[0], b[1], b[2]])
}

// ---------------------------------------------------------------------------
// Bit reader (MSB first, suitable for GRIB packed integers up to 32 bits)
// ---------------------------------------------------------------------------

struct BitReader<'a> {
    bytes: &'a [u8],
    bit_offset: usize,
}

impl<'a> BitReader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, bit_offset: 0 }
    }

    fn read_bits(&mut self, n: u8) -> Result<u32, FieldglassError> {
        if n == 0 {
            return Ok(0);
        }
        let end_bit = self.bit_offset + n as usize;
        if end_bit > self.bytes.len() * 8 {
            return Err(FieldglassError::Parse(format!(
                "BDS bit reader exhausted at offset {} reading {n} bits",
                self.bit_offset
            )));
        }

        let mut value: u64 = 0;
        let mut bits_collected = 0u8;
        let mut bit = self.bit_offset;
        while bits_collected < n {
            let byte_idx = bit / 8;
            let bit_in_byte = bit % 8;
            let take = (8 - bit_in_byte).min((n - bits_collected) as usize) as u8;
            let shift = 8 - bit_in_byte - take as usize;
            let mask = ((1u16 << take) - 1) as u8;
            let chunk = (self.bytes[byte_idx] >> shift) & mask;
            value = (value << take) | chunk as u64;
            bits_collected += take;
            bit += take as usize;
        }
        self.bit_offset = end_bit;
        Ok(value as u32)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ibm_float_zero() {
        assert_eq!(ibm_float_to_f64(0x0000_0000), 0.0);
    }

    #[test]
    fn ibm_float_one_half() {
        // 0.5 = 0x40 80 00 00 in IBM single: char=64 → exp 0, fraction = 0x800000/2^24 = 0.5
        assert!((ibm_float_to_f64(0x4080_0000) - 0.5).abs() < 1e-12);
    }

    #[test]
    fn ibm_float_negative_one_half() {
        assert!((ibm_float_to_f64(0xC080_0000) + 0.5).abs() < 1e-12);
    }

    #[test]
    fn ibm_float_one() {
        // 1.0 = 0x41 10 00 00: char=65 → exp 1, fraction = 0x100000/2^24 = 1/16, *16 = 1.0
        assert!((ibm_float_to_f64(0x4110_0000) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn sign_magnitude_positive() {
        assert_eq!(sign_magnitude_i16(0x0005), 5);
    }

    #[test]
    fn sign_magnitude_negative() {
        assert_eq!(sign_magnitude_i16(0x8005), -5);
    }

    #[test]
    fn sign_magnitude_zero_signed() {
        // Negative zero is well-defined in sign-magnitude; we collapse it to 0.
        assert_eq!(sign_magnitude_i16(0x8000), 0);
    }

    #[test]
    fn bit_reader_byte_aligned() {
        let mut r = BitReader::new(&[0xAB, 0xCD]);
        assert_eq!(r.read_bits(8).unwrap(), 0xAB);
        assert_eq!(r.read_bits(8).unwrap(), 0xCD);
    }

    #[test]
    fn bit_reader_unaligned() {
        // 0b1010_0101_1100_0011 — read as 3,5,8 bits MSB-first.
        let mut r = BitReader::new(&[0b1010_0101, 0b1100_0011]);
        assert_eq!(r.read_bits(3).unwrap(), 0b101);
        assert_eq!(r.read_bits(5).unwrap(), 0b00101);
        assert_eq!(r.read_bits(8).unwrap(), 0b1100_0011);
    }

    #[test]
    fn bit_reader_crosses_byte_boundary() {
        let mut r = BitReader::new(&[0xFF, 0x00]);
        assert_eq!(r.read_bits(12).unwrap(), 0xFF0);
    }

    #[test]
    fn bit_reader_exhaustion() {
        let mut r = BitReader::new(&[0x00]);
        assert!(r.read_bits(9).is_err());
    }

    #[test]
    fn decode_constant_field() {
        // bits_per_value = 0 → all points equal R / 10^D.
        let header = BdsHeader {
            section_len: BDS_DATA_OFFSET as u32,
            is_spherical_harmonic: false,
            is_complex_packing: false,
            is_integer_data: false,
            has_extra_flags: false,
            unused_trailing_bits: 0,
            binary_scale_factor: 0,
            reference_value: 42.0,
            bits_per_value: 0,
        };
        let bds = vec![0u8; BDS_DATA_OFFSET];
        let out = decode_values(&bds, &header, 0, None, 4).unwrap();
        assert_eq!(out, vec![Some(42.0); 4]);
    }

    #[test]
    fn decode_simple_packing_round_trip() {
        // 4 values packed at 8 bits each, R=0, E=0, D=0 → identity.
        let mut bds = vec![0u8; BDS_DATA_OFFSET];
        bds.extend_from_slice(&[1, 2, 3, 4]);
        let section_len = bds.len() as u32;
        bds[0..3].copy_from_slice(&[(section_len >> 16) as u8, (section_len >> 8) as u8, section_len as u8]);
        bds[10] = 8; // N
        let header = parse_bds_header(&bds).unwrap();
        let out = decode_values(&bds, &header, 0, None, 4).unwrap();
        assert_eq!(out, vec![Some(1.0), Some(2.0), Some(3.0), Some(4.0)]);
    }

    #[test]
    fn decode_with_bitmap_inserts_none() {
        let mut bds = vec![0u8; BDS_DATA_OFFSET];
        bds.extend_from_slice(&[7, 9]);
        let section_len = bds.len() as u32;
        bds[0..3].copy_from_slice(&[(section_len >> 16) as u8, (section_len >> 8) as u8, section_len as u8]);
        bds[10] = 8;
        let header = parse_bds_header(&bds).unwrap();
        let bitmap = [true, false, true, false];
        let out = decode_values(&bds, &header, 0, Some(&bitmap), 4).unwrap();
        assert_eq!(out, vec![Some(7.0), None, Some(9.0), None]);
    }

    #[test]
    fn rejects_complex_packing() {
        let mut bds = vec![0u8; BDS_DATA_OFFSET];
        bds[0..3].copy_from_slice(&[0, 0, BDS_DATA_OFFSET as u8]);
        bds[3] = 0x40; // complex packing flag
        let header = parse_bds_header(&bds).unwrap();
        assert!(matches!(
            decode_values(&bds, &header, 0, None, 1).unwrap_err(),
            FieldglassError::UnsupportedSection
        ));
    }
}
