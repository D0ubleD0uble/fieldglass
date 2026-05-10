//! Pure-Rust parser for the NetCDF classic on-disk header — covering CDF-1
//! (32-bit offsets), CDF-2 (64-bit offsets), and CDF-5 (64-bit sizes / offsets
//! / extended numeric types).
//!
//! This is a *header-only* parser: it walks the dim_list, gatt_list, and
//! var_list at the start of the file and exposes their contents as Rust
//! structs. Per-variable value decoding is intentionally out of scope (see
//! issue #29 non-goals).
//!
//! Reference: Unidata classic format spec
//! <https://docs.unidata.ucar.edu/netcdf-c/current/file_format_specifications.html#classic_format_spec>.
//!
//! All multi-byte integers are big-endian. Strings are UTF-8. Everything is
//! padded to 4-byte boundaries — including odd-length strings, attribute
//! values, and the implicit "fill to next word" after each variable record.

use fieldglass_core::FieldglassError;

/// Three on-disk variants of NetCDF classic. They differ in the width of size
/// and offset fields and (for CDF-5) the set of supported numeric types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClassicVersion {
    /// `CDF\x01` — 32-bit `nelems`, dim length, vsize, and var begin.
    Cdf1,
    /// `CDF\x02` — like CDF-1 but `begin` (offset) is 64-bit so files can
    /// exceed 2 GiB.
    Cdf2,
    /// `CDF\x05` — `nelems`, dim length, vsize, and `begin` are all 64-bit;
    /// adds unsigned and 64-bit numeric `nc_type`s.
    Cdf5,
}

impl ClassicVersion {
    pub fn from_byte(b: u8) -> Option<Self> {
        match b {
            1 => Some(Self::Cdf1),
            2 => Some(Self::Cdf2),
            5 => Some(Self::Cdf5),
            _ => None,
        }
    }

    /// Width in bytes of `NON_NEG` (counts, dim lengths, attribute counts,
    /// variable element counts, vsize): 4 for CDF-1/2, 8 for CDF-5.
    fn nonneg_width(self) -> usize {
        match self {
            Self::Cdf1 | Self::Cdf2 => 4,
            Self::Cdf5 => 8,
        }
    }

    /// Width of variable `begin` (file offset): 4 for CDF-1, 8 for CDF-2/5.
    fn offset_width(self) -> usize {
        match self {
            Self::Cdf1 => 4,
            Self::Cdf2 | Self::Cdf5 => 8,
        }
    }
}

/// NetCDF external type codes (`nc_type`). See the Unidata classic format
/// spec; the CDF-5 entries (UBYTE…UINT64) are only legal in CDF-5 files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NcType {
    Byte,   // 1
    Char,   // 2
    Short,  // 3
    Int,    // 4
    Float,  // 5
    Double, // 6
    UByte,  // 7  (CDF-5)
    UShort, // 8  (CDF-5)
    UInt,   // 9  (CDF-5)
    Int64,  // 10 (CDF-5)
    UInt64, // 11 (CDF-5)
}

impl NcType {
    fn from_code(code: u32, version: ClassicVersion) -> Result<Self, FieldglassError> {
        let allow_cdf5 = matches!(version, ClassicVersion::Cdf5);
        match code {
            1 => Ok(Self::Byte),
            2 => Ok(Self::Char),
            3 => Ok(Self::Short),
            4 => Ok(Self::Int),
            5 => Ok(Self::Float),
            6 => Ok(Self::Double),
            7 if allow_cdf5 => Ok(Self::UByte),
            8 if allow_cdf5 => Ok(Self::UShort),
            9 if allow_cdf5 => Ok(Self::UInt),
            10 if allow_cdf5 => Ok(Self::Int64),
            11 if allow_cdf5 => Ok(Self::UInt64),
            _ => Err(FieldglassError::Parse(format!(
                "unknown nc_type code {code}"
            ))),
        }
    }

    /// Short canonical name used in attribute / variable display.
    pub fn name(self) -> &'static str {
        match self {
            Self::Byte => "byte",
            Self::Char => "char",
            Self::Short => "short",
            Self::Int => "int",
            Self::Float => "float",
            Self::Double => "double",
            Self::UByte => "ubyte",
            Self::UShort => "ushort",
            Self::UInt => "uint",
            Self::Int64 => "int64",
            Self::UInt64 => "uint64",
        }
    }

    /// Width in bytes of one element of this type on disk.
    fn element_size(self) -> usize {
        match self {
            Self::Byte | Self::UByte | Self::Char => 1,
            Self::Short | Self::UShort => 2,
            Self::Int | Self::UInt | Self::Float => 4,
            Self::Double | Self::Int64 | Self::UInt64 => 8,
        }
    }
}

/// A NetCDF dimension. `is_record == true` for the unlimited / record
/// dimension, which on disk is encoded with `dim_length == 0`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dimension {
    pub name: String,
    pub length: u64,
    pub is_record: bool,
}

/// A NetCDF attribute. The raw on-disk bytes are parsed into a single
/// human-readable string — for `Char` attributes this is the UTF-8 text;
/// for numeric types it's a comma-separated decimal list. Decoding to typed
/// values is out of scope here.
#[derive(Debug, Clone)]
pub struct Attribute {
    pub name: String,
    pub nc_type: NcType,
    /// Number of elements in the attribute's value.
    pub nelems: u64,
    /// Display value: UTF-8 for `Char`, decimal list for numeric types.
    pub value: String,
}

/// A NetCDF variable. Attribute decoding is shared with global attributes.
/// `begin` and `vsize` come straight from the on-disk record and are surfaced
/// for completeness, but per-variable value decode is a separate issue.
#[derive(Debug, Clone)]
pub struct Variable {
    pub name: String,
    /// Indices into the parent header's `dimensions` list. May be empty
    /// (scalar variable).
    pub dim_ids: Vec<u32>,
    pub nc_type: NcType,
    pub attributes: Vec<Attribute>,
    pub vsize: u64,
    pub begin: u64,
}

/// Top-level on-disk header for a NetCDF classic / 64-bit / CDF-5 file.
#[derive(Debug, Clone)]
pub struct ClassicHeader {
    pub version: ClassicVersion,
    /// Number of records in the unlimited dimension. `None` for streaming
    /// files (`numrecs == 0xFFFFFFFF`).
    pub numrecs: Option<u64>,
    pub dimensions: Vec<Dimension>,
    pub global_attributes: Vec<Attribute>,
    pub variables: Vec<Variable>,
}

/// Tag values for component lists in the header.
const NC_DIMENSION: u32 = 0x0A;
const NC_VARIABLE: u32 = 0x0B;
const NC_ATTRIBUTE: u32 = 0x0C;

/// Parse a NetCDF classic header from the start of `bytes`. Stops walking
/// after `var_list`; the rest of the file is variable data, which we ignore.
pub fn parse_header(bytes: &[u8]) -> Result<ClassicHeader, FieldglassError> {
    let mut p = Parser::new(bytes)?;

    // numrecs follows the 4-byte magic.
    let numrecs = p.read_numrecs()?;

    let dimensions = p.read_dim_list()?;
    let global_attributes = p.read_att_list()?;
    let variables = p.read_var_list(dimensions.len())?;

    Ok(ClassicHeader {
        version: p.version,
        numrecs,
        dimensions,
        global_attributes,
        variables,
    })
}

// ---------------------------------------------------------------------------
// Parser
// ---------------------------------------------------------------------------

struct Parser<'a> {
    bytes: &'a [u8],
    pos: usize,
    version: ClassicVersion,
}

impl<'a> Parser<'a> {
    fn new(bytes: &'a [u8]) -> Result<Self, FieldglassError> {
        if bytes.len() < 4 {
            return Err(FieldglassError::Parse(
                "NetCDF classic header requires at least the 4-byte magic".to_string(),
            ));
        }
        if &bytes[0..3] != b"CDF" {
            return Err(FieldglassError::InvalidMagic);
        }
        let version = ClassicVersion::from_byte(bytes[3]).ok_or_else(|| {
            FieldglassError::Parse(format!(
                "unsupported NetCDF classic version byte 0x{:02x}",
                bytes[3]
            ))
        })?;
        Ok(Self {
            bytes,
            pos: 4,
            version,
        })
    }

    fn need(&self, n: usize) -> Result<(), FieldglassError> {
        if self.pos + n > self.bytes.len() {
            return Err(FieldglassError::Parse(format!(
                "truncated NetCDF header: needed {} bytes at offset {}, only {} remain",
                n,
                self.pos,
                self.bytes.len().saturating_sub(self.pos)
            )));
        }
        Ok(())
    }

    fn read_u32_be(&mut self) -> Result<u32, FieldglassError> {
        self.need(4)?;
        let v = u32::from_be_bytes(self.bytes[self.pos..self.pos + 4].try_into().unwrap());
        self.pos += 4;
        Ok(v)
    }

    fn read_u64_be(&mut self) -> Result<u64, FieldglassError> {
        self.need(8)?;
        let v = u64::from_be_bytes(self.bytes[self.pos..self.pos + 8].try_into().unwrap());
        self.pos += 8;
        Ok(v)
    }

    /// `NON_NEG`: 4 bytes for CDF-1/2, 8 bytes for CDF-5. Always returned as
    /// `u64` for uniformity.
    fn read_nonneg(&mut self) -> Result<u64, FieldglassError> {
        match self.version.nonneg_width() {
            4 => Ok(self.read_u32_be()? as u64),
            8 => self.read_u64_be(),
            _ => unreachable!(),
        }
    }

    /// Variable `begin` field width depends on the version.
    fn read_offset(&mut self) -> Result<u64, FieldglassError> {
        match self.version.offset_width() {
            4 => Ok(self.read_u32_be()? as u64),
            8 => self.read_u64_be(),
            _ => unreachable!(),
        }
    }

    /// Read `n` bytes verbatim, then advance past zero padding so `pos`
    /// returns to a 4-byte boundary.
    fn read_bytes_padded(&mut self, n: usize) -> Result<&'a [u8], FieldglassError> {
        self.need(n)?;
        let start = self.pos;
        self.pos += n;
        // Pad to next 4-byte boundary.
        let pad = (4 - (n % 4)) % 4;
        self.need(pad)?;
        self.pos += pad;
        Ok(&self.bytes[start..start + n])
    }

    /// Read a `name` field: NON_NEG length, then UTF-8 bytes, then padding.
    fn read_name(&mut self) -> Result<String, FieldglassError> {
        let n = self.read_nonneg()?;
        // Hard cap to keep a corrupt header from triggering huge allocations.
        if n > self.bytes.len() as u64 {
            return Err(FieldglassError::Parse(format!(
                "name length {n} exceeds file size"
            )));
        }
        let raw = self.read_bytes_padded(n as usize)?;
        Ok(String::from_utf8_lossy(raw).into_owned())
    }

    /// `numrecs`: width matches NON_NEG. The value `0xFFFFFFFF` (CDF-1/2) or
    /// `0xFFFFFFFFFFFFFFFF` (CDF-5) means "streaming, count not known yet",
    /// which we surface as `None`.
    fn read_numrecs(&mut self) -> Result<Option<u64>, FieldglassError> {
        let v = self.read_nonneg()?;
        let streaming = match self.version {
            ClassicVersion::Cdf1 | ClassicVersion::Cdf2 => v == 0xFFFF_FFFF,
            ClassicVersion::Cdf5 => v == u64::MAX,
        };
        Ok(if streaming { None } else { Some(v) })
    }

    /// Read a list header: either `NC_<TAG>` followed by a NON_NEG count, or
    /// `ABSENT` (two zero NON_NEGs) — return `None` for absent.
    fn read_list_header(&mut self, expected_tag: u32) -> Result<Option<u64>, FieldglassError> {
        let tag = self.read_u32_be()?;
        if tag == 0 {
            // ABSENT: the count word that follows must also be zero.
            let zero = self.read_nonneg()?;
            if zero != 0 {
                return Err(FieldglassError::Parse(format!(
                    "ABSENT list tag followed by non-zero count {zero}"
                )));
            }
            return Ok(None);
        }
        if tag != expected_tag {
            return Err(FieldglassError::Parse(format!(
                "expected list tag 0x{expected_tag:02x} (or ABSENT), got 0x{tag:08x} at offset {}",
                self.pos - 4
            )));
        }
        let n = self.read_nonneg()?;
        Ok(Some(n))
    }

    fn read_dim_list(&mut self) -> Result<Vec<Dimension>, FieldglassError> {
        let count = match self.read_list_header(NC_DIMENSION)? {
            Some(n) => n,
            None => return Ok(Vec::new()),
        };
        // Sanity-cap: dim count can't exceed remaining bytes.
        if count > self.bytes.len() as u64 {
            return Err(FieldglassError::Parse(format!(
                "dim_list count {count} exceeds file size"
            )));
        }
        let mut dims = Vec::with_capacity(count as usize);
        for _ in 0..count {
            let name = self.read_name()?;
            let length = self.read_nonneg()?;
            dims.push(Dimension {
                name,
                length,
                is_record: length == 0,
            });
        }
        Ok(dims)
    }

    fn read_att_list(&mut self) -> Result<Vec<Attribute>, FieldglassError> {
        let count = match self.read_list_header(NC_ATTRIBUTE)? {
            Some(n) => n,
            None => return Ok(Vec::new()),
        };
        if count > self.bytes.len() as u64 {
            return Err(FieldglassError::Parse(format!(
                "att_list count {count} exceeds file size"
            )));
        }
        let mut atts = Vec::with_capacity(count as usize);
        for _ in 0..count {
            atts.push(self.read_attribute()?);
        }
        Ok(atts)
    }

    fn read_attribute(&mut self) -> Result<Attribute, FieldglassError> {
        let name = self.read_name()?;
        let type_code = self.read_u32_be()?;
        let nc_type = NcType::from_code(type_code, self.version)?;
        let nelems = self.read_nonneg()?;

        let total_bytes = (nelems as usize)
            .checked_mul(nc_type.element_size())
            .ok_or_else(|| {
                FieldglassError::Parse(format!(
                    "attribute {name:?} declares element count {nelems} that overflows usize"
                ))
            })?;
        if total_bytes > self.bytes.len() {
            return Err(FieldglassError::Parse(format!(
                "attribute {name:?} value ({total_bytes} bytes) exceeds file size"
            )));
        }
        let raw = self.read_bytes_padded(total_bytes)?;

        let value = match nc_type {
            NcType::Char => {
                // Trim trailing NULs which some writers emit.
                let s = String::from_utf8_lossy(raw);
                s.trim_end_matches('\0').to_string()
            }
            _ => render_numeric_values(raw, nc_type),
        };

        Ok(Attribute {
            name,
            nc_type,
            nelems,
            value,
        })
    }

    fn read_var_list(&mut self, num_dims: usize) -> Result<Vec<Variable>, FieldglassError> {
        let count = match self.read_list_header(NC_VARIABLE)? {
            Some(n) => n,
            None => return Ok(Vec::new()),
        };
        if count > self.bytes.len() as u64 {
            return Err(FieldglassError::Parse(format!(
                "var_list count {count} exceeds file size"
            )));
        }
        let mut vars = Vec::with_capacity(count as usize);
        for _ in 0..count {
            vars.push(self.read_variable(num_dims)?);
        }
        Ok(vars)
    }

    fn read_variable(&mut self, num_dims: usize) -> Result<Variable, FieldglassError> {
        let name = self.read_name()?;
        let dimensionality = self.read_nonneg()?;
        if dimensionality > num_dims as u64 + 1024 {
            // Defensive cap so a corrupt header can't trigger a huge allocation.
            return Err(FieldglassError::Parse(format!(
                "variable {name:?} declares {dimensionality} dimensions but file has only {num_dims}"
            )));
        }
        let mut dim_ids = Vec::with_capacity(dimensionality as usize);
        for _ in 0..dimensionality {
            let d = self.read_u32_be()?;
            // dim ids are written as 4-byte BE integers regardless of CDF
            // version; the spec is explicit on this.
            if (d as usize) >= num_dims {
                return Err(FieldglassError::Parse(format!(
                    "variable {name:?} references dim id {d} but only {num_dims} dimensions exist"
                )));
            }
            dim_ids.push(d);
        }
        let attributes = self.read_att_list()?;
        let type_code = self.read_u32_be()?;
        let nc_type = NcType::from_code(type_code, self.version)?;
        let vsize = self.read_nonneg()?;
        let begin = self.read_offset()?;
        Ok(Variable {
            name,
            dim_ids,
            nc_type,
            attributes,
            vsize,
            begin,
        })
    }
}

/// Render a numeric attribute value as a comma-separated decimal string. Each
/// chunk of `element_size` bytes is decoded according to `nc_type`. Truncated
/// trailing bytes are silently ignored — the caller has already verified the
/// length matches `nelems * element_size`.
fn render_numeric_values(raw: &[u8], nc_type: NcType) -> String {
    let elem = nc_type.element_size();
    if elem == 0 {
        return String::new();
    }
    let mut parts: Vec<String> = Vec::with_capacity(raw.len() / elem);
    for chunk in raw.chunks_exact(elem) {
        let s = match nc_type {
            NcType::Byte => (chunk[0] as i8).to_string(),
            NcType::UByte => chunk[0].to_string(),
            NcType::Char => {
                // Handled by the caller, but be defensive.
                (chunk[0] as char).to_string()
            }
            NcType::Short => i16::from_be_bytes([chunk[0], chunk[1]]).to_string(),
            NcType::UShort => u16::from_be_bytes([chunk[0], chunk[1]]).to_string(),
            NcType::Int => i32::from_be_bytes(chunk.try_into().unwrap()).to_string(),
            NcType::UInt => u32::from_be_bytes(chunk.try_into().unwrap()).to_string(),
            NcType::Float => {
                let v = f32::from_be_bytes(chunk.try_into().unwrap());
                format_float(v as f64)
            }
            NcType::Double => {
                let v = f64::from_be_bytes(chunk.try_into().unwrap());
                format_float(v)
            }
            NcType::Int64 => i64::from_be_bytes(chunk.try_into().unwrap()).to_string(),
            NcType::UInt64 => u64::from_be_bytes(chunk.try_into().unwrap()).to_string(),
        };
        parts.push(s);
    }
    parts.join(", ")
}

/// Format a float compactly: trim trailing zeros while keeping enough
/// precision that the displayed value round-trips for typical attributes.
fn format_float(v: f64) -> String {
    if !v.is_finite() {
        return v.to_string();
    }
    // 6 significant digits is enough for human-readable attribute display
    // (precision-critical use is per-variable decode, which is out of scope).
    let formatted = format!("{v:.6}");
    if formatted.contains('.') {
        let trimmed = formatted.trim_end_matches('0').trim_end_matches('.');
        if trimmed.is_empty() || trimmed == "-" {
            "0".to_string()
        } else {
            trimmed.to_string()
        }
    } else {
        formatted
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build the smallest plausible CDF-1 file: magic, numrecs=0, no dims, no
    /// global atts, no vars.
    fn empty_cdf1() -> Vec<u8> {
        let mut v = Vec::new();
        v.extend_from_slice(b"CDF\x01");
        v.extend_from_slice(&0u32.to_be_bytes()); // numrecs
        v.extend_from_slice(&0u32.to_be_bytes()); // dim_list ABSENT tag
        v.extend_from_slice(&0u32.to_be_bytes()); // dim_list count 0
        v.extend_from_slice(&0u32.to_be_bytes()); // gatt_list ABSENT tag
        v.extend_from_slice(&0u32.to_be_bytes()); // gatt_list count 0
        v.extend_from_slice(&0u32.to_be_bytes()); // var_list ABSENT tag
        v.extend_from_slice(&0u32.to_be_bytes()); // var_list count 0
        v
    }

    #[test]
    fn empty_header_round_trips() {
        let h = parse_header(&empty_cdf1()).unwrap();
        assert_eq!(h.version, ClassicVersion::Cdf1);
        assert_eq!(h.numrecs, Some(0));
        assert!(h.dimensions.is_empty());
        assert!(h.global_attributes.is_empty());
        assert!(h.variables.is_empty());
    }

    #[test]
    fn truncated_after_magic_errors() {
        let bytes = b"CDF\x01\x00\x00";
        let err = parse_header(bytes).unwrap_err();
        assert!(matches!(err, FieldglassError::Parse(_)));
    }

    #[test]
    fn unknown_version_errors() {
        let bytes = b"CDF\x09\x00\x00\x00\x00";
        let err = parse_header(bytes).unwrap_err();
        assert!(matches!(err, FieldglassError::Parse(_)));
    }

    #[test]
    fn bogus_magic_errors() {
        let bytes = b"NOPE\x00\x00\x00\x00";
        let err = parse_header(bytes).unwrap_err();
        assert!(matches!(err, FieldglassError::InvalidMagic));
    }

    #[test]
    fn streaming_numrecs_is_none() {
        let mut v = Vec::new();
        v.extend_from_slice(b"CDF\x01");
        v.extend_from_slice(&0xFFFF_FFFFu32.to_be_bytes());
        v.extend_from_slice(&0u32.to_be_bytes()); // ABSENT dim
        v.extend_from_slice(&0u32.to_be_bytes());
        v.extend_from_slice(&0u32.to_be_bytes()); // ABSENT gatt
        v.extend_from_slice(&0u32.to_be_bytes());
        v.extend_from_slice(&0u32.to_be_bytes()); // ABSENT var
        v.extend_from_slice(&0u32.to_be_bytes());
        let h = parse_header(&v).unwrap();
        assert_eq!(h.numrecs, None);
    }
}
