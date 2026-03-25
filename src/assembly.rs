use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::FileExt;
use std::path::Path;

use crate::checksum::calc_sha1_stream;
use crate::control::ControlFile;
use crate::http::{
    DEFAULT_RANGE_GAP_THRESHOLD, HttpClient, byte_ranges_from_block_ranges, merge_byte_ranges,
};
use crate::matcher::BlockMatcher;
use crate::matcher::MatchError;

const STREAM_CHUNK_SIZE: usize = 1024 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum AssemblyError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("HTTP error: {0}")]
    Http(#[from] crate::http::HttpError),
    #[error("Matcher error: {0}")]
    Matcher(#[from] MatchError),
    #[error("Control file error: {0}")]
    Control(String),
    #[error("Checksum mismatch: expected {expected}, got {actual}")]
    ChecksumMismatch { expected: String, actual: String },
    #[error("No URLs available")]
    NoUrls,
}

pub type ProgressCallback = Box<dyn Fn(u64, u64) + Send + Sync>;

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
}

impl ZsyncAssembly {
    pub fn new(control: ControlFile, output_path: &Path) -> Result<Self, AssemblyError> {
        Self::with_base_url(control, output_path, None)
    }

    pub fn with_base_url(
        control: ControlFile,
        output_path: &Path,
        base_url: Option<&str>,
    ) -> Result<Self, AssemblyError> {
        let matcher = BlockMatcher::new(&control);
        let http = HttpClient::new();
        let temp_path = output_path.with_extension("zsync-tmp");

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
        })
    }

    pub fn from_url(control_url: &str, output_path: &Path) -> Result<Self, AssemblyError> {
        let http = HttpClient::new();
        let control = http.fetch_control_file(control_url)?;
        let base_url = extract_base_url(control_url);
        Self::with_base_url(control, output_path, Some(&base_url))
    }

    pub fn set_range_gap_threshold(&mut self, threshold: u64) {
        self.range_gap_threshold = threshold;
    }

    pub fn set_progress_callback<F>(&mut self, callback: F)
    where
        F: Fn(u64, u64) + Send + Sync + 'static,
    {
        self.progress_callback = Some(Box::new(callback));
    }

    fn report_progress(&self) {
        if let Some(ref cb) = self.progress_callback {
            let (done, total) = self.progress();
            cb(done, total);
        }
    }

    pub fn progress(&self) -> (u64, u64) {
        let total = self.control.length;
        let got = self.matcher.blocks_todo();
        let blocks_done = self.matcher.total_blocks() - got;
        let done_bytes = (blocks_done * self.control.blocksize) as u64;
        (done_bytes.min(total), total)
    }

    pub fn is_complete(&self) -> bool {
        self.matcher.is_complete()
    }

    pub fn block_stats(&self) -> (usize, usize) {
        let total = self.matcher.total_blocks();
        let todo = self.matcher.blocks_todo();
        (total - todo, total)
    }

    pub fn submit_source_file(&mut self, path: &Path) -> Result<usize, AssemblyError> {
        let file = File::open(path)?;
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

        loop {
            let overlap_start = file_offset.saturating_sub(context);
            let overlap_len = file_offset - overlap_start;

            if overlap_len > 0 {
                file.read_at(&mut buf[..overlap_len], overlap_start as u64)?;
            }

            let read_start = overlap_len;
            let read_len = chunk_size;

            let bytes_read = file.read_at(
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
                file.read_at(
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

            if bytes_read < read_len {
                break;
            }
        }

        Ok(total_matched)
    }

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

        loop {
            let overlap_start = file_offset.saturating_sub(context);
            let overlap_len = file_offset - overlap_start;

            if overlap_len > 0 {
                file.read_at(&mut buf[..overlap_len], overlap_start as u64)?;
            }

            let read_start = overlap_len;
            let read_len = chunk_size;

            let bytes_read = file.read_at(
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
                file.read_at(
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

            if bytes_read < read_len {
                break;
            }
        }

        Ok(total_matched)
    }

    fn write_at_offset(file: &File, data: &[u8], offset: u64) -> Result<(), AssemblyError> {
        file.write_all_at(data, offset)?;
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

        for (range_start, range_end) in merged_ranges {
            let mut reader = self.http.fetch_range_reader(&url, range_start, range_end)?;
            let block_start = (range_start / blocksize as u64) as usize;
            let initial_offset = (range_start % blocksize as u64) as usize;

            let mut buf = vec![0u8; blocksize + 64 * 1024];
            buf[..initial_offset].fill(0);
            let mut buf_len = initial_offset;
            let mut current_block_id = block_start;

            let mut read_buf = [0u8; 64 * 1024];
            loop {
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

        drop(self.file);
        std::fs::rename(&self.temp_path, &self.output_path)?;

        Ok(())
    }

    pub fn abort(self) {
        let _ = std::fs::remove_file(&self.temp_path);
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{:02x}", b)).collect()
}

fn extract_base_url(url: &str) -> String {
    url.rfind('/')
        .map(|i| url[..=i].to_string())
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
