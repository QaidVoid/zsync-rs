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

# Retry a dropped download up to 5 times instead of the default 3
zsync https://example.com/file.zsync --retries 5
```

A download that fails on a network or I/O error is retried, waiting 2 s,
then 4 s, then 8 s, and each retry fetches only the blocks still missing.
The count starts over whenever a retry makes progress.

### Progress output

The CLI reports progress while it scans each seed file given with `-i`,
while it rescans the assembled data for repeated blocks, and during the
download, updated at most every 100 ms. On a terminal it draws a single bar
that redraws in place:

```
Scanning old.AppImage...
  [##############################] 100%  286.10 MB / 286.10 MB
Matched 48828 blocks from old.AppImage (66.67%)
Scanning for repeated blocks...
  [##############################] 100%  190.73 MB / 190.73 MB
Target: 286.10 MB (73243 blocks) - 66.67% complete
Need to download: 95.37 MB
Downloading missing blocks...
  [#############                 ]  45%  128.75 MB / 286.10 MB
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
{"event":"start","total_bytes":300000000,"total_blocks":73243,"blocksize":4096}
{"event":"progress","phase":"scan","file":"old.AppImage","done_bytes":33554432,"total_bytes":300000000,"elapsed_ms":39}
{"event":"progress","phase":"scan","file":"old.AppImage","done_bytes":300000000,"total_bytes":300000000,"elapsed_ms":251,"final":true}
{"event":"progress","phase":"self_scan","done_bytes":33554432,"total_bytes":199999488,"elapsed_ms":283}
{"event":"progress","phase":"self_scan","done_bytes":199999488,"total_bytes":199999488,"elapsed_ms":421,"final":true}
{"event":"progress","phase":"download","done_bytes":200003584,"total_bytes":300000000,"elapsed_ms":422}
{"event":"progress","phase":"download","done_bytes":300000000,"total_bytes":300000000,"elapsed_ms":5600,"final":true}
{"event":"phase","phase":"verify"}
{"event":"done","path":"app.AppImage"}
```

Every line is one JSON object with an `event` field. The events are `start`,
`progress`, `phase`, `retry`, `done`, and `error`. A `retry` event carries
the `attempt` number, the `retries` allowed, the `delay_ms` before it, and
the error `message`. A `progress` event's `phase` is
`scan` for a seed file, which it names in `file`, `self_scan` for the rescan
of the assembled data, or `download`. In a scan, the byte counts are of the
file being scanned; in the download, they are of the target. Each phase ends
with a `progress` event carrying `"final":true` and its completed count,
whatever the throttle allowed through. A seed too small to hold a block
reports no scan events.

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
