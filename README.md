# zsync-rs

A fast, pure Rust implementation of [zsync](http://zsync.moria.org.uk/) — the efficient file transfer algorithm that downloads only the parts of a file you don't already have.

## How it works

zsync uses the rsync rolling checksum algorithm over HTTP. Given an old version of a file, it identifies which blocks are unchanged and downloads only the differences:

1. Fetches the `.zsync` control file (block checksums for the target file)
2. Scans local seed files for matching blocks using rolling checksums
3. Downloads only the missing blocks via HTTP range requests
4. Assembles the final file and verifies its SHA-1 checksum

## Features

- **Accurate matching** — sequential block chaining, proper `rsum_a_mask` handling, and zero-padded EOF scanning match the reference C implementation exactly
- **Fast lookups** — flat hash table with a bithash for O(1) negative filtering, matching the C data structure design
- **Parallel scanning** — splits large source files across CPU cores for matching (something the single-threaded C implementations can't do)
- **Optimized downloads** — merges HTTP range requests to minimize round-trips
- **Multiple seed files** — pass multiple `-i` flags to scan several local files
- **Self-referential scanning** — after seed matching, scans the partially-assembled output to find duplicate target blocks without downloading them
- **Fuzz-tested** — control file parser, rolling checksums, and block matcher are all fuzz-tested with cargo-fuzz

## Installation

```bash
cargo install --path .
```

## Usage

```bash
# Basic: download a file using its .zsync control file
zsync https://example.com/file.AppImage.zsync

# Use a local file as seed to avoid re-downloading unchanged blocks
zsync https://example.com/v2.0/app.AppImage.zsync -i app-v1.9.AppImage

# Multiple seed files for maximum local matching
zsync https://example.com/v3.0/app.zsync -i app-v2.9.AppImage -i app-v2.8.AppImage

# Specify output filename
zsync https://example.com/file.zsync -o output.bin
```

### Progress output

The CLI reports progress during the download, updated at most every 100 ms.
On a terminal it draws a single bar that redraws in place:

```
Downloading missing blocks...
  [#############                 ]  45%  8.62 MB / 19.07 MB
```

When stderr is redirected to a file or a pipe, it prints one line per update
instead, skipping updates that would repeat the previous percent:

```
Downloading missing blocks...
  2.00 KB / 19.07 MB (0%)
  1.71 MB / 19.07 MB (8%)
  ...
  19.07 MB / 19.07 MB (100%)
```

Pass `--json` to put newline-delimited JSON on stdout instead, for a wrapper
or a UI to consume. Human-readable messages stay on stderr, so stdout carries
nothing but JSON:

```json
{"event":"start","total_bytes":20000000,"total_blocks":9766,"blocksize":2048}
{"event":"progress","phase":"download","done_bytes":1816576,"total_bytes":20000000,"elapsed_ms":105}
{"event":"progress","phase":"download","done_bytes":20000000,"total_bytes":20000000,"elapsed_ms":1112,"final":true}
{"event":"phase","phase":"verify"}
{"event":"done","path":"output.bin"}
```

Every line is one JSON object with an `event` field. The events are `start`,
`progress`, `phase`, `done`, and `error`. The last `progress` event always
carries `"final":true` and the completed byte count, whatever the throttle
allowed through.

## Library Usage

zsync-rs is library-first — the CLI is a thin wrapper around the public API:

```rust
use std::path::Path;
use zsync_rs::ZsyncAssembly;

let mut assembly = ZsyncAssembly::from_url(
    "https://example.com/file.zsync",
    Path::new("output.bin"),
)?;

// Match blocks from a local seed file
assembly.submit_source_file(Path::new("old-version.bin"))?;

// Download whatever's missing, until a pass adds nothing
while assembly.download_missing_blocks()? > 0 {}

// Fail if any block is still missing, then verify checksum and finalize
assembly.complete()?;
```

## Building

```bash
cargo build --release
```

## Testing

```bash
cargo test

# Fuzzing (requires nightly)
cargo fuzz run fuzz_control_file
cargo fuzz run fuzz_rsum
cargo fuzz run fuzz_matcher
```
