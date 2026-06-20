//! HDF5 Filter Pipeline message (`0x000B`) decoder and the read-side filters
//! (issue #121, under #33). A chunked dataset may pass each chunk through a
//! pipeline of filters on write; reading reverses them, in the opposite order.
//!
//! Two filters cover the overwhelming majority of NetCDF-4 climate data, and
//! both are decoded here in pure Rust:
//!
//! * **deflate** (filter id 1) — a zlib stream, undone with `miniz_oxide`.
//! * **shuffle** (filter id 2) — reorders an element's bytes so like-significance
//!   bytes sit together (improving deflate); undone by transposing back.
//!
//! Any other filter (szip, fletcher32, nbit, scale-offset, …) is recognised by
//! id and rejected with a clear error rather than silently mis-decoded.
//!
//! Reference: HDF5 file format specification version 3, "Data Storage - Filter
//! Pipeline Message".

use super::object_header::read_uint_le;
use fieldglass_core::FieldglassError;

/// HDF5 reserved filter identifiers we know how to reverse.
const FILTER_DEFLATE: u16 = 1;
const FILTER_SHUFFLE: u16 = 2;

/// Upper bound on filters in one pipeline — guards a corrupt count.
const MAX_FILTERS: usize = 32;

/// One stage of a filter pipeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filter {
    pub id: u16,
    /// Client data values (filter parameters); shuffle stores the element size
    /// here, deflate the compression level.
    pub client_data: Vec<u32>,
}

/// A dataset's decoded filter pipeline, in write (application) order.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FilterPipeline {
    pub filters: Vec<Filter>,
}

impl FilterPipeline {
    /// Decode a Filter Pipeline message body (versions 1 and 2).
    pub fn decode(body: &[u8]) -> Result<Self, FieldglassError> {
        let version = *body
            .first()
            .ok_or_else(|| FieldglassError::Parse("empty filter pipeline message".into()))?;
        if version != 1 && version != 2 {
            return Err(FieldglassError::Parse(format!(
                "unsupported filter pipeline message version {version}"
            )));
        }
        let count = *body
            .get(1)
            .ok_or_else(|| FieldglassError::Parse("truncated filter pipeline message".into()))?
            as usize;
        if count > MAX_FILTERS {
            return Err(FieldglassError::Parse(format!(
                "filter pipeline declares {count} filters, exceeds cap of {MAX_FILTERS}"
            )));
        }
        // Version 1 has a 6-byte reserved field after the count; version 2 has none.
        let mut pos = if version == 1 { 8 } else { 2 };

        let mut filters = Vec::with_capacity(count);
        for _ in 0..count {
            let id = read_uint_le(body, pos, 2)? as u16;
            pos += 2;
            // Name length: always present in version 1; in version 2 only when
            // the filter id is >= 256 (the optional-name range).
            let name_len = if version == 1 || id >= 256 {
                let n = read_uint_le(body, pos, 2)? as usize;
                pos += 2;
                n
            } else {
                0
            };
            // flags (2) + number of client-data values (2).
            let _flags = read_uint_le(body, pos, 2)?;
            let nvalues = read_uint_le(body, pos + 2, 2)? as usize;
            pos += 4;
            // Name, padded to an 8-byte multiple in version 1 only.
            let name_padded = if version == 1 {
                name_len.div_ceil(8) * 8
            } else {
                name_len
            };
            pos = pos
                .checked_add(name_padded)
                .filter(|&p| p <= body.len())
                .ok_or_else(|| FieldglassError::Parse("filter name overruns message".into()))?;
            // Client data values: `nvalues` 4-byte integers, padded with one more
            // in version 1 when the count is odd (to an 8-byte boundary).
            let mut client_data = Vec::with_capacity(nvalues);
            for _ in 0..nvalues {
                client_data.push(read_uint_le(body, pos, 4)? as u32);
                pos += 4;
            }
            if version == 1 && nvalues % 2 == 1 {
                pos += 4; // padding value
            }
            filters.push(Filter { id, client_data });
        }
        Ok(Self { filters })
    }

    /// Reverse the pipeline over one chunk's raw bytes. Filters run in the
    /// opposite of write order; a filter whose bit is set in `filter_mask` was
    /// skipped on write for this chunk, so it is skipped on read too.
    /// `element_size` is the dataset's element width, used by shuffle when the
    /// filter itself doesn't carry it.
    pub fn reverse(
        &self,
        mut data: Vec<u8>,
        filter_mask: u32,
        element_size: usize,
    ) -> Result<Vec<u8>, FieldglassError> {
        for (index, filter) in self.filters.iter().enumerate().rev() {
            if filter_mask & (1u32 << index) != 0 {
                continue; // filter was not applied to this chunk
            }
            data = match filter.id {
                FILTER_DEFLATE => inflate(&data)?,
                FILTER_SHUFFLE => {
                    let elem = filter
                        .client_data
                        .first()
                        .map(|&v| v as usize)
                        .filter(|&v| v > 0)
                        .unwrap_or(element_size);
                    unshuffle(&data, elem)
                }
                other => {
                    return Err(FieldglassError::UnsupportedSection(format!(
                        "HDF5 filter id {other} is not supported (only deflate and \
                         shuffle are decoded)"
                    )));
                }
            };
        }
        Ok(data)
    }
}

/// Inflate a zlib stream (the HDF5 deflate filter's on-disk form).
fn inflate(data: &[u8]) -> Result<Vec<u8>, FieldglassError> {
    miniz_oxide::inflate::decompress_to_vec_zlib(data)
        .map_err(|e| FieldglassError::Parse(format!("deflate (zlib) inflate failed: {e:?}")))
}

/// Undo the shuffle filter: bytes were grouped by position-within-element (all
/// byte 0s, then all byte 1s, …); regroup them back into consecutive elements.
fn unshuffle(data: &[u8], element_size: usize) -> Vec<u8> {
    // A trailing partial element (or element_size <= 1) means nothing to undo.
    if element_size <= 1 || data.len() % element_size != 0 {
        return data.to_vec();
    }
    let count = data.len() / element_size;
    let mut out = vec![0u8; data.len()];
    for byte_pos in 0..element_size {
        let base = byte_pos * count;
        for elem in 0..count {
            out[elem * element_size + byte_pos] = data[base + elem];
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a version-2 filter pipeline body for the given filters.
    fn pipeline_v2(filters: &[(u16, &[u32])]) -> Vec<u8> {
        let mut body = vec![2u8, filters.len() as u8];
        for &(id, cdata) in filters {
            body.extend_from_slice(&id.to_le_bytes());
            // version-2 names omitted for id < 256
            body.extend_from_slice(&0u16.to_le_bytes()); // flags
            body.extend_from_slice(&(cdata.len() as u16).to_le_bytes());
            for &v in cdata {
                body.extend_from_slice(&v.to_le_bytes());
            }
        }
        body
    }

    #[test]
    fn decodes_shuffle_then_deflate() {
        let body = pipeline_v2(&[(FILTER_SHUFFLE, &[4]), (FILTER_DEFLATE, &[6])]);
        let p = FilterPipeline::decode(&body).unwrap();
        assert_eq!(p.filters.len(), 2);
        assert_eq!(p.filters[0].id, FILTER_SHUFFLE);
        assert_eq!(p.filters[0].client_data, vec![4]);
        assert_eq!(p.filters[1].id, FILTER_DEFLATE);
    }

    #[test]
    fn unshuffle_round_trips_a_known_layout() {
        // Two 4-byte elements: 0x01020304 and 0x05060708 (little-endian bytes
        // 04 03 02 01 and 08 07 06 05). Shuffled groups byte positions:
        // [04 08][03 07][02 06][01 05].
        let shuffled = vec![0x04, 0x08, 0x03, 0x07, 0x02, 0x06, 0x01, 0x05];
        let restored = unshuffle(&shuffled, 4);
        assert_eq!(
            restored,
            vec![0x04, 0x03, 0x02, 0x01, 0x08, 0x07, 0x06, 0x05]
        );
    }

    #[test]
    fn reverse_applies_deflate_then_shuffle() {
        // Round-trip: shuffle(4) then deflate the bytes, then reverse should
        // recover the original.
        let original: Vec<u8> = (0u8..16).collect();
        // shuffle
        let count = original.len() / 4;
        let mut shuffled = vec![0u8; original.len()];
        for elem in 0..count {
            for byte_pos in 0..4 {
                shuffled[byte_pos * count + elem] = original[elem * 4 + byte_pos];
            }
        }
        let compressed = miniz_oxide::deflate::compress_to_vec_zlib(&shuffled, 6);
        let pipeline = FilterPipeline {
            filters: vec![
                Filter {
                    id: FILTER_SHUFFLE,
                    client_data: vec![4],
                },
                Filter {
                    id: FILTER_DEFLATE,
                    client_data: vec![6],
                },
            ],
        };
        let recovered = pipeline.reverse(compressed, 0, 4).unwrap();
        assert_eq!(recovered, original);
    }

    #[test]
    fn masked_filter_is_skipped() {
        // A pipeline with deflate, but the chunk's mask says filter 0 was not
        // applied — reverse should pass the bytes through untouched.
        let pipeline = FilterPipeline {
            filters: vec![Filter {
                id: FILTER_DEFLATE,
                client_data: vec![6],
            }],
        };
        let raw = vec![1u8, 2, 3, 4];
        let out = pipeline.reverse(raw.clone(), 0b1, 4).unwrap();
        assert_eq!(out, raw);
    }

    #[test]
    fn rejects_unknown_filter() {
        let pipeline = FilterPipeline {
            filters: vec![Filter {
                id: 4, // szip
                client_data: vec![],
            }],
        };
        assert!(matches!(
            pipeline.reverse(vec![0; 8], 0, 4),
            Err(FieldglassError::UnsupportedSection(_))
        ));
    }
}
