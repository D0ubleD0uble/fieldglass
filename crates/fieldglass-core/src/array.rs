//! The shape model of a chunked array: where a value lives, and what the
//! object holding it is called.
//!
//! A container that stores named arrays cuts each one into a grid of chunks
//! and stores every chunk separately — a key in an object store, a record in a
//! file, a byte range inside some larger archive. Reaching one value is two
//! questions, and neither needs a single byte of the data: **which** chunk
//! covers it, and **what key** that chunk is stored under.
//!
//! This module answers both, in integer arithmetic and nothing else. It parses
//! no documents and decodes no bytes, which is what lets it sit on the parsing
//! surface with no dependency behind it: a consumer that takes only
//! `fieldglass-grib2` links none of this, and links nothing new because of it
//! (ADR-0010 decision 6).
//!
//! # Why this model, and why here
//!
//! It is Zarr v3's array model, adopted as a lingua franca rather than as one
//! format's quirk (ADR-0010 decision 2). The same shape describes NetCDF-4's
//! chunked layout — a regular grid with an index in front of it — and a
//! classic NetCDF variable, which is one chunk per record. Three readers, one
//! set of types, so the arithmetic is written and tested once.
//!
//! Everything here is written from the specifications: the Zarr v3 core spec
//! and the v2 storage spec. The names are this project's own.
//!
//! ```
//! use fieldglass_core::array::{ChunkGrid, ChunkKeyEncoding};
//!
//! // Six values across, three to a chunk, four down in chunks of two.
//! let grid = ChunkGrid::new(vec![4, 6], vec![2, 3])?;
//! assert_eq!(grid.grid_shape(), vec![2, 2]);
//!
//! // The value at row 3, column 4 is in chunk (1, 1) …
//! assert_eq!(grid.chunk_containing(&[3, 4])?, vec![1, 1]);
//! // … and that chunk is stored under `1.1` or `c/1/1`, depending on the
//! // convention the array declares.
//! assert_eq!(ChunkKeyEncoding::V2.key(&[1, 1], '.'), "1.1");
//! assert_eq!(ChunkKeyEncoding::Default.key(&[1, 1], '/'), "c/1/1");
//! # Ok::<(), fieldglass_core::array::ArrayError>(())
//! ```

use std::fmt::Write as _;
use std::ops::Range;

/// The most chunk indices [`ChunkGrid::chunks_covering`] will build a list of.
///
/// Without a bound, an array's declared shape is an allocation instruction: a
/// shape of `[1000000, 1000000]` in chunks of one is sixty bytes of metadata
/// and a list of a trillion indices, which is thirty-two terabytes before
/// anything is fetched. That metadata arrives over a network or out of a file
/// somebody else wrote, so its numbers are as untrusted as any other length in
/// a header.
///
/// A million is far past any real plan. A host issuing a million requests has
/// a different problem than this cap, and a viewer asking for a region that
/// touches a million chunks has asked for the whole archive.
pub const MAX_PLANNED_CHUNKS: u64 = 1 << 20;

/// What the arithmetic here refuses.
///
/// Its own type rather than [`FieldglassError`](crate::FieldglassError)
/// variants, and the compiler settles it rather than taste: a caller wants to
/// carry these in an error enum of its own that derives `Clone` and
/// `PartialEq`, which `FieldglassError` cannot because it holds a
/// `std::io::Error`. `FieldglassError` converts from this one, so a reader
/// returning that type still writes `?`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ArrayError {
    /// The array's shape and its chunk shape have different numbers of axes.
    #[error("the shape has {shape} axes and the chunk shape has {chunks}")]
    RankMismatch {
        /// How many axes the shape states.
        shape: usize,
        /// How many the chunk shape states.
        chunks: usize,
    },

    /// A chunk extent of zero, which no array has and which every division
    /// here would divide by.
    ///
    /// A zero *shape* extent is legal — that axis simply holds no chunks — so
    /// the two are not the same refusal.
    #[error("the chunk shape is zero on axis {axis}")]
    ZeroChunkExtent {
        /// Which axis, counted from zero.
        axis: usize,
    },

    /// An index, point or region with the wrong number of axes for the array.
    #[error("this array has {expected} axes, but {found} were given")]
    WrongRank {
        /// How many the array has.
        expected: usize,
        /// How many the caller supplied.
        found: usize,
    },

    /// A chunk index past the end of the chunk grid.
    #[error("chunk index {index} is past the end of axis {axis}, which has {extent} chunks")]
    ChunkIndexOutOfRange {
        /// Which axis, counted from zero.
        axis: usize,
        /// The index asked for.
        index: u64,
        /// How many chunks that axis has.
        extent: u64,
    },

    /// An element index past the end of the array.
    #[error("index {index} is past the end of axis {axis}, which has {extent} elements")]
    PointOutOfRange {
        /// Which axis, counted from zero.
        axis: usize,
        /// The index asked for.
        index: u64,
        /// How many elements that axis has.
        extent: u64,
    },

    /// A region touching more chunks than this crate will build a list of.
    /// See [`MAX_PLANNED_CHUNKS`].
    #[error("this region touches more than {limit} chunks, which is more than a plan will hold")]
    RegionTooLarge {
        /// The most chunks a region may touch.
        limit: u64,
    },

    /// A chunk key separator that is not one of the two the conventions use.
    #[error("{found:?} is not a chunk key separator; expected \".\" or \"/\"")]
    BadSeparator {
        /// What the metadata said.
        found: String,
    },
}

/// How a chunk grid index is spelled as a key in the store.
///
/// # The separator default is per encoding, not per document
///
/// This is the thing a reader gets wrong. Each encoding has its **own**
/// default separator, so an array that names an encoding and no separator does
/// not inherit the other one's: `Default` means `/` and `V2` means `.`, even
/// where they appear in the same kind of document. A v3 array declaring the
/// `v2` encoding with no configuration spells `1.0`, not `1/0`.
///
/// # Zero dimensions
///
/// The other trap, and it is not what the pattern suggests. A zero-dimensional
/// array holds exactly one chunk and has no index to write, and the two
/// encodings disagree about what to call it: `Default` spells it `c`, and `V2`
/// spells it `0` — not the empty string.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ChunkKeyEncoding {
    /// A `c` prefix, then the index, all joined by the separator — `c/1/0`.
    /// Defaults to `/`. A zero-dimensional array's chunk is `c`.
    Default,
    /// The index joined by the separator, with no prefix — `1.0`. Defaults to
    /// `.`. A zero-dimensional array's chunk is `0`.
    ///
    /// The only convention Zarr v2 has, and offered by v3 as well so a store
    /// can be migrated without renaming every object.
    V2,
}

impl ChunkKeyEncoding {
    /// The separator this encoding uses when the metadata names none.
    ///
    /// See the type's own documentation: this is per encoding, and reading it
    /// off the wrong one is how a key is built that finds no object.
    pub const fn default_separator(self) -> char {
        match self {
            Self::Default => '/',
            Self::V2 => '.',
        }
    }

    /// The key one chunk of the grid is stored under, relative to the array.
    ///
    /// Relative is where a store puts it: an array named `temp` in a group
    /// holds this chunk at `temp/` plus this key. The prefix is the caller's,
    /// because nothing here invents a name.
    pub fn key(self, index: &[u64], separator: char) -> String {
        let mut key = String::new();
        if self == Self::Default {
            key.push('c');
        }
        if index.is_empty() {
            if self == Self::V2 {
                key.push('0');
            }
            return key;
        }
        for (axis, position) in index.iter().enumerate() {
            if axis > 0 || self == Self::Default {
                key.push(separator);
            }
            write!(key, "{position}").expect("writing to a String cannot fail");
        }
        key
    }

    /// Read a separator a document states, refusing anything the conventions
    /// do not use.
    ///
    /// Refused rather than passed through: a key built with an arbitrary
    /// separator fetches nothing, and the key is the whole output. Shared so
    /// that every reader of an array's metadata refuses the same set.
    pub fn separator_from_str(text: &str) -> Result<char, ArrayError> {
        match text {
            "." => Ok('.'),
            "/" => Ok('/'),
            other => Err(ArrayError::BadSeparator {
                found: other.to_string(),
            }),
        }
    }
}

/// An array cut into equal chunks: the shape, the chunk shape, and the
/// arithmetic between an index and a chunk.
///
/// "Regular" is the whole of the assumption — every chunk has the same shape,
/// including the ones at a ragged edge, which are *stored* full-size and
/// trimmed on read. [`chunk_extent`](Self::chunk_extent) is that trim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkGrid {
    shape: Vec<u64>,
    chunk_shape: Vec<u64>,
}

impl ChunkGrid {
    /// A grid of `chunk_shape`-sized chunks over an array of `shape`.
    ///
    /// Takes the two extents directly and parses nothing: every container
    /// spells its metadata differently, and reading it is the container
    /// reader's job.
    pub fn new(shape: Vec<u64>, chunk_shape: Vec<u64>) -> Result<Self, ArrayError> {
        if shape.len() != chunk_shape.len() {
            return Err(ArrayError::RankMismatch {
                shape: shape.len(),
                chunks: chunk_shape.len(),
            });
        }
        if let Some(axis) = chunk_shape.iter().position(|extent| *extent == 0) {
            return Err(ArrayError::ZeroChunkExtent { axis });
        }
        Ok(Self { shape, chunk_shape })
    }

    /// The array's shape, in elements.
    pub fn shape(&self) -> &[u64] {
        &self.shape
    }

    /// One stored chunk's shape, in elements.
    pub fn chunk_shape(&self) -> &[u64] {
        &self.chunk_shape
    }

    /// How many dimensions the array has.
    pub fn rank(&self) -> usize {
        self.shape.len()
    }

    /// How many chunks there are along each axis.
    ///
    /// The ceiling of the shape over the chunk shape: an axis of 7 in chunks
    /// of 3 has three chunks, the last holding one value and two of padding.
    pub fn grid_shape(&self) -> Vec<u64> {
        self.shape
            .iter()
            .zip(&self.chunk_shape)
            .map(|(extent, chunk)| extent.div_ceil(*chunk))
            .collect()
    }

    /// Which chunk holds one element of the array.
    pub fn chunk_containing(&self, point: &[u64]) -> Result<Vec<u64>, ArrayError> {
        self.check_rank(point.len())?;
        point
            .iter()
            .zip(&self.shape)
            .zip(&self.chunk_shape)
            .enumerate()
            .map(|(axis, ((position, extent), chunk))| {
                if position >= extent {
                    return Err(ArrayError::PointOutOfRange {
                        axis,
                        index: *position,
                        extent: *extent,
                    });
                }
                Ok(position / chunk)
            })
            .collect()
    }

    /// The key one chunk of *this* grid is stored under, checked against the
    /// grid first.
    ///
    /// [`ChunkKeyEncoding::key`] spells any index, because a writer naming a
    /// chunk it is about to create has no grid to check against. A reader
    /// does, and an index off the end is a caller error worth catching before
    /// it becomes a fetch for an object that cannot exist.
    pub fn chunk_key(
        &self,
        index: &[u64],
        encoding: ChunkKeyEncoding,
        separator: char,
    ) -> Result<String, ArrayError> {
        self.check_index(index)?;
        Ok(encoding.key(index, separator))
    }

    /// How much of a chunk is real data, axis by axis.
    ///
    /// Equal to the chunk shape everywhere except at a ragged edge, where the
    /// array's shape is not a multiple of the chunk shape: the last chunk on
    /// that axis is *stored* full-size and padded, and this is where the
    /// padding starts. Getting it wrong reads the padding as data, or reads
    /// the next row's values as this row's.
    pub fn chunk_extent(&self, index: &[u64]) -> Result<Vec<u64>, ArrayError> {
        self.check_index(index)?;
        Ok(index
            .iter()
            .zip(&self.shape)
            .zip(&self.chunk_shape)
            .map(|((position, extent), chunk)| {
                // `position` is a valid chunk index, so `position * chunk` is
                // below `extent` and the subtraction cannot wrap.
                let start = position * chunk;
                (*extent - start).min(*chunk)
            })
            .collect())
    }

    /// Every chunk a region of the array touches, in row-major order.
    ///
    /// What a reader asks before fetching a slice: a request for
    /// `[0..1, 2..5]` of a `[2, 3]`-chunked array needs two chunks, not one
    /// and not six. The half-open ranges are element indices; an empty range
    /// on any axis selects nothing and yields no chunks, which is the honest
    /// answer rather than an error.
    ///
    /// A region touching more than [`MAX_PLANNED_CHUNKS`] chunks is refused
    /// rather than built.
    pub fn chunks_covering(&self, region: &[Range<u64>]) -> Result<Vec<Vec<u64>>, ArrayError> {
        self.check_rank(region.len())?;

        let mut spans = Vec::with_capacity(self.rank());
        for (axis, (range, (extent, chunk))) in region
            .iter()
            .zip(self.shape.iter().zip(&self.chunk_shape))
            .enumerate()
        {
            if range.end > *extent {
                return Err(ArrayError::PointOutOfRange {
                    axis,
                    index: range.end,
                    extent: *extent,
                });
            }
            if range.start >= range.end {
                return Ok(Vec::new());
            }
            // Inclusive of the chunk holding the last element, exclusive of
            // the one after it: `end` is one past the region, so the last
            // element is `end - 1` and a region ending exactly on a chunk
            // boundary must not pull in the chunk beyond it.
            spans.push(range.start / chunk..(range.end - 1) / chunk + 1);
        }

        // Counted before anything is allocated, and with `checked_mul`: the
        // product of the spans is exactly what the walk below would hold, and
        // a high-rank array overflows a `u64` long before it runs out of
        // memory.
        let mut planned: u64 = 1;
        for span in &spans {
            planned = planned
                .checked_mul(span.end - span.start)
                .filter(|count| *count <= MAX_PLANNED_CHUNKS)
                .ok_or(ArrayError::RegionTooLarge {
                    limit: MAX_PLANNED_CHUNKS,
                })?;
        }

        let mut out = vec![Vec::with_capacity(self.rank())];
        for span in spans {
            let width = usize::try_from(span.end - span.start).unwrap_or(usize::MAX);
            let mut next = Vec::with_capacity(out.len().saturating_mul(width));
            for prefix in &out {
                for position in span.clone() {
                    let mut index = prefix.clone();
                    index.push(position);
                    next.push(index);
                }
            }
            out = next;
        }
        Ok(out)
    }

    fn check_rank(&self, found: usize) -> Result<(), ArrayError> {
        if found != self.rank() {
            return Err(ArrayError::WrongRank {
                expected: self.rank(),
                found,
            });
        }
        Ok(())
    }

    fn check_index(&self, index: &[u64]) -> Result<(), ArrayError> {
        self.check_rank(index.len())?;
        for (axis, (position, extent)) in index.iter().zip(self.grid_shape()).enumerate() {
            if *position >= extent {
                return Err(ArrayError::ChunkIndexOutOfRange {
                    axis,
                    index: *position,
                    extent,
                });
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two encodings spell one grid two ways. Pinned against the values
    /// the reference implementation emits, read off its own encoders rather
    /// than out of prose about the specification.
    #[test]
    fn the_two_encodings_spell_one_index_two_ways() {
        assert_eq!(ChunkKeyEncoding::V2.key(&[1, 0], '.'), "1.0");
        assert_eq!(ChunkKeyEncoding::Default.key(&[1, 0], '/'), "c/1/0");
        // The separator is the array's, not the encoding's, once one is stated.
        assert_eq!(ChunkKeyEncoding::Default.key(&[1, 0], '.'), "c.1.0");
        assert_eq!(ChunkKeyEncoding::V2.key(&[3], '/'), "3");
    }

    /// The default separator is a property of the *encoding*. An array naming
    /// `v2` and no separator means `.`, even though the encoding beside it in
    /// the same specification would have meant `/`.
    #[test]
    fn each_encoding_has_its_own_default_separator() {
        assert_eq!(ChunkKeyEncoding::Default.default_separator(), '/');
        assert_eq!(ChunkKeyEncoding::V2.default_separator(), '.');
    }

    /// The zero-dimensional cases, which are not what the pattern suggests and
    /// are the reason this is a table rather than a join.
    #[test]
    fn a_zero_dimensional_array_has_one_chunk_with_a_name_of_its_own() {
        assert_eq!(ChunkKeyEncoding::Default.key(&[], '/'), "c");
        assert_eq!(ChunkKeyEncoding::V2.key(&[], '.'), "0");
    }

    /// Only the two separators the conventions use, so a key built here always
    /// has a chance of finding an object.
    #[test]
    fn a_separator_outside_the_conventions_is_refused() {
        assert_eq!(ChunkKeyEncoding::separator_from_str("."), Ok('.'));
        assert_eq!(ChunkKeyEncoding::separator_from_str("/"), Ok('/'));
        assert!(matches!(
            ChunkKeyEncoding::separator_from_str("-"),
            Err(ArrayError::BadSeparator { found }) if found == "-"
        ));
        assert!(ChunkKeyEncoding::separator_from_str("").is_err());
    }

    /// A ragged edge rounds up: the grid has to cover the array, and the last
    /// chunk is stored full-size regardless.
    #[test]
    fn the_grid_covers_a_shape_that_is_not_a_multiple_of_the_chunk() {
        let grid = ChunkGrid::new(vec![7, 10], vec![3, 4]).unwrap();
        assert_eq!(grid.grid_shape(), vec![3, 3]);
        assert_eq!(grid.chunk_containing(&[6, 9]).unwrap(), vec![2, 2]);
        // There is no chunk past the last one.
        assert!(grid.chunk_extent(&[3, 0]).is_err());
    }

    /// The trim at a ragged edge. A reader that took the chunk shape here
    /// would read padding as data.
    #[test]
    fn a_ragged_edge_chunk_reports_how_much_of_it_is_real() {
        let grid = ChunkGrid::new(vec![7, 10], vec![3, 4]).unwrap();
        // Interior chunks are full.
        assert_eq!(grid.chunk_extent(&[0, 0]).unwrap(), vec![3, 4]);
        assert_eq!(grid.chunk_extent(&[1, 1]).unwrap(), vec![3, 4]);
        // 7 = 3 + 3 + 1, so the last row of chunks holds one element.
        assert_eq!(grid.chunk_extent(&[2, 0]).unwrap(), vec![1, 4]);
        // 10 = 4 + 4 + 2, so the last column holds two.
        assert_eq!(grid.chunk_extent(&[0, 2]).unwrap(), vec![3, 2]);
        // The far corner is ragged on both axes at once.
        assert_eq!(grid.chunk_extent(&[2, 2]).unwrap(), vec![1, 2]);

        // An array that divides evenly is never trimmed.
        let even = ChunkGrid::new(vec![4, 6], vec![2, 3]).unwrap();
        for index in [[0, 0], [0, 1], [1, 0], [1, 1]] {
            assert_eq!(even.chunk_extent(&index).unwrap(), vec![2, 3]);
        }
    }

    /// A region ending exactly on a chunk boundary must not pull in the chunk
    /// after it — the off-by-one that fetches twice the bytes it needs.
    #[test]
    fn a_region_covers_the_chunks_it_touches_and_no_more() {
        let grid = ChunkGrid::new(vec![4, 6], vec![2, 3]).unwrap();

        assert_eq!(
            grid.chunks_covering(&[0..2, 0..3]).unwrap(),
            vec![vec![0, 0]]
        );
        assert_eq!(
            grid.chunks_covering(&[0..2, 0..4]).unwrap(),
            vec![vec![0, 0], vec![0, 1]]
        );
        assert_eq!(
            grid.chunks_covering(&[0..4, 0..6]).unwrap(),
            vec![vec![0, 0], vec![0, 1], vec![1, 0], vec![1, 1]]
        );
        assert!(grid.chunks_covering(&[0..0, 0..6]).unwrap().is_empty());
        assert!(grid.chunks_covering(&[0..5, 0..6]).is_err());
    }

    /// A chunk extent of zero is a division by zero, and arrives from a
    /// document rather than from a caller.
    #[test]
    fn a_zero_chunk_extent_is_refused_at_construction() {
        assert!(matches!(
            ChunkGrid::new(vec![4, 6], vec![2, 0]),
            Err(ArrayError::ZeroChunkExtent { axis: 1 })
        ));
        assert!(matches!(
            ChunkGrid::new(vec![4, 6], vec![2]),
            Err(ArrayError::RankMismatch {
                shape: 2,
                chunks: 1
            })
        ));
    }

    /// A region is counted before it is built. The shape comes out of metadata
    /// somebody else wrote, so "how many chunks is that" is an untrusted
    /// number, and the list this would otherwise allocate is measured in
    /// terabytes.
    #[test]
    fn an_enormous_region_is_refused_rather_than_allocated() {
        let grid = ChunkGrid::new(vec![1_000_000, 1_000_000], vec![1, 1]).unwrap();
        assert_eq!(grid.grid_shape(), vec![1_000_000, 1_000_000]);
        assert!(matches!(
            grid.chunks_covering(&[0..1_000_000, 0..1_000_000]),
            Err(ArrayError::RegionTooLarge { .. })
        ));
        // Neither axis alone is over the cap, so a check that looked at one
        // span at a time would have let this through.
        assert!(grid.chunks_covering(&[0..1_000_000, 0..1]).is_ok());

        // The product of the spans wraps a `u64` long before it exhausts
        // memory, so the count is checked and not merely compared.
        let wide = ChunkGrid::new(vec![u64::MAX, u64::MAX, u64::MAX], vec![1, 1, 1]).unwrap();
        assert!(matches!(
            wide.chunks_covering(&[0..u64::MAX, 0..u64::MAX, 0..u64::MAX]),
            Err(ArrayError::RegionTooLarge { .. })
        ));

        // A region inside the cap is still planned.
        let ordinary = ChunkGrid::new(vec![64, 64], vec![1, 1]).unwrap();
        assert_eq!(
            ordinary.chunks_covering(&[0..64, 0..64]).unwrap().len(),
            4096
        );
    }

    /// Every entry point refuses a rank the array does not have, before it
    /// indexes anything.
    #[test]
    fn a_wrong_rank_is_refused_by_every_entry_point() {
        let grid = ChunkGrid::new(vec![4, 6], vec![2, 3]).unwrap();
        assert!(matches!(
            grid.chunk_containing(&[0]),
            Err(ArrayError::WrongRank {
                expected: 2,
                found: 1
            })
        ));
        assert!(matches!(
            grid.chunk_extent(&[0, 0, 0]),
            Err(ArrayError::WrongRank { expected: 2, .. })
        ));
        // Three axes against a two-axis grid. Too *many* rather than too few,
        // because a single-element array of `Range` reads to clippy as a
        // `vec![start..end]` that meant to be a range; the property under test
        // is the same either way.
        assert!(matches!(
            grid.chunks_covering(&[0..1, 0..1, 0..1]),
            Err(ArrayError::WrongRank {
                expected: 2,
                found: 3
            })
        ));
    }

    /// A zero-length axis is a legal array with no chunks on it, which is a
    /// different thing from a zero chunk extent.
    #[test]
    fn an_empty_axis_holds_no_chunks_and_is_not_an_error() {
        let grid = ChunkGrid::new(vec![0, 6], vec![2, 3]).unwrap();
        assert_eq!(grid.grid_shape(), vec![0, 2]);
        assert!(grid.chunks_covering(&[0..0, 0..6]).unwrap().is_empty());
    }

    /// The grid checks an index before spelling it, which the encoding alone
    /// cannot do.
    #[test]
    fn spelling_a_key_through_the_grid_checks_it_against_the_grid() {
        let grid = ChunkGrid::new(vec![4, 6], vec![2, 3]).unwrap();
        assert_eq!(
            grid.chunk_key(&[1, 1], ChunkKeyEncoding::V2, '.').unwrap(),
            "1.1"
        );
        assert!(matches!(
            grid.chunk_key(&[2, 0], ChunkKeyEncoding::V2, '.'),
            Err(ArrayError::ChunkIndexOutOfRange {
                axis: 0,
                index: 2,
                extent: 2
            })
        ));
        // The encoding on its own still spells anything, which is what a
        // writer naming a chunk it is creating needs.
        assert_eq!(ChunkKeyEncoding::V2.key(&[2, 0], '.'), "2.0");
    }
}
