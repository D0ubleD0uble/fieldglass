//! The element type of a Zarr array, in both editions' spellings.
//!
//! Zarr v2 states it as a NumPy dtype string — `"<f4"`, `">i2"`, `"|u1"` —
//! whose first character is the byte order and whose remainder is a kind letter
//! and a width in bytes. Zarr v3 states it as a name — `"float32"`,
//! `"int16"`, `"uint8"` — and carries the byte order separately, in the `bytes`
//! codec's `endian` configuration.
//!
//! The two spellings meet here because everything downstream — the width a
//! shuffle filter transposes by, the number of elements a chunk holds, the
//! conversion to `f64` — is the same question in either edition.
//!
//! **Only the fixed-width numeric types are decoded.** Zarr's dtype vocabulary
//! also covers structured records, datetimes, fixed-length strings and complex
//! numbers; each is a different question about what a *value* even is, and a
//! reader that guessed would hand a host numbers that are not the array's. They
//! are named in the error rather than silently mis-decoded.

use fieldglass_core::FieldglassError;

/// The byte order of a multi-byte element as it is stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Endian {
    /// Least-significant byte first — `<` in a NumPy dtype string, `"little"`
    /// in a v3 `bytes` codec.
    Little,
    /// Most-significant byte first — `>` in a NumPy dtype string, `"big"` in a
    /// v3 `bytes` codec.
    Big,
}

impl Endian {
    /// The order this build's `f64::from_le_bytes` family reads natively.
    ///
    /// Not a constant: the decode has to byte-swap when the stored order and
    /// the host's disagree, and the workspace cross-compiles to targets of both
    /// orders.
    #[must_use]
    pub const fn native() -> Self {
        if cfg!(target_endian = "little") {
            Self::Little
        } else {
            Self::Big
        }
    }
}

/// What kind of number an element is, once its bytes are in host order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScalarKind {
    /// A one-byte boolean: 0 is false, anything else true (NumPy `b1`).
    Bool,
    /// Two's-complement signed integer.
    Int,
    /// Unsigned integer.
    Uint,
    /// IEEE 754 binary floating point.
    Float,
}

/// One array's element type: a kind, a width, and (for multi-byte widths) an
/// order.
///
/// A single-byte type has no order to state, so [`endian`](Self::endian) is
/// `None` there rather than an arbitrary choice — which is also what a v2
/// dtype string spells with `|`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DType {
    /// Whether the element is a boolean, an integer, or a float.
    pub kind: ScalarKind,
    /// The element's width in bytes: 1, 2, 4 or 8.
    pub size: usize,
    /// The stored byte order, or `None` for a one-byte element.
    pub endian: Option<Endian>,
}

impl DType {
    /// Parse a Zarr v2 (NumPy) dtype string: an order character, a kind letter,
    /// and a width.
    ///
    /// The order character is required — NumPy writes one for every dtype,
    /// using `|` for the widths where it has no meaning — so a string missing
    /// it is a malformed `.zarray` rather than a defaulting opportunity.
    pub fn parse_v2(spec: &str) -> Result<Self, FieldglassError> {
        let mut chars = spec.chars();
        let order = chars
            .next()
            .ok_or_else(|| FieldglassError::Parse("empty Zarr v2 dtype string".to_string()))?;
        let kind_char = chars.next().ok_or_else(|| {
            FieldglassError::Parse(format!("Zarr v2 dtype {spec:?} states no kind"))
        })?;
        let width: usize = chars.as_str().parse().map_err(|_| {
            FieldglassError::Parse(format!("Zarr v2 dtype {spec:?} states no element width"))
        })?;

        let kind = match kind_char {
            'b' => ScalarKind::Bool,
            'i' => ScalarKind::Int,
            'u' => ScalarKind::Uint,
            'f' => ScalarKind::Float,
            other => {
                return Err(FieldglassError::UnsupportedSection(format!(
                    "Zarr dtype kind {other:?} (in {spec:?}) is not a fixed-width number \
                     (only b, i, u and f are decoded)"
                )));
            }
        };
        let endian = match order {
            '<' => Some(Endian::Little),
            '>' => Some(Endian::Big),
            // NumPy writes `|` for a type with no byte order, and `=` for
            // "whatever the writer's host was". `=` in a stored file is
            // ambiguous by construction, so it is refused rather than guessed.
            '|' => None,
            other => {
                return Err(FieldglassError::UnsupportedSection(format!(
                    "Zarr v2 dtype {spec:?} states byte order {other:?}, which names no \
                     definite order"
                )));
            }
        };
        Self::assemble(kind, width, endian, spec)
    }

    /// Parse a Zarr v3 `data_type` name. The order is not part of it; it comes
    /// from the `bytes` codec and is applied with [`with_endian`](Self::with_endian).
    pub fn parse_v3(name: &str) -> Result<Self, FieldglassError> {
        let (kind, width) = match name {
            "bool" => (ScalarKind::Bool, 1),
            "int8" => (ScalarKind::Int, 1),
            "int16" => (ScalarKind::Int, 2),
            "int32" => (ScalarKind::Int, 4),
            "int64" => (ScalarKind::Int, 8),
            "uint8" => (ScalarKind::Uint, 1),
            "uint16" => (ScalarKind::Uint, 2),
            "uint32" => (ScalarKind::Uint, 4),
            "uint64" => (ScalarKind::Uint, 8),
            "float32" => (ScalarKind::Float, 4),
            "float64" => (ScalarKind::Float, 8),
            other => {
                return Err(FieldglassError::UnsupportedSection(format!(
                    "Zarr v3 data_type {other:?} is not a fixed-width number (float16, \
                     complex, string, and the extension types are not decoded)"
                )));
            }
        };
        // A v3 array states its order in the `bytes` codec; until that is read
        // a multi-byte type has none. `little` is the near-universal case and
        // the codec chain overwrites this, so the placeholder never survives a
        // real array — `decode` refuses a multi-byte v3 array with no `bytes`
        // codec rather than falling back to it.
        Self::assemble(kind, width, (width > 1).then_some(Endian::Little), name)
    }

    /// The same type with its byte order replaced, which is how a v3 `bytes`
    /// codec's `endian` reaches the element type.
    #[must_use]
    pub fn with_endian(mut self, endian: Endian) -> Self {
        if self.size > 1 {
            self.endian = Some(endian);
        }
        self
    }

    fn assemble(
        kind: ScalarKind,
        width: usize,
        endian: Option<Endian>,
        spelling: &str,
    ) -> Result<Self, FieldglassError> {
        if !matches!(width, 1 | 2 | 4 | 8) {
            return Err(FieldglassError::UnsupportedSection(format!(
                "Zarr dtype {spelling:?} is {width} bytes wide; only 1, 2, 4 and 8 are decoded"
            )));
        }
        if kind == ScalarKind::Float && width < 4 {
            return Err(FieldglassError::UnsupportedSection(format!(
                "Zarr dtype {spelling:?} is a {}-byte float; float16 is not decoded",
                width
            )));
        }
        if kind == ScalarKind::Bool && width != 1 {
            return Err(FieldglassError::Parse(format!(
                "Zarr dtype {spelling:?} is a {width}-byte boolean, which NumPy does not define"
            )));
        }
        Ok(Self {
            kind,
            size: width,
            // A one-byte element has no order whatever the string said.
            endian: if width > 1 { endian } else { None },
        })
    }

    /// How many whole elements a buffer of `len` bytes holds.
    #[must_use]
    pub const fn elements_in(&self, len: usize) -> usize {
        len / self.size
    }

    /// Read a chunk's raw bytes as numbers.
    ///
    /// The buffer must be a whole number of elements: a partial trailing
    /// element means the codec chain produced the wrong length, which is a
    /// decode failure rather than something to truncate silently.
    ///
    /// Integers wider than 53 bits do not all survive the conversion to `f64`,
    /// which is what the whole render and analysis surface takes. That is a
    /// real, documented loss rather than a hidden one: an `int64` count past
    /// 2^53 rounds. Nothing in this crate is the right place to fix it — the
    /// host's value type is `f64` — so it is stated here and nowhere else.
    pub fn read_values(&self, bytes: &[u8]) -> Result<Vec<f64>, FieldglassError> {
        if !bytes.len().is_multiple_of(self.size) {
            return Err(FieldglassError::Parse(format!(
                "decoded chunk is {} bytes, not a whole number of {}-byte elements",
                bytes.len(),
                self.size
            )));
        }
        let big = self.endian == Some(Endian::Big);
        let mut out = Vec::with_capacity(self.elements_in(bytes.len()));
        for raw in bytes.chunks_exact(self.size) {
            // One owned array per element, ordered to the host, so the
            // `from_*_bytes` calls below need no per-order duplication.
            let mut buf = [0u8; 8];
            buf[..self.size].copy_from_slice(raw);
            if big {
                buf[..self.size].reverse();
            }
            out.push(match (self.kind, self.size) {
                (ScalarKind::Bool, _) => f64::from(u8::from(buf[0] != 0)),
                (ScalarKind::Uint, 1) => f64::from(buf[0]),
                (ScalarKind::Uint, 2) => f64::from(u16::from_le_bytes([buf[0], buf[1]])),
                (ScalarKind::Uint, 4) => {
                    f64::from(u32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]))
                }
                (ScalarKind::Uint, _) => u64::from_le_bytes(buf) as f64,
                (ScalarKind::Int, 1) => f64::from(buf[0] as i8),
                (ScalarKind::Int, 2) => f64::from(i16::from_le_bytes([buf[0], buf[1]])),
                (ScalarKind::Int, 4) => {
                    f64::from(i32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]))
                }
                (ScalarKind::Int, _) => i64::from_le_bytes(buf) as f64,
                (ScalarKind::Float, 4) => {
                    f64::from(f32::from_le_bytes([buf[0], buf[1], buf[2], buf[3]]))
                }
                (ScalarKind::Float, _) => f64::from_le_bytes(buf),
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_numpy_spellings_zarr_python_writes() {
        assert_eq!(
            DType::parse_v2("<f4").unwrap(),
            DType {
                kind: ScalarKind::Float,
                size: 4,
                endian: Some(Endian::Little)
            }
        );
        assert_eq!(
            DType::parse_v2(">i2").unwrap(),
            DType {
                kind: ScalarKind::Int,
                size: 2,
                endian: Some(Endian::Big)
            }
        );
        // A one-byte element has no order, and `|` is how NumPy spells that.
        assert_eq!(
            DType::parse_v2("|u1").unwrap(),
            DType {
                kind: ScalarKind::Uint,
                size: 1,
                endian: None
            }
        );
        assert_eq!(DType::parse_v2("|b1").unwrap().kind, ScalarKind::Bool);
    }

    /// The two editions' spellings have to land on the same type, or a v3 store
    /// and a v2 store of the same array would decode differently.
    #[test]
    fn the_two_editions_agree_on_the_same_element_type() {
        for (v2, v3) in [
            ("<f4", "float32"),
            ("<f8", "float64"),
            ("<i2", "int16"),
            ("<u4", "uint32"),
            ("|u1", "uint8"),
            ("|b1", "bool"),
        ] {
            assert_eq!(
                DType::parse_v2(v2).unwrap(),
                DType::parse_v3(v3).unwrap(),
                "{v2} and {v3} name the same element type"
            );
        }
    }

    /// `=` means "the writer's native order", which a stored file cannot
    /// answer. Guessing little-endian would be right nearly always and silently
    /// wrong on the file that mattered.
    #[test]
    fn refuses_a_dtype_whose_order_is_the_writers_own() {
        let err = DType::parse_v2("=f4").unwrap_err();
        assert!(
            matches!(&err, FieldglassError::UnsupportedSection(m) if m.contains("no definite order")),
            "got {err:?}"
        );
    }

    #[test]
    fn refuses_the_types_that_are_not_fixed_width_numbers() {
        for spec in ["<c8", "<M8", "|S8", "<f2"] {
            assert!(
                DType::parse_v2(spec).is_err(),
                "{spec} must not be decoded as a number"
            );
        }
        assert!(DType::parse_v3("float16").is_err());
        assert!(DType::parse_v3("complex64").is_err());
        assert!(DType::parse_v3("string").is_err());
    }

    /// Byte order is applied on read, not assumed. The same four bytes are two
    /// different numbers under the two orders.
    #[test]
    fn byte_order_is_honoured_on_read() {
        let bytes = 1.0f32.to_le_bytes();
        let le = DType::parse_v2("<f4").unwrap().read_values(&bytes).unwrap();
        let be = DType::parse_v2(">f4").unwrap().read_values(&bytes).unwrap();
        assert_eq!(le, vec![1.0]);
        assert_ne!(be, vec![1.0]);
        assert_eq!(
            DType::parse_v2(">f4")
                .unwrap()
                .read_values(&1.0f32.to_be_bytes())
                .unwrap(),
            vec![1.0]
        );
    }

    /// Signedness is not cosmetic: the same byte is 255 or −1.
    #[test]
    fn signedness_changes_the_value() {
        assert_eq!(
            DType::parse_v2("|u1")
                .unwrap()
                .read_values(&[0xFF])
                .unwrap(),
            vec![255.0]
        );
        assert_eq!(
            DType::parse_v2("|i1")
                .unwrap()
                .read_values(&[0xFF])
                .unwrap(),
            vec![-1.0]
        );
    }

    /// A trailing partial element means the codec chain produced the wrong
    /// length. Truncating would hand the host a chunk one element short and say
    /// nothing.
    #[test]
    fn refuses_a_buffer_that_is_not_whole_elements() {
        let err = DType::parse_v2("<f4")
            .unwrap()
            .read_values(&[0, 1, 2])
            .unwrap_err();
        assert!(matches!(err, FieldglassError::Parse(_)), "got {err:?}");
    }
}
