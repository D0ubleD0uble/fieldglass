//! Pure-Rust reader for the NetCDF classic on-disk format — covering CDF-1
//! (32-bit offsets), CDF-2 (64-bit offsets), and CDF-5 (64-bit sizes / offsets
//! / extended numeric types).
//!
//! [`parse_header`] walks the dim_list, gatt_list, and var_list at the start of
//! the file and exposes their contents as Rust structs;
//! [`decode_variable_values`] reads a variable's data array from its `begin`
//! offset (fixed-size and interleaved record variables, with `_FillValue`
//! masking).
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
/// for numeric types it's a comma-separated decimal list.
#[derive(Debug, Clone)]
pub struct Attribute {
    pub name: String,
    pub nc_type: NcType,
    /// Number of elements in the attribute's value.
    pub nelems: u64,
    /// Display value: UTF-8 for `Char`, decimal list for numeric types.
    pub value: String,
    /// First element decoded to `f64` in native domain (`None` for `Char`
    /// attributes and empty values). Kept alongside the display string so
    /// value decode can read a typed `_FillValue` without a lossy round-trip
    /// through the rendered decimal text.
    pub first_value: Option<f64>,
}

/// A NetCDF variable. Attribute decoding is shared with global attributes.
/// `begin` and `vsize` come straight from the on-disk record; they locate the
/// variable's data for [`decode_variable_values`].
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

impl Variable {
    /// Typed `_FillValue` (first element, widened to `f64`) when the variable
    /// declares one. Value decode reports points equal to this as `None`,
    /// matching how `libnetcdf` masks fills. Returns `None` when no
    /// `_FillValue` attribute is present — the default fill is intentionally
    /// *not* treated as missing, mirroring the oracle's accounting.
    pub fn fill_value(&self) -> Option<f64> {
        self.attributes
            .iter()
            .find(|a| a.name == "_FillValue")
            .and_then(|a| a.first_value)
    }
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

/// Hard cap on a variable's dimensionality. Real NetCDF variables top out
/// at a few dozen dims; anything beyond this is treated as corrupt.
pub const MAX_VAR_DIMS: u64 = 4096;

/// Hard cap on the number of elements `decode_variable_values` will allocate
/// for one variable, guarding against a corrupt header that declares a huge
/// shape. Matches the GRIB2 decode cap (200M points ≈ 1.6 GiB of `f64`).
pub const MAX_VAR_ELEMENTS: usize = 200_000_000;

/// Convert a NON_NEG (u64) read from the wire to usize, surfacing 32-bit
/// truncation as a parse error instead of a silent wrap.
fn nonneg_to_usize(n: u64, what: &str) -> Result<usize, FieldglassError> {
    usize::try_from(n)
        .map_err(|_| FieldglassError::Parse(format!("NetCDF {what} count {n} exceeds usize")))
}

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
// Value decode
// ---------------------------------------------------------------------------

/// Resolve a variable's runtime shape: every fixed dimension contributes its
/// declared length, and the unlimited / record dimension contributes the
/// header's `numrecs` (0 for an empty or streaming file). Returned in declared
/// (C / row-major) order.
pub fn variable_shape(
    header: &ClassicHeader,
    var_index: usize,
) -> Result<Vec<u64>, FieldglassError> {
    let var = header
        .variables
        .get(var_index)
        .ok_or(FieldglassError::OutOfRange)?;
    let numrecs = header.numrecs.unwrap_or(0);
    var.dim_ids
        .iter()
        .map(|&dim_id| {
            let dim = header.dimensions.get(dim_id as usize).ok_or_else(|| {
                FieldglassError::Parse(format!(
                    "variable {:?} references dim id {dim_id} but only {} dimensions exist",
                    var.name,
                    header.dimensions.len()
                ))
            })?;
            Ok(if dim.is_record { numrecs } else { dim.length })
        })
        .collect()
}

/// Decode one classic variable's values into row-major (C / on-disk order)
/// `Vec<Option<f64>>`, mirroring the GRIB `decode_message_values` surface:
/// `Some(v)` for a present point, `None` where the on-disk element equals the
/// variable's `_FillValue`. Numeric `nc_type`s widen to `f64`.
///
/// `data` must be the whole file `header` was parsed from. `char` variables
/// hold text (decoded into the attribute/header path), not numbers, and are
/// rejected here.
pub fn decode_variable_values(
    header: &ClassicHeader,
    data: &[u8],
    var_index: usize,
) -> Result<Vec<Option<f64>>, FieldglassError> {
    let var = header
        .variables
        .get(var_index)
        .ok_or(FieldglassError::OutOfRange)?;

    if matches!(var.nc_type, NcType::Char) {
        return Err(FieldglassError::UnsupportedSection(format!(
            "variable {:?} is char (text); numeric value decode does not apply",
            var.name
        )));
    }

    let elem = var.nc_type.element_size();
    let numrecs = header.numrecs.unwrap_or(0);

    // The unlimited dimension, when present, must be the most significant
    // (first) axis — NetCDF classic stores records by interleaving each record
    // variable's per-record slab, so a record dim anywhere else is malformed.
    let is_record_var = match var.dim_ids.first() {
        Some(&first) => header
            .dimensions
            .get(first as usize)
            .is_some_and(|d| d.is_record),
        None => false,
    };
    for (axis, &dim_id) in var.dim_ids.iter().enumerate() {
        let is_record = header
            .dimensions
            .get(dim_id as usize)
            .is_some_and(|d| d.is_record);
        if is_record && axis != 0 {
            return Err(FieldglassError::Parse(format!(
                "variable {:?} places the unlimited dimension at axis {axis}; \
                 NetCDF classic requires it first",
                var.name
            )));
        }
    }

    let shape = variable_shape(header, var_index)?;
    let total_u64 = shape
        .iter()
        .try_fold(1u64, |acc, &d| acc.checked_mul(d))
        .ok_or_else(|| {
            FieldglassError::Parse(format!(
                "variable {:?} shape {shape:?} overflows the element count",
                var.name
            ))
        })?;
    let total = nonneg_to_usize(total_u64, "variable element count")?;
    if total > MAX_VAR_ELEMENTS {
        return Err(FieldglassError::Parse(format!(
            "variable {:?} has {total} elements, exceeds cap of {MAX_VAR_ELEMENTS}",
            var.name
        )));
    }
    if total == 0 {
        return Ok(Vec::new());
    }

    let fill = var.fill_value();
    let begin = nonneg_to_usize(var.begin, "variable begin")?;
    let mut out: Vec<Option<f64>> = Vec::with_capacity(total);

    if is_record_var {
        // Record variable: `numrecs` records laid `recsize` bytes apart, where
        // `recsize` is the sum of every record variable's per-record `vsize`.
        let numrecs = nonneg_to_usize(numrecs, "numrecs")?;
        let per_record = total / numrecs; // shape[0] == numrecs, so this is exact
        let recsize = record_size(header)?;
        for r in 0..numrecs {
            let rec_offset = r
                .checked_mul(recsize)
                .and_then(|o| begin.checked_add(o))
                .ok_or_else(|| {
                    FieldglassError::Parse(format!(
                        "variable {:?} record {r} offset overflows usize",
                        var.name
                    ))
                })?;
            read_slab(
                data,
                rec_offset,
                per_record,
                elem,
                var.nc_type,
                fill,
                &mut out,
            )?;
        }
    } else {
        // Fixed (non-record) variable: one contiguous slab at `begin`.
        read_slab(data, begin, total, elem, var.nc_type, fill, &mut out)?;
    }

    Ok(out)
}

/// Sum of every record variable's per-record `vsize` — the byte stride from
/// one record to the next. The single-record-variable special case (where the
/// writer drops 4-byte record padding) falls out for free: with one record
/// variable the sum is just that variable's unpadded `vsize`.
fn record_size(header: &ClassicHeader) -> Result<usize, FieldglassError> {
    let mut total = 0usize;
    for v in &header.variables {
        let is_record = v
            .dim_ids
            .first()
            .and_then(|&d| header.dimensions.get(d as usize))
            .is_some_and(|d| d.is_record);
        if is_record {
            let vsize = nonneg_to_usize(v.vsize, "vsize")?;
            total = total.checked_add(vsize).ok_or_else(|| {
                FieldglassError::Parse("record size sum overflows usize".to_string())
            })?;
        }
    }
    Ok(total)
}

/// Read `count` big-endian elements of `nc_type` starting at byte offset
/// `start`, decode each to `f64`, mask fills to `None`, and append to `out`.
fn read_slab(
    data: &[u8],
    start: usize,
    count: usize,
    elem: usize,
    nc_type: NcType,
    fill: Option<f64>,
    out: &mut Vec<Option<f64>>,
) -> Result<(), FieldglassError> {
    let span = count
        .checked_mul(elem)
        .ok_or_else(|| FieldglassError::Parse("variable slab size overflows usize".to_string()))?;
    let end = start
        .checked_add(span)
        .ok_or_else(|| FieldglassError::Parse("variable slab end overflows usize".to_string()))?;
    if end > data.len() {
        return Err(FieldglassError::Parse(format!(
            "variable data region [{start}, {end}) exceeds file size {}",
            data.len()
        )));
    }
    for i in 0..count {
        let off = start + i * elem;
        let v = decode_element_f64(&data[off..off + elem], nc_type);
        // Mask on value equality, matching how `libnetcdf` / numpy compare a
        // masked array against `_FillValue` (so a `NaN` fill, like `NaN == NaN`,
        // masks nothing). The compare is in the `f64` domain, so for `int64` /
        // `uint64` fills two values past 2^53 apart can collide — the same
        // precision limit the rest of this pipeline carries.
        out.push(match fill {
            Some(f) if v == f => None,
            _ => Some(v),
        });
    }
    Ok(())
}

/// Decode a single element of `nc_type` from exactly its `element_size()`
/// big-endian bytes into `f64`. Integer types widen (`i64`/`u64` may lose
/// precision beyond 2^53, as elsewhere in the `f64` value pipeline).
fn decode_element_f64(bytes: &[u8], nc_type: NcType) -> f64 {
    match nc_type {
        NcType::Byte => (bytes[0] as i8) as f64,
        NcType::UByte => bytes[0] as f64,
        // Defensive: value decode rejects `Char` before reaching here.
        NcType::Char => bytes[0] as f64,
        NcType::Short => i16::from_be_bytes([bytes[0], bytes[1]]) as f64,
        NcType::UShort => u16::from_be_bytes([bytes[0], bytes[1]]) as f64,
        NcType::Int => i32::from_be_bytes(bytes.try_into().unwrap()) as f64,
        NcType::UInt => u32::from_be_bytes(bytes.try_into().unwrap()) as f64,
        NcType::Float => f32::from_be_bytes(bytes.try_into().unwrap()) as f64,
        NcType::Double => f64::from_be_bytes(bytes.try_into().unwrap()),
        NcType::Int64 => i64::from_be_bytes(bytes.try_into().unwrap()) as f64,
        NcType::UInt64 => u64::from_be_bytes(bytes.try_into().unwrap()) as f64,
    }
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
        // checked_add: an n near usize::MAX would wrap past the bounds check.
        let end = self.pos.checked_add(n).ok_or_else(|| {
            FieldglassError::Parse(format!(
                "NetCDF read length {n} at offset {} overflows usize",
                self.pos
            ))
        })?;
        if end > self.bytes.len() {
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
        let raw = self.read_bytes_padded(nonneg_to_usize(n, "name length")?)?;
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
        let count = nonneg_to_usize(count, "dim_list")?;
        // No with_capacity — count is wire-derived; let push grow naturally.
        let mut dims = Vec::new();
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
        let count = nonneg_to_usize(count, "att_list")?;
        let mut atts = Vec::new();
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

        let nelems_usize = nonneg_to_usize(nelems, "attribute element count")?;
        let total_bytes = nelems_usize
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

        // Typed first element, used by value decode to recognise `_FillValue`.
        // Skipped for `Char` (text, not a number) and for empty values.
        let first_value = match nc_type {
            NcType::Char => None,
            _ if nelems_usize == 0 => None,
            _ => Some(decode_element_f64(&raw[..nc_type.element_size()], nc_type)),
        };

        Ok(Attribute {
            name,
            nc_type,
            nelems,
            value,
            first_value,
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
        let count = nonneg_to_usize(count, "var_list")?;
        let mut vars = Vec::new();
        for _ in 0..count {
            vars.push(self.read_variable(num_dims)?);
        }
        Ok(vars)
    }

    fn read_variable(&mut self, num_dims: usize) -> Result<Variable, FieldglassError> {
        let name = self.read_name()?;
        let dimensionality = self.read_nonneg()?;
        if dimensionality > MAX_VAR_DIMS {
            return Err(FieldglassError::Parse(format!(
                "variable {name:?} declares {dimensionality} dimensions, exceeds cap of {MAX_VAR_DIMS}"
            )));
        }
        let dimensionality = nonneg_to_usize(dimensionality, "variable dimensionality")?;
        let mut dim_ids = Vec::with_capacity(dimensionality);
        for _ in 0..dimensionality {
            // `dimid` is `NON_NEG`: 4 bytes for CDF-1/2, 8 bytes for CDF-5
            // (matching what `libnetcdf` and PnetCDF write — verified against
            // a CDF-5 file produced by the canonical `netCDF4` Python writer).
            let raw = self.read_nonneg()?;
            if raw >= num_dims as u64 {
                return Err(FieldglassError::Parse(format!(
                    "variable {name:?} references dim id {raw} but only {num_dims} dimensions exist"
                )));
            }
            dim_ids.push(raw as u32);
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
    // 6 significant digits is enough for human-readable attribute display.
    // Precision-critical reads go through `decode_variable_values` (which
    // decodes from the raw bytes), not this display string.
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

    // -- value decode -------------------------------------------------------
    //
    // The bundled fixtures all have `numrecs == 0`, so the record-interleave
    // path is exercised here against hand-built headers + data buffers, with
    // public struct fields standing in for the parser. (The fixtures cover
    // fixed-variable decode against a `netCDF4` oracle.)

    fn var(name: &str, dim_ids: Vec<u32>, nc_type: NcType, vsize: u64, begin: u64) -> Variable {
        Variable {
            name: name.to_string(),
            dim_ids,
            nc_type,
            attributes: Vec::new(),
            vsize,
            begin,
        }
    }

    fn fill_attr(value: f64, nc_type: NcType) -> Attribute {
        Attribute {
            name: "_FillValue".to_string(),
            nc_type,
            nelems: 1,
            value: value.to_string(),
            first_value: Some(value),
        }
    }

    #[test]
    fn fixed_variable_decodes_contiguously() {
        let header = ClassicHeader {
            version: ClassicVersion::Cdf1,
            numrecs: Some(0),
            dimensions: vec![Dimension {
                name: "z".to_string(),
                length: 3,
                is_record: false,
            }],
            global_attributes: Vec::new(),
            variables: vec![var("z", vec![0], NcType::Double, 24, 8)],
        };
        let mut data = vec![0u8; 8];
        for v in [1.5f64, -2.0, 4.25] {
            data.extend_from_slice(&v.to_be_bytes());
        }
        let out = decode_variable_values(&header, &data, 0).unwrap();
        assert_eq!(out, vec![Some(1.5), Some(-2.0), Some(4.25)]);
    }

    #[test]
    fn fixed_variable_masks_fill() {
        let mut v = var("t", vec![0], NcType::Float, 8, 4);
        v.attributes.push(fill_attr(-999.0, NcType::Float));
        let header = ClassicHeader {
            version: ClassicVersion::Cdf1,
            numrecs: Some(0),
            dimensions: vec![Dimension {
                name: "x".to_string(),
                length: 2,
                is_record: false,
            }],
            global_attributes: Vec::new(),
            variables: vec![v],
        };
        let mut data = vec![0u8; 4];
        data.extend_from_slice(&12.5f32.to_be_bytes());
        data.extend_from_slice(&(-999.0f32).to_be_bytes());
        let out = decode_variable_values(&header, &data, 0).unwrap();
        assert_eq!(out, vec![Some(12.5), None]);
    }

    #[test]
    fn record_variables_interleave() {
        // Two record variables share a 3-record unlimited dim. Per-record
        // layout: [a: 1 double = 8B][b: 2 doubles = 16B], recsize = 24.
        let dimensions = vec![
            Dimension {
                name: "time".to_string(),
                length: 0,
                is_record: true,
            },
            Dimension {
                name: "z".to_string(),
                length: 2,
                is_record: false,
            },
        ];
        let begin_a = 100u64;
        let begin_b = 108u64; // begin_a + vsize_a (8)
        let mut b = var("b", vec![0, 1], NcType::Double, 16, begin_b);
        b.attributes.push(fill_attr(-1.0, NcType::Double));
        let header = ClassicHeader {
            version: ClassicVersion::Cdf1,
            numrecs: Some(3),
            dimensions,
            global_attributes: Vec::new(),
            variables: vec![var("a", vec![0], NcType::Double, 8, begin_a), b],
        };

        let recsize = 24usize;
        let mut data = vec![0u8; 100 + 3 * recsize];
        for r in 0..3usize {
            let base = 100 + r * recsize;
            data[base..base + 8].copy_from_slice(&((r as f64) * 10.0).to_be_bytes());
            let b0 = (r as f64) * 10.0 + 1.0;
            // Mask the very first b element to exercise fill handling.
            let b0 = if r == 0 { -1.0 } else { b0 };
            data[base + 8..base + 16].copy_from_slice(&b0.to_be_bytes());
            data[base + 16..base + 24].copy_from_slice(&((r as f64) * 10.0 + 2.0).to_be_bytes());
        }

        let a = decode_variable_values(&header, &data, 0).unwrap();
        assert_eq!(a, vec![Some(0.0), Some(10.0), Some(20.0)]);
        assert_eq!(variable_shape(&header, 0).unwrap(), vec![3]);

        let b = decode_variable_values(&header, &data, 1).unwrap();
        assert_eq!(
            b,
            vec![
                None,       // record 0, z=0 — masked fill
                Some(2.0),  // record 0, z=1
                Some(11.0), // record 1, z=0
                Some(12.0),
                Some(21.0),
                Some(22.0),
            ]
        );
        assert_eq!(variable_shape(&header, 1).unwrap(), vec![3, 2]);
    }

    #[test]
    fn single_record_variable_has_unpadded_stride() {
        // One record variable of an odd byte width: `vsize` is the unpadded
        // 2-byte slab, so recsize == 2 and records pack with no padding.
        let header = ClassicHeader {
            version: ClassicVersion::Cdf1,
            numrecs: Some(3),
            dimensions: vec![Dimension {
                name: "time".to_string(),
                length: 0,
                is_record: true,
            }],
            global_attributes: Vec::new(),
            variables: vec![var("s", vec![0], NcType::Short, 2, 0)],
        };
        let mut data = Vec::new();
        for s in [7i16, -8, 9] {
            data.extend_from_slice(&s.to_be_bytes());
        }
        let out = decode_variable_values(&header, &data, 0).unwrap();
        assert_eq!(out, vec![Some(7.0), Some(-8.0), Some(9.0)]);
    }

    #[test]
    fn empty_record_variable_decodes_to_nothing() {
        let header = ClassicHeader {
            version: ClassicVersion::Cdf1,
            numrecs: Some(0),
            dimensions: vec![Dimension {
                name: "time".to_string(),
                length: 0,
                is_record: true,
            }],
            global_attributes: Vec::new(),
            variables: vec![var("a", vec![0], NcType::Double, 8, 8)],
        };
        let out = decode_variable_values(&header, &[0u8; 8], 0).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn char_variable_is_rejected() {
        let header = ClassicHeader {
            version: ClassicVersion::Cdf1,
            numrecs: Some(0),
            dimensions: vec![Dimension {
                name: "n".to_string(),
                length: 4,
                is_record: false,
            }],
            global_attributes: Vec::new(),
            variables: vec![var("label", vec![0], NcType::Char, 4, 8)],
        };
        let err = decode_variable_values(&header, &[0u8; 12], 0).unwrap_err();
        assert!(matches!(err, FieldglassError::UnsupportedSection(_)));
    }

    #[test]
    fn out_of_range_index_errors() {
        let header = ClassicHeader {
            version: ClassicVersion::Cdf1,
            numrecs: Some(0),
            dimensions: Vec::new(),
            global_attributes: Vec::new(),
            variables: Vec::new(),
        };
        assert!(matches!(
            decode_variable_values(&header, &[], 0).unwrap_err(),
            FieldglassError::OutOfRange
        ));
    }

    #[test]
    fn data_region_past_eof_errors() {
        let header = ClassicHeader {
            version: ClassicVersion::Cdf1,
            numrecs: Some(0),
            dimensions: vec![Dimension {
                name: "z".to_string(),
                length: 4,
                is_record: false,
            }],
            global_attributes: Vec::new(),
            variables: vec![var("z", vec![0], NcType::Double, 32, 8)],
        };
        // Declares 4 doubles at offset 8 but the buffer is far too short.
        let err = decode_variable_values(&header, &[0u8; 16], 0).unwrap_err();
        assert!(matches!(err, FieldglassError::Parse(_)));
    }
}
