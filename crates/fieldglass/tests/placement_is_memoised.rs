//! A slice's placement is derived once per session, not once per call
//! (ADR-0011, #662).
//!
//! The unit tests in `session.rs` assert the memo's *keying*, which is private.
//! What a host actually feels is here, at the seam it pays through: placing the
//! same slice twice must touch the store **once**. A store's coordinate arrays
//! are objects like any other, so a re-derived placement re-fetches and
//! re-decompresses them — on a remote store, over the network.
//!
//! Asserted by counting what the session asked the store for, rather than by
//! timing anything: a wall-clock assertion on a shared runner is a machine-speed
//! assertion, and it cannot tell a memo from a fast rebuild.

use std::borrow::Cow;
use std::path::Path;
use std::rc::Rc;

use fieldglass::{DecodeOptions, Session};
use fieldglass_core::bytes::{MemoryObjects, ObjectSource};
use fieldglass_core::error::FieldglassError;
use fieldglass_core::testing::Recording;

const FIXTURES: &str = "../fieldglass-zarr/tests/fixtures";

/// A fixture store directory as the objects a host would hand over.
fn load(dir: &str) -> MemoryObjects {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        for entry in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{dir:?}: {e}")) {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                let key = path
                    .strip_prefix(root)
                    .expect("under the root")
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect::<Vec<_>>()
                    .join("/");
                out.push((key, std::fs::read(&path).expect("read")));
            }
        }
    }
    let root = Path::new(dir);
    let mut entries = Vec::new();
    walk(root, root, &mut entries);
    MemoryObjects::from_iter(entries)
}

/// The recording store, kept reachable after the session has taken it.
///
/// `Session::open_store` takes the store by value, so a test that wants to read
/// the counters afterwards has to hand over a handle rather than the recorder
/// itself. Otherwise transparent: every method forwards, including the
/// `list_children` the wrapped store may override.
struct Shared(Rc<Recording<MemoryObjects>>);

impl ObjectSource for Shared {
    fn get(&self, key: &str) -> Result<Option<Cow<'_, [u8]>>, FieldglassError> {
        self.0
            .get(key)
            .map(|v| v.map(|v| Cow::Owned(v.into_owned())))
    }

    fn list(&self, prefix: &str) -> Result<Vec<String>, FieldglassError> {
        self.0.list(prefix)
    }

    fn list_children(&self, prefix: &str) -> Result<Vec<String>, FieldglassError> {
        self.0.list_children(prefix)
    }

    fn prefetch(&self, keys: &[&str]) -> Result<(), FieldglassError> {
        self.0.prefetch(keys)
    }
}

/// The store, the session over it, and the first renderable variable's axes.
fn opened() -> (Rc<Recording<MemoryObjects>>, Session, u32, u32, u32, usize) {
    let recorder = Rc::new(Recording::new(load(&format!("{FIXTURES}/stores/cf_v3"))));
    let session = Session::open_store(Shared(Rc::clone(&recorder))).expect("the store opens");
    let vars = session.variables();
    let (index, var) = vars
        .iter()
        .enumerate()
        .find(|(_, v)| v.detected_y_dim.is_some() && v.detected_x_dim.is_some())
        .map(|(i, v)| (i as u32, v))
        .expect("a variable with detected horizontal axes");
    let (y, x) = (
        var.detected_y_dim.expect("a detected Y"),
        var.detected_x_dim.expect("a detected X"),
    );
    let rank = var.dims.len();
    (recorder, session, index, y, x, rank)
}

/// Placing the same slice twice reads the store once.
#[test]
fn the_second_placement_touches_no_objects() {
    let (recorder, session, index, y, x, _) = opened();

    let first = session.place_slice(index, y, x).expect("places");
    let after_first = recorder.gets().len();
    assert!(
        after_first > 0,
        "the first placement reads the coordinate arrays"
    );

    let second = session.place_slice(index, y, x).expect("places again");
    assert_eq!(
        recorder.gets().len(),
        after_first,
        "the second placement asked the store for nothing: {:?}",
        &recorder.gets()[after_first..]
    );

    // Memoised, not merely cheap: the same answer, to the byte.
    assert_eq!(first.family(), second.family());
    assert_eq!((first.ni(), first.nj()), (second.ni(), second.nj()));
    assert_eq!(first.scan(), second.scan());
    assert_eq!(
        format!("{:?}", first.geometry()),
        format!("{:?}", second.geometry()),
        "the cached placement is the one the first call derived"
    );
}

/// A decode after a placement reuses it, rather than deriving its own.
///
/// `decode_slice` and `place_slice` shared the derivation before this; they
/// share the memo of it now, which is what keeps the geometry a host paints
/// with from being a second opinion about where the cells are.
#[test]
fn decoding_after_placing_derives_no_second_geometry() {
    let (recorder, session, index, y, x, rank) = opened();

    let placed = session.place_slice(index, y, x).expect("places");
    let after_place = recorder.gets().len();

    let field = session
        .decode_slice(index, y, x, &vec![0; rank], &DecodeOptions::default())
        .expect("decodes");
    let coordinate_keys: Vec<_> = recorder.gets()[after_place..]
        .iter()
        .filter(|k| k.contains("lat") || k.contains("lon"))
        .cloned()
        .collect();
    assert!(
        coordinate_keys.is_empty(),
        "the decode reused the placement rather than re-reading the coordinates: {coordinate_keys:?}"
    );

    // And it is the same placement, so the two cannot disagree.
    assert_eq!(field.georef.label, placed.family());
    assert_eq!((field.ni, field.nj), (placed.ni(), placed.nj()));
    assert_eq!(
        format!("{:?}", field.georef.geometry),
        format!("{:?}", placed.geometry())
    );
}
