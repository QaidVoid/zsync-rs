//! Assembling a target file from local sources and HTTP ranges.
//!
//! A transfer runs in three steps on one [`ZsyncAssembly`]. Local files are
//! scanned first, and every block they contain is copied into a temporary
//! file beside the output. The blocks still missing are then downloaded and
//! verified one at a time. Last, [`ZsyncAssembly::complete`] checks the whole
//! file against the control file's SHA-1 and renames it into place.
//!
//! The temporary file is the output path with `.zsync-tmp` appended, so
//! outputs that differ only in their extension never share one. Dropping the
//! assembly before it completes removes it, so a failed or abandoned transfer
//! leaves nothing behind.

use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::checksum::calc_sha1_stream;
use crate::control::ControlFile;
use crate::http::{
    DEFAULT_RANGE_GAP_THRESHOLD, HttpClient, byte_ranges_from_block_ranges, merge_byte_ranges,
};
use crate::matcher::BlockMatcher;
use crate::matcher::MatchError;

/// How much of a file a scan reads at once.
///
/// Large enough that the matcher splits each chunk across every core, which
/// it only does for inputs of several MiB, while keeping a scan's buffer to
/// tens of MiB whatever the file's size.
const STREAM_CHUNK_SIZE: usize = 32 * 1024 * 1024;

/// Why a step of an assembly failed.
#[derive(Debug, thiserror::Error)]
pub enum AssemblyError {
    /// Reading a source file, or writing, truncating or renaming the
    /// temporary file, failed.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    /// Fetching the control file or a block range failed.
    #[error(transparent)]
    Http(#[from] crate::http::HttpError),
    /// The block matcher failed.
    #[error("Matcher error: {0}")]
    Matcher(#[from] MatchError),
    /// The assembly cannot do what was asked of it, such as completing while
    /// blocks are still missing.
    #[error("Control file error: {0}")]
    Control(String),
    /// The assembled file's SHA-1 differs from the control file's.
    #[error("Checksum mismatch: expected {expected}, got {actual}")]
    ChecksumMismatch {
        /// The digest from the control file, as written there.
        expected: String,
        /// The digest of the assembled file, in lowercase hex.
        actual: String,
    },
    /// The control file lists no URL to download blocks from.
    #[error("No URLs available")]
    NoUrls,
    /// The abort flag was raised during a scan or a download.
    #[error("Aborted")]
    Aborted,
}

/// Reports whether the caller has raised the abort flag.
///
/// Free function rather than a method so the scan and download loops can poll
/// it while still holding a mutable borrow of the assembly.
fn aborted(flag: &Option<Arc<AtomicBool>>) -> bool {
    flag.as_ref().is_some_and(|f| f.load(Ordering::Relaxed))
}

/// A callback receiving `(done_bytes, total_bytes)`, as
/// [`ZsyncAssembly::progress`] reports them.
pub type ProgressCallback = Box<dyn Fn(u64, u64) + Send + Sync>;

/// One transfer of a target file, from a control file to the finished output.
pub struct ZsyncAssembly {
    control: ControlFile,
    base_url: Option<String>,
    matcher: BlockMatcher,
    http: HttpClient,
    output_path: std::path::PathBuf,
    temp_path: std::path::PathBuf,
    file: Option<File>,
    range_gap_threshold: u64,
    progress_callback: Option<ProgressCallback>,
    scan_progress_callback: Option<ProgressCallback>,
    abort_flag: Option<Arc<AtomicBool>>,
}

impl ZsyncAssembly {
    /// Assemble the target `control` describes into `output_path`.
    ///
    /// There is no base URL, so the control file's URLs must be absolute.
    /// Nothing is written until a block is found or downloaded.
    pub fn new(control: ControlFile, output_path: &Path) -> Result<Self, AssemblyError> {
        Self::with_base_url(control, output_path, None)
    }

    /// Assemble with relative control file URLs resolved against `base_url`.
    ///
    /// The base is joined as given, so for a directory it should end in `/`.
    pub fn with_base_url(
        control: ControlFile,
        output_path: &Path,
        base_url: Option<&str>,
    ) -> Result<Self, AssemblyError> {
        Self::with_client(control, output_path, base_url, HttpClient::new())
    }

    /// Assemble using a caller-supplied HTTP client.
    ///
    /// The client carries the transport policy for every range request the
    /// assembly makes, so an embedder that needs its own TLS roots or that
    /// must refuse a redirect to plain HTTP can set it once here.
    pub fn with_client(
        control: ControlFile,
        output_path: &Path,
        base_url: Option<&str>,
        http: HttpClient,
    ) -> Result<Self, AssemblyError> {
        let matcher = BlockMatcher::new(&control);
        let mut temp_path = output_path.as_os_str().to_owned();
        temp_path.push(".zsync-tmp");
        let temp_path = std::path::PathBuf::from(temp_path);

        Ok(Self {
            control,
            base_url: base_url.map(|s| s.to_string()),
            matcher,
            http,
            output_path: output_path.to_path_buf(),
            temp_path,
            file: None,
            range_gap_threshold: DEFAULT_RANGE_GAP_THRESHOLD,
            progress_callback: None,
            scan_progress_callback: None,
            abort_flag: None,
        })
    }

    /// Fetch the control file at `control_url` and assemble into
    /// `output_path`.
    ///
    /// Relative URLs in the control file resolve against `control_url`'s
    /// directory.
    ///
    /// # Errors
    ///
    /// Returns [`AssemblyError::Http`] when the control file cannot be
    /// fetched or does not parse.
    pub fn from_url(control_url: &str, output_path: &Path) -> Result<Self, AssemblyError> {
        Self::from_url_with_client(control_url, output_path, HttpClient::new())
    }

    /// Fetch the control file and assemble, both using `http`.
    ///
    /// The same client serves the control-file fetch and every subsequent
    /// range request, so one policy covers the whole transfer.
    pub fn from_url_with_client(
        control_url: &str,
        output_path: &Path,
        http: HttpClient,
    ) -> Result<Self, AssemblyError> {
        let control = http.fetch_control_file(control_url)?;
        let base_url = extract_base_url(control_url);
        Self::with_client(control, output_path, Some(&base_url), http)
    }

    /// Sets how many known bytes may separate two needed ranges before they
    /// are fetched with separate requests.
    ///
    /// Ranges closer than this are fetched as one, downloading the gap again
    /// to save a round trip. The default is
    /// [`DEFAULT_RANGE_GAP_THRESHOLD`].
    pub fn set_range_gap_threshold(&mut self, threshold: u64) {
        self.range_gap_threshold = threshold;
    }

    /// Sets a flag polled during the scan and download loops.
    ///
    /// Once the caller raises it, the running operation stops at the next
    /// chunk boundary and returns [`AssemblyError::Aborted`], so a long
    /// transfer can be cancelled without waiting for it to finish.
    pub fn set_abort_flag(&mut self, flag: Arc<AtomicBool>) {
        self.abort_flag = Some(flag);
    }

    /// Sets a callback to run after each block is downloaded and verified.
    ///
    /// It receives the values [`Self::progress`] returns at that point, and
    /// runs on the thread calling [`Self::download_missing_blocks`], so a slow
    /// callback slows the download. Blocks found in local files do not
    /// trigger it; [`Self::set_scan_progress_callback`] covers those scans.
    pub fn set_progress_callback<F>(&mut self, callback: F)
    where
        F: Fn(u64, u64) + Send + Sync + 'static,
    {
        self.progress_callback = Some(Box::new(callback));
    }

    /// Sets a callback to run as a scan works through a file.
    ///
    /// It receives the bytes scanned so far and the size of the file being
    /// scanned, once per chunk of about 32 MiB, and last with both equal. Both
    /// [`Self::submit_source_file`] and [`Self::submit_self_referential`]
    /// report through it, on the calling thread. A file too small to hold a
    /// match is not scanned and reports nothing.
    pub fn set_scan_progress_callback<F>(&mut self, callback: F)
    where
        F: Fn(u64, u64) + Send + Sync + 'static,
    {
        self.scan_progress_callback = Some(Box::new(callback));
    }

    fn report_progress(&self) {
        if let Some(ref cb) = self.progress_callback {
            let (done, total) = self.progress();
            cb(done, total);
        }
    }

    /// Bytes of the target known so far, and the target's length.
    ///
    /// The count is known blocks times the block size, capped at the length.
    pub fn progress(&self) -> (u64, u64) {
        let total = self.control.length;
        let got = self.matcher.blocks_todo();
        let blocks_done = self.matcher.total_blocks() - got;
        let done_bytes = (blocks_done * self.control.blocksize) as u64;
        (done_bytes.min(total), total)
    }

    /// Whether every block of the target is known.
    pub fn is_complete(&self) -> bool {
        self.matcher.is_complete()
    }

    /// The block size the control file declares.
    ///
    /// Progress consumers need it to convert between the block counts that
    /// [`Self::block_stats`] reports and the byte counts in [`Self::progress`].
    pub fn blocksize(&self) -> usize {
        self.control.blocksize
    }

    /// Blocks known so far, and the target's total block count.
    pub fn block_stats(&self) -> (usize, usize) {
        let total = self.matcher.total_blocks();
        let todo = self.matcher.blocks_todo();
        (total - todo, total)
    }

    /// Scans the file at `path` for blocks of the target, at any offset, and
    /// copies each one found into the temporary file.
    ///
    /// Returns the number of blocks found. A file shorter than one block, or
    /// two when [`HashLengths::seq_matches`](crate::control::HashLengths::seq_matches)
    /// is 2, cannot contain a match and returns 0.
    ///
    /// # Errors
    ///
    /// Returns [`AssemblyError::Io`] when reading the source or writing the
    /// temporary file fails, and [`AssemblyError::Aborted`] when the abort
    /// flag is raised.
    pub fn submit_source_file(&mut self, path: &Path) -> Result<usize, AssemblyError> {
        let mut file = File::open(path)?;
        let file_size = file.metadata()?.len() as usize;

        let blocksize = self.control.blocksize;
        let context = blocksize * self.control.hash_lengths.seq_matches as usize;

        if file_size < context {
            return Ok(0);
        }

        let chunk_size = STREAM_CHUNK_SIZE.max(context * 2);
        let mut total_matched = 0;
        let mut buf = vec![0u8; chunk_size + 2 * context];
        let mut file_offset = 0usize;

        let abort_flag = self.abort_flag.clone();

        loop {
            if aborted(&abort_flag) {
                return Err(AssemblyError::Aborted);
            }

            let overlap_start = file_offset.saturating_sub(context);
            let overlap_len = file_offset - overlap_start;

            if overlap_len > 0 {
                Self::read_at_offset(&mut file, &mut buf[..overlap_len], overlap_start as u64)?;
            }

            let read_start = overlap_len;
            let read_len = chunk_size;

            let bytes_read = Self::read_at_offset(
                &mut file,
                &mut buf[read_start..read_start + read_len],
                file_offset as u64,
            )?;
            if bytes_read == 0 {
                break;
            }

            let data_len = read_start + bytes_read;
            let chunk_context = if file_offset + bytes_read < file_size {
                let context_start = file_offset + bytes_read;
                let context_available = file_size.saturating_sub(context_start).min(context);
                Self::read_at_offset(
                    &mut file,
                    &mut buf[data_len..data_len + context_available],
                    context_start as u64,
                )?;
                if context_available < context {
                    buf[data_len + context_available..data_len + context].fill(0);
                }
                data_len + context
            } else {
                buf[data_len..data_len + context].fill(0);
                data_len + context
            };

            let matched_blocks = self
                .matcher
                .submit_source_data(&buf[..chunk_context], overlap_start as u64);

            for (block_id, source_offset) in &matched_blocks {
                let file_handle = self.ensure_file()?;
                let offset = (block_id * blocksize) as u64;
                let buf_offset = source_offset.saturating_sub(overlap_start);
                debug_assert!(
                    buf_offset + blocksize <= chunk_context,
                    "buf_offset {} + blocksize {} > chunk_context {} (source_offset={}, overlap_start={})",
                    buf_offset,
                    blocksize,
                    chunk_context,
                    source_offset,
                    overlap_start
                );
                let block_data = &buf[buf_offset..buf_offset + blocksize];
                Self::write_at_offset(file_handle, block_data, offset)?;
            }

            total_matched += matched_blocks.len();
            file_offset += bytes_read;
            if let Some(cb) = &self.scan_progress_callback {
                cb(file_offset as u64, file_size as u64);
            }

            if bytes_read < read_len {
                break;
            }
        }

        Ok(total_matched)
    }

    /// Scans the temporary file itself for further blocks of the target.
    ///
    /// A target that repeats a block, such as a run of zeros, needs it only
    /// once to fill every copy. Run this after the source files, when the
    /// temporary file holds what they supplied. Returns the number of blocks
    /// found, which is 0 when nothing has been written yet.
    ///
    /// # Errors
    ///
    /// Returns [`AssemblyError::Io`] when reading or writing the temporary
    /// file fails, and [`AssemblyError::Aborted`] when the abort flag is
    /// raised.
    pub fn submit_self_referential(&mut self) -> Result<usize, AssemblyError> {
        if self.file.is_none() {
            return Ok(0);
        }

        let file = self.file.as_mut().unwrap();
        file.sync_all()?;

        let file_size = file.metadata()?.len() as usize;

        let blocksize = self.control.blocksize;
        let context = blocksize * self.control.hash_lengths.seq_matches as usize;

        if file_size < context {
            return Ok(0);
        }

        let chunk_size = STREAM_CHUNK_SIZE.max(context * 2);
        let mut total_matched = 0;
        let mut buf = vec![0u8; chunk_size + 2 * context];
        let mut file_offset = 0usize;

        let abort_flag = self.abort_flag.clone();

        loop {
            if aborted(&abort_flag) {
                return Err(AssemblyError::Aborted);
            }

            let overlap_start = file_offset.saturating_sub(context);
            let overlap_len = file_offset - overlap_start;

            if overlap_len > 0 {
                Self::read_at_offset(file, &mut buf[..overlap_len], overlap_start as u64)?;
            }

            let read_start = overlap_len;
            let read_len = chunk_size;

            let bytes_read = Self::read_at_offset(
                file,
                &mut buf[read_start..read_start + read_len],
                file_offset as u64,
            )?;
            if bytes_read == 0 {
                break;
            }

            let data_len = read_start + bytes_read;
            let chunk_context = if file_offset + bytes_read < file_size {
                let context_start = file_offset + bytes_read;
                let context_available = file_size.saturating_sub(context_start).min(context);
                Self::read_at_offset(
                    file,
                    &mut buf[data_len..data_len + context_available],
                    context_start as u64,
                )?;
                if context_available < context {
                    buf[data_len + context_available..data_len + context].fill(0);
                }
                data_len + context
            } else {
                buf[data_len..data_len + context].fill(0);
                data_len + context
            };

            let matched_blocks = self
                .matcher
                .submit_source_data(&buf[..chunk_context], overlap_start as u64);

            for (block_id, source_offset) in &matched_blocks {
                let offset = (block_id * blocksize) as u64;
                let buf_offset = source_offset.saturating_sub(overlap_start);
                debug_assert!(
                    buf_offset + blocksize <= chunk_context,
                    "buf_offset {} + blocksize {} > chunk_context {} (source_offset={}, overlap_start={})",
                    buf_offset,
                    blocksize,
                    chunk_context,
                    source_offset,
                    overlap_start
                );
                let block_data = &buf[buf_offset..buf_offset + blocksize];
                Self::write_at_offset(file, block_data, offset)?;
            }

            total_matched += matched_blocks.len();
            file_offset += bytes_read;
            if let Some(cb) = &self.scan_progress_callback {
                cb(file_offset as u64, file_size as u64);
            }

            if bytes_read < read_len {
                break;
            }
        }

        Ok(total_matched)
    }

    fn read_at_offset(
        file: &mut File,
        data: &mut [u8],
        offset: u64,
    ) -> Result<usize, AssemblyError> {
        file.seek(SeekFrom::Start(offset))?;
        Ok(file.read(data)?)
    }

    fn write_at_offset(file: &mut File, data: &[u8], offset: u64) -> Result<(), AssemblyError> {
        file.seek(SeekFrom::Start(offset))?;
        file.write_all(data)?;
        Ok(())
    }

    fn ensure_file(&mut self) -> Result<&mut File, AssemblyError> {
        if self.file.is_none() {
            let file = OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(&self.temp_path)?;
            self.file = Some(file);
        }
        Ok(self.file.as_mut().unwrap())
    }

    /// Downloads the blocks still missing and writes those that verify.
    ///
    /// Blocks are fetched from the control file's first URL, with nearby
    /// ranges merged as [`Self::set_range_gap_threshold`] allows. Each block
    /// is checked against its checksum before it is written. One that fails
    /// stays missing, so a server sending wrong data cannot corrupt the
    /// output.
    ///
    /// Returns the number of blocks written. Call it until it returns 0, then
    /// check [`Self::is_complete`]: 0 means nothing was missing or nothing
    /// that arrived verified, and looping on [`Self::is_complete`] alone
    /// never ends against a server that sends the wrong bytes.
    ///
    /// # Errors
    ///
    /// Returns [`AssemblyError::NoUrls`] when the control file lists no URL,
    /// [`AssemblyError::Http`] or [`AssemblyError::Io`] when a request or a
    /// write fails, and [`AssemblyError::Aborted`] when the abort flag is
    /// raised.
    pub fn download_missing_blocks(&mut self) -> Result<usize, AssemblyError> {
        let relative_url = self
            .control
            .urls
            .first()
            .ok_or(AssemblyError::NoUrls)?
            .clone();

        let url = self
            .base_url
            .as_ref()
            .map(|base| resolve_url(base, &relative_url))
            .unwrap_or(relative_url);

        let block_ranges = self.matcher.needed_block_ranges();
        if block_ranges.is_empty() {
            return Ok(0);
        }

        let byte_ranges = byte_ranges_from_block_ranges(
            &block_ranges,
            self.control.blocksize,
            self.control.length,
        );
        let merged_ranges = merge_byte_ranges(&byte_ranges, self.range_gap_threshold);
        let mut downloaded_blocks = 0;
        let blocksize = self.control.blocksize;
        let total_blocks = self.matcher.total_blocks();
        let mut padded_buf = vec![0u8; blocksize];

        let abort_flag = self.abort_flag.clone();

        for (range_start, range_end) in merged_ranges {
            if aborted(&abort_flag) {
                return Err(AssemblyError::Aborted);
            }

            let mut reader = self.http.fetch_range_reader(&url, range_start, range_end)?;
            let block_start = (range_start / blocksize as u64) as usize;
            let initial_offset = (range_start % blocksize as u64) as usize;

            let mut buf = vec![0u8; blocksize + 64 * 1024];
            buf[..initial_offset].fill(0);
            let mut buf_len = initial_offset;
            let mut current_block_id = block_start;

            let mut read_buf = [0u8; 64 * 1024];
            loop {
                if aborted(&abort_flag) {
                    return Err(AssemblyError::Aborted);
                }

                let n = reader.read(&mut read_buf)?;
                if n == 0 {
                    break;
                }

                if buf_len + n > buf.len() {
                    buf.resize(buf_len + n, 0);
                }
                buf[buf_len..buf_len + n].copy_from_slice(&read_buf[..n]);
                buf_len += n;

                while buf_len >= blocksize {
                    if current_block_id >= total_blocks {
                        break;
                    }

                    if !self.matcher.is_block_known(current_block_id) {
                        let block_data_end = if current_block_id == total_blocks - 1 {
                            let last_block_size = (self.control.length as usize) % blocksize;
                            if last_block_size == 0 {
                                blocksize
                            } else {
                                last_block_size
                            }
                        } else {
                            blocksize
                        };

                        let block_data = &buf[..block_data_end];
                        padded_buf[..block_data_end].copy_from_slice(block_data);
                        if block_data_end < blocksize {
                            padded_buf[block_data_end..].fill(0);
                        }

                        if self.matcher.submit_blocks(&padded_buf, current_block_id)? {
                            let file = self.ensure_file()?;
                            let file_offset = (current_block_id * blocksize) as u64;
                            Self::write_at_offset(file, block_data, file_offset)?;
                            downloaded_blocks += 1;
                            self.report_progress();
                        }
                    }

                    current_block_id += 1;
                    buf.copy_within(blocksize..buf_len, 0);
                    buf_len -= blocksize;
                }
            }

            if buf_len > 0
                && current_block_id < total_blocks
                && !self.matcher.is_block_known(current_block_id)
            {
                let block_data = &buf[..buf_len];
                padded_buf[..buf_len].copy_from_slice(block_data);
                padded_buf[buf_len..].fill(0);

                if self.matcher.submit_blocks(&padded_buf, current_block_id)? {
                    let file = self.ensure_file()?;
                    let file_offset = (current_block_id * blocksize) as u64;
                    Self::write_at_offset(file, block_data, file_offset)?;
                    downloaded_blocks += 1;
                    self.report_progress();
                }
            }
        }

        Ok(downloaded_blocks)
    }

    /// Verifies the assembled file and moves it to the output path.
    ///
    /// The temporary file is cut to the target's length, its SHA-1 is
    /// compared with the control file's when the control file has one, and
    /// it is flushed to disk and renamed to the output path, so a crash
    /// cannot leave a partly written file there. A file already at the output
    /// path is replaced, and its permissions carry over to the new one. On any error the assembly is
    /// dropped, which removes the temporary file.
    ///
    /// # Errors
    ///
    /// Returns [`AssemblyError::Control`] when blocks are still missing,
    /// [`AssemblyError::ChecksumMismatch`] when the digests differ, and
    /// [`AssemblyError::Io`] when reading, truncating, flushing or renaming
    /// fails.
    pub fn complete(mut self) -> Result<(), AssemblyError> {
        if !self.matcher.is_complete() {
            return Err(AssemblyError::Control(
                "Not all blocks downloaded".to_string(),
            ));
        }

        let file_length = self.control.length;
        let expected_sha1 = self.control.sha1.clone();

        let file = self.ensure_file()?;
        file.set_len(file_length)?;

        if let Some(ref expected) = expected_sha1 {
            file.seek(SeekFrom::Start(0))?;
            let actual_checksum = calc_sha1_stream(file)?;
            let actual_hex = hex_encode(&actual_checksum);

            if !actual_hex.eq_ignore_ascii_case(expected) {
                return Err(AssemblyError::ChecksumMismatch {
                    expected: expected.clone(),
                    actual: actual_hex,
                });
            }
        }
        file.sync_all()?;

        self.file.take();
        // Replacing an existing file, such as an executable AppImage updated
        // in place, keeps its permissions rather than the temp file's.
        if let Ok(existing) = std::fs::metadata(&self.output_path) {
            std::fs::set_permissions(&self.temp_path, existing.permissions())?;
        }
        std::fs::rename(&self.temp_path, &self.output_path)?;

        Ok(())
    }

    /// Discards the assembly and its partial output.
    ///
    /// Equivalent to dropping it; both remove the temp file.
    pub fn abort(self) {}
}

impl Drop for ZsyncAssembly {
    /// Removes the temp file unless [`ZsyncAssembly::complete`] already renamed
    /// it into place.
    ///
    /// Every failure path would otherwise leave a partial `.zsync-tmp` beside
    /// the output. Nothing resumes from it: a later run builds a fresh matcher
    /// that cannot tell which of its blocks are already written.
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.temp_path);
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

/// The directory of `url`, against which relative URLs resolve.
///
/// The query and fragment are dropped first, because a signed URL's query can
/// itself contain `/`.
fn extract_base_url(url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    path.rfind('/')
        .map(|i| path[..=i].to_string())
        .unwrap_or_default()
}

fn resolve_url(base: &str, relative: &str) -> String {
    if relative.contains("://") {
        return relative.to_string();
    }
    if relative.starts_with('/') {
        let scheme_end = base.find("://").map(|i| i + 3).unwrap_or(0);
        let host_end = base[scheme_end..]
            .find('/')
            .map(|i| scheme_end + i)
            .unwrap_or(base.len());
        format!("{}{}", &base[..host_end], relative)
    } else {
        format!("{}{}", base, relative)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_scan_reports_progress_through_the_whole_file() {
        let dir = std::env::temp_dir().join(format!("zsync-scan-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        // Over two 32 MiB chunks, so the scan reports more than once and each
        // chunk is large enough for the matcher to split across threads.
        // Xorshift bytes, because simpler generators repeat at unaligned
        // offsets, which lets a greedy scan legitimately skip a block.
        let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
        let data: Vec<u8> = (0..70_000_000)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                (x >> 32) as u8
            })
            .collect();
        let seed = dir.join("seed.bin");
        std::fs::write(&seed, &data).expect("write seed");

        let control =
            ControlFile::generate(&mut std::io::Cursor::new(&data), "f", "f", None).expect("gen");
        let mut assembly = ZsyncAssembly::new(control, &dir.join("out.bin")).expect("assembly");
        let calls = Arc::new(std::sync::Mutex::new(Vec::new()));
        let sink = Arc::clone(&calls);
        assembly.set_scan_progress_callback(move |done, total| {
            sink.lock().unwrap().push((done, total));
        });
        let found = assembly.submit_source_file(&seed).expect("scan");
        assert_eq!(
            found,
            assembly.block_stats().1,
            "every block is in the seed"
        );
        assert!(assembly.is_complete());

        let calls = calls.lock().unwrap();
        let size = data.len() as u64;
        assert!(
            calls.len() > 1,
            "one report for a multi-chunk file: {calls:?}"
        );
        assert!(calls.iter().all(|&(_, total)| total == size));
        assert!(calls.windows(2).all(|w| w[0].0 < w[1].0), "{calls:?}");
        assert_eq!(calls.last(), Some(&(size, size)));

        drop(assembly);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn replacing_an_executable_keeps_it_executable() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("zsync-perms-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("mkdir");
        let data: Vec<u8> = (0..10_000u32).map(|i| (i % 253) as u8).collect();
        let seed = dir.join("new.bin");
        std::fs::write(&seed, &data).expect("write seed");
        let out = dir.join("app.AppImage");
        std::fs::write(&out, b"old version").expect("write old");
        std::fs::set_permissions(&out, std::fs::Permissions::from_mode(0o755)).expect("chmod");

        let control =
            ControlFile::generate(&mut std::io::Cursor::new(&data), "f", "f", None).expect("gen");
        let mut assembly = ZsyncAssembly::new(control, &out).expect("assembly");
        assembly.submit_source_file(&seed).expect("scan");
        assembly.complete().expect("complete");

        assert_eq!(std::fs::read(&out).expect("read"), data);
        let mode = std::fs::metadata(&out).expect("stat").permissions().mode();
        assert_eq!(mode & 0o777, 0o755, "mode {mode:o}");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn outputs_differing_in_extension_get_their_own_temp_file() {
        let mut header = b"zsync: 0.6.2\nBlocksize: 2048\nLength: 1\n\n".to_vec();
        header.extend_from_slice(&[0u8; 20]);
        let control = ControlFile::parse(&header[..]).expect("parse");

        let iso = ZsyncAssembly::new(control.clone(), Path::new("dir/x.iso")).expect("iso");
        let img = ZsyncAssembly::new(control, Path::new("dir/x.img")).expect("img");
        assert_eq!(iso.temp_path, Path::new("dir/x.iso.zsync-tmp"));
        assert_eq!(img.temp_path, Path::new("dir/x.img.zsync-tmp"));
    }

    #[test]
    fn a_wrapped_http_error_is_not_prefixed_twice() {
        let err = AssemblyError::from(crate::http::HttpError::Http("timed out".to_string()));
        assert_eq!(err.to_string(), "HTTP error: timed out");
    }

    #[test]
    fn test_hex_encode() {
        assert_eq!(hex_encode(&[0x00, 0xff, 0x10]), "00ff10");
    }

    #[test]
    fn test_extract_base_url() {
        assert_eq!(
            extract_base_url("https://example.com/path/file.zsync"),
            "https://example.com/path/"
        );
        assert_eq!(
            extract_base_url("https://example.com/file.zsync"),
            "https://example.com/"
        );
        assert_eq!(
            extract_base_url("https://example.com/a/file.zsync?sig=ab/cd&e=1"),
            "https://example.com/a/"
        );
        assert_eq!(
            extract_base_url("https://example.com/a/file.zsync#x/y"),
            "https://example.com/a/"
        );
    }

    #[test]
    fn test_resolve_url() {
        assert_eq!(
            resolve_url("https://example.com/path/", "file.bin"),
            "https://example.com/path/file.bin"
        );
        assert_eq!(
            resolve_url("https://example.com/path/", "/file.bin"),
            "https://example.com/file.bin"
        );
        assert_eq!(
            resolve_url("https://example.com/path/", "https://other.com/file.bin"),
            "https://other.com/file.bin"
        );
    }
}
