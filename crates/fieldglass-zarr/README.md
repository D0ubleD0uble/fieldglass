# fieldglass-zarr

The Zarr format stores large multi-dimensional arrays as many small compressed
chunks, so a program can read just the part of an array it needs.
`fieldglass-zarr` reads Zarr v2 and v3 stores in pure Rust from the bytes you
hand it, which can come from a local folder, objects fetched over HTTP, or a
cloud bucket. This crate is usable on its own, and is part of
[Fieldglass](https://github.com/D0ubleD0uble/fieldglass).

## What it reads

A Zarr store is a folder (or a bucket, or a set of web addresses) holding one
or more arrays: grids of numbers such as a temperature for every latitude and
longitude, often with a third axis for time or height. Arrays can sit in
groups, like files in subfolders. Each array is cut into chunks, and each chunk
is saved as its own small, usually compressed, file.

- It reads **both versions of the format**, v2 and v3, with or without a
  consolidated index file at the top of the store.
- It reads the **whole store's layout**: every array and group, the size of
  each array, the names of its axes, and its attributes such as units.
- It reads **any part of an array**. Ask for a region, say the first ten time
  steps over Europe, and it reads only the chunks that region covers. A
  "sharded" array packs many chunks into one larger file. It reads just the
  bytes of the chunks it needs from inside that file.
- It gives you **real-world values**. Arrays often save numbers in a packed
  form, such as whole numbers plus a scale and offset, with one special value
  meaning "no data". You can get the numbers exactly as stored, or converted
  back to real units with gaps marked as missing.
- It reads **numbers** of any common kind: true/false, whole numbers up to 64
  bits, and decimal numbers (32- or 64-bit).
- It undoes the **common compression methods**: blosc (with any of its five
  compressors), gzip, zlib, zstd and LZ4, plus the reordering steps that often
  come with them. The full list is in [Compression and
  encodings](#compression-and-encodings).

What it doesn't do:

- It doesn't read **every kind** of value. Text, dates, complex numbers,
  records with named fields and 16-bit decimals are left out, as is any
  compression method not listed below. It names what it skipped and why,
  instead of returning wrong numbers.
- It doesn't **fetch** anything. It never opens files or makes network requests
  itself. You read the store's files and hand it the bytes, as the examples
  show. To read a store over the network, `fieldglass-fetchplan` can work out
  which files a region needs.
- It doesn't **write**. It reads Zarr stores, but it can't create or change
  one.

## Examples

Run each example with `cargo run -p fieldglass-zarr --example <name>` from a
checkout of the repository. Each one reads a small store that ships with the
crate.

| Example | What it answers |
|---|---|
| [`what_is_in_this_store`](https://github.com/D0ubleD0uble/fieldglass/blob/master/crates/fieldglass-zarr/examples/what_is_in_this_store.rs) | What arrays are in this store, how big, over which axes, in which units? Start here. |
| [`what_was_the_temperature`](https://github.com/D0ubleD0uble/fieldglass/blob/master/crates/fieldglass-zarr/examples/what_was_the_temperature.rs) | What is the value at one latitude and longitude, as stored and in real units? |
| [`read_only_what_you_need`](https://github.com/D0ubleD0uble/fieldglass/blob/master/crates/fieldglass-zarr/examples/read_only_what_you_need.rs) | How little of a store does reading one corner touch, and which files would I fetch? |
| [`why_wont_this_array_open`](https://github.com/D0ubleD0uble/fieldglass/blob/master/crates/fieldglass-zarr/examples/why_wont_this_array_open.rs) | Which arrays can't be read, and why? |
| [`see_it`](https://github.com/D0ubleD0uble/fieldglass/blob/master/crates/fieldglass-zarr/examples/see_it.rs) | What does this array look like? Drawn in the terminal. |
| [`decode`](https://github.com/D0ubleD0uble/fieldglass/blob/master/crates/fieldglass-zarr/examples/decode.rs) | How do I decode one chunk whose bytes I already have? |

## Feature flags

| Feature | Default | What it adds |
|---|---|---|
| `codecs` | on | Decoding chunks. Turn it off if you only need to read an array's description, and your program won't include any of the compression code. |

## Compression and encodings

Each chunk is usually compressed to save space, and is sometimes rearranged
first so it compresses better. The store records which steps it used, and this
crate undoes them in reverse. These are the steps it can undo:

| Step | What it does | Named in the store as |
|---|---|---|
| blosc | A wrapper around one of five compressors (blosclz, lz4, lz4hc, zlib, zstd), often with a shuffle built in. The snappy compressor is not supported. | `blosc` |
| gzip | General-purpose compression. | `gzip` |
| zlib | The same compression as gzip, with a smaller header. | `zlib` (v2 only) |
| zstd | Fast, modern compression. | `zstd` |
| LZ4 | Very fast compression. | `lz4` (v2 only) |
| shuffle | Regroups the bytes of each number so similar bytes sit together and compress better. | `shuffle` (v2 only) |
| bytes | Says whether multi-byte numbers are stored smallest byte first or largest first. | `bytes` (v3 only) |
| transpose | Stores the array's axes in a different order. | `transpose` (v3), `order: "F"` (v2) |
| crc32c | A checksum that catches damaged chunks. | `crc32c` (v3 only) |
| sharding | Packs many chunks into one file, with an index to find each one. | `sharding_indexed` (v3 only) |

## License

MIT OR Apache-2.0

## AI disclosure

Much of this crate's code, tests and documentation was written with AI
assistance (Anthropic's Claude), under the maintainer's direction and review.

### How do I maintain quality?

I treat the AI as a fast contributor whose work I don't take on trust:

- Fieldglass is designed against the specifications and reference libraries are
  used to check against the resulting behaviors. If behaviors differ between
  Fieldglass and a reference, Fieldglass follows the specification and records
  why.
- Every example comes from as close to real-world files as possible while also
  trying not to bloat the size of our libraries with data. In
  `fieldglass-zarr`, the stores are written by `zarr-python` and `numcodecs`
  and read for comparison.
- I take a personal hand in testing each feature and delivery before a new tag
  is released. I maintain a testing plan in which I test each change.
- I have many strict automatic checks for formatting, lints, the test suite,
  dependency and security scans, and fuzzing with damaged or hostile files to
  make sure we refuse them safely.

### Where do I currently notice gaps?

- I dislike the current documentation. There's a lot of it, which would be fine
  if it made the docs more readable but instead it's hard to understand. I am
  undertaking efforts to rewrite the user-facing documentation with my own
  words. This file has been rewritten by me.
- While I have been ensuring proper functionality, behaviors, and
  larger-picture dependency graphs, I haven't been reading over every line of
  code. As the shape of things starts to become a bit more stable, I hope to
  review more of the code closely, but to be totally transparent this is more
  of a 'desire' than a reality currently.
