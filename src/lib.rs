//! Efficient partial file transfer over HTTP, compatible with zsync.
//!
//! A `.zsync` control file describes a target file as a list of block
//! checksums. A client that already holds similar data, such as an older
//! version of the same file, finds the blocks it has locally and downloads
//! only the rest with HTTP range requests. Nothing beyond a plain web server
//! that honours `Range` is needed on the serving side.
//!
//! [`ZsyncAssembly`] drives a whole transfer and is the place to start. The
//! modules beneath it are public for callers that need one piece on its own:
//! [`control`] reads and writes control files, [`matcher`] finds known
//! blocks in local data, [`http`] fetches ranges, and [`checksum`] and
//! [`rsum`] are the hashes the format is built on.
//!
//! ```no_run
//! use std::path::Path;
//! use zsync_rs::ZsyncAssembly;
//!
//! # fn main() -> Result<(), zsync_rs::AssemblyError> {
//! let mut assembly =
//!     ZsyncAssembly::from_url("https://example.com/file.zsync", Path::new("file.bin"))?;
//!
//! assembly.submit_source_file(Path::new("old-file.bin"))?;
//! while assembly.download_missing_blocks()? > 0 {}
//!
//! // Fails if any block is still missing, then verifies the SHA-1.
//! assembly.complete()?;
//! # Ok(())
//! # }
//! ```

#![deny(missing_docs)]

pub mod assembly;
pub mod checksum;
pub mod control;
pub mod http;
pub mod matcher;
pub mod rsum;

pub use assembly::{AssemblyError, ZsyncAssembly};
pub use control::{ControlFile, GenerateError, WriteError};
pub use http::{HttpClient, HttpError};
pub use matcher::BlockMatcher;
