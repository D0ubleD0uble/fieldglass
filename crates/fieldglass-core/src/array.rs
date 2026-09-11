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

/// The most points one decoded **field** holds: a GRIB message's grid, a Zarr
/// region or chunk, a GRIB2 flattened matrix.
///
/// Every one of those is `ni · nj` — or a product of extents — out of a header
/// somebody else wrote, so it is an allocation instruction an attacker
/// controls. The output is one `Option<f64>` per point, sixteen bytes, so this
/// bounds a single decode at a gigabyte. Real grids top out around 25 M points,
/// and no viewer asks for a slice past this.
///
/// **This is not [`MAX_VARIABLE_ELEMENTS`].** That one bounds a whole variable
/// across every record; this bounds one field of it, which is what a reader
/// materialises for a single call and what a viewer draws. Three caps in three
/// crates used to state this number, two of them disagreeing while their doc
/// comments claimed to match (#707).
pub const MAX_FIELD_POINTS: usize = 64 * 1024 * 1024;

/// The most elements one **whole-variable** read holds: a NetCDF classic
/// variable or an HDF5 dataset, every record and every dimension of it.
///
/// Larger than [`MAX_FIELD_POINTS`] because it is a larger question. A
/// reanalysis variable is routinely an order of magnitude bigger than any one
/// slice of it, and this path hands back typed values — eight bytes an element,
/// so 1.6 GiB — rather than the `Option<f64>` per point a field decode builds.
/// A shape out of a corrupt header is still the thing being bounded.
pub const MAX_VARIABLE_ELEMENTS: usize = 200_000_000;

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

    /// The index a key names: the inverse of [`Self::key`] for an array of
    /// `rank` dimensions, or `None` when `key` is not one this encoding spells.
    ///
    /// The rank is needed, not merely checked: `V2` spells a zero-dimensional
    /// array's one chunk `0`, which is also the first chunk of a
    /// one-dimensional array. Strict the way the spelling is — decimal digits,
    /// no sign, no leading zero on a non-zero position — so every key it
    /// accepts is one [`Self::key`] would write, and an object stored beside
    /// the chunks is not read as one of them.
    pub fn index_of(self, key: &str, separator: char, rank: usize) -> Option<Vec<u64>> {
        let body = match self {
            Self::Default => {
                let rest = key.strip_prefix('c')?;
                if rank == 0 {
                    return rest.is_empty().then(Vec::new);
                }
                rest.strip_prefix(separator)?
            }
            Self::V2 => {
                if rank == 0 {
                    return (key == "0").then(Vec::new);
                }
                key
            }
        };
        let index: Vec<u64> = body
            .split(separator)
            .map(Self::position)
            .collect::<Option<_>>()?;
        (index.len() == rank).then_some(index)
    }

    /// One position, as [`Self::key`] writes it.
    fn position(text: &str) -> Option<u64> {
        let canonical = !text.is_empty()
            && text.bytes().all(|b| b.is_ascii_digit())
            && (text == "0" || !text.starts_with('0'));
        if canonical { text.parse().ok() } else { None }
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

// ── The dataset structure a container of named arrays has ───────────────────
//
// The one set (#678, ADR-0010 decision 1): plain structs, no dependency, on the
// parsing surface where `GridGeometry` already is. `fieldglass-netcdf` builds
// its `DatasetView` out of them (#684) and a Zarr store walker reads into the
// same ones; the umbrella's `DimensionInfo` and `VariableInfo` are the wire
// form a host receives, not a second description.

/// One named axis, and how long it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dimension {
    /// The axis's name, path-qualified for a nested group.
    pub name: String,
    /// How many points it has. A container with a growable axis reports the
    /// count it actually holds, not the zero it stores to mean "unlimited".
    pub length: u64,
}

/// What an attribute holds.
///
/// **Numbers stay numbers.** The view this replaces kept every attribute as its
/// display string and read the numeric ones back out with `parse`, which meant
/// a `scale_factor` had to survive a round trip through `format!` to stay
/// faithful — a real hazard, since a GOES scale factor near 6.7e-7 prints as
/// `0.000001` under any rounding format. Holding an `f64` removes the round
/// trip rather than documenting it.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum AttributeValue {
    /// Text, as the container stores it.
    Text(String),
    /// One or more numbers. A scalar is a single element, so a reader of
    /// `valid_range` and a reader of `scale_factor` take the same path.
    Numbers(Vec<f64>),
    /// Something neither this model nor its readers interpret, kept as the
    /// container's own rendering so a host can still show it.
    ///
    /// A fallback rather than a failure: an attribute Fieldglass does not
    /// understand is not a reason to refuse a file, and a user reading a
    /// variable's metadata is often looking for exactly the odd one.
    Opaque(String),
}

impl AttributeValue {
    /// The first number, when this holds any. What a scalar-valued CF
    /// attribute is read with.
    pub fn number(&self) -> Option<f64> {
        match self {
            Self::Numbers(values) => values.first().copied(),
            _ => None,
        }
    }

    /// Every number, or an empty slice.
    pub fn numbers(&self) -> &[f64] {
        match self {
            Self::Numbers(values) => values,
            _ => &[],
        }
    }

    /// The text, when this holds text.
    pub fn text(&self) -> Option<&str> {
        match self {
            Self::Text(text) | Self::Opaque(text) => Some(text),
            Self::Numbers(_) => None,
        }
    }
}

/// One attribute: a name and what it holds.
#[derive(Debug, Clone, PartialEq)]
pub struct Attribute {
    /// The attribute's name, as the container spells it.
    pub name: String,
    /// Its value.
    pub value: AttributeValue,
}

impl Attribute {
    /// An attribute holding text.
    pub fn text(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: AttributeValue::Text(value.into()),
        }
    }

    /// An attribute holding one number.
    pub fn number(name: impl Into<String>, value: f64) -> Self {
        Self {
            name: name.into(),
            value: AttributeValue::Numbers(vec![value]),
        }
    }

    /// An attribute holding several numbers.
    pub fn numbers(name: impl Into<String>, values: Vec<f64>) -> Self {
        Self {
            name: name.into(),
            value: AttributeValue::Numbers(values),
        }
    }
}

/// Find one attribute by name in a list.
///
/// A free function over a slice rather than a method on a collection type,
/// because every reader here already holds a `Vec<Attribute>` and wrapping it
/// would buy nothing.
pub fn attribute<'a>(attributes: &'a [Attribute], name: &str) -> Option<&'a AttributeValue> {
    attributes.iter().find(|a| a.name == name).map(|a| &a.value)
}

/// An array's element type, in terms every container this project reads can be
/// mapped onto.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ElementType {
    /// Signed integers, by width in bits.
    Int(u8),
    /// Unsigned integers, by width in bits.
    Uint(u8),
    /// IEEE floating point, by width in bits.
    Float(u8),
    /// Text or bytes, which no grid is made of but every container stores.
    Text,
    /// A type this model does not name, kept as the container's own spelling.
    ///
    /// The same reason [`AttributeValue::Opaque`] exists: an array of a type
    /// Fieldglass cannot decode should still appear in a listing, saying what
    /// it is, rather than making the whole container unreadable.
    Other(String),
}

/// One array in a container, described rather than decoded.
///
/// What a listing shows and a slice picker offers: enough to say what the array
/// is and which axes it has, and none of its values.
#[derive(Debug, Clone, PartialEq)]
pub struct ArrayDescription {
    /// The array's name, path-qualified for a nested group.
    pub name: String,
    /// Its element type.
    pub element_type: ElementType,
    /// Its axes, by dimension name, in the container's declared order.
    pub dimensions: Vec<String>,
    /// Its own attributes.
    pub attributes: Vec<Attribute>,
    /// How it is chunked, when the container chunks it. `None` for a container
    /// that stores an array contiguously, and for a reader that describes an
    /// array without reading its storage layout — the NetCDF view, which
    /// learns the layout only when it decodes.
    pub chunk_grid: Option<ChunkGrid>,
}

/// A tree of groups, each holding arrays and more groups.
///
/// NetCDF-4 has groups, Zarr has groups, and a classic NetCDF file is one
/// unnamed group — so a reader of any of them produces this and a host walks
/// one shape.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Group {
    /// This group's own name, empty for the root.
    pub name: String,
    /// Its attributes. The root group's are what a container calls global.
    pub attributes: Vec<Attribute>,
    /// Its dimensions. An axis is shared by every array in the group that names
    /// it, which is what lets a host offer one time slider for a file rather
    /// than one per array.
    pub dimensions: Vec<Dimension>,
    /// The arrays directly in it.
    pub arrays: Vec<ArrayDescription>,
    /// The groups directly in it.
    pub groups: Vec<Group>,
}

impl Group {
    /// Every array in the tree, with its path-qualified name, depth first.
    ///
    /// The qualification is `outer/inner/array`, with the root contributing no
    /// segment. A reader that flattens its container into the root group keeps
    /// whatever spelling it put in [`ArrayDescription::name`]: the NetCDF view
    /// does that, and a nested NetCDF-4 name comes back with the leading `/`
    /// its HDF5 path has (`/PRODUCT/latitude`).
    pub fn arrays_qualified(&self) -> Vec<(String, &ArrayDescription)> {
        let mut out = Vec::new();
        self.walk(&mut String::new(), &mut out);
        out
    }

    fn walk<'a>(&'a self, prefix: &mut String, out: &mut Vec<(String, &'a ArrayDescription)>) {
        for array in &self.arrays {
            out.push((qualify(prefix, &array.name), array));
        }
        for group in &self.groups {
            let mark = prefix.len();
            if !group.name.is_empty() {
                if !prefix.is_empty() {
                    prefix.push('/');
                }
                prefix.push_str(&group.name);
            }
            group.walk(prefix, out);
            prefix.truncate(mark);
        }
    }

    /// Every dimension in the tree, path-qualified the same way.
    pub fn dimensions_qualified(&self) -> Vec<(String, &Dimension)> {
        let mut out = Vec::new();
        self.walk_dimensions(&mut String::new(), &mut out);
        out
    }

    fn walk_dimensions<'a>(&'a self, prefix: &mut String, out: &mut Vec<(String, &'a Dimension)>) {
        for dimension in &self.dimensions {
            out.push((qualify(prefix, &dimension.name), dimension));
        }
        for group in &self.groups {
            let mark = prefix.len();
            if !group.name.is_empty() {
                if !prefix.is_empty() {
                    prefix.push('/');
                }
                prefix.push_str(&group.name);
            }
            group.walk_dimensions(prefix, out);
            prefix.truncate(mark);
        }
    }
}

fn qualify(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        name.to_string()
    } else {
        format!("{prefix}/{name}")
    }
}

/// Where a container's arrays are read from: the array-level IO seam (#658).
///
/// One rung above [`ByteSource`](crate::bytes::ByteSource) and
/// [`ObjectSource`](crate::bytes::ObjectSource). Those answer "give me these
/// bytes"; this answers "give me this array's values", and each implementation
/// decides what that costs — a Zarr store spells chunk keys and reads them from
/// an `ObjectSource`, a NetCDF file walks its own header or chunk index over a
/// `ByteSource`. What sits above — CF conventions, placing a slice on the
/// Earth, a host's variable list — is written once against this and never
/// learns which container it has.
///
/// The model types stay plain structs, as ADR-0010 decision 1 has them: this is
/// a trait because it is IO, the way the two byte seams are, and it is
/// object-safe so a caller can hold any container as `&dyn ArraySource`.
///
/// # Raw, and where CF happens
///
/// [`read_region`](Self::read_region) returns what the container **stores**:
/// the element values, with no `scale_factor`, `add_offset` or `_FillValue`
/// applied. `None` marks only a cell the container itself can give no number
/// for. The CF mask-and-scale is [`read_region_physical`](Self::read_region_physical),
/// which reads the rule out of the array's own attributes through
/// [`CfUnpacking`] — so there is one CF path, whatever the container, and a
/// container whose convention keeps a CF attribute somewhere else (Zarr v2
/// keeps `_FillValue` as the array's `fill_value`) presents it as an attribute
/// rather than growing a second rule.
pub trait ArraySource {
    /// The container's structure: its groups, their dimensions and
    /// attributes, and the arrays in them, described rather than decoded.
    fn group(&self) -> &Group;

    /// One array's stored values over a region, in C order.
    ///
    /// `array` is path-qualified the way [`Group::arrays_qualified`] spells
    /// it. `region` holds one half-open element range per axis, in the
    /// array's declared axis order; the result has the product of their
    /// lengths, last axis fastest.
    ///
    /// # Errors
    ///
    /// An array this container does not hold, a region of the wrong rank or
    /// past the array's shape, and any failure reading or decoding the
    /// storage behind it. A failure is the one array's: the rest of the
    /// container stays readable.
    fn read_region(
        &self,
        array: &str,
        region: &[Range<u64>],
    ) -> Result<Vec<Option<f64>>, crate::FieldglassError>;

    /// One array's description, by its path-qualified name.
    fn array(&self, name: &str) -> Option<&ArrayDescription> {
        self.group()
            .arrays_qualified()
            .into_iter()
            .find_map(|(qualified, array)| (qualified == name).then_some(array))
    }

    /// [`read_region`](Self::read_region) with the array's CF mask-and-scale
    /// applied — see [`CfUnpacking`] for the rule and the order it runs in.
    fn read_region_physical(
        &self,
        array: &str,
        region: &[Range<u64>],
    ) -> Result<Vec<Option<f64>>, crate::FieldglassError> {
        let raw = self.read_region(array, region)?;
        let rule = self
            .array(array)
            .map(|description| CfUnpacking::from_attributes(&description.attributes))
            .ok_or_else(|| {
                crate::FieldglassError::Parse(format!("this container holds no array {array:?}"))
            })?;
        Ok(rule.apply(&raw))
    }
}

/// Row-major strides for a box of `extents`.
fn strides(extents: &[u64]) -> Vec<u64> {
    let mut out = vec![1u64; extents.len()];
    for axis in (0..extents.len().saturating_sub(1)).rev() {
        out[axis] = out[axis + 1] * extents[axis + 1];
    }
    out
}

/// Copy the part of one block of an array that falls inside `region` into
/// `out`, which holds the region in C order.
///
/// The block is `block_shape` elements with its first at `origin`. A Zarr
/// chunk at a ragged edge is stored full-size, and because a region never
/// reaches past the array, the part beyond the edge is never inside it — the
/// intersection is the trim. A container that decodes a whole array at once
/// (a NetCDF variable) is one block at the origin, and this is the region cut
/// out of it. `out` must be the product of the region's lengths long; a block
/// shorter than its shape leaves the cells it lacks untouched.
///
/// Written once for both (#704): the Zarr walker assembles chunks with it, the
/// NetCDF array source cuts a region out of a decoded variable with it.
pub fn copy_block(
    block: &[Option<f64>],
    block_shape: &[u64],
    origin: &[u64],
    region: &[Range<u64>],
    out: &mut [Option<f64>],
) {
    let rank = region.len();
    if rank == 0 {
        if let (Some(slot), Some(value)) = (out.first_mut(), block.first()) {
            *slot = *value;
        }
        return;
    }
    let lens: Vec<u64> = region
        .iter()
        .map(|r| r.end.saturating_sub(r.start))
        .collect();
    let lo: Vec<u64> = (0..rank).map(|d| origin[d].max(region[d].start)).collect();
    let hi: Vec<u64> = (0..rank)
        .map(|d| (origin[d] + block_shape[d]).min(region[d].end))
        .collect();
    if (0..rank).any(|d| lo[d] >= hi[d]) {
        return;
    }
    let (from, to) = (strides(block_shape), strides(&lens));
    let last = rank - 1;
    // `hi - lo` along the last axis is within one block, so it fits.
    let run = (hi[last] - lo[last]) as usize;
    let mut at = lo.clone();
    loop {
        let src: u64 = (0..rank).map(|d| (at[d] - origin[d]) * from[d]).sum();
        let dst: u64 = (0..rank).map(|d| (at[d] - region[d].start) * to[d]).sum();
        // Offsets into buffers the caller sized, so they fit a `usize`.
        let (src, dst) = (src as usize, dst as usize);
        if let (Some(target), Some(source)) =
            (out.get_mut(dst..dst + run), block.get(src..src + run))
        {
            target.copy_from_slice(source);
        }
        let mut axis = last;
        loop {
            if axis == 0 {
                return;
            }
            axis -= 1;
            at[axis] += 1;
            if at[axis] < hi[axis] {
                break;
            }
            at[axis] = lo[axis];
        }
    }
}

/// One anonymous axis [`PhonyDimensions`] invented.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhonyDimension {
    /// `phony_dim_N`.
    pub name: String,
    /// How many points it has.
    pub length: u64,
    /// Whether any array using it says it can grow.
    pub unlimited: bool,
}

/// Names the axes of arrays that name none, by netCDF-C's rule, so that
/// `ncdump -h`, the NetCDF-4 reader and the Zarr walker all call the same axis
/// the same thing (moved from `fieldglass-netcdf` for #704, so the Zarr walker
/// uses it rather than a second rule).
///
/// Measured against netCDF-C (through netCDF4-python) on a file whose datasets
/// deliberately repeat, differ and transpose their shapes:
///
/// ```text
/// a_8x8 → (phony_dim_0=8, phony_dim_1=8)    b_8x8 → (phony_dim_0, phony_dim_1)
/// c_4x6 → (phony_dim_2=4, phony_dim_3=6)    d_6x4 → (phony_dim_3, phony_dim_2)
/// e_1d7 → (phony_dim_4=7)
/// ```
///
/// So the rule is per *axis*, not per shape: reuse the lowest-numbered existing
/// anonymous axis of that extent which this array is not already using, and
/// otherwise allocate the next number. `a_8x8` is the case that rules out
/// deduplicating by length alone — both its axes are 8 long and it still gets
/// two — and `d_6x4` is the case that rules out matching whole shapes, since it
/// reuses `c_4x6`'s pair transposed.
#[derive(Debug, Default)]
pub struct PhonyDimensions {
    /// The length of `phony_dim_N`, indexed by `N`.
    lengths: Vec<u64>,
    /// Whether `phony_dim_N` is extensible. netCDF-C carries `H5S_UNLIMITED`
    /// through to the dimension it invents — `hdf5_ea_chunk_index.h5` reads back
    /// as `phony_dim_0 = 600 (unlimited)` — so this does too.
    unlimited: Vec<bool>,
}

impl PhonyDimensions {
    /// The ordered axis names for an array of these extents, allocating as it
    /// goes. `unlimited_axes` may be shorter than `extents`; a missing entry is
    /// a fixed axis.
    pub fn axes_for(&mut self, extents: &[u64], unlimited_axes: &[bool]) -> Vec<String> {
        let mut taken: Vec<usize> = Vec::with_capacity(extents.len());
        for (axis, &length) in extents.iter().enumerate() {
            let extensible = unlimited_axes.get(axis).copied().unwrap_or(false);
            let existing =
                (0..self.lengths.len()).find(|i| self.lengths[*i] == length && !taken.contains(i));
            let index = match existing {
                Some(i) => i,
                None => {
                    self.lengths.push(length);
                    self.unlimited.push(false);
                    self.lengths.len() - 1
                }
            };
            // Shared by extent, so two arrays can disagree about whether the
            // axis grows. One writer saying it does is enough: the dimension
            // describes what the container permits, not what any one array uses.
            self.unlimited[index] |= extensible;
            taken.push(index);
        }
        taken.iter().map(|i| format!("phony_dim_{i}")).collect()
    }

    /// Everything allocated so far, in allocation order.
    pub fn dimensions(&self) -> Vec<PhonyDimension> {
        self.lengths
            .iter()
            .enumerate()
            .map(|(index, &length)| PhonyDimension {
                name: format!("phony_dim_{index}"),
                length,
                unlimited: self.unlimited[index],
            })
            .collect()
    }
}

/// The CF mask-and-scale an array's attributes call for, read once.
///
/// # The rule, stated in one place
///
/// The CF conventions store a physical quantity as integers plus a linear
/// transform, and mark the cells that mean nothing. Reading that is three
/// steps, and the **order matters**:
///
/// 1. cells equal to a fill sentinel (`_FillValue`, `missing_value`) are
///    absent;
/// 2. cells outside the valid bounds (`valid_range`, or `valid_min` /
///    `valid_max`) are absent — compared in **packed** units, inclusive, since
///    the bounds describe what is stored;
/// 3. what survives becomes `value * scale_factor + add_offset`.
///
/// Applying the transform before the bounds test compares a physical value
/// against a packed threshold and throws away good data. Both readers that need
/// this now read it from here, so the order cannot differ between them.
///
/// A `NaN` survives the bounds test — both comparisons are false for `NaN` —
/// and stays `NaN`, which is what the reference implementations do: they mask
/// only the declared sentinels.
#[derive(Debug, Clone, PartialEq)]
pub struct CfUnpacking {
    scale: f64,
    offset: f64,
    low: Option<f64>,
    high: Option<f64>,
    fill: Vec<f64>,
}

impl CfUnpacking {
    /// Read the rule out of an array's attributes.
    pub fn from_attributes(attributes: &[Attribute]) -> Self {
        let scale = attribute(attributes, "scale_factor")
            .and_then(AttributeValue::number)
            .unwrap_or(1.0);
        let offset = attribute(attributes, "add_offset")
            .and_then(AttributeValue::number)
            .unwrap_or(0.0);

        // `valid_range` is a pair and wins over the separate bounds, which is
        // what the conventions say and what the reference readers do. Its two
        // values are ordered rather than assumed, because a file may state them
        // either way round.
        let (low, high) = match attribute(attributes, "valid_range").map(AttributeValue::numbers) {
            Some([a, b, ..]) => (Some(a.min(*b)), Some(a.max(*b))),
            _ => (
                attribute(attributes, "valid_min").and_then(AttributeValue::number),
                attribute(attributes, "valid_max").and_then(AttributeValue::number),
            ),
        };

        // **Every** value of a multi-valued sentinel, not just the first. A
        // container may declare several, and honouring one of them leaves the
        // others in the field as ordinary numbers.
        let mut fill = Vec::new();
        for name in ["_FillValue", "missing_value"] {
            if let Some(value) = attribute(attributes, name) {
                fill.extend_from_slice(value.numbers());
            }
        }

        Self {
            scale,
            offset,
            low,
            high,
            fill,
        }
    }

    /// Whether this does nothing, so a caller can skip it.
    pub fn is_identity(&self) -> bool {
        self.scale == 1.0
            && self.offset == 0.0
            && self.low.is_none()
            && self.high.is_none()
            && self.fill.is_empty()
    }

    /// `scale_factor`, defaulting to 1.
    pub fn scale(&self) -> f64 {
        self.scale
    }

    /// `add_offset`, defaulting to 0.
    pub fn offset(&self) -> f64 {
        self.offset
    }

    /// The declared fill sentinels, in packed units.
    pub fn fill_values(&self) -> &[f64] {
        &self.fill
    }

    /// Whether one packed value is a declared sentinel.
    ///
    /// Exact equality, because a sentinel is a stored bit pattern rather than a
    /// measurement: `-9999` means absent and `-9998.9` is data.
    pub fn is_fill(&self, packed: f64) -> bool {
        self.fill.contains(&packed)
    }

    /// Apply the rule to one packed value. `None` when the cell means nothing.
    pub fn value(&self, packed: f64) -> Option<f64> {
        if self.is_fill(packed) {
            return None;
        }
        if self.low.is_some_and(|low| packed < low) || self.high.is_some_and(|high| packed > high) {
            return None;
        }
        Some(packed * self.scale + self.offset)
    }

    /// Apply the rule to a plane. Cells already absent stay absent.
    pub fn apply(&self, packed: &[Option<f64>]) -> Vec<Option<f64>> {
        if self.is_identity() {
            return packed.to_vec();
        }
        packed.iter().map(|cell| self.value((*cell)?)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `index_of` reads back exactly what `key` writes, for both encodings,
    /// both separators and every rank — and nothing that `key` would not.
    #[test]
    fn a_key_reads_back_to_the_index_it_spells() {
        for encoding in [ChunkKeyEncoding::Default, ChunkKeyEncoding::V2] {
            for separator in ['.', '/'] {
                for index in [vec![], vec![0], vec![7], vec![1, 0], vec![12, 3, 400]] {
                    let key = encoding.key(&index, separator);
                    assert_eq!(
                        encoding.index_of(&key, separator, index.len()),
                        Some(index.clone()),
                        "{encoding:?} {separator:?} {key}"
                    );
                }
            }
        }
        // V2's zero-dimensional chunk and a one-dimensional array's first chunk
        // are the same text, and only the rank tells them apart.
        assert_eq!(ChunkKeyEncoding::V2.index_of("0", '.', 0), Some(vec![]));
        assert_eq!(ChunkKeyEncoding::V2.index_of("0", '.', 1), Some(vec![0]));

        // Not chunk keys: the wrong rank, no prefix, a leading zero, a sign, an
        // empty position, a metadata document, the other encoding's prefix,
        // and a position past `u64`.
        let d = ChunkKeyEncoding::Default;
        assert_eq!(d.index_of("c/1/0", '/', 3), None);
        assert_eq!(d.index_of("1/0", '/', 2), None);
        assert_eq!(d.index_of("c/01/0", '/', 2), None);
        assert_eq!(d.index_of("c/+1/0", '/', 2), None);
        assert_eq!(d.index_of("c//0", '/', 2), None);
        let v2 = ChunkKeyEncoding::V2;
        assert_eq!(v2.index_of(".zarray", '.', 1), None);
        assert_eq!(v2.index_of("c.1.0", '.', 2), None);
        assert_eq!(v2.index_of("18446744073709551616", '.', 1), None);
    }

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

    // ── The dataset structure types (#678) ──────────────────────────────────

    fn packed_attributes() -> Vec<Attribute> {
        vec![
            Attribute::number("scale_factor", 0.0625),
            Attribute::number("add_offset", 250.0),
            Attribute::number("_FillValue", -9999.0),
            Attribute::numbers("valid_range", vec![0.0, 10000.0]),
            Attribute::text("units", "kelvin"),
        ]
    }

    /// The three steps in the order that matters. Testing the order is the
    /// point: applying the transform first compares a physical value against a
    /// packed threshold and throws away good data, and every number below is
    /// still finite either way, so only the order distinguishes them.
    #[test]
    fn cf_masks_then_bounds_then_scales() {
        let cf = CfUnpacking::from_attributes(&packed_attributes());

        // In range: 250 * 0.0625 + 250.
        assert_eq!(cf.value(250.0), Some(265.625));
        // The bounds are inclusive and in packed units.
        assert_eq!(cf.value(0.0), Some(250.0));
        assert_eq!(cf.value(10000.0), Some(875.0));
        // Outside them, either side.
        assert_eq!(cf.value(-50.0), None);
        assert_eq!(cf.value(15000.0), None);
        // The sentinel, which is itself outside the range — masked either way,
        // but by the first rule.
        assert_eq!(cf.value(-9999.0), None);
        assert!(cf.is_fill(-9999.0));

        // Had the transform run first, 15000 would have become 1187.5 and been
        // compared against a packed 10000 — and survived.
        assert!(
            15000.0 * cf.scale() + cf.offset() < 10000.0,
            "this fixture only tests the ordering if the scaled value is inside \
             the packed bound"
        );
    }

    /// A plane keeps its absent cells absent, and the no-op case is untouched.
    #[test]
    fn cf_over_a_plane_preserves_absence() {
        let cf = CfUnpacking::from_attributes(&packed_attributes());
        let packed = [Some(0.0), None, Some(250.0), Some(-9999.0), Some(15000.0)];
        assert_eq!(
            cf.apply(&packed),
            [Some(250.0), None, Some(265.625), None, None]
        );

        let identity = CfUnpacking::from_attributes(&[Attribute::text("units", "K")]);
        assert!(identity.is_identity());
        assert_eq!(identity.apply(&packed), packed);
    }

    /// Every value of a multi-valued sentinel is honoured, not just the first.
    /// Honouring one leaves the others in the field as ordinary numbers.
    #[test]
    fn every_declared_sentinel_masks() {
        let cf = CfUnpacking::from_attributes(&[
            Attribute::numbers("missing_value", vec![-999.0, -888.0]),
            Attribute::number("_FillValue", -9999.0),
        ]);
        assert_eq!(cf.fill_values(), [-9999.0, -999.0, -888.0]);
        for sentinel in [-9999.0, -999.0, -888.0] {
            assert_eq!(cf.value(sentinel), None, "{sentinel} should be absent");
        }
        assert_eq!(cf.value(-9998.0), Some(-9998.0));
    }

    /// `valid_range` wins over the separate bounds, and its pair is ordered
    /// rather than assumed — a file may state them either way round.
    #[test]
    fn valid_range_wins_and_is_ordered() {
        let both = CfUnpacking::from_attributes(&[
            Attribute::numbers("valid_range", vec![10.0, 20.0]),
            Attribute::number("valid_min", 0.0),
            Attribute::number("valid_max", 100.0),
        ]);
        assert_eq!(both.value(5.0), None);
        assert_eq!(both.value(15.0), Some(15.0));

        let backwards =
            CfUnpacking::from_attributes(&[Attribute::numbers("valid_range", vec![20.0, 10.0])]);
        assert_eq!(backwards.value(15.0), Some(15.0));
        assert_eq!(backwards.value(25.0), None);

        let separate = CfUnpacking::from_attributes(&[
            Attribute::number("valid_min", 10.0),
            Attribute::number("valid_max", 20.0),
        ]);
        assert_eq!(separate.value(5.0), None);
        assert_eq!(separate.value(15.0), Some(15.0));
    }

    /// A `NaN` survives the bounds test and stays `NaN`, which is what the
    /// reference readers do: they mask the declared sentinels and nothing else.
    #[test]
    fn a_nan_is_not_masked_by_the_bounds() {
        let cf = CfUnpacking::from_attributes(&packed_attributes());
        assert!(cf.value(f64::NAN).is_some_and(f64::is_nan));
    }

    /// Numbers stay numbers. The view this replaces round-tripped them through
    /// a display string, where a GOES scale factor near 6.7e-7 could round to
    /// `0.000001` and mis-scale a whole grid.
    #[test]
    fn a_numeric_attribute_keeps_its_precision() {
        let tiny = 6.7e-7_f64;
        let cf = CfUnpacking::from_attributes(&[Attribute::number("scale_factor", tiny)]);
        assert_eq!(cf.scale(), tiny, "the f64 must survive exactly");
        assert_eq!(cf.value(2.0), Some(2.0 * tiny));
    }

    /// A group tree resolves to path-qualified names, with the root
    /// contributing no segment.
    #[test]
    fn a_group_tree_qualifies_its_names() {
        let tree = Group {
            name: String::new(),
            dimensions: vec![Dimension {
                name: "time".into(),
                length: 3,
            }],
            arrays: vec![ArrayDescription {
                name: "surface".into(),
                element_type: ElementType::Float(32),
                dimensions: vec!["time".into()],
                attributes: Vec::new(),
                chunk_grid: None,
            }],
            groups: vec![Group {
                name: "forecast".into(),
                dimensions: vec![Dimension {
                    name: "level".into(),
                    length: 5,
                }],
                arrays: vec![ArrayDescription {
                    name: "temp".into(),
                    element_type: ElementType::Int(16),
                    dimensions: vec!["level".into()],
                    attributes: Vec::new(),
                    chunk_grid: Some(ChunkGrid::new(vec![5], vec![2]).unwrap()),
                }],
                groups: vec![Group {
                    name: "inner".into(),
                    arrays: vec![ArrayDescription {
                        name: "wind".into(),
                        element_type: ElementType::Float(64),
                        dimensions: Vec::new(),
                        attributes: Vec::new(),
                        chunk_grid: None,
                    }],
                    ..Group::default()
                }],
                ..Group::default()
            }],
            ..Group::default()
        };

        let names: Vec<String> = tree
            .arrays_qualified()
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert_eq!(names, ["surface", "forecast/temp", "forecast/inner/wind"]);

        let dims: Vec<String> = tree
            .dimensions_qualified()
            .into_iter()
            .map(|(name, _)| name)
            .collect();
        assert_eq!(dims, ["time", "forecast/level"]);
    }

    /// An attribute this model does not interpret is kept, not dropped: a user
    /// reading a variable's metadata is often looking for the odd one.
    #[test]
    fn an_uninterpreted_attribute_survives() {
        let attrs = vec![
            Attribute {
                name: "history".into(),
                value: AttributeValue::Opaque("{nested: json}".into()),
            },
            Attribute::text("units", "K"),
        ];
        assert_eq!(
            attribute(&attrs, "history").and_then(AttributeValue::text),
            Some("{nested: json}")
        );
        assert!(
            attribute(&attrs, "units")
                .and_then(AttributeValue::number)
                .is_none()
        );
        assert!(attribute(&attrs, "absent").is_none());
    }
}

/// The anonymous-axis rule, pinned against netCDF-C (moved with
/// [`PhonyDimensions`] from `fieldglass-netcdf` for #704).
///
/// These are the exact names netCDF-C gives a scale-less file whose datasets
/// repeat, differ and transpose their shapes — read back through netCDF4-python
/// and reproduced here so the rule cannot drift silently. `fieldglass-netcdf`'s
/// `hdf5_phony_dims.rs` checks the same expectations end to end on the
/// fixture `build_hdf5_fixtures.py` writes.
#[cfg(test)]
mod phony_tests {
    use super::*;

    fn lengths(phony: &PhonyDimensions) -> Vec<u64> {
        phony.dimensions().iter().map(|d| d.length).collect()
    }

    #[test]
    fn anonymous_dimensions_are_numbered_the_way_netcdf_c_numbers_them() {
        let mut phony = PhonyDimensions::default();

        // Two axes of the same length still get two dimensions: an anonymous
        // dimension is per axis, not per distinct length.
        assert_eq!(phony.axes_for(&[8, 8], &[]), ["phony_dim_0", "phony_dim_1"]);
        // An identical shape reuses them rather than allocating more.
        assert_eq!(phony.axes_for(&[8, 8], &[]), ["phony_dim_0", "phony_dim_1"]);
        // New extents allocate.
        assert_eq!(phony.axes_for(&[4, 6], &[]), ["phony_dim_2", "phony_dim_3"]);
        // A transposed shape reuses the same pair the other way round, which is
        // what rules out matching whole shapes instead of individual extents.
        assert_eq!(phony.axes_for(&[6, 4], &[]), ["phony_dim_3", "phony_dim_2"]);
        assert_eq!(phony.axes_for(&[7], &[]), ["phony_dim_4"]);

        assert_eq!(
            phony
                .dimensions()
                .iter()
                .map(|d| (d.name.as_str(), d.length, d.unlimited))
                .collect::<Vec<_>>(),
            [
                ("phony_dim_0", 8, false),
                ("phony_dim_1", 8, false),
                ("phony_dim_2", 4, false),
                ("phony_dim_3", 6, false),
                ("phony_dim_4", 7, false),
            ]
        );
    }

    /// Three axes of one length need three dimensions, not two — the `taken`
    /// check has to exclude everything this array already holds, not just the
    /// one it matched last.
    #[test]
    fn an_array_never_reuses_a_dimension_within_itself() {
        let mut phony = PhonyDimensions::default();
        assert_eq!(
            phony.axes_for(&[5, 5, 5], &[]),
            ["phony_dim_0", "phony_dim_1", "phony_dim_2"]
        );
        assert_eq!(lengths(&phony), [5, 5, 5]);
    }

    /// A dimension is shared by extent, so two arrays can disagree about
    /// whether that axis grows. One writer saying it does is enough — the
    /// dimension describes what the container permits.
    #[test]
    fn a_shared_dimension_is_unlimited_if_any_user_says_so() {
        let mut phony = PhonyDimensions::default();
        // Bounded first, then the same extent declared extensible.
        assert_eq!(phony.axes_for(&[4], &[false]), ["phony_dim_0"]);
        assert_eq!(phony.axes_for(&[4], &[true]), ["phony_dim_0"]);
        assert!(phony.dimensions()[0].unlimited);

        // And it does not leak the other way: an untouched dimension stays bounded.
        let mut bounded = PhonyDimensions::default();
        bounded.axes_for(&[4, 9], &[true, false]);
        assert_eq!(
            bounded
                .dimensions()
                .iter()
                .map(|d| d.unlimited)
                .collect::<Vec<_>>(),
            [true, false]
        );
    }

    /// A scalar array has no axes, and must not invent one.
    #[test]
    fn a_scalar_array_allocates_nothing() {
        let mut phony = PhonyDimensions::default();
        assert!(phony.axes_for(&[], &[]).is_empty());
        assert!(phony.dimensions().is_empty());
    }
}
