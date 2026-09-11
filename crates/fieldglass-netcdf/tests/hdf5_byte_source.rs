//! The HDF5 reader decodes through the `ByteSource` seam (#682, ADR-0005).
//!
//! The classic reader got this first (#438), and `classic_byte_source.rs` can
//! assert the strong property for it: the plan is exactly what the decode
//! reads, because every offset is in the header. HDF5 cannot make that claim —
//! ADR-0005 says so in as many words, because its traversal is a chain of
//! dependent reads and the address of the next structure lives inside the
//! current one.
//!
//! So what is checked here is the property HDF5 *can* hold, and it is the one
//! that decides whether a byte-range transport is usable at all:
//!
//! * the decode goes through the seam rather than around it — a source that is
//!   not a slice decodes identically, and one that lies about its length is
//!   caught rather than silently truncating the variable;
//! * the chunk fetch, which is the one place a real plan exists, is **one**
//!   prefetch batch and then reads, not a read per chunk with no batch;
//! * the traversal reads what the structure needs rather than the whole file,
//!   which is what makes the walk affordable over a transport.

use fieldglass_core::{ByteRange, ByteSource, FieldglassError};
use fieldglass_netcdf::{ChildKind, NetcdfBacking, NetcdfReader, list_root_children};
use std::borrow::Cow;
use std::cell::RefCell;

/// A version-1 B-tree chunk index, the legacy `libver=earliest` form.
const V1_SYMBOLTABLE: &[u8] = include_bytes!("fixtures/hdf5_v1_symboltable.h5");
/// A version-4 chunk index, filtered, so the deflate path is exercised too.
const EA_FILTERED: &[u8] = include_bytes!("fixtures/hdf5_ea_filtered.h5");
/// A real NetCDF-4 file: nested groups, dimension scales, dense attributes.
const GOES: &[u8] = include_bytes!("fixtures/goes16_abi_cmip.nc");

/// Records every range asked for and every prefetch batch, so a test can say
/// what a decode actually touched rather than that it succeeded.
struct Recording<'a> {
    bytes: &'a [u8],
    reads: RefCell<Vec<ByteRange>>,
    prefetches: RefCell<Vec<Vec<ByteRange>>>,
    /// How many reads had happened when the first prefetch batch arrived, so a
    /// test can separate the traversal's reads from the data reads that follow
    /// the batch.
    reads_before_batch: RefCell<Option<usize>>,
}

impl<'a> Recording<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            reads: RefCell::new(Vec::new()),
            prefetches: RefCell::new(Vec::new()),
            reads_before_batch: RefCell::new(None),
        }
    }

    fn reads(&self) -> Vec<ByteRange> {
        self.reads.borrow().clone()
    }

    fn prefetches(&self) -> Vec<Vec<ByteRange>> {
        self.prefetches.borrow().clone()
    }

    fn forget(&self) {
        self.reads.borrow_mut().clear();
        self.prefetches.borrow_mut().clear();
        *self.reads_before_batch.borrow_mut() = None;
    }

    /// The reads that came after the first prefetch batch: a variable's stored
    /// data, as opposed to the traversal that located it.
    fn reads_after_batch(&self) -> Vec<ByteRange> {
        match *self.reads_before_batch.borrow() {
            Some(mark) => self.reads.borrow()[mark..].to_vec(),
            None => Vec::new(),
        }
    }
}

impl ByteSource for Recording<'_> {
    fn size(&self) -> u64 {
        self.bytes.len() as u64
    }

    fn identity(&self) -> Option<fieldglass_core::SourceIdentity> {
        self.bytes.identity()
    }

    fn prefetch(&self, ranges: &[ByteRange]) -> Result<(), FieldglassError> {
        let mut mark = self.reads_before_batch.borrow_mut();
        if mark.is_none() {
            *mark = Some(self.reads.borrow().len());
        }
        self.prefetches.borrow_mut().push(ranges.to_vec());
        Ok(())
    }

    fn read(&self, range: ByteRange) -> Result<Cow<'_, [u8]>, FieldglassError> {
        self.reads.borrow_mut().push(range);
        self.bytes.read(range)
    }
}

/// A source that owns nothing the caller can borrow: every read copies out of
/// a cache, which is what a transport that fetched into a `RefCell` has to do
/// (see the `bytes` module doc in `fieldglass-core`). Decoding through it
/// proves the reader never depends on `Cow::Borrowed`.
struct Copying<'a> {
    bytes: &'a [u8],
}

impl ByteSource for Copying<'_> {
    fn size(&self) -> u64 {
        self.bytes.len() as u64
    }

    fn identity(&self) -> Option<fieldglass_core::SourceIdentity> {
        // A transport names what it fetched; the point here is only that it is
        // an identity the memo can key on without being a buffer address.
        Some(fieldglass_core::SourceIdentity::named(
            "test://copying",
            self.bytes.len() as u64,
        ))
    }

    fn read(&self, range: ByteRange) -> Result<Cow<'_, [u8]>, FieldglassError> {
        Ok(Cow::Owned(self.bytes.read(range)?.into_owned()))
    }
}

/// A source that serves one byte fewer than it was asked for, once it is past
/// the header. A buffer cannot do this; a truncated HTTP response can.
struct Short<'a> {
    bytes: &'a [u8],
    /// Reads at or past this address come back one byte short.
    from: u64,
}

impl ByteSource for Short<'_> {
    fn size(&self) -> u64 {
        self.bytes.len() as u64
    }

    fn identity(&self) -> Option<fieldglass_core::SourceIdentity> {
        self.bytes.identity()
    }

    fn read(&self, range: ByteRange) -> Result<Cow<'_, [u8]>, FieldglassError> {
        let full = self.bytes.read(range)?;
        if range.start >= self.from && !full.is_empty() {
            let keep = full.len() - 1;
            return Ok(Cow::Owned(full[..keep].to_vec()));
        }
        Ok(full)
    }
}

/// Every root dataset of a fixture, decoded **through the given source**.
///
/// `read_dataset_values` rather than `NetcdfReader::decode_variable_raw`, which
/// would read the reader's own buffer and not the source at all — comparing two
/// such runs would compare a value against itself and pass however the decode
/// behaved.
fn decode_all<S: ByteSource>(bytes: &[u8], source: &S) -> Vec<Result<Vec<Option<f64>>, String>> {
    let reader = NetcdfReader::from_bytes(bytes.to_vec()).expect("recognised NetCDF");
    assert!(
        matches!(reader.backing, NetcdfBacking::Hdf5(_)),
        "fixture is not HDF5, so this proves nothing"
    );
    let addresses: Vec<u64> = list_root_children(source, probe(&reader))
        .expect("list children")
        .into_iter()
        .filter(|c| c.kind == ChildKind::Dataset)
        .map(|c| c.object_header_address)
        .collect();
    assert!(
        !addresses.is_empty(),
        "fixture has no datasets, so this proves nothing"
    );
    addresses
        .into_iter()
        .map(|addr| {
            fieldglass_netcdf::hdf5::values::read_dataset_values(source, addr, probe(&reader))
                .map_err(|e| e.to_string())
        })
        .collect()
}

fn probe(reader: &NetcdfReader) -> &fieldglass_netcdf::Hdf5Probe {
    match &reader.backing {
        NetcdfBacking::Hdf5(p) => p,
        other => panic!("expected an HDF5 backing, got {}", other.label()),
    }
}

/// A source that hands out copies rather than slices decodes byte-for-byte what
/// the in-memory path does.
///
/// This is the shape every transport has to take — `read` takes `&self`, so a
/// source that fetched during `prefetch` caches behind interior mutability and
/// cannot lend a reference out of the guard. If any HDF5 reader had come to
/// depend on the bytes outliving the call, it would fail here and nowhere else.
#[test]
fn a_source_that_cannot_lend_decodes_identically() {
    for (label, bytes) in [
        ("v1 symbol table", V1_SYMBOLTABLE),
        ("filtered extensible array", EA_FILTERED),
        ("goes16", GOES),
    ] {
        let direct = decode_all(bytes, &bytes);
        let copied = decode_all(bytes, &Copying { bytes });
        assert_eq!(
            direct, copied,
            "{label}: decode differs through a copying source"
        );
        // Two empty lists are equal too. The comparison is only worth making
        // if something actually decoded to values.
        let decoded: usize = direct
            .iter()
            .filter_map(|r| r.as_ref().ok())
            .filter(|v| !v.is_empty())
            .count();
        assert!(
            decoded > 0,
            "{label}: nothing decoded to any values, so the comparison is vacuous"
        );
    }
}

/// A variable's stored bytes are resolved in one batch and then read, and
/// nothing outside that batch is read afterwards.
///
/// This is the property a byte-range transport depends on. Without it the chunk
/// loop would issue one request per chunk; with an *incomplete* batch it would
/// issue the batch and then a request per chunk the batch missed, which is
/// worse than either. So the check is not that a prefetch happened but that it
/// named exactly what the decode went on to read.
///
/// One batch *per variable*, which is the unit a caller decodes in. The
/// traversal that finds the chunk addresses issues none, deliberately:
/// ADR-0005 records that HDF5 fails the strong form of the constraint, so there
/// is nothing to resolve until the chunk index has been walked.
///
/// Some variables have **no** batch, and that is not a gap: a compact dataset
/// carries its values inside the object header, and a dataset whose storage was
/// never allocated has no bytes on disk at all. Neither has anything to fetch.
#[test]
fn a_variable_is_one_batch_and_then_only_what_it_named() {
    for (label, bytes) in [
        ("v1 symbol table", V1_SYMBOLTABLE),
        ("filtered extensible array", EA_FILTERED),
        ("goes16", GOES),
    ] {
        let source = Recording::new(bytes);
        let reader = NetcdfReader::from_bytes(bytes.to_vec()).expect("recognised NetCDF");
        let datasets: Vec<u64> = list_root_children(&source, probe(&reader))
            .expect("list children")
            .into_iter()
            .filter(|c| c.kind == ChildKind::Dataset)
            .map(|c| c.object_header_address)
            .collect();
        assert!(!datasets.is_empty(), "{label}: no datasets");

        let mut batched = 0usize;
        for addr in datasets {
            source.forget();
            if fieldglass_netcdf::hdf5::values::read_dataset_values(&source, addr, probe(&reader))
                .is_err()
            {
                // A string dataset is a clean refusal, not a read pattern.
                continue;
            }
            let batches = source.prefetches();
            assert!(
                batches.len() <= 1,
                "{label}: dataset at {addr} issued {} prefetch batches, expected at most one",
                batches.len()
            );
            let Some(plan) = batches.first() else {
                continue;
            };
            batched += 1;
            assert!(
                !plan.is_empty(),
                "{label}: dataset at {addr} prefetched an empty plan"
            );

            // Everything read after the batch is the variable's stored data,
            // and every one of those ranges has to be in the plan.
            let after = source.reads_after_batch();
            for range in &after {
                assert!(
                    plan.contains(range),
                    "{label}: dataset at {addr} read {range:?} after the batch without naming \
                     it — a transport would issue an extra request for it"
                );
            }
            // And everything planned is read, so the batch is not fetching
            // bytes nobody wants.
            for range in plan {
                assert!(
                    after.contains(range),
                    "{label}: dataset at {addr} prefetched {range:?} and never read it"
                );
            }
        }
        assert!(
            batched > 0,
            "{label}: no variable was planned, so nothing was proved"
        );
    }
}

/// The traversal reads the structures it needs, not the file, and the windowing
/// does not turn that into a re-read of the object.
///
/// Two numbers, because one alone is misleading. *Distinct* bytes touched says
/// how much of the file the walk genuinely depends on — for a small, metadata-
/// dense NetCDF-4 file that is a large fraction, and honestly so. *Total* bytes
/// read against it says what the cursor's window costs on top, which is the
/// part this crate controls: a window that read ahead too eagerly shows up here
/// as a multiple, and did — a fixed 4 KiB window read six times the whole file
/// before it was made to grow from a small one instead.
#[test]
fn the_walk_reads_what_it_needs_and_not_much_more() {
    for (label, bytes) in [
        ("goes16", GOES),
        (
            "netcdf4 dimension scales",
            &include_bytes!("fixtures/netcdf4_dimscale.nc")[..],
        ),
        // The legacy symbol-table path, which the other two do not take. It is
        // the one that reads link names out of a local heap, and the only thing
        // that would catch that read growing a fixed large window.
        ("v1 symbol table", V1_SYMBOLTABLE),
    ] {
        let source = Recording::new(bytes);
        let reader = NetcdfReader::from_bytes(bytes.to_vec()).expect("recognised NetCDF");
        let meta = fieldglass_netcdf::resolve_hdf5_metadata(&source, probe(&reader))
            .expect("metadata resolves");
        assert!(
            !meta.variables.is_empty(),
            "{label}: no variables, so the walk proved nothing"
        );

        let reads = source.reads();
        let total: u64 = reads.iter().map(|r| r.len).sum();
        let mut touched = vec![false; bytes.len()];
        for range in &reads {
            for byte in &mut touched[range.start as usize..(range.start + range.len) as usize] {
                *byte = true;
            }
        }
        let distinct = touched.iter().filter(|&&t| t).count() as u64;

        assert!(
            distinct < bytes.len() as u64,
            "{label}: the walk touched the whole file ({distinct} bytes)"
        );
        assert!(
            total < 2 * distinct,
            "{label}: read {total} bytes to reach {distinct} distinct ones — the window is \
             reading ahead further than the structures justify"
        );
    }
}

/// A source that serves short is caught, rather than yielding a variable that
/// looks complete and is not.
///
/// The in-memory path cannot produce this, which is exactly why it needs a
/// test: the guard exists only for the transports that do not exist yet, so
/// nothing else would notice it being removed.
#[test]
fn a_source_that_serves_short_is_an_error_not_a_short_variable() {
    let reader = NetcdfReader::from_bytes(V1_SYMBOLTABLE.to_vec()).expect("recognised NetCDF");
    let children = list_root_children(&V1_SYMBOLTABLE, probe(&reader)).expect("list children");
    let datasets: Vec<_> = children
        .iter()
        .filter(|c| c.kind == ChildKind::Dataset)
        .collect();
    assert!(!datasets.is_empty(), "fixture has no datasets");

    let mut caught = 0usize;
    for child in datasets {
        // A fresh probe per dataset: the memo binds to the first source it is
        // used with, and the short source is a different one.
        let probe = fieldglass_netcdf::hdf5::probe(&V1_SYMBOLTABLE).expect("superblock");
        let full = fieldglass_netcdf::hdf5::values::read_dataset_values(
            &V1_SYMBOLTABLE,
            child.object_header_address,
            &probe,
        );
        let Ok(full) = full else { continue };
        if full.is_empty() {
            continue;
        }
        let probe = fieldglass_netcdf::hdf5::probe(&V1_SYMBOLTABLE).expect("superblock");
        let short = Short {
            bytes: V1_SYMBOLTABLE,
            // Past every header: only data and index reads are shortened.
            from: 512,
        };
        let got = fieldglass_netcdf::hdf5::values::read_dataset_values(
            &short,
            child.object_header_address,
            &probe,
        );
        match got {
            Err(_) => caught += 1,
            Ok(values) => assert_eq!(
                values, full,
                "{}: a short source produced a different variable without erroring",
                child.name
            ),
        }
    }
    assert!(
        caught > 0,
        "no dataset noticed the short source, so the guard is untested"
    );
}
