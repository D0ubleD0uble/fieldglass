//! The numcodecs `fixedscaleoffset` filter (#1823).
//!
//! The definition is numcodecs' own, `numcodecs/fixedscaleoffset.py` (read at
//! 0.16.5). Encode is `round((x - offset) * scale)`, rounded half to even by
//! `np.around` and stored as `astype`. Decode is
//!
//! ```python
//! dec = (enc / self.scale) + self.offset
//! dec = dec.astype(self.dtype, copy=False)
//! ```
//!
//! with `enc` the stored codes viewed as `astype`. Decoding rounds nothing,
//! so what this has to match is numpy's arithmetic on that line:
//!
//! - **The type it divides in.** `scale` and `offset` are Python numbers,
//!   which numpy does not let widen an array (NEP 50, and the value-based
//!   casting before it). float32 codes are divided and offset in float32,
//!   `scale` and `offset` first rounded to float32. Any other code, integer
//!   or float64, is divided in float64: numpy's true division of integers
//!   gives float64.
//! - **The cast to `dtype`.** A float `dtype` takes the result rounded to
//!   nearest. An integer `dtype` takes it truncated toward zero, which is C's
//!   cast and numpy's. A value the integer type cannot hold, NaN included,
//!   is undefined behavior in C and junk in numpy, so it is refused here.
//!
//! The filter decodes inside the codec chain, before any CF `scale_factor` or
//! `add_offset` is applied. It divides by its scale where CF multiplies.

use crate::dtype::{DType, Endian, ScalarKind};
use fieldglass_core::FieldglassError;
use serde_json::{Number, Value};

/// One `fixedscaleoffset` filter, read from its v2 configuration.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FixedScaleOffset {
    /// The element type it decodes to.
    pub(crate) dtype: DType,
    /// The element type of the codes it decodes from.
    pub(crate) astype: DType,
    arithmetic: Arithmetic,
}

/// The type numpy divides and offsets in, with the filter's two numbers in
/// that type.
#[derive(Debug, Clone, Copy)]
enum Arithmetic {
    /// float32 codes.
    Single { scale: f32, offset: f32 },
    /// Every other code.
    Double { scale: f64, offset: f64 },
}

/// Bit for bit: the numbers are checked finite, so this is an equivalence.
impl PartialEq for FixedScaleOffset {
    fn eq(&self, other: &Self) -> bool {
        self.dtype == other.dtype
            && self.astype == other.astype
            && match (self.arithmetic, other.arithmetic) {
                (
                    Arithmetic::Single { scale, offset },
                    Arithmetic::Single {
                        scale: s,
                        offset: o,
                    },
                ) => scale.to_bits() == s.to_bits() && offset.to_bits() == o.to_bits(),
                (
                    Arithmetic::Double { scale, offset },
                    Arithmetic::Double {
                        scale: s,
                        offset: o,
                    },
                ) => scale.to_bits() == s.to_bits() && offset.to_bits() == o.to_bits(),
                _ => false,
            }
    }
}

impl Eq for FixedScaleOffset {}

impl FixedScaleOffset {
    /// Read `{"id": "fixedscaleoffset", "scale", "offset", "dtype", "astype"}`.
    ///
    /// `astype` absent or `null` is `dtype`, as numcodecs' constructor reads
    /// it. A scale that is zero, or a scale or offset that is not a finite
    /// number in the type the decode divides in, is refused: every value
    /// would decode to NaN or an infinity.
    pub(crate) fn from_v2(config: &Value) -> Result<Self, FieldglassError> {
        let dtype = match config.get("dtype") {
            Some(Value::String(spec)) => numeric(spec, "dtype")?,
            _ => {
                return Err(FieldglassError::Parse(
                    "a Zarr v2 `fixedscaleoffset` filter states no `dtype` string".to_string(),
                ));
            }
        };
        let astype = match config.get("astype") {
            None | Some(Value::Null) => dtype,
            Some(Value::String(spec)) => numeric(spec, "astype")?,
            Some(other) => {
                return Err(FieldglassError::Parse(format!(
                    "a Zarr v2 `fixedscaleoffset` filter states `astype` {other}, which is not \
                     a dtype string"
                )));
            }
        };
        let scale = number(config, "scale")?;
        let offset = number(config, "offset")?;
        let arithmetic = if astype.kind == ScalarKind::Float && astype.size == 4 {
            let (scale, offset) = (single(scale), single(offset));
            check(f64::from(scale), f64::from(offset), "float32")?;
            Arithmetic::Single { scale, offset }
        } else {
            let (scale, offset) = (double(scale), double(offset));
            check(scale, offset, "float64")?;
            Arithmetic::Double { scale, offset }
        };
        Ok(Self {
            dtype,
            astype,
            arithmetic,
        })
    }

    /// Decode `codes`, a whole number of `astype` elements, to as many
    /// `dtype` elements, in `dtype`'s byte order.
    pub(crate) fn decode(&self, codes: &[u8]) -> Result<Vec<u8>, FieldglassError> {
        let width = self.astype.size;
        if !codes.len().is_multiple_of(width) {
            return Err(FieldglassError::Parse(format!(
                "{} bytes of `fixedscaleoffset` codes are not a whole number of {width}-byte \
                 elements",
                codes.len()
            )));
        }
        let len = (codes.len() / width)
            .checked_mul(self.dtype.size)
            .ok_or_else(|| {
                FieldglassError::Parse(
                    "a chunk of `fixedscaleoffset` codes decodes past an address".to_string(),
                )
            })?;
        let mut out = Vec::with_capacity(len);
        for raw in codes.chunks_exact(width) {
            let decoded = match self.arithmetic {
                Arithmetic::Single { scale, offset } => {
                    Decoded::Single(read_f32(raw, self.astype.endian) / scale + offset)
                }
                Arithmetic::Double { scale, offset } => {
                    Decoded::Double(read_f64(raw, self.astype) / scale + offset)
                }
            };
            self.write(decoded, &mut out)?;
        }
        Ok(out)
    }

    /// One decoded value cast to `dtype`, appended in its byte order.
    fn write(&self, decoded: Decoded, out: &mut Vec<u8>) -> Result<(), FieldglassError> {
        let dtype = self.dtype;
        let mut buf = [0u8; 8];
        match (dtype.kind, dtype.size) {
            (ScalarKind::Float, 4) => {
                let value = match decoded {
                    Decoded::Single(v) => v,
                    // Rounded to nearest, ties to even, as C and numpy cast.
                    Decoded::Double(v) => v as f32,
                };
                buf[..4].copy_from_slice(&value.to_le_bytes());
            }
            (ScalarKind::Float, _) => {
                let value = match decoded {
                    Decoded::Single(v) => f64::from(v),
                    Decoded::Double(v) => v,
                };
                buf.copy_from_slice(&value.to_le_bytes());
            }
            (ScalarKind::Int | ScalarKind::Uint, size) => {
                let value = match decoded {
                    Decoded::Single(v) => f64::from(v),
                    Decoded::Double(v) => v,
                };
                let truncated = value.trunc();
                // Powers of two, so exact in an f64 at every width.
                let bits = (size * 8) as i32;
                let (low, high) = if dtype.kind == ScalarKind::Int {
                    (-(2f64.powi(bits - 1)), 2f64.powi(bits - 1))
                } else {
                    (0.0, 2f64.powi(bits))
                };
                // NaN fails both comparisons.
                if !(truncated >= low && truncated < high) {
                    return Err(FieldglassError::Parse(format!(
                        "a `fixedscaleoffset` code decodes to {value}, which a {bits}-bit {} \
                         cannot hold",
                        if dtype.kind == ScalarKind::Int {
                            "signed integer"
                        } else {
                            "unsigned integer"
                        }
                    )));
                }
                // In range, so the cast is exact; the low bytes of the
                // two's-complement value are the narrower type's.
                let bytes = if dtype.kind == ScalarKind::Int {
                    (truncated as i64).to_le_bytes()
                } else {
                    (truncated as u64).to_le_bytes()
                };
                buf[..size].copy_from_slice(&bytes[..size]);
            }
            // Refused when the filter is read.
            (ScalarKind::Bool, _) => {
                return Err(FieldglassError::UnsupportedSection(
                    "a `fixedscaleoffset` filter over booleans is not decoded".to_string(),
                ));
            }
        }
        let element = &mut buf[..dtype.size];
        if dtype.endian == Some(Endian::Big) {
            element.reverse();
        }
        out.extend_from_slice(element);
        Ok(())
    }
}

/// A decoded value in the type it was computed in.
#[derive(Clone, Copy)]
enum Decoded {
    Single(f32),
    Double(f64),
}

/// Parse a dtype the filter states, which must be a number this crate reads.
fn numeric(spec: &str, key: &str) -> Result<DType, FieldglassError> {
    let dtype = DType::parse_v2(spec)?;
    if dtype.kind == ScalarKind::Bool {
        return Err(FieldglassError::UnsupportedSection(format!(
            "a Zarr v2 `fixedscaleoffset` filter states `{key}` {spec:?}, a boolean, which it \
             does not scale"
        )));
    }
    Ok(dtype)
}

fn number<'a>(config: &'a Value, key: &str) -> Result<&'a Number, FieldglassError> {
    match config.get(key) {
        Some(Value::Number(n)) => Ok(n),
        other => Err(FieldglassError::Parse(format!(
            "a Zarr v2 `fixedscaleoffset` filter's `{key}` is {}, not a number",
            other.map_or_else(|| "missing".to_string(), Value::to_string)
        ))),
    }
}

/// A JSON number as numpy takes a Python one into float32: an integer
/// straight to the nearest float32, not through a float64 first.
fn single(n: &Number) -> f32 {
    if let Some(i) = n.as_i64() {
        i as f32
    } else if let Some(u) = n.as_u64() {
        u as f32
    } else {
        n.as_f64().map_or(f32::NAN, |v| v as f32)
    }
}

/// A JSON number as a float64.
fn double(n: &Number) -> f64 {
    if let Some(i) = n.as_i64() {
        i as f64
    } else if let Some(u) = n.as_u64() {
        u as f64
    } else {
        n.as_f64().unwrap_or(f64::NAN)
    }
}

/// Refuse a scale or offset no decode can use, given in the type the decode
/// divides in.
fn check(scale: f64, offset: f64, arithmetic: &str) -> Result<(), FieldglassError> {
    if scale == 0.0 || !scale.is_finite() || !offset.is_finite() {
        return Err(FieldglassError::Parse(format!(
            "a Zarr v2 `fixedscaleoffset` filter's scale {scale} and offset {offset}, as the \
             {arithmetic} it decodes in, divide by zero or are not finite"
        )));
    }
    Ok(())
}

/// One element, in host order.
fn host_order<const N: usize>(raw: &[u8], endian: Option<Endian>) -> [u8; N] {
    let mut buf = [0u8; N];
    buf.copy_from_slice(&raw[..N]);
    if endian == Some(Endian::Big) {
        buf.reverse();
    }
    buf
}

fn read_f32(raw: &[u8], endian: Option<Endian>) -> f32 {
    f32::from_le_bytes(host_order::<4>(raw, endian))
}

/// One code of any type but float32, as the float64 numpy divides it in.
fn read_f64(raw: &[u8], astype: DType) -> f64 {
    let endian = astype.endian;
    match (astype.kind, astype.size) {
        (ScalarKind::Float, 4) => f64::from(read_f32(raw, endian)),
        (ScalarKind::Float, _) => f64::from_le_bytes(host_order::<8>(raw, endian)),
        (ScalarKind::Int, 1) => f64::from(raw[0] as i8),
        (ScalarKind::Int, 2) => f64::from(i16::from_le_bytes(host_order::<2>(raw, endian))),
        (ScalarKind::Int, 4) => f64::from(i32::from_le_bytes(host_order::<4>(raw, endian))),
        (ScalarKind::Int, _) => i64::from_le_bytes(host_order::<8>(raw, endian)) as f64,
        (ScalarKind::Uint | ScalarKind::Bool, 1) => f64::from(raw[0]),
        (ScalarKind::Uint | ScalarKind::Bool, 2) => {
            f64::from(u16::from_le_bytes(host_order::<2>(raw, endian)))
        }
        (ScalarKind::Uint | ScalarKind::Bool, 4) => {
            f64::from(u32::from_le_bytes(host_order::<4>(raw, endian)))
        }
        (ScalarKind::Uint | ScalarKind::Bool, _) => {
            u64::from_le_bytes(host_order::<8>(raw, endian)) as f64
        }
    }
}
