//! NetCDF classic decodes through the `ByteSource` seam (#438, ADR-0005).
//!
//! ADR-0005 asks decoders to record the addresses they discover so a fetch plan
//! can be replayed without re-traversing, and credits NetCDF classic with the
//! *strong* form of that: every offset is in the header, so a complete plan
//! exists before a data byte is touched.
//!
//! This file is where that claim is checked rather than asserted. The plan must
//! be exactly what the decode reads — not a superset, which would make a remote
//! transport fetch bytes nobody wants, and not a subset, which would make it
//! miss some and fall back to a per-slab round trip.

use fieldglass_core::{ByteRange, ByteSource, FieldglassError};
use fieldglass_netcdf::classic::{
    decode_variable_values, decode_variable_values_from, parse_header, variable_plan,
};
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::HashMap;

/// A classic file with a record (unlimited) variable, so the multi-range plan is
/// exercised and not just the contiguous one.
const WRF: &[u8] = include_bytes!("fixtures/wrf_lambert.nc");
/// A classic file with only fixed variables.
const ERSST: &[u8] = include_bytes!("fixtures/ersst_v5_187001_cdf1.nc");

/// Records every range asked for, and every prefetch batch, so a test can say
/// what a decode actually touched rather than that it succeeded.
struct Recording<'a> {
    bytes: &'a [u8],
    reads: RefCell<Vec<ByteRange>>,
    prefetches: RefCell<Vec<Vec<ByteRange>>>,
}

impl<'a> Recording<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            reads: RefCell::new(Vec::new()),
            prefetches: RefCell::new(Vec::new()),
        }
    }

    fn reads(&self) -> Vec<ByteRange> {
        self.reads.borrow().clone()
    }

    fn prefetches(&self) -> Vec<Vec<ByteRange>> {
        self.prefetches.borrow().clone()
    }
}

impl ByteSource for Recording<'_> {
    fn size(&self) -> u64 {
        self.bytes.len() as u64
    }

    fn prefetch(&self, ranges: &[ByteRange]) -> Result<(), FieldglassError> {
        self.prefetches.borrow_mut().push(ranges.to_vec());
        Ok(())
    }

    fn read(&self, range: ByteRange) -> Result<Cow<'_, [u8]>, FieldglassError> {
        self.reads.borrow_mut().push(range);
        self.bytes.read(range)
    }
}

/// Every variable of both fixtures, so a plan bug in one shape cannot hide
/// behind the other.
fn each_variable(
    bytes: &[u8],
    mut check: impl FnMut(&fieldglass_netcdf::classic::ClassicHeader, usize),
) {
    let header = parse_header(bytes).expect("classic header");
    assert!(
        !header.variables.is_empty(),
        "fixture has no variables, so this proves nothing"
    );
    for index in 0..header.variables.len() {
        check(&header, index);
    }
}

/// The plan is exactly what the decode reads — same ranges, same order.
///
/// This is the property a remote transport depends on: fetch the plan, and the
/// decode that follows asks for nothing else.
#[test]
fn the_plan_is_exactly_what_the_decode_reads() {
    let mut checked = 0usize;
    for bytes in [WRF, ERSST] {
        each_variable(bytes, |header, index| {
            let Ok(plan) = variable_plan(header, index) else {
                return; // char/text variables have no numeric plan
            };
            let source = Recording::new(bytes);
            if decode_variable_values_from(header, &source, index).is_err() {
                return;
            }
            assert_eq!(
                source.reads(),
                plan,
                "variable {index} of a {} byte file read something its plan did not name",
                bytes.len()
            );
            checked += 1;
        });
    }
    assert!(checked > 5, "only {checked} variables were compared");
}

/// The whole plan is resolved in one batch, before any of it is read.
///
/// Prefetch-then-decode is the shape ADR-0005 fixes; a decode that prefetched
/// per slab would be the per-read design the record rejects, and would work
/// locally while being one round trip per record remotely.
#[test]
fn the_whole_plan_is_prefetched_once_before_any_read() {
    let header = parse_header(WRF).expect("classic header");
    let index = header
        .variables
        .iter()
        .position(|v| v.name == "T2")
        .expect("T2 is a record variable in this fixture");

    let source = Recording::new(WRF);
    decode_variable_values_from(&header, &source, index).expect("decode");

    let prefetches = source.prefetches();
    assert_eq!(
        prefetches.len(),
        1,
        "expected one batch, got {prefetches:?}"
    );
    assert_eq!(
        prefetches[0],
        variable_plan(&header, index).expect("plan"),
        "the batch must be the whole plan"
    );
    assert_eq!(source.reads(), prefetches[0]);
}

/// Decoding the same variable twice reads the same ranges and discovers nothing
/// new — the header was traversed once, and the plan is replayable.
#[test]
fn a_repeated_decode_re_traverses_nothing() {
    let header = parse_header(WRF).expect("classic header");
    let index = header
        .variables
        .iter()
        .position(|v| v.name == "T2")
        .expect("T2");

    let first = Recording::new(WRF);
    let a = decode_variable_values_from(&header, &first, index).expect("decode");
    let second = Recording::new(WRF);
    let b = decode_variable_values_from(&header, &second, index).expect("decode");

    assert_eq!(
        first.reads(),
        second.reads(),
        "the second decode read differently"
    );
    assert_eq!(a, b, "the second decode produced different values");
    // And the plan itself needs no source at all: it comes from the header.
    assert_eq!(variable_plan(&header, index).expect("plan"), first.reads());
}

/// A record variable really does produce one range per record, not one big one.
///
/// Classic interleaves each record variable's per-record slab, so a decode that
/// asked for `[begin, begin + total)` would read its neighbours' bytes and
/// return them as values. That is the bug this shape exists to make impossible,
/// so the shape is pinned.
#[test]
fn a_record_variable_plans_one_range_per_record() {
    let header = parse_header(WRF).expect("classic header");
    let numrecs = header
        .numrecs
        .expect("the fixture has an unlimited dimension");
    assert!(numrecs >= 1);
    let index = header
        .variables
        .iter()
        .position(|v| v.name == "T2")
        .expect("T2");

    let plan = variable_plan(&header, index).expect("plan");
    assert_eq!(plan.len(), numrecs as usize, "one range per record");

    // The ranges are a constant stride apart and none of them overlap — the
    // interleaving, seen from the plan.
    if plan.len() > 1 {
        let stride = plan[1].start - plan[0].start;
        assert!(
            stride >= plan[0].len,
            "records overlap: stride {stride} < len {}",
            plan[0].len
        );
        for pair in plan.windows(2) {
            assert_eq!(
                pair[1].start - pair[0].start,
                stride,
                "uneven record stride"
            );
            assert_eq!(pair[1].len, pair[0].len, "uneven record length");
        }
    }
}

/// A fixed variable is one contiguous range.
#[test]
fn a_fixed_variable_plans_one_contiguous_range() {
    let header = parse_header(ERSST).expect("classic header");
    let index = header
        .variables
        .iter()
        .position(|v| v.name == "lat")
        .expect("lat");
    let plan = variable_plan(&header, index).expect("plan");
    assert_eq!(plan.len(), 1);
    assert_eq!(plan[0].start, header.variables[index].begin);
}

/// The slice entry point and the `ByteSource` one agree, for every variable of
/// both fixtures. That is what makes the migration invisible to callers.
#[test]
fn the_slice_and_byte_source_paths_agree() {
    for bytes in [WRF, ERSST] {
        each_variable(bytes, |header, index| {
            let via_slice = decode_variable_values(header, bytes, index);
            let via_source = decode_variable_values_from(header, &bytes.to_vec(), index);
            match (via_slice, via_source) {
                (Ok(a), Ok(b)) => assert_eq!(a, b, "variable {index} decoded differently"),
                (Err(_), Err(_)) => {}
                (a, b) => panic!(
                    "variable {index}: one path succeeded and the other did not: {a:?} / {b:?}"
                ),
            }
        });
    }
}

/// A source that refuses a range fails the decode rather than returning short
/// or partial values. A remote fetch that 404s or truncates is the realistic
/// version of this.
#[test]
fn a_source_that_cannot_serve_a_range_fails_the_decode() {
    let header = parse_header(ERSST).expect("classic header");
    let index = header
        .variables
        .iter()
        .position(|v| v.name == "lat")
        .expect("lat");

    // The file, one byte short of what the plan asks for.
    let plan = variable_plan(&header, index).expect("plan");
    let truncated = &ERSST[..(plan[0].end().expect("end") as usize - 1)];
    let err = decode_variable_values_from(&header, &truncated.to_vec(), index)
        .expect_err("a short source must not decode");
    assert!(
        format!("{err}").contains("exceeds source size"),
        "unexpected error: {err}"
    );
}

/// A source shaped like the remote one that does not exist yet: it holds
/// nothing until `prefetch` fills a cache, and serves **owned** bytes out of
/// it.
///
/// This is the test the local-only mock above cannot be. Every other source
/// here is backed by a slice, so every read comes back `Cow::Borrowed`, and a
/// decode path that had quietly assumed borrowing would pass. This one cannot
/// borrow: `read(&self)` cannot hand a reference out of a `RefCell` guard, so a
/// cache-backed implementation is *forced* to clone — which is the shape any
/// HTTP-range or object-store source will have, and the reason `read` returns
/// `Cow` rather than `&[u8]`.
///
/// The cost that fixes in place: a remote source copies once per slab. Against
/// a network fetch that is nothing, and it buys the local path a plain slice.
struct Fetching<'a> {
    origin: &'a [u8],
    cache: RefCell<HashMap<(u64, u64), Vec<u8>>>,
    fetches: RefCell<usize>,
}

impl<'a> Fetching<'a> {
    fn new(origin: &'a [u8]) -> Self {
        Self {
            origin,
            cache: RefCell::new(HashMap::new()),
            fetches: RefCell::new(0),
        }
    }
}

impl ByteSource for Fetching<'_> {
    fn size(&self) -> u64 {
        self.origin.len() as u64
    }

    fn prefetch(&self, ranges: &[ByteRange]) -> Result<(), FieldglassError> {
        // One "request" per batch, which is the whole point of the batch.
        *self.fetches.borrow_mut() += 1;
        let mut cache = self.cache.borrow_mut();
        for range in ranges {
            let bytes = self.origin.read(*range)?.into_owned();
            cache.insert((range.start, range.len), bytes);
        }
        Ok(())
    }

    fn read(&self, range: ByteRange) -> Result<Cow<'_, [u8]>, FieldglassError> {
        if let Some(hit) = self.cache.borrow().get(&(range.start, range.len)) {
            return Ok(Cow::Owned(hit.clone()));
        }
        // `read` must work whether or not a range was prefetched — skipping the
        // batch costs latency, not correctness. A remote source would issue a
        // single-range request here.
        *self.fetches.borrow_mut() += 1;
        Ok(Cow::Owned(self.origin.read(range)?.into_owned()))
    }
}

/// The trait is implementable by something that owns nothing until asked, and
/// the decode is identical through it.
///
/// This is the closest thing to an answer for "will the shape survive its first
/// remote implementation" that can be written before there is one.
#[test]
fn a_cache_backed_source_that_never_borrows_decodes_identically() {
    for bytes in [WRF, ERSST] {
        each_variable(bytes, |header, index| {
            let expected = decode_variable_values(header, bytes, index);
            let source = Fetching::new(bytes);
            let got = decode_variable_values_from(header, &source, index);
            match (expected, got) {
                (Ok(a), Ok(b)) => {
                    assert_eq!(a, b, "variable {index} decoded differently through a cache");
                    // One batch for the whole variable, and nothing fetched
                    // singly afterwards: every read was a cache hit.
                    assert_eq!(
                        *source.fetches.borrow(),
                        1,
                        "variable {index} went back to the origin after prefetching"
                    );
                }
                (Err(_), Err(_)) => {}
                (a, b) => panic!("variable {index}: paths disagree: {a:?} / {b:?}"),
            }
        });
    }
}

/// `read` works without `prefetch`. The batch is advisory — a source that
/// ignores it must still be correct, and a decoder that forgot to call it must
/// still work, just slower.
#[test]
fn reads_work_without_a_prefetch() {
    let header = parse_header(ERSST).expect("classic header");
    let index = header
        .variables
        .iter()
        .position(|v| v.name == "lat")
        .expect("lat");
    let plan = variable_plan(&header, index).expect("plan");

    let source = Fetching::new(ERSST);
    // Deliberately skip the batch and read cold.
    for range in &plan {
        assert!(
            source.read(*range).is_ok(),
            "{range:?} failed without a prefetch"
        );
    }
    assert_eq!(
        *source.fetches.borrow(),
        plan.len(),
        "each read should have gone to the origin"
    );
}

/// A source that under-delivers is an error, not a short variable.
///
/// `decode_slab` does not bounds-check — the range was bounded when the source
/// served it — so a source returning fewer bytes than asked is the one way the
/// seam could silently truncate a variable where the old slice path could not.
/// An in-memory source cannot do it; a transport with a truncated response can,
/// which is exactly the case that has no test until there is a transport.
#[test]
fn a_short_serving_source_is_an_error_not_a_short_variable() {
    struct Short<'a>(&'a [u8]);
    impl ByteSource for Short<'_> {
        fn size(&self) -> u64 {
            self.0.len() as u64
        }
        fn read(&self, range: ByteRange) -> Result<Cow<'_, [u8]>, FieldglassError> {
            // Serve one element short of what was asked for.
            let shrunk = ByteRange::new(range.start, range.len.saturating_sub(4));
            self.0.read(shrunk)
        }
    }

    let header = parse_header(ERSST).expect("classic header");
    let index = header
        .variables
        .iter()
        .position(|v| v.name == "lat")
        .expect("lat");
    let full = decode_variable_values(&header, ERSST, index).expect("decode");
    assert!(
        !full.is_empty(),
        "the fixture variable is empty, so this proves nothing"
    );

    let err = decode_variable_values_from(&header, &Short(ERSST), index)
        .expect_err("a short-serving source must not produce a variable at all");
    assert!(
        format!("{err}").contains("served short"),
        "unexpected error: {err}"
    );
}

/// The plan and the decode agree about which variables have no plan at all —
/// `char` (text) variables and any shape the decode rejects.
#[test]
fn the_plan_refuses_exactly_what_the_decode_refuses() {
    for bytes in [WRF, ERSST] {
        each_variable(bytes, |header, index| {
            let plan = variable_plan(header, index);
            let decoded = decode_variable_values(header, bytes, index);
            assert_eq!(
                plan.is_ok(),
                decoded.is_ok(),
                "variable {index}: plan and decode disagree about whether it is decodable"
            );
        });
    }
}
