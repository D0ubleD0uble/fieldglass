use crate::bms::{
    BMS_INDICATOR_NONE, BMS_INDICATOR_OFFSET, BMS_SECTION_NUMBER, parse_bit_map_with_header,
};
use crate::drs::{
    DRS_SECTION_NUMBER, DataRepresentationSection, parse_data_representation_with_header,
};
use crate::ds::{
    DS_SECTION_NUMBER, decode_jpeg2000_reduced, decode_values, parse_data_section_body,
    undo_second_order_boustrophedonic,
};
use crate::gds::{
    GDS_SECTION_NUMBER, GridDefinitionSection, GridTemplate, HealpixTemplate, SCAN_ALTERNATE_ROWS,
    SCAN_J_CONSECUTIVE, parse_grid_definition_with_header, undo_alternate_reduced_rows,
    undo_alternate_rows,
};
use crate::ids::{IDS_SECTION_NUMBER, IdentificationSection, parse_identification_with_header};
use crate::is::{
    END_SECTION_LEN, GRIB2_EDITION, INDICATOR_SECTION_LEN, IndicatorSection, parse_indicator,
};
use crate::lus::{LUS_SECTION_NUMBER, parse_local_use_with_header};
use crate::pds::{
    PDS_SECTION_NUMBER, ProductDefinitionSection, parse_product_definition_with_header,
};
use crate::reduced::{DecodeOptions, DisplayRaster};
use crate::section::{SECTION_HEADER_LEN, SectionHeader, parse_section_header};
use crate::spectral::{
    BiFourierCoefficients, SpectralCoefficients, decode_bifourier, decode_spectral_complex,
    decode_spectral_simple,
};
use fieldglass_core::bytes::{
    ByteRange, ByteSource, FileCursor, find_forward, read_at, read_exact, read_up_to,
};
use fieldglass_core::{FieldglassError, GlobalGrid, GridGeometry, StoredRuns, SynthesisedField};
use std::borrow::Cow;

/// Hard cap on `ni · nj` for `decode_message_values`.
///
/// [`fieldglass_core::MAX_FIELD_POINTS`], which every reader that materialises
/// one field refers to. This used to be `200_000_000` under a doc comment
/// saying it matched the GRIB1 reader's cap, which it did not: a field between
/// 67 M and 200 M points was accepted by this edition and refused by the other
/// (#707). Now it cannot disagree.
const MAX_GRID_POINTS: usize = fieldglass_core::MAX_FIELD_POINTS;

/// Parsed metadata for a single GRIB2 message. Surfaces §0–§5 inline (the
/// fixed-size fields); §6 (BMS) and §7 (DS) live behind byte ranges so the
/// reader doesn't eagerly decode payloads.
///
/// The offsets are `u64` because they address a file, not a buffer: a
/// [`Grib2Reader`] over a [`ByteSource`] may be reading one far larger than a
/// 32-bit `usize` can index (#697).
#[derive(Debug, Clone)]
pub struct Grib2Message {
    /// Zero-based index of this message within the reader — its position in
    /// the file for a scanned reader, `0` for one built by
    /// [`Grib2Reader::from_message_at`], which holds only that message.
    pub message_index: usize,
    /// Byte offset of the start of this message ("GRIB" magic) within the file.
    pub byte_offset: u64,
    /// Parsed Indicator Section (Section 0).
    pub is: IndicatorSection,
    /// Parsed Identification Section (Section 1) — required in every message.
    pub ids: IdentificationSection,
    /// Byte range of the Local Use Section (Section 2) within the file, if
    /// present. The section is optional per WMO spec.
    pub lus_range: Option<ByteRange>,
    /// Parsed Grid Definition Section (Section 3) — required by spec.
    pub gds: GridDefinitionSection,
    /// Parsed Product Definition Section (Section 4) — required by spec.
    pub pds: ProductDefinitionSection,
    /// Parsed Data Representation Section (Section 5) — required by spec.
    pub drs: DataRepresentationSection,
    /// Byte range of the Bit-Map Section (Section 6) within the file.
    /// Required by spec; presence of an inline bitmap is signalled by §6's
    /// own indicator byte (0=inline, 255=none).
    pub bms_range: ByteRange,
    /// Byte range of the Data Section (Section 7) within the file.
    pub ds_range: ByteRange,
}

/// Whether this grid stores meridians rather than parallels — §3 Flag Table
/// 3.4 bit 3, `jPointsAreConsecutive`.
///
/// A grid template that states no scanning mode at all — spherical harmonics
/// and bi-Fourier, whose coefficients are not a grid — is row-major by default,
/// which is also the answer that leaves every other layout alone. (HEALPix does
/// state one; it is the absent `dimensions()` that keeps its pixel list out of
/// both callers.) Mirrors `fieldglass_grib1::reader`'s namesake.
fn j_consecutive(gds: &GridDefinitionSection) -> bool {
    gds.scanning_mode()
        .is_some_and(|sm| sm & SCAN_J_CONSECUTIVE != 0)
}

/// Refuse a grid that alternates its rows *and* stores columns.
///
/// §3 Flag Table 3.4 bit 3 makes the stored run a column, so the reversal bit 4
/// asks for is not a contiguous slice of the decoded field and the row flips
/// below — uniform or ragged, both of which assume the run is a row — would
/// silently scramble it. Called only where one of them is about to be applied,
/// so a grid with no rows to reorder is not caught by it.
///
/// **Refused rather than unrepresentable.** Since #602 the pieces to decode it
/// exist: eccodes' `pointer_to_data` computes
/// `out[j][i] = data[(j odd ? nx-1-i : i)·ny + j]`, which is
/// [`fieldglass_core::transpose_j_consecutive`] followed by
/// [`undo_alternate_rows`] — the two calls this file already makes, in that
/// order. Composing them would mean moving the alternate-row undo out of
/// [`Grib2Reader::decode_message_values`] and into
/// [`Grib2Reader::decode_message_raster`], which changes what the stored-order
/// method returns for every alternate-row message in the corpus. No message
/// setting both bits is known in the wild, and no oracle for one exists here,
/// so the combination stays refused until one turns up to check against. The
/// alternative (returning the field with every other row backwards, which is
/// what the napi layer used to do) is the one outcome a caller cannot detect.
fn reject_j_consecutive(scanning_mode: u8) -> Result<(), FieldglassError> {
    if scanning_mode & SCAN_J_CONSECUTIVE == 0 {
        return Ok(());
    }
    Err(FieldglassError::UnsupportedSection(format!(
        "scanning mode {scanning_mode} sets both alternate-row scanning (§3 Flag Table 3.4 \
         bit 4) and j-consecutive point order (bit 3); this decoder regularises each on its \
         own but not the two together"
    )))
}

/// Which rasterless family a message is, and what its arm of the resolve seam
/// needs — see [`Grib2Reader::synthesis_family`].
///
/// Private: the *grid* is the public answer ([`Grib2Reader::synthesis_grid`]),
/// and a host that could match on the family would be holding the rule this
/// exists to keep in one place.
#[derive(Debug, Clone, Copy)]
enum Synthesis {
    /// §3.50 spherical-harmonic, carrying the truncation its grid comes from.
    Spectral(u32),
    /// §3.150 HEALPix, carrying the template the resample reads `Nside` and the
    /// ordering off.
    Healpix(HealpixTemplate),
}

/// A decoded `grid_simple_matrix` field (template 5.1, `matrixBitmapsPresent =
/// 1`): an `NR × NC` matrix at every grid point. `values` is `ni·nj·nr·nc` long,
/// grid-point major (scan order) with each point's `nr·nc` matrix cells stored
/// consecutively; `None` marks a bitmap-masked cell or absent grid point.
/// Mirrors GRIB1's `MatrixField`.
#[derive(Debug, Clone, PartialEq)]
pub struct MatrixField {
    /// Grid width `Ni`.
    pub ni: usize,
    /// Grid height `Nj`.
    pub nj: usize,
    /// First matrix dimension.
    pub nr: usize,
    /// Second matrix dimension.
    pub nc: usize,
    /// Flattened matrix values — see the type docs for the layout.
    pub values: Vec<Option<f64>>,
}

/// Top-level reader for a GRIB2 file: where its bytes come from, and a
/// per-message metadata vector populated by scanning them.
///
/// The bytes are read through a [`ByteSource`] ([ADR-0005], #697), so the same
/// reader runs over a buffer, over ranges a host has already fetched, or over
/// a transport. `S` defaults to `Vec<u8>`, which is what
/// [`from_bytes`](Self::from_bytes) builds and what every caller that holds the
/// whole file already has. Mirrors `fieldglass_grib1::Grib1Reader`.
///
/// Every decode entry point resolves the sections it reads — §7, and §6 where
/// the bitmap matters — prefetches them in one batch, and reads exactly those.
/// The scan that finds the messages reads in growing windows rather than a byte
/// at a time; see [`from_source`](Self::from_source).
///
/// [ADR-0005]: https://github.com/D0ubleD0uble/fieldglass/blob/master/docs/decisions/0005-byte-access-and-the-remote-seam.md
// No `Clone` and no `PartialEq`, deliberately (#556). By default this owns
// the whole file buffer, so a derived `Clone` would duplicate it silently at a
// call site that reads like a cheap copy, and two readers over identical bytes
// are not a comparison anyone needs to make. It is a handle, not a value.
#[derive(Debug)]
pub struct Grib2Reader<S = Vec<u8>> {
    source: S,
    /// Every message in the file, in the order they appear in it.
    pub messages: Vec<Grib2Message>,
}

impl Grib2Reader<Vec<u8>> {
    /// Parse a GRIB2 file from raw bytes, scanning for all messages by
    /// walking IS total-length offsets. See [`from_source`](Self::from_source).
    pub fn from_bytes(data: Vec<u8>) -> Result<Self, FieldglassError> {
        Self::from_source(data)
    }
}

impl<S: ByteSource> Grib2Reader<S> {
    /// Scan `source` for every GRIB2 message in it, walking IS total-length
    /// offsets. Mirrors the GRIB1 reader's boundary-walking shape.
    ///
    /// Anything that is not the start of an edition-2 message — leading
    /// garbage, a GRIB1 message, the letters `GRIB` inside a payload — is
    /// stepped over one byte at a time, exactly as a buffer scan would. The
    /// bytes behind that search come in growing windows (see
    /// [`fieldglass_core::bytes::find_forward`]), so skipping costs a handful
    /// of reads rather than one per byte. Each message found then costs a few
    /// more: its indicator, its trailing `7777`, and a window over §1–§5 and
    /// the §6 and §7 headers. §6's bitmap and §7's data are not read.
    ///
    /// Discovery is still a chain — message `N + 1` starts where message `N`'s
    /// length says — which is ADR-0005's reason GRIB answers "which bytes?"
    /// with a sidecar index rather than a scan. For that case see
    /// [`from_message_at`](Self::from_message_at).
    pub fn from_source(source: S) -> Result<Self, FieldglassError> {
        let messages = scan_messages(&source)?;
        Ok(Self { source, messages })
    }

    /// Read the one message whose `GRIB` magic sits at `offset`, without
    /// scanning anything else.
    ///
    /// What a sidecar index makes possible: an `.idx` line says where a message
    /// begins, so a host fetches that range and decodes it here, and the source
    /// need hold nothing but the message's own bytes. The result is a reader of
    /// exactly one message, at index `0`, whose byte offsets are still the
    /// file's — so they match the ranges the index gave.
    ///
    /// Errors, rather than searching, when `offset` is not the start of an
    /// edition-2 message.
    pub fn from_message_at(source: S, offset: u64) -> Result<Self, FieldglassError> {
        let message = read_message_at(&source, offset, 0)?;
        Ok(Self {
            source,
            messages: vec![message],
        })
    }

    /// Where the reader's bytes come from.
    pub fn source(&self) -> &S {
        &self.source
    }

    /// How many messages the scan found. Message indices run `0..this`.
    pub fn message_count(&self) -> usize {
        self.messages.len()
    }

    /// Prefetch `ranges` as one batch, then read the first of them back.
    ///
    /// The shape every entry point below takes, so a remote source is asked once
    /// per decode rather than once per section: the rest of the batch is read by
    /// the caller with [`read_exact`] after this returns.
    fn prefetch_then_read(&self, ranges: &[ByteRange]) -> Result<Cow<'_, [u8]>, FieldglassError> {
        self.source.prefetch(ranges)?;
        read_exact(&self.source, ranges[0])
    }

    /// Decode the grid values for one message, mirroring the GRIB1 reader's
    /// API. Returns one entry per stored data point: `Some(value)` for present
    /// points, `None` for points masked out by the §6 bitmap or substituted
    /// as missing by §5 missing-value management.
    ///
    /// The values come back **in the layout the message stores them**, with one
    /// exception named below. For a regular grid that is `Ni·Nj` in row-major
    /// order, unless §3 Flag Table 3.4 bit 3 (`jPointsAreConsecutive`) is set,
    /// in which case the message stores columns and `values[i·nj + j]` is the
    /// point at `(i, j)`. For a reduced grid it is `sum(PL)` values, row by
    /// row, which is *not* the `Ni·Nj`
    /// [`GridDefinitionSection::dimensions`] reports, and for HEALPix it is
    /// `12·Nside²` pixels with no rows at all. Use
    /// [`Self::decode_message_raster`] for the rectangle; use this when the
    /// stored field itself is what you want (a statistic, a re-encode, a point
    /// count checked against eccodes).
    ///
    /// **The exception is Flag Table 3.4 bit 4** (alternate rows), which is
    /// undone here rather than in the raster method — see the comment at its
    /// call site for why, and `eccodes_reference.rs` for the storage order the
    /// value cross-check has to put back before comparing. Bit 3 is *not*
    /// undone here; it is what [`Self::decode_message_raster`] resolves.
    ///
    /// Currently supports DRS templates 5.0 (simple packing), 5.2 / 5.3
    /// (complex packing, with and without spatial differencing — both
    /// splitting methods, inline missing-value management 0/1/2), 5.4 (IEEE
    /// floating point), 5.40 (JPEG 2000 packing), 5.41 (PNG packing), 5.42
    /// (CCSDS / AEC packing), 5.61 (simple packing with logarithmic
    /// pre-processing), and 5.200 (run-length packing). Other packing templates
    /// return [`FieldglassError::UnsupportedSection`].
    pub fn decode_message_values(
        &self,
        message_index: usize,
    ) -> Result<Vec<Option<f64>>, FieldglassError> {
        let msg = self
            .messages
            .get(message_index)
            .ok_or_else(|| FieldglassError::out_of_range(message_index, self.messages.len()))?;

        // Spherical-harmonic messages carry coefficients, not a grid, so they
        // have no dimensions and decode through `decode_spectral_message`.
        if msg.gds.spherical_harmonic().is_some() {
            return Err(FieldglassError::UnsupportedSection(
                "message holds spherical-harmonic coefficients (§3.50), which are not values \
                 on a grid — decode them with `Grib2Reader::decode_spectral_message`, or \
                 `Grib2Reader::synthesize_spectral_global` to run the inverse transform \
                 and get a lat/lon field"
                    .to_string(),
            ));
        }

        // Bi-Fourier messages likewise carry coefficients, not a grid.
        if msg.gds.bifourier().is_some() {
            return Err(FieldglassError::UnsupportedSection(
                "message holds bi-Fourier spectral coefficients (§3.61/62/63), which are not \
                 values on a grid — decode them with `Grib2Reader::decode_bifourier_message`"
                    .to_string(),
            ));
        }

        // How many values the grid layout says the message holds. Rasters get
        // it from `ni × nj`; HEALPix has no raster shape but states `Nside`,
        // and `12·Nside²` is the same fact by a different route. Either way it
        // is cross-checked against §3's own count below, so a template that
        // disagrees with its section is refused rather than decoded.
        let (shape, expected_count, raster_dims) = match msg.gds.template {
            GridTemplate::Healpix(t) => {
                let npix = usize::try_from(t.npix()).map_err(|_| {
                    FieldglassError::Parse(format!(
                        "HEALPix Nside {} implies more points than this platform can address",
                        t.nside
                    ))
                })?;
                // No rows to alternate: HEALPix is one list of pixels, so the
                // boustrophedonic undo below has nothing to act on.
                (format!("HEALPix Nside {}", t.nside), npix, None)
            }
            // A reduced grid stores `sum(PL)` points, not `Ni·Nj` — its rows
            // differ in width, so `dimensions()` reports the raster they expand
            // into rather than a shape the message contains. Size the decode to
            // the storage layout; widening to that raster happens in
            // `decode_message_raster` below, which is the one boundary that
            // does it, in both format crates (#543).
            _ if msg.gds.points_per_row().is_some() => {
                let pl = msg.gds.points_per_row().expect("just matched");
                let stored = pl.iter().map(|&n| n as usize).sum::<usize>();
                // The raster is what a consumer allocates after expanding, and
                // it can be far larger than the stored field: one wide row
                // among a million narrow ones is a small file describing a
                // 10^14-point rectangle. Cap it here, where the shape is known,
                // rather than at the allocation.
                let width = fieldglass_core::reduced_raster_width(pl) as usize;
                let raster = width.checked_mul(pl.len()).ok_or_else(|| {
                    FieldglassError::Parse(format!(
                        "reduced grid raster {width}×{} overflows usize",
                        pl.len()
                    ))
                })?;
                if raster > MAX_GRID_POINTS {
                    return Err(FieldglassError::Parse(format!(
                        "reduced grid expands to a raster of {raster} points, \
                         exceeding the cap of {MAX_GRID_POINTS}"
                    )));
                }
                // No single column count: the rows are of differing width, so
                // the boustrophedonic undo below has nothing uniform to act on.
                (format!("reduced grid of {} rows", pl.len()), stored, None)
            }
            _ => {
                let (ni, nj) = msg.gds.dimensions().ok_or_else(|| {
                    FieldglassError::Parse("grid template has no declared dimensions".to_string())
                })?;
                // checked_mul guards 32-bit usize overflow; MAX_GRID_POINTS
                // guards OOM.
                let count = (ni as usize).checked_mul(nj as usize).ok_or_else(|| {
                    FieldglassError::Parse(format!("grid dimensions {ni}×{nj} overflow usize"))
                })?;
                (format!("grid dimensions {ni}×{nj}"), count, Some((ni, nj)))
            }
        };
        if expected_count > MAX_GRID_POINTS {
            return Err(FieldglassError::Parse(format!(
                "{shape} = {expected_count} points exceeds cap of {MAX_GRID_POINTS}"
            )));
        }
        // The grid geometry (ni×nj) must agree with the point count the GDS
        // declares for itself (§3 octets 7–10). A mismatch means the grid
        // template and the section's own count disagree — a malformed message.
        // Without this, a corrupted ni/nj can name a hundred-million-point grid
        // (still under MAX_GRID_POINTS) whose constant-field decode then
        // allocates gigabytes, even though the file carries no such data — an
        // OOM found by the decode fuzz target. A reduced grid checks the same
        // fact about its own layout: `sum(PL)` must be the count §3 declares,
        // which is what makes the `PL` list trustworthy enough to expand by.
        if expected_count != msg.gds.num_data_points as usize {
            return Err(FieldglassError::Parse(format!(
                "{shape} = {expected_count} points disagree with the \
                 GDS-declared {} data points",
                msg.gds.num_data_points
            )));
        }

        // §6 BMS — decode the bitmap once (or skip it when indicator == 255).
        // §6 and §7 are one batch, so a remote source is asked once for the
        // message rather than once per section.
        let bms_bytes = self.prefetch_then_read(&[msg.bms_range, msg.ds_range])?;
        let bms_header = parse_section_header(&bms_bytes)?;
        let bms = parse_bit_map_with_header(&bms_bytes, bms_header, expected_count)?;
        let bitmap = if bms.has_inline_bitmap() {
            Some(bms.bitmap.as_slice())
        } else {
            None
        };

        // §7 DS — strip the section header, hand the packed bytes to the
        // packing decoder selected by §5.
        let ds_bytes = read_exact(&self.source, msg.ds_range)?;
        let ds_header = parse_section_header(&ds_bytes)?;
        let ds_payload = parse_data_section_body(&ds_bytes, ds_header)?;
        // The DRS template is small for every packing except run-length, whose
        // level table is heap-allocated; `decode_message_values` runs once per
        // message render (not per point), so the clone is not on any hot path.
        let mut values =
            decode_values(ds_payload, msg.drs.template.clone(), bitmap, expected_count)?;
        // The runs the message stored, which is what both boustrophedonic
        // undos below step by — decided once here rather than re-derived by
        // each. `None` is the shape with no rows at all: HEALPix is one list of
        // pixels, so there is nothing to reorder and nothing to refuse.
        let stored_runs = match (msg.gds.points_per_row(), raster_dims) {
            (Some(pl), _) => Some(StoredRuns::Ragged(pl)),
            (None, Some((ni, nj))) => Some(StoredRuns::Uniform(if j_consecutive(&msg.gds) {
                nj as usize
            } else {
                ni as usize
            })),
            (None, None) => None,
        };
        // Second-order packing (template 5.50002) may store alternate runs
        // backwards; undo it now that the grid shape is known. A no-op for
        // every other template and for 5.50001, and inapplicable to a grid with
        // no rows.
        //
        // The run is a *stored* run, not a parallel: under j-consecutive
        // scanning (§3 Flag Table 3.4 bit 3) the message stores meridians, so
        // the run is `Nj` points long. eccodes 2.34.1 agrees — setting the bit
        // on `second_order_boust_regular_latlon.grib2` (16×31) and diffing
        // `grib_get_data` leaves its odd 16-runs un-reversed and reverses its
        // odd 31-runs instead, which is what
        // `decode_j_consecutive.rs` pins (#602).
        //
        // A reduced grid's runs are its `PL` rows, and it gets the ragged undo:
        // second-order packing is ECMWF's and ECMWF's operational grids are
        // reduced Gaussian, so skipping it there — which is what `raster_dims`
        // being `None` used to do — left the commonest combination of the two
        // with every second row backwards (#605). `PL` wins over the
        // j-consecutive flag because a reduced grid's stored runs are its rows
        // either way; eccodes' `DataApplyBoustrophedonic` branches on the `pl`
        // key alone and never consults `numberOfColumns` when it is there.
        if let Some(runs) = stored_runs {
            undo_second_order_boustrophedonic(&mut values, &msg.drs.template, runs);
        }
        // §3 Flag Table 3.4 bit 4 — adjacent rows scan in opposite directions.
        // The packing stores points in scan order, so every second row lands
        // column-reversed; a caller addressing the field as `values[j·ni + i]`
        // (which is what a raster is) needs them flipped back. This is the same
        // fixup eccodes applies in `transform_iterator_data` when it builds the
        // (lat, lon, value) triples `grib_get_data` prints — note it keys the
        // parity off the *storage* row, as this does. eccodes' `values` key
        // does not do it (that is the separate, opt-in
        // `swapScanningAlternativeRows`), so a fixture's snapshot samples are
        // in storage order and the eccodes value cross-check undoes this again
        // before comparing.
        //
        // It composes with the second-order undo above rather than replacing
        // it: 5.50002's `boustrophedonicOrdering` is a property of the packing
        // and this is a property of the grid, and eccodes applies both, in this
        // order.
        if let Some(sm) = msg.gds.scanning_mode()
            && sm & SCAN_ALTERNATE_ROWS != 0
        {
            // A reduced grid is flipped by its own row widths, on the stored
            // field — reversing a row after expansion is not the same
            // operation, because expansion maps columns by longitude. Both
            // shapes are the `stored_runs` the second-order undo above already
            // resolved, so the layout is decided once.
            if let Some(runs) = stored_runs {
                reject_j_consecutive(sm)?;
                match runs {
                    StoredRuns::Uniform(ni) => undo_alternate_rows(&mut values, ni),
                    StoredRuns::Ragged(pl) => undo_alternate_reduced_rows(&mut values, pl),
                }
            }
        }
        Ok(values)
    }

    /// Decode one message onto the raster
    /// [`GridDefinitionSection::dimensions`] describes: `ni · nj` values in
    /// row-major order, ready to index as `raster[j·ni + i]`.
    ///
    /// The same values [`Self::decode_message_values`] returns for every grid
    /// whose rows are all `Ni` wide and stored west-to-east *first*. Two
    /// layouts are regularised here, at the decode boundary, so a consumer
    /// never has to know which one it was handed:
    ///
    /// - **Reduced grids.** Each stored row is widened to `max(PL)`
    ///   ([`fieldglass_core::expand_reduced_to_regular`] documents how a column
    ///   is chosen — nearest by longitude, wrapping at the antimeridian). The
    ///   matching extent is [`GridDefinitionSection::raster_bounds`], whose
    ///   eastern corner is derived for the same reason.
    /// - **`j`-consecutive grids** (§3 Flag Table 3.4 bit 3). The message
    ///   stores meridians, not parallels, so the field is transposed into
    ///   `raster[j·ni + i]` by [`fieldglass_core::transpose_j_consecutive`],
    ///   which the GRIB1 reader calls for the same flag. Without this a caller
    ///   painting the stored order gets a transposed picture with no way to
    ///   tell (#602).
    ///
    /// The *directions* the scanning mode also carries are not touched, here or
    /// anywhere: row 0 is the first scanned row and column 0 the first scanned
    /// point, and the geometry says where those are. Only the storage *order*
    /// is normalised. Mirrors
    /// `fieldglass_grib1::Grib1Reader::decode_message_raster`.
    ///
    /// A layout with no raster shape comes back untouched: there is nothing to
    /// widen and nothing to widen it to. HEALPix is that case — one list of
    /// `12·Nside²` pixels, which #443 resamples onto a lat/lon grid rather than
    /// pretending it is a one-row raster.
    pub fn decode_message_raster(
        &self,
        message_index: usize,
    ) -> Result<Vec<Option<f64>>, FieldglassError> {
        let values = self.decode_message_values(message_index)?;
        // `decode_message_values` has already resolved the message and capped
        // the raster, so this can only be a grid it accepted.
        let Some(gds) = self.messages.get(message_index).map(|m| &m.gds) else {
            return Ok(values);
        };
        match (gds.points_per_row(), gds.dimensions()) {
            // The raster width is `max(PL)`, which is what `dimensions()`
            // reports for exactly these arms — so the expansion derives it from
            // `pl` rather than taking a second accessor's word for it.
            (Some(pl), _) => Ok(fieldglass_core::expand_reduced_to_regular(&values, pl)),
            // A quasi-regular grid has no columns to store, and eccodes 2.34.1
            // ignores the bit on one (its reduced geoiterator walks rows either
            // way, verified by setting `0x20` on
            // `reduced_gaussian_pressure_level.grib2` and
            // `octahedral_gaussian_o32.grib2` and diffing `grib_get_data`), so
            // only the regular arm transposes.
            (None, Some((ni, nj))) if j_consecutive(gds) => Ok(
                fieldglass_core::transpose_j_consecutive(&values, ni as usize, nj as usize),
            ),
            _ => Ok(values),
        }
    }

    /// Decode one message onto a raster to **draw**, at the resolution
    /// `options` asks for — the coarse half of the pyramid a JPEG 2000 (§5.40)
    /// message already carries (#463).
    ///
    /// At [`DecodeOptions::resolution_reduction`] zero this is
    /// [`Self::decode_message_raster`] carried beside the message's own
    /// geometry, and the values are bit-identical to it — for every layout that
    /// *has* a raster. The two that do not (spherical-harmonic and bi-Fourier
    /// coefficients, and HEALPix pixels) are refused here where
    /// `decode_message_raster` hands the stored field back untouched, because a
    /// [`DisplayRaster`] promises an `ni × nj` rectangle and those have none. Above zero it is
    /// `ceil(ni / 2^r) × ceil(nj / 2^r)` points of the codestream's wavelet
    /// low-pass, decoded *without* entropy-decoding the levels it discards —
    /// 39.9 ms → 10.5 ms → 2.8 ms on the committed RAP fixture at reduction
    /// 0 / 1 / 2, natively in release (`examples/bench_reduce.rs`).
    ///
    /// Those values are averages the message does not contain, so
    /// [`DisplayRaster`] is a type of its own and
    /// [`DisplayRaster::exact_values`] declines to hand them to anything that
    /// measures rather than draws. Its
    /// [`geometry`](DisplayRaster::geometry) is derived
    /// ([`GridGeometry::subsampled`]) and is the only geometry these values
    /// belong to: paired with the message's own GDS they would draw the field
    /// at `1/2^r` of its size.
    ///
    /// # A non-zero reduction is refused rather than approximated
    ///
    /// Every refusal below is [`FieldglassError::UnsupportedSection`], so a
    /// caller that asked for a level this message cannot serve falls back to
    /// reduction zero rather than being handed a differently-wrong field.
    ///
    /// - **Any packing but §5.40.** There is no pyramid to climb: 5.0 is
    ///   random-access already, and 5.2 / 5.3 / 5.41 / 5.42 are sequential and
    ///   would have to be decoded in full first, which is the cost this exists
    ///   to avoid. Refused rather than silently decoded in full, so that a
    ///   caller measuring a zoom-out never attributes 5.3's time to this.
    /// - **A §6 bitmap.** The bitmap is one flag per full-resolution point and
    ///   has no low-pass; there is no defensible mask for the coarse raster,
    ///   and the codestream carries only the *present* points, so its image is
    ///   not the grid.
    /// - **A reduced grid.** Its rows differ in width, so the codestream is not
    ///   the raster and the widening
    ///   ([`fieldglass_core::expand_reduced_to_regular`]) maps columns by
    ///   longitude — an operation that does not commute with a low-pass.
    /// - **Alternate-row or `j`-consecutive scanning** (§3 Flag Table 3.4 bits
    ///   3 and 4). Both are storage orders this crate normalises *after* the
    ///   packing decodes, and a wavelet transform of a scrambled raster has no
    ///   normalised form: reversing every second coarse row is not the coarse
    ///   form of reversing every second row.
    /// - **A family with no derivable coarse geometry** — Gaussian, and any
    ///   template this build does not place points on. See
    ///   [`GridGeometry::subsampled`] for why each declines.
    pub fn decode_message_raster_with(
        &self,
        message_index: usize,
        options: DecodeOptions,
    ) -> Result<DisplayRaster, FieldglassError> {
        let msg = self
            .messages
            .get(message_index)
            .ok_or_else(|| FieldglassError::out_of_range(message_index, self.messages.len()))?;
        let geometry = GridGeometry::from(&msg.gds);
        let reduction = options.resolution_reduction;

        if reduction == 0 {
            // The message's own field, so every layout `decode_message_raster`
            // regularises is served here too — including the ones a non-zero
            // reduction refuses.
            let values = self.decode_message_raster(message_index)?;
            let (ni, nj) = msg.gds.dimensions().ok_or_else(|| {
                FieldglassError::UnsupportedSection(format!(
                    "a {} message is not laid out as a raster, so it has none to display — \
                     decode it with `Grib2Reader::decode_message_values`",
                    msg.gds.template_name()
                ))
            })?;
            return DisplayRaster::new(values, ni, nj, geometry, 0);
        }

        // The raster question comes first, and it is the reason: a message with
        // no rectangle at all has no coarse one either, whatever its packing,
        // and reporting the packing there would name the one thing that could
        // change rather than the one that cannot.
        let (ni, nj) = msg.gds.dimensions().ok_or_else(|| {
            FieldglassError::UnsupportedSection(format!(
                "a {} message is not laid out as a raster, so it has no coarse one either — \
                 decode it with `Grib2Reader::decode_message_values`",
                msg.gds.template_name()
            ))
        })?;
        let Some(jpeg) = msg.drs.jpeg2000() else {
            return Err(FieldglassError::UnsupportedSection(format!(
                "resolution reduction {reduction} was asked of {} packing, and only JPEG 2000 \
                 (§5.40) carries the resolution pyramid a reduced decode reads",
                msg.drs.template_name()
            )));
        };
        if msg.gds.points_per_row().is_some() {
            return Err(FieldglassError::UnsupportedSection(format!(
                "resolution reduction {reduction} was asked of a reduced grid, whose rows differ \
                 in width — the codestream is not the raster the rows expand into"
            )));
        }
        if let Some(sm) = msg.gds.scanning_mode() {
            if sm & SCAN_ALTERNATE_ROWS != 0 {
                return Err(FieldglassError::UnsupportedSection(format!(
                    "resolution reduction {reduction} was asked of an alternate-row grid (§3 Flag \
                     Table 3.4 bit 4), whose rows are undone after the packing decodes — a \
                     wavelet low-pass of that order has no undo"
                )));
            }
            if sm & SCAN_J_CONSECUTIVE != 0 {
                return Err(FieldglassError::UnsupportedSection(format!(
                    "resolution reduction {reduction} was asked of a j-consecutive grid (§3 Flag \
                     Table 3.4 bit 3), which stores meridians — the codestream is the transpose \
                     of the raster"
                )));
            }
        }
        // The same two guards `decode_message_values` applies before it sizes
        // anything, restated because this path does not go through it: the
        // product must fit and stay under the cap, and §3's own point count
        // must agree with the shape the template declares. Without the second,
        // a corrupted `ni`/`nj` names a grid the file has no data for.
        let full_count = (ni as usize).checked_mul(nj as usize).ok_or_else(|| {
            FieldglassError::Parse(format!("grid dimensions {ni}×{nj} overflow usize"))
        })?;
        if full_count > MAX_GRID_POINTS {
            return Err(FieldglassError::Parse(format!(
                "grid dimensions {ni}×{nj} = {full_count} points exceeds cap of {MAX_GRID_POINTS}"
            )));
        }
        if full_count != msg.gds.num_data_points as usize {
            return Err(FieldglassError::Parse(format!(
                "grid dimensions {ni}×{nj} = {full_count} points disagree with the \
                 GDS-declared {} data points",
                msg.gds.num_data_points
            )));
        }

        // §6's indicator byte, and nothing else. `parse_bit_map_with_header`
        // would answer the same question, but it materialises a `bool` per grid
        // point on the way — up to `MAX_GRID_POINTS` of them, from a §6 payload
        // an eighth that size — and this refuses the message on the next line,
        // so the whole allocation would be an eight-fold amplification on input
        // guaranteed to be declined.
        //
        // Every indicator but "none" is refused, not only the inline one:
        // 1..=254 name a bitmap held somewhere else (a predefined table, the
        // previous message), which `decode_message_values` declines outright and
        // which is no more reducible than an inline one.
        //
        // **A bounded §6, and §7 still in the batch.** The read wants the section
        // header and the indicator octet — `BMS_INDICATOR_OFFSET + 1` bytes — and
        // used to fetch the whole of §6, which for a message that *does* carry a
        // bitmap is one flag per full-resolution point: megabytes downloaded to
        // read one byte and refuse (#709).
        //
        // §7 stays in the same batch deliberately, and that is the trade the
        // issue asks to be stated rather than fixed. Asking §6 first and §7 only
        // on success would spare the data section on the refusal path and cost a
        // second round trip on the *success* path, which is the common one. One
        // batch of a six-byte prefix plus §7 is strictly better than the two
        // whole sections this used to fetch, and no worse than the split when the
        // request is granted.
        let bms_prefix = ByteRange::new(
            msg.bms_range.start,
            msg.bms_range.len.min(BMS_INDICATOR_OFFSET as u64 + 1),
        );
        let bms = self.prefetch_then_read(&[bms_prefix, msg.ds_range])?;
        let bms_header = parse_section_header(&bms)?;
        if bms_header.number != BMS_SECTION_NUMBER {
            return Err(FieldglassError::Parse(format!(
                "expected BMS (section {BMS_SECTION_NUMBER}), got section {}",
                bms_header.number
            )));
        }
        let indicator = *bms.get(BMS_INDICATOR_OFFSET).ok_or_else(|| {
            FieldglassError::Parse(format!(
                "BMS is {} bytes, too short to carry its indicator",
                bms.len()
            ))
        })?;
        if indicator != BMS_INDICATOR_NONE {
            return Err(FieldglassError::UnsupportedSection(format!(
                "resolution reduction {reduction} was asked of a message whose §6 declares a \
                 bitmap (indicator {indicator}), which is one flag per full-resolution point and \
                 has no low-pass"
            )));
        }

        // The geometry decides the coarse shape, and the codestream is then
        // held to it: one answer, not two that could disagree.
        let coarse = geometry.subsampled(reduction).ok_or_else(|| {
            FieldglassError::UnsupportedSection(format!(
                "resolution reduction {reduction} has no derivable geometry on a {} grid",
                geometry.label()
            ))
        })?;
        let (coarse_ni, coarse_nj) = coarse
            .dims()
            .expect("a geometry `subsampled` answered for has dimensions");

        let ds_bytes = read_exact(&self.source, msg.ds_range)?;
        let ds_header = parse_section_header(&ds_bytes)?;
        let ds_payload = parse_data_section_body(&ds_bytes, ds_header)?;
        let values = decode_jpeg2000_reduced(ds_payload, jpeg, reduction, coarse_ni, coarse_nj)?;
        DisplayRaster::new(values, coarse_ni, coarse_nj, coarse, reduction)
    }

    /// Decode a `grid_simple_matrix` message (template 5.1) that carries an
    /// `NR × NC` matrix at every grid point (`matrixBitmapsPresent = 1`).
    ///
    /// Returns a [`MatrixField`] whose `values` is `Ni·Nj·NR·NC` long, grid-point
    /// major with each point's `NR·NC` matrix cells consecutive; `None` marks a
    /// bitmap-masked cell or absent grid point. Use
    /// [`decode_message_values`](Self::decode_message_values) for the flat
    /// `matrixBitmapsPresent = 0` form — this errors on it. eccodes cannot decode
    /// this variant (it crashes), so it follows the GRIBEX interpretation the
    /// GRIB1 matrix path uses; see [`crate::matrix`].
    pub fn decode_matrix_message(
        &self,
        message_index: usize,
    ) -> Result<MatrixField, FieldglassError> {
        let msg = self
            .messages
            .get(message_index)
            .ok_or_else(|| FieldglassError::out_of_range(message_index, self.messages.len()))?;
        let t = msg.drs.matrix_simple().ok_or_else(|| {
            FieldglassError::UnsupportedSection(format!(
                "message {message_index} uses §5 packing {}, not grid_simple_matrix (5.1)",
                msg.drs.template_name()
            ))
        })?;
        if t.matrix_bitmaps_present == 0 {
            return Err(FieldglassError::UnsupportedSection(
                "grid_simple_matrix message has matrixBitmapsPresent = 0 (a scalar field); \
                 decode it with `decode_message_values`, not as a matrix."
                    .to_string(),
            ));
        }

        let (ni, nj) = msg.gds.dimensions().ok_or_else(|| {
            FieldglassError::Parse("matrix message has no declared grid dimensions".to_string())
        })?;
        let expected_count = (ni as usize).checked_mul(nj as usize).ok_or_else(|| {
            FieldglassError::Parse(format!("grid dimensions {ni}×{nj} overflow usize"))
        })?;
        if expected_count > MAX_GRID_POINTS {
            return Err(FieldglassError::Parse(format!(
                "grid {ni}×{nj} = {expected_count} points exceeds cap of {MAX_GRID_POINTS}"
            )));
        }
        // Keep the ni×nj geometry and the GDS-declared point count in agreement,
        // like the scalar path — a corrupted ni/nj naming a huge grid unbacked by
        // data is rejected here rather than driving a large allocation.
        if expected_count != msg.gds.num_data_points as usize {
            return Err(FieldglassError::Parse(format!(
                "grid dimensions {ni}×{nj} = {expected_count} points disagree with the \
                 GDS-declared {} data points",
                msg.gds.num_data_points
            )));
        }

        let bms_bytes = self.prefetch_then_read(&[msg.bms_range, msg.ds_range])?;
        let bms_header = parse_section_header(&bms_bytes)?;
        let bms = parse_bit_map_with_header(&bms_bytes, bms_header, expected_count)?;
        let bitmap = if bms.has_inline_bitmap() {
            Some(bms.bitmap.as_slice())
        } else {
            None
        };

        let ds_bytes = read_exact(&self.source, msg.ds_range)?;
        let ds_header = parse_section_header(&ds_bytes)?;
        let ds_payload = parse_data_section_body(&ds_bytes, ds_header)?;
        let values = crate::matrix::decode_matrix_of_values(ds_payload, t, bitmap, expected_count)?;
        Ok(MatrixField {
            ni: ni as usize,
            nj: nj as usize,
            nr: t.nr as usize,
            nc: t.nc as usize,
            values,
        })
    }

    /// Decode a spherical-harmonic message (§3.50 + §5.50) into its spectral
    /// coefficients.
    ///
    /// A spectral message stores the field in wavenumber space, not on a grid,
    /// so it has no `Ni`/`Nj` and cannot go through
    /// [`Grib2Reader::decode_message_values`]. What you get here is what
    /// eccodes' `grib_get_data` prints for the same message; to turn the
    /// coefficients back into a grid, run the inverse transform with
    /// [`Grib2Reader::synthesize_spectral_global`]. Errors if the message is not
    /// spherical-harmonic, or its §5 packing is not one the spectral decoder
    /// supports (`spectral_simple` / 5.50 and `spectral_complex` / 5.51).
    pub fn decode_spectral_message(
        &self,
        message_index: usize,
    ) -> Result<SpectralCoefficients, FieldglassError> {
        let msg = self
            .messages
            .get(message_index)
            .ok_or_else(|| FieldglassError::out_of_range(message_index, self.messages.len()))?;

        let sh = msg.gds.spherical_harmonic().ok_or_else(|| {
            FieldglassError::UnsupportedSection(format!(
                "message {message_index} is a {} grid, not spherical-harmonic coefficients — \
                 use `decode_message_values`",
                msg.gds.template_name()
            ))
        })?;

        let ds_bytes = self.prefetch_then_read(&[msg.ds_range])?;
        let ds_header = parse_section_header(&ds_bytes)?;
        let ds_payload = parse_data_section_body(&ds_bytes, ds_header)?;

        if let Some(t) = msg.drs.spectral_simple() {
            decode_spectral_simple(ds_payload, t, sh.j, sh.k, sh.m)
        } else if let Some(t) = msg.drs.spectral_complex() {
            decode_spectral_complex(ds_payload, t, sh.j, sh.k, sh.m)
        } else {
            Err(FieldglassError::UnsupportedSection(format!(
                "spherical-harmonic message {message_index} uses §5 packing {} — only \
                 spectral_simple (5.50) and spectral_complex (5.51) decode today",
                msg.drs.template_name()
            )))
        }
    }

    /// Synthesize a spherical-harmonic message onto a regular lat/lon grid via
    /// the inverse spherical-harmonic transform.
    ///
    /// Decodes the coefficients (see
    /// [`decode_spectral_message`](Self::decode_spectral_message)) and evaluates
    /// the field at every `(latitude, longitude)` in `latitudes_deg` ×
    /// `longitudes_deg`, returning `latitudes_deg.len() · longitudes_deg.len()`
    /// values in latitude-major scan order. This is the transform no other tool
    /// in the ecosystem performs, letting a spectral message be turned back into
    /// a grid for rendering. The numerics are validated against ECMWF's
    /// definitive spectral definition (see [`fieldglass_core::sht`]).
    ///
    /// Choosing the grid is a separate question from evaluating the field on
    /// it, and every host has answered it the same way: use
    /// [`synthesize_spectral_global`](Self::synthesize_spectral_global) unless
    /// you want a grid of your own.
    pub fn synthesize_spectral_message(
        &self,
        message_index: usize,
        latitudes_deg: &[f64],
        longitudes_deg: &[f64],
    ) -> Result<Vec<f64>, FieldglassError> {
        let coeffs = self.decode_spectral_message(message_index)?;
        fieldglass_core::sht::synthesize_spherical_harmonic(
            &coeffs.coefficients,
            coeffs.j,
            latitudes_deg,
            longitudes_deg,
        )
    }

    /// Synthesize a spherical-harmonic message onto the global lat/lon grid
    /// [`fieldglass_core::sht::spectral_render_grid`] chooses for its
    /// truncation, and hand that grid back with it.
    ///
    /// The convention — pole-to-pole latitudes, longitudes `0 … 360 − Δ` with no
    /// duplicated wrap column, at the 0.5° pin — lives once, in
    /// [`fieldglass_core::global_grid`]. Getting the wrap column wrong doubles
    /// the field at the antimeridian, so pairing the values with the grid they
    /// were evaluated on is what this call is for: a host that builds its render
    /// geometry from the returned [`GlobalGrid`] cannot declare one shape and
    /// evaluate at another.
    ///
    /// Errors exactly where
    /// [`synthesize_spectral_message`](Self::synthesize_spectral_message) does —
    /// the message is not spherical-harmonic, or its coefficients do not decode.
    pub fn synthesize_spectral_global(
        &self,
        message_index: usize,
    ) -> Result<(GlobalGrid, Vec<f64>), FieldglassError> {
        let coeffs = self.decode_spectral_message(message_index)?;
        let grid = fieldglass_core::sht::spectral_render_grid(coeffs.j);
        let (lats, lons) = grid.axes();
        let values = fieldglass_core::sht::synthesize_spherical_harmonic(
            &coeffs.coefficients,
            coeffs.j,
            &lats,
            &lons,
        )?;
        Ok((grid, values))
    }

    /// The global lat/lon grid a message with no raster of its own would be
    /// synthesised onto, read from the grid definition alone.
    ///
    /// `None` for a message that already carries a raster — the ordinary case —
    /// and for an index this file does not hold. Two families answer: §3.50
    /// spherical-harmonic, whose grid comes from its truncation, and §3.150
    /// HEALPix, whose grid comes from its `Nside`. §3.61/62/63 bi-Fourier is
    /// rasterless too and is deliberately **not** here: recovering its grid
    /// needs an inverse bi-Fourier transform this build does not have, so it
    /// stays a refusal rather than becoming a wrong picture.
    ///
    /// Cheap on purpose: it reads §3 and decodes nothing, so a host with a
    /// geometry-only path (an overlay projection, a message list) can ask where
    /// the field will land without paying for the transform or the resample.
    #[must_use]
    pub fn synthesis_grid(&self, message_index: usize) -> Option<GlobalGrid> {
        Some(match self.synthesis_family(message_index)? {
            Synthesis::Spectral(truncation) => {
                fieldglass_core::sht::spectral_render_grid(truncation)
            }
            Synthesis::Healpix(t) => fieldglass_core::healpix::healpix_render_grid(t.nside),
        })
    }

    /// Which rasterless family a message is, if any, and what each arm needs to
    /// do its work.
    ///
    /// **The family list, written once.** Both public halves of the resolve
    /// seam read this, so a family added to one and not the other is
    /// unrepresentable rather than caught by an assertion: `synthesis_grid`
    /// cannot answer a grid that [`synthesize_message_global`] then declines to
    /// fill, which is the one failure a host cannot see — it would size its
    /// render meta from a grid nothing produced. The GRIB1 seam gets the same
    /// property from having a single arm.
    ///
    /// [`synthesize_message_global`]: Self::synthesize_message_global
    fn synthesis_family(&self, message_index: usize) -> Option<Synthesis> {
        let msg = self.messages.get(message_index)?;
        if let Some(sh) = msg.gds.spherical_harmonic() {
            return Some(Synthesis::Spectral(sh.j));
        }
        match msg.gds.template {
            GridTemplate::Healpix(t) => Some(Synthesis::Healpix(t)),
            _ => None,
        }
    }

    /// Synthesize a message that carries no raster of its own onto
    /// [`synthesis_grid`](Self::synthesis_grid)'s grid, or `Ok(None)` when the
    /// message has a raster and [`decode_message_raster`] is the call to make.
    ///
    /// This is the seam every host resolves a message through: asking it first
    /// and falling through on `None` puts the whole rasterless family — which
    /// ones they are, and what grid each lands on — in this crate rather than
    /// in each host (#580). The values come back in the same
    /// `Vec<Option<f64>>` shape [`decode_message_raster`] uses, so the caller
    /// substitutes one for the other and changes nothing else.
    ///
    /// The two families reach their grid by different routes and mean different
    /// things by it: a spectral field is band-limited, so the grid *evaluates*
    /// it exactly, while HEALPix is sampled, so this is a genuine resample (see
    /// [`fieldglass_core::global_grid`]).
    ///
    /// # Errors
    ///
    /// Where [`synthesize_spectral_global`](Self::synthesize_spectral_global)
    /// does for a spectral message, and where
    /// [`decode_message_values`](Self::decode_message_values) does for a
    /// HEALPix one. A message that is not a synthesis family cannot fail here
    /// at all: it answers `Ok(None)` before anything is decoded.
    ///
    /// The pixel-count mismatch [`fieldglass_core::healpix::resample_to_global`]
    /// can report is **not** reachable through this call, and is reported
    /// rather than unwrapped only so the arm is total:
    /// [`decode_message_values`](Self::decode_message_values) already
    /// cross-checks `12·Nside²` against §3's own point count, and §3.150 parsing
    /// refuses `Nside = 0` and a non-power-of-two `Nside` under NESTED.
    ///
    /// [`decode_message_raster`]: Self::decode_message_raster
    pub fn synthesize_message_global(
        &self,
        message_index: usize,
    ) -> Result<Option<SynthesisedField>, FieldglassError> {
        // The same list `synthesis_grid` reads, so the two cannot name
        // different families and this has no residual arm to decline in.
        let Some(family) = self.synthesis_family(message_index) else {
            return Ok(None);
        };
        let (grid, values) = match family {
            Synthesis::Spectral(_) => {
                let (grid, values) = self.synthesize_spectral_global(message_index)?;
                (grid, values.into_iter().map(Some).collect())
            }
            Synthesis::Healpix(t) => {
                let pixels = self.decode_message_values(message_index)?;
                fieldglass_core::healpix::resample_to_global(t.nside, t.nested, &pixels)
                    .ok_or_else(|| {
                        FieldglassError::Parse(format!(
                            "HEALPix field has {} values, not the 12*{}^2 its geometry declares",
                            pixels.len(),
                            t.nside
                        ))
                    })?
            }
        };
        // Two derivations of the same grid: this one from the transform or the
        // resample, `synthesis_grid`'s from the GDS a host sizes its meta from
        // without decoding. Neither can disagree today — the truncation and
        // `Nside` are read off the same template both times — so this is here
        // for the day a grid rule stops being a pure function of the GDS.
        debug_assert_eq!(
            Some(grid),
            self.synthesis_grid(message_index),
            "the synthesised grid disagrees with the one read from the GDS"
        );
        Ok(Some((grid, values)))
    }

    /// Decode a bi-Fourier message (§3.61/62/63 + §5.53) into its spectral
    /// coefficients.
    ///
    /// Like [`decode_spectral_message`](Self::decode_spectral_message), a
    /// bi-Fourier message stores the field as spectral coefficients (four per
    /// `(i, j)` wavenumber pair), not on a grid, so it has no `Ni`/`Nj` and
    /// cannot go through [`decode_message_values`](Self::decode_message_values).
    /// Recovering a grid needs an inverse bi-Fourier transform, which is not
    /// implemented yet; what you get here is what eccodes' `grib_get_data`
    /// prints for the same message. Errors if the message is not bi-Fourier, or
    /// its §5 packing is not `bifourier_complex` (template 5.53).
    pub fn decode_bifourier_message(
        &self,
        message_index: usize,
    ) -> Result<BiFourierCoefficients, FieldglassError> {
        let msg = self
            .messages
            .get(message_index)
            .ok_or_else(|| FieldglassError::out_of_range(message_index, self.messages.len()))?;

        let bf = msg.gds.bifourier().ok_or_else(|| {
            FieldglassError::UnsupportedSection(format!(
                "message {message_index} is a {} grid, not bi-Fourier coefficients — \
                 use `decode_message_values`",
                msg.gds.template_name()
            ))
        })?;
        let drs = msg.drs.bifourier().ok_or_else(|| {
            FieldglassError::UnsupportedSection(format!(
                "bi-Fourier message {message_index} uses §5 packing {} — only \
                 bifourier_complex (5.53) decodes here",
                msg.drs.template_name()
            ))
        })?;

        let ds_bytes = self.prefetch_then_read(&[msg.ds_range])?;
        let ds_header = parse_section_header(&ds_bytes)?;
        let ds_payload = parse_data_section_body(&ds_bytes, ds_header)?;

        decode_bifourier(ds_payload, drs, bf, msg.drs.num_data_points as usize)
    }
}

/// Whether a 16-byte window is the Indicator Section of an edition-2 message.
///
/// The edition is part of the match, not a check after it: a GRIB1 message
/// sharing the same magic shouldn't be a hard error here, just stepped past
/// one byte at a time exactly as garbage is.
fn is_edition_2_indicator(window: &[u8]) -> bool {
    &window[..4] == b"GRIB" && window[7] == GRIB2_EDITION
}

fn scan_messages<S: ByteSource + ?Sized>(source: &S) -> Result<Vec<Grib2Message>, FieldglassError> {
    let mut messages = Vec::new();
    let mut from = 0u64;
    while let Some(offset) =
        find_forward(source, from, INDICATOR_SECTION_LEN, is_edition_2_indicator)?
    {
        let message = read_message_at(source, offset, messages.len())?;
        // `read_message_at` has checked that the message ends inside the
        // source, so this cannot overflow.
        from = offset + message.is.total_length;
        messages.push(message);
    }
    Ok(messages)
}

/// The common 5-byte header of the section at the cursor.
///
/// Handed at most the bytes left in the message, which is what the header
/// parser counts in its error when there are fewer than five.
fn section_header<S: ByteSource + ?Sized>(
    cursor: &mut FileCursor<'_, S>,
) -> Result<SectionHeader, FieldglassError> {
    parse_section_header(cursor.peek_up_to(SECTION_HEADER_LEN)?)
}

/// The section at the cursor, as its parser is handed it: its declared length
/// of bytes, or every byte left in the message when it declares more.
///
/// Each `parse_*_with_header` checks the declared length against what it was
/// given and reports the count it got, so this reproduces exactly what slicing
/// a whole-file buffer to the end of the message did.
fn section_bytes<'c, S: ByteSource + ?Sized>(
    cursor: &'c mut FileCursor<'_, S>,
    header: SectionHeader,
) -> Result<&'c [u8], FieldglassError> {
    cursor.peek_up_to(header.length as usize)
}

/// Parse the one message whose `GRIB` magic is at `offset`, reading its
/// indicator, its trailing `7777`, §1–§5, and the §6 and §7 headers — none of
/// the bitmap and none of the data.
fn read_message_at<S: ByteSource + ?Sized>(
    source: &S,
    offset: u64,
    message_index: usize,
) -> Result<Grib2Message, FieldglassError> {
    let size = source.size();
    // Checks the magic and the edition itself, so an offset handed in directly
    // rather than found by the scan is held to both.
    let is = parse_indicator(&read_up_to(source, offset, INDICATOR_SECTION_LEN)?)?;

    if is.total_length < INDICATOR_SECTION_LEN as u64 + END_SECTION_LEN as u64 {
        return Err(FieldglassError::Parse(format!(
            "Message at offset {offset} declares an impossibly small length {}",
            is.total_length
        )));
    }

    // `total_length` is an attacker-controlled u64; a value near u64::MAX
    // would overflow `offset + total_length`. checked_add turns that into
    // the same "claims more than the source holds" error as a merely-too-big
    // length, instead of a panic under overflow checks.
    let msg_end = match offset.checked_add(is.total_length) {
        Some(end) if end <= size => end,
        _ => {
            return Err(FieldglassError::Parse(format!(
                "Message at offset {offset} claims length {} but only {} bytes remain",
                is.total_length,
                size.saturating_sub(offset)
            )));
        }
    };

    // The "impossibly small length" guard above already implies
    // `msg_end >= offset + END_SECTION_LEN >= END_SECTION_LEN`, but assert it
    // locally so the subtraction below can't underflow even if that guard is
    // ever loosened.
    if msg_end < END_SECTION_LEN as u64 {
        return Err(FieldglassError::Parse(format!(
            "Message at offset {offset} ends before its trailing 7777 marker"
        )));
    }

    // Trailing 4-byte End Section "7777".
    if &*read_at(source, msg_end - END_SECTION_LEN as u64, END_SECTION_LEN)? != b"7777" {
        return Err(FieldglassError::Parse(format!(
            "Message at offset {offset} is missing trailing 7777 marker"
        )));
    }

    // §1 IDS — always immediately follows §0. The earlier "impossibly
    // small length" guard ensures at least END_SECTION_LEN bytes follow
    // the IS, so a malformed-but-non-empty section header here will
    // surface from parse_section_header with a coherent error.
    //
    // One cursor walks the sections from here, bounded to this message. It
    // parses §1–§5 out of its window and skips §6's and §7's bodies, which
    // are only recorded — so finding the bitmap and the data costs nothing.
    let ids_offset = offset + INDICATOR_SECTION_LEN as u64;
    let mut cursor = FileCursor::within(source, ids_offset, msg_end)?;
    let ids_header = section_header(&mut cursor)?;
    if ids_header.number != IDS_SECTION_NUMBER {
        return Err(FieldglassError::Parse(format!(
            "Message at offset {offset}: expected IDS (section {IDS_SECTION_NUMBER}) \
             immediately after IS, got section {}",
            ids_header.number
        )));
    }
    let ids =
        parse_identification_with_header(section_bytes(&mut cursor, ids_header)?, ids_header)?;
    // Every `parse_*_with_header` below has held its declared length to the
    // bytes left in the message, so each skip stays inside it.
    cursor.skip(ids_header.length as usize)?;

    // §2 LUS is optional; peek the next header and consume it only if it
    // claims to be section 2. Anything else (typically §3 GDS) is left
    // for the GDS step below.
    let lus_range = {
        let next = section_header(&mut cursor)?;
        if next.number == LUS_SECTION_NUMBER {
            let lus = parse_local_use_with_header(section_bytes(&mut cursor, next)?, next)?;
            let range = ByteRange::new(cursor.position(), u64::from(lus.section_length));
            cursor.skip(lus.section_length as usize)?;
            Some(range)
        } else {
            None
        }
    };

    // §3 GDS — required by the WMO spec in every message.
    let gds_header = section_header(&mut cursor)?;
    if gds_header.number != GDS_SECTION_NUMBER {
        return Err(FieldglassError::Parse(format!(
            "Message at offset {offset}: expected GDS (section {GDS_SECTION_NUMBER}), \
             got section {}",
            gds_header.number
        )));
    }
    let gds =
        parse_grid_definition_with_header(section_bytes(&mut cursor, gds_header)?, gds_header)?;
    cursor.skip(gds_header.length as usize)?;

    // §4 PDS — required by the WMO spec in every message.
    let pds_header = section_header(&mut cursor)?;
    if pds_header.number != PDS_SECTION_NUMBER {
        return Err(FieldglassError::Parse(format!(
            "Message at offset {offset}: expected PDS (section {PDS_SECTION_NUMBER}), \
             got section {}",
            pds_header.number
        )));
    }
    let pds =
        parse_product_definition_with_header(section_bytes(&mut cursor, pds_header)?, pds_header)?;
    cursor.skip(pds_header.length as usize)?;

    // §5 DRS — required by the WMO spec in every message.
    let drs_header = section_header(&mut cursor)?;
    if drs_header.number != DRS_SECTION_NUMBER {
        return Err(FieldglassError::Parse(format!(
            "Message at offset {offset}: expected DRS (section {DRS_SECTION_NUMBER}), \
             got section {}",
            drs_header.number
        )));
    }
    let drs =
        parse_data_representation_with_header(section_bytes(&mut cursor, drs_header)?, drs_header)?;
    cursor.skip(drs_header.length as usize)?;

    // §6 BMS — required by spec (its "indicator" byte signals
    // bitmap-present vs no-bitmap; we just record the byte range here
    // and defer body parsing to decode time).
    let bms_header = section_header(&mut cursor)?;
    if bms_header.number != BMS_SECTION_NUMBER {
        return Err(FieldglassError::Parse(format!(
            "Message at offset {offset}: expected BMS (section {BMS_SECTION_NUMBER}), \
             got section {}",
            bms_header.number
        )));
    }
    // The other sections' parsers validate their declared length against
    // the bytes available; BMS/DS are recorded lazily, so do it here.
    // Without this an oversized BMS length pushes the cursor past the end of
    // the message, and an oversized DS length records a range that over-reads
    // the source at decode time.
    if u64::from(bms_header.length) > cursor.remaining() {
        return Err(FieldglassError::Parse(format!(
            "Message at offset {offset}: BMS declares length {} but only {} bytes remain",
            bms_header.length,
            cursor.remaining()
        )));
    }
    let bms_range = ByteRange::new(cursor.position(), u64::from(bms_header.length));
    cursor.skip(bms_header.length as usize)?;

    // §7 DS — required by spec. Same lazy treatment as §6: record the
    // byte range, decode on demand.
    let ds_header = section_header(&mut cursor)?;
    if ds_header.number != DS_SECTION_NUMBER {
        return Err(FieldglassError::Parse(format!(
            "Message at offset {offset}: expected DS (section {DS_SECTION_NUMBER}), \
             got section {}",
            ds_header.number
        )));
    }
    if u64::from(ds_header.length) > cursor.remaining() {
        return Err(FieldglassError::Parse(format!(
            "Message at offset {offset}: DS declares length {} but only {} bytes remain",
            ds_header.length,
            cursor.remaining()
        )));
    }
    let ds_range = ByteRange::new(cursor.position(), u64::from(ds_header.length));

    Ok(Grib2Message {
        message_index,
        byte_offset: offset,
        is,
        ids,
        lus_range,
        gds,
        pds,
        drs,
        bms_range,
        ds_range,
    })
}
