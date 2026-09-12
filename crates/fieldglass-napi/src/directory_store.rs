//! A directory on disk as an [`ObjectSource`] (#659, ADR-0005 decision 1).
//!
//! A Zarr store is a **directory**, not a file: a key per metadata document and
//! a key per chunk, laid out as nested folders. Every other container this addon
//! opens is one file the extension hands over as a `Buffer`, so this is the one
//! place the host reads from the filesystem itself — which ADR-0005 puts on the
//! host deliberately, and `planned/02-trait-seams.md` names as napi's job
//! ("the napi host may implement it over `std::fs`, because napi is the host").
//!
//! # Lazy, not slurped
//!
//! The obvious implementation reads the whole tree into a
//! [`MemoryObjects`](fieldglass::MemoryObjects) at open. That is wrong for the
//! thing this exists to read: a store's chunks are the bulk of it, a viewer
//! looks at one slice, and the metadata documents that describe the whole
//! hierarchy are a few kilobytes. So a `get` is one `read`, and nothing is held
//! but the root path.
//!
//! # Why [`list_children`] and not [`list`]
//!
//! [`list`](ObjectSource::list) returns every key beneath a prefix, which for a
//! directory means walking the whole tree — every chunk of every array — to find
//! a handful of metadata documents. [`list_children`] returns the immediate
//! children, which is one `read_dir`, and the store walker descends through it
//! (#708). Overriding it is the entire reason a directory-backed source is cheap
//! to open: on a store with a million chunks, opening reads the directories the
//! groups live in and never enumerates a chunk.
//!
//! [`list_children`]: ObjectSource::list_children

use std::borrow::Cow;
use std::path::{Path, PathBuf};

use fieldglass::{FieldglassError, ObjectSource};

/// The documents whose presence makes a directory a Zarr store.
///
/// Both editions, and both layouts of each: a v2 store roots at `.zgroup` (a
/// hierarchy) or `.zarray` (a single array), a v3 store at `zarr.json`.
/// `.zmetadata` is consolidated v2 metadata, which is a store even when the
/// root `.zgroup` sits beside it.
///
/// **Not the `.zarr` extension**, which is a convention and not a guarantee:
/// stores are routinely named `something.zarr`, and just as routinely not.
pub(crate) const ROOT_MARKERS: [&str; 4] = ["zarr.json", ".zmetadata", ".zgroup", ".zarray"];

/// Whether `root` looks like the root of a Zarr store.
///
/// A cheap, honest check: the presence of a root metadata document. It is what
/// lets the extension tell a user "that folder is not a Zarr store" instead of
/// opening an empty editor — which is the failure the issue asks to avoid.
pub(crate) fn is_store_root(root: &Path) -> bool {
    ROOT_MARKERS.iter().any(|name| root.join(name).is_file())
}

/// A directory read as keyed objects.
#[derive(Debug, Clone)]
pub(crate) struct DirectoryObjects {
    root: PathBuf,
}

impl DirectoryObjects {
    /// Read the store rooted at `root`.
    pub(crate) fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The path a key names, or `None` when the key escapes the root.
    ///
    /// **The containment check is the point.** A key comes out of a metadata
    /// document, which is a file somebody else wrote, so `..` in one is an
    /// attacker asking this host to read outside the folder the user chose.
    /// Rejecting the component outright is simpler than canonicalising and
    /// comparing, and cannot be defeated by a symlink race — there is nothing
    /// to race, because no path containing `..` is ever built.
    fn path_of(&self, key: &str) -> Option<PathBuf> {
        let mut path = self.root.clone();
        for segment in key.split('/') {
            if segment.is_empty() || segment == "." || segment == ".." {
                return None;
            }
            // A segment holding a separator of the *host's* spelling would
            // escape on Windows even though it is one component here.
            if segment.contains('\\') || segment.contains(std::path::MAIN_SEPARATOR) {
                return None;
            }
            path.push(segment);
        }
        Some(path)
    }

    /// Every key under `prefix`, by walking. See the module note on why the
    /// walker does not use this.
    fn walk(&self, dir: &Path, prefix: &str, out: &mut Vec<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            // A directory that cannot be read lists as empty rather than
            // failing the walk: a store with one unreadable group should still
            // present the groups beside it, which is the same rule
            // `ArraySource` states for an array that fails on its own.
            return;
        };
        for entry in entries.flatten() {
            let Ok(name) = entry.file_name().into_string() else {
                continue; // a name that is not UTF-8 is not a Zarr key
            };
            let key = if prefix.is_empty() {
                name
            } else {
                format!("{prefix}/{name}")
            };
            if entry.path().is_dir() {
                self.walk(&entry.path(), &key, out);
            } else {
                out.push(key);
            }
        }
    }
}

impl ObjectSource for DirectoryObjects {
    fn get(&self, key: &str) -> Result<Option<Cow<'_, [u8]>>, FieldglassError> {
        let Some(path) = self.path_of(key) else {
            // A key that escapes the root is absent, not an error: it is a
            // malformed key in somebody's document, and this seam's answer for
            // "no such object" is `None`.
            return Ok(None);
        };
        match std::fs::read(&path) {
            Ok(bytes) => Ok(Some(Cow::Owned(bytes))),
            // Absent is ordinary here — a sparse array stores no object for a
            // chunk that is entirely fill value, and that is the common case
            // rather than a fault.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            // A directory where a key is expected is also "no object".
            Err(e) if e.kind() == std::io::ErrorKind::IsADirectory => Ok(None),
            Err(e) => Err(FieldglassError::Io(e)),
        }
    }

    fn list(&self, prefix: &str) -> Result<Vec<String>, FieldglassError> {
        let mut out = Vec::new();
        self.walk(&self.root, "", &mut out);
        out.retain(|key| key.starts_with(prefix));
        // The seam promises sorted keys, and `read_dir` promises nothing about
        // order — so two hosts listing one store hand their reader the same
        // sequence only because of this line.
        out.sort_unstable();
        Ok(out)
    }

    fn list_children(&self, prefix: &str) -> Result<Vec<String>, FieldglassError> {
        // One `read_dir`, which is the whole reason this override exists: the
        // default filters `list`, and `list` walks every chunk in the store.
        let dir = if prefix.is_empty() {
            Some(self.root.clone())
        } else {
            self.path_of(prefix.trim_end_matches('/'))
        };
        let Some(dir) = dir else {
            return Ok(Vec::new());
        };
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return Ok(Vec::new());
        };
        let mut out = Vec::new();
        for entry in entries.flatten() {
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            let child = format!("{prefix}{name}");
            out.push(if entry.path().is_dir() {
                format!("{child}/")
            } else {
                child
            });
        }
        out.sort_unstable();
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fieldglass::MemoryObjects;

    /// A committed Zarr fixture store, by a path relative to the crate dir.
    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(format!("../fieldglass-zarr/tests/fixtures/stores/{name}"))
    }

    #[test]
    fn a_store_root_is_recognised_by_its_metadata_and_not_its_name() {
        // Every committed store, whatever it is called — none of them ends
        // `.zarr`, which is the point: the convention is not the guarantee.
        for name in ["v2_nested", "v2_slash", "v3_nested", "cf_v2", "cf_v3"] {
            assert!(is_store_root(&fixture(name)), "{name} is a store");
        }
        // A directory of fixtures is not a store, and neither is one that does
        // not exist.
        assert!(!is_store_root(&PathBuf::from(
            "../fieldglass-zarr/tests/fixtures"
        )));
        assert!(!is_store_root(&PathBuf::from("../nowhere-at-all")));

        // **An array's own directory _is_ a store**, and deliberately: `.zarray`
        // at the root is a single-array store, which is why it is a marker. So a
        // user who picks `store.zarr/temp` instead of `store.zarr` gets that one
        // array rather than an error — a better answer than refusing, and the
        // reason this is asserted rather than left to be discovered.
        assert!(is_store_root(&fixture("v2_slash/temp")));
    }

    #[test]
    fn a_directory_reads_the_same_keys_an_in_memory_store_holds() {
        let root = fixture("v2_slash");
        let dir = DirectoryObjects::new(&root);

        // The oracle: the same tree slurped into memory, which is what this
        // replaces. Built here rather than reusing the crate's own loader, so
        // the two do not share a walk.
        let mut entries = Vec::new();
        fn slurp(root: &Path, at: &Path, out: &mut Vec<(String, Vec<u8>)>) {
            for e in std::fs::read_dir(at).expect("readable").flatten() {
                let p = e.path();
                if p.is_dir() {
                    slurp(root, &p, out);
                } else {
                    let key = p
                        .strip_prefix(root)
                        .expect("under the root")
                        .components()
                        .map(|c| c.as_os_str().to_string_lossy().into_owned())
                        .collect::<Vec<_>>()
                        .join("/");
                    out.push((key, std::fs::read(&p).expect("readable")));
                }
            }
        }
        slurp(&root, &root, &mut entries);
        let memory = MemoryObjects::from_iter(entries.clone());

        assert_eq!(
            dir.list("").expect("lists"),
            memory.list("").expect("lists")
        );
        for (key, bytes) in &entries {
            assert_eq!(
                dir.get(key).expect("reads").map(Cow::into_owned),
                Some(bytes.clone()),
                "{key} differed"
            );
        }
        // Absence is ordinary, not an error: a sparse array's missing chunk.
        assert!(dir.get("temp/9.9").expect("absent, not broken").is_none());
        assert!(dir.get("no/such/key").expect("absent").is_none());
        // A directory where a key is expected is also "no object".
        assert!(
            dir.get("temp")
                .expect("a directory is not an object")
                .is_none()
        );
    }

    /// `list_children` is one `read_dir`, and that is measurable: it must not
    /// name anything below the prefix.
    ///
    /// This is the property the whole override exists for (#708). The default
    /// filters `list`, which walks every chunk in the store — so on a real store
    /// opening would enumerate the data to find the metadata.
    #[test]
    fn listing_children_names_one_level_and_not_the_tree() {
        let dir = DirectoryObjects::new(fixture("v2_slash"));

        assert_eq!(
            dir.list_children("").expect("lists"),
            [".zattrs", ".zgroup", "sub/", "temp/"]
        );
        // The array's own documents and its chunk rows as directories — not the
        // four chunks two levels down, which `list` would have named.
        assert_eq!(
            dir.list_children("temp/").expect("lists"),
            ["temp/.zarray", "temp/.zattrs", "temp/0/", "temp/1/"]
        );
        let whole = dir.list("temp/").expect("lists");
        assert!(
            whole.iter().any(|k| k == "temp/0/0"),
            "the chunks are there to be found: {whole:?}"
        );
        assert!(
            !dir.list_children("temp/")
                .expect("lists")
                .iter()
                .any(|k| k.contains("0/0")),
            "and `list_children` does not name them"
        );
        // A prefix that is not a directory lists nothing rather than failing.
        assert!(
            dir.list_children("temp/.zarray/")
                .expect("not a dir")
                .is_empty()
        );
        assert!(dir.list_children("nowhere/").expect("absent").is_empty());
    }

    /// A key cannot escape the directory the user chose.
    ///
    /// Keys come out of metadata documents, which are files somebody else wrote,
    /// so `..` in one is that author asking this host to read outside the folder.
    /// Refused as *absent*, which is this seam's answer for "no such object" —
    /// and refused by never building the path, so there is no canonicalise-then-
    /// open window to race.
    #[test]
    fn a_key_cannot_escape_the_root() {
        let dir = DirectoryObjects::new(fixture("v2_slash"));
        // The sibling store really is there, so these would read something if
        // the guard were absent.
        assert!(fixture("v2_nested/.zgroup").is_file());

        for escape in [
            "../v2_nested/.zgroup",
            "sub/../../v2_nested/.zgroup",
            "./.zgroup",
            "..",
            "sub//.zgroup",
        ] {
            assert!(
                dir.get(escape).expect("refused as absent").is_none(),
                "{escape} was served"
            );
            assert!(
                dir.list_children(escape).expect("refused").is_empty(),
                "{escape} was listed"
            );
        }
        // And the honest key for the same document still reads, or the guard
        // would be refusing everything and proving nothing.
        assert!(dir.get(".zgroup").expect("reads").is_some());
    }
}

#[cfg(test)]
mod opens_a_store_tests {
    use super::*;
    use fieldglass::Session;

    /// A directory opens as a session, and reads the values the store holds.
    ///
    /// The claim that matters: `Session::open_store` over this source needs no
    /// change to the walker or the decoder, which is what ADR-0010 means by a
    /// store being an IO shape rather than a format. `cf_v3` is used because it
    /// carries CF attributes, so a variable comes back in physical units — the
    /// whole path, not just the key lookup.
    #[test]
    fn a_directory_opens_as_a_session_and_lists_its_variables() {
        let root = PathBuf::from("../fieldglass-zarr/tests/fixtures/stores/cf_v3");
        assert!(is_store_root(&root), "the fixture is a store");

        let session = Session::open_store(DirectoryObjects::new(&root)).expect("opens");
        assert_eq!(session.format(), fieldglass::SourceFormat::Zarr);
        assert_eq!(session.addressing(), fieldglass::Addressing::Variables);
        assert!(session.left_out().is_empty(), "{:?}", session.left_out());

        let variables = session.variables();
        assert!(!variables.is_empty(), "the store lists variables");
        let t = variables
            .iter()
            .find(|v| v.name == "t")
            .expect("the packed variable");

        // And a slice decodes, through the same call a NetCDF file takes.
        let field = session
            .decode_slice(
                t.index,
                t.detected_y_dim.expect("a latitude axis"),
                t.detected_x_dim.expect("a longitude axis"),
                &[0, 0],
                &Default::default(),
            )
            .expect("decodes");
        assert_eq!((field.ni, field.nj), (4, 3));
        assert!(
            field.stats.valid_count > 0,
            "the slice holds values: {:?}",
            field.stats
        );
    }

    /// A directory with nothing Zarr in it is refused, and the message says so.
    ///
    /// The acceptance criterion behind "reports that plainly rather than opening
    /// an empty editor": the host checks `is_store_root` first, and the walker
    /// refuses too, so a user who picks the wrong folder learns why.
    #[test]
    fn a_directory_that_is_not_a_store_is_refused_plainly() {
        let root = PathBuf::from("../fieldglass-zarr/tests/fixtures");
        assert!(!is_store_root(&root));

        let err = Session::open_store(DirectoryObjects::new(&root)).expect_err("not a store");
        let message = err.message();
        assert!(
            message.contains("zarr.json") || message.contains(".zgroup"),
            "the refusal should name what it looked for: {message}"
        );
    }
}
