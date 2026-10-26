//! A contiguous HDF5 dataset that declares more data than the file holds is
//! refused before its buffer is built (#997).
//!
//! `hdf5_phony_dims.h5` (provenance in the NetCDF fixtures' `NOTICE.md`) holds
//! `c_4x6`, a contiguous 4 × 6 float32. The test rewrites its dataspace to
//! 8,192 × 8,191, just under the 64 Mi-value field cap, and re-checksums the
//! version-2 object header that holds it. The file is still a few KB, so the
//! data cannot be there. The reader used to build the region's run list and
//! a 256 MiB fill buffer before its first read found that out; it now checks
//! the region's end against the file first.
//!
//! A tracking global allocator records the peak of live bytes allocated on
//! this thread while it is armed, so tests on other threads cannot add to it.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use fieldglass::netcdf::NetcdfReader;
use fieldglass::{DecodeOptions, Session};

const PHONY_DIMS: &[u8] =
    include_bytes!("../../fieldglass-netcdf/tests/fixtures/hdf5_phony_dims.h5");

struct Tracking;

thread_local! {
    static ARMED: Cell<bool> = const { Cell::new(false) };
    static LIVE: Cell<isize> = const { Cell::new(0) };
    static PEAK: Cell<isize> = const { Cell::new(0) };
}

fn note(delta: isize) {
    // `try_with`: the allocator also runs while thread-locals are torn down.
    let _ = ARMED.try_with(|armed| {
        if armed.get() {
            let _ = LIVE.try_with(|live| {
                live.set(live.get() + delta);
                let _ = PEAK.try_with(|peak| peak.set(peak.get().max(live.get())));
            });
        }
    });
}

// SAFETY: every method forwards to `System` unchanged; tracking touches only
// const-initialized thread-locals, which never allocate.
unsafe impl GlobalAlloc for Tracking {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note(layout.size() as isize);
        // SAFETY: the caller's contract for `alloc`, passed through.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note(layout.size() as isize);
        // SAFETY: as above.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note(new_size as isize - layout.size() as isize);
        // SAFETY: as above.
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        note(-(layout.size() as isize));
        // SAFETY: as above.
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static ALLOCATOR: Tracking = Tracking;

/// The most bytes live at once, counted from zero when `f` starts, while `f`
/// runs on this thread.
fn peak_while<R>(f: impl FnOnce() -> R) -> (isize, R) {
    LIVE.with(|l| l.set(0));
    PEAK.with(|p| p.set(0));
    ARMED.with(|a| a.set(true));
    let result = f();
    ARMED.with(|a| a.set(false));
    (PEAK.with(Cell::get), result)
}

/// What the refusal may hold at once: the header and index reads it makes,
/// nowhere near the 256 MiB the declared dataset would take.
const BUDGET: isize = 4 << 20;

/// The Jenkins lookup3 hash HDF5 checksums metadata with (`H5_checksum_lookup3`,
/// initial value 0).
fn lookup3(data: &[u8]) -> u32 {
    fn rot(x: u32, k: u32) -> u32 {
        x.rotate_left(k)
    }
    let mut a = 0xdeadbeef_u32.wrapping_add(data.len() as u32);
    let (mut b, mut c) = (a, a);
    let word = |s: &[u8]| {
        let mut w = [0u8; 4];
        w[..s.len()].copy_from_slice(s);
        u32::from_le_bytes(w)
    };
    let mut k = data;
    while k.len() > 12 {
        a = a.wrapping_add(word(&k[0..4]));
        b = b.wrapping_add(word(&k[4..8]));
        c = c.wrapping_add(word(&k[8..12]));
        a = a.wrapping_sub(c);
        a ^= rot(c, 4);
        c = c.wrapping_add(b);
        b = b.wrapping_sub(a);
        b ^= rot(a, 6);
        a = a.wrapping_add(c);
        c = c.wrapping_sub(b);
        c ^= rot(b, 8);
        b = b.wrapping_add(a);
        a = a.wrapping_sub(c);
        a ^= rot(c, 16);
        c = c.wrapping_add(b);
        b = b.wrapping_sub(a);
        b ^= rot(a, 19);
        a = a.wrapping_add(c);
        c = c.wrapping_sub(b);
        c ^= rot(b, 4);
        b = b.wrapping_add(a);
        k = &k[12..];
    }
    if k.is_empty() {
        return c;
    }
    let tail = |s: &[u8]| {
        if s.is_empty() {
            0
        } else {
            word(&s[..s.len().min(4)])
        }
    };
    a = a.wrapping_add(tail(k));
    b = b.wrapping_add(tail(k.get(4..).unwrap_or(&[])));
    c = c.wrapping_add(tail(k.get(8..).unwrap_or(&[])));
    c ^= b;
    c = c.wrapping_sub(rot(b, 14));
    a ^= c;
    a = a.wrapping_sub(rot(c, 11));
    b ^= a;
    b = b.wrapping_sub(rot(a, 25));
    c ^= b;
    c = c.wrapping_sub(rot(b, 16));
    a ^= c;
    a = a.wrapping_sub(rot(c, 4));
    b ^= a;
    b = b.wrapping_sub(rot(a, 14));
    c ^= b;
    c = c.wrapping_sub(rot(b, 24));
    c
}

/// `hdf5_phony_dims.h5` with `c_4x6` declaring 8,192 × 8,191 values.
fn declares_more_than_it_holds() -> Vec<u8> {
    let mut file = PHONY_DIMS.to_vec();
    // The dataspace's current and maximum dimensions, eight bytes each.
    let dims: Vec<u8> = [4u64, 6, 4, 6]
        .iter()
        .flat_map(|d| d.to_le_bytes())
        .collect();
    let at: Vec<usize> = file
        .windows(dims.len())
        .enumerate()
        .filter(|(_, w)| *w == dims.as_slice())
        .map(|(i, _)| i)
        .collect();
    assert_eq!(at.len(), 1, "one 4 × 6 dataspace");
    let at = at[0];
    let wide: Vec<u8> = [8192u64, 8191, 8192, 8191]
        .iter()
        .flat_map(|d| d.to_le_bytes())
        .collect();
    file[at..at + wide.len()].copy_from_slice(&wide);

    // Chunk 0 of the version-2 header holding it: signature, version, flags,
    // optional times (16) and attribute phase change (4), a 1-8 byte chunk
    // size, the messages, then a checksum over everything before it.
    let header = file[..at]
        .windows(4)
        .rposition(|w| w == b"OHDR")
        .expect("the dataset's object header");
    let flags = file[header + 5];
    let mut pos = header + 6;
    if flags & 0x20 != 0 {
        pos += 16;
    }
    if flags & 0x10 != 0 {
        pos += 4;
    }
    let width = 1usize << (flags & 0x03);
    let mut size = [0u8; 8];
    size[..width].copy_from_slice(&file[pos..pos + width]);
    let end = pos + width + u64::from_le_bytes(size) as usize;
    assert!(end > at, "the dataspace is in chunk 0");
    let stored = u32::from_le_bytes(file[end..end + 4].try_into().unwrap());
    assert_eq!(
        lookup3(&PHONY_DIMS[header..end]),
        stored,
        "chunk 0's checksum"
    );
    let sum = lookup3(&file[header..end]);
    file[end..end + 4].copy_from_slice(&sum.to_le_bytes());
    file
}

#[test]
fn the_reader_refuses_it_before_building_the_buffer() {
    let file = declares_more_than_it_holds();
    let reader = NetcdfReader::from_bytes(file).expect("the file opens");
    let index = (0..)
        .take_while(|&i| reader.variable_shape(i).is_ok())
        .find(|&i| reader.variable_shape(i).unwrap() == [8192, 8191])
        .expect("c_4x6, widened");

    let (peak, outcome) = peak_while(|| reader.decode_variable_raw(index).map(drop));
    let err = outcome.expect_err("the data is not in the file");
    assert!(peak < BUDGET, "the refusal held {peak} bytes at once");
    assert!(err.to_string().contains("past the file size"), "{err}");
}

#[test]
fn a_session_refuses_it_before_building_the_buffer() {
    let session = Session::open(declares_more_than_it_holds()).expect("the file opens");
    let index = session
        .variables()
        .iter()
        .position(|v| v.name == "c_4x6")
        .expect("c_4x6 is offered") as u32;

    let (peak, outcome) = peak_while(|| {
        session
            .decode_slice(index, 0, 1, &[0, 0], &DecodeOptions::default())
            .map(drop)
    });
    let err = outcome.expect_err("the data is not in the file");
    assert!(peak < BUDGET, "the refusal held {peak} bytes at once");
    assert!(err.to_string().contains("past the file size"), "{err}");
}
