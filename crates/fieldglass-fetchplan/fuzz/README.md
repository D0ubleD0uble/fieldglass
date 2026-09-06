# fieldglass-fetchplan fuzzing

A [`cargo-fuzz`](https://github.com/rust-fuzz/cargo-fuzz) target for the
cloud-native manifest parsers. `fieldglass-fetchplan` reads sidecars **fetched
over the network** and turns them into the byte ranges a host then asks a bucket
for, so nothing about their shape is this crate's to assume. #652 hardened three
hostile-input paths in it — an `i64::MIN` clock that overflowed `abs()`, a byte
index into non-ASCII text that could land inside a character, and an O(n²) walk
that a 1 MB download could stall a request handler with — which is evidence the
surface has them rather than evidence it is now clean.

Three arms run on every input:

* **wgrib2 `.idx`**, a colon-delimited line grammar whose records state an
  offset and never a length: each record's extent comes from the next *distinct*
  offset, which is the walk #652 replaced.
* **ECMWF `.index`**, JSON lines, each stating its own offset and length.
* **Run discovery**, a `SourceSpec` deserialized from the same buffer with a
  clock and a forecast step read off its last twelve bytes. The catalog is the
  host's data rather than this crate's, so the key template is a string this
  crate scans, and the clock is a parameter the fuzzer should own.

A parse that succeeds is then walked the way a host walks it — every record, the
collapsed one-per-message list, a query, `close` against an object size, the
HTTP `Range` header, and the §0 envelope check against the input read back as
message bytes. The parse is only half the surface: the offset arithmetic after
it is where a sidecar's numbers become a fetch.

This crate is intentionally **not** a member of the workspace, so the standard
stable-toolchain gates (`cargo fmt/clippy/test --workspace`) never try to build
the nightly-only libFuzzer target.

## Run

```sh
# from crates/fieldglass-fetchplan/fuzz
cargo +nightly fuzz run parse
```

The seed corpus under `corpus/parse/` is the crate's own sidecar fixtures — five
NCEP `.idx` files and one ECMWF `.index` — whose provenance is
`tests/fixtures/NOTICE.md`, plus one hand-written `SourceSpec` document with the
twelve-byte tail appended so the discovery arm starts from a spec that
validates. CI runs this target time-boxed on pull requests that touch the crate;
see `.github/workflows/fuzz.yml`.
