use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::FileExt;
use std::path::Path;

use crate::checksum::calc_sha1;
use crate::control::ControlFile;
use crate::http::{HttpClient, byte_ranges_from_block_ranges, merge_byte_ranges};
use crate::matcher::BlockMatcher;
use crate::matcher::MatchError;

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

pub struct ZsyncAssembly {
    control: ControlFile,
    base_url: Option<String>,
    matcher: BlockMatcher,
    http: HttpClient,
    output_path: std::path::PathBuf,
    temp_path: std::path::PathBuf,
    file: Option<File>,
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
        })
    }

    pub fn from_url(control_url: &str, output_path: &Path) -> Result<Self, AssemblyError> {
        let http = HttpClient::new();
        let control = http.fetch_control_file(control_url)?;
        let base_url = extract_base_url(control_url);
        Self::with_base_url(control, output_path, Some(&base_url))
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
        let mut file = File::open(path)?;
        let mut buf = Vec::new();
        file.read_to_end(&mut buf)?;

        let blocksize = self.control.blocksize;
        let context = blocksize * self.control.hash_lengths.seq_matches as usize;

        if buf.len() < context {
            return Ok(0);
        }

        // Zero-pad to allow scanning the last context bytes of the source
        let original_len = buf.len();
        buf.resize(original_len + context, 0);

        let matched_blocks = self.matcher.submit_source_data(&buf, 0);

        for (block_id, source_offset) in &matched_blocks {
            let file_handle = self.ensure_file()?;
            let offset = (block_id * blocksize) as u64;
            let block_data = &buf[*source_offset..source_offset + blocksize];
            Self::write_at_offset(file_handle, block_data, offset)?;
        }

        Ok(matched_blocks.len())
    }

    /// Scan the partially-assembled output for duplicate target blocks.
    /// If the target has the same content at positions A and B, and A was matched
    /// from a seed file, this finds B without downloading it.
    pub fn submit_self_referential(&mut self) -> Result<usize, AssemblyError> {
        if self.file.is_none() {
            return Ok(0);
        }

        // Flush writes and read the temp file
        let file = self.file.as_mut().unwrap();
        file.sync_all()?;

        let mut buf = Vec::new();
        file.seek(SeekFrom::Start(0))?;
        file.read_to_end(&mut buf)?;

        let blocksize = self.control.blocksize;
        let context = blocksize * self.control.hash_lengths.seq_matches as usize;

        if buf.len() < context {
            return Ok(0);
        }

        let original_len = buf.len();
        buf.resize(original_len + context, 0);

        let matched_blocks = self.matcher.submit_source_data(&buf, 0);

        for (block_id, source_offset) in &matched_blocks {
            let offset = (block_id * blocksize) as u64;
            let block_data = &buf[*source_offset..source_offset + blocksize];
            Self::write_at_offset(self.file.as_ref().unwrap(), block_data, offset)?;
        }

        Ok(matched_blocks.len())
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
        let merged_ranges = merge_byte_ranges(&byte_ranges);
        let mut downloaded_blocks = 0;
        let blocksize = self.control.blocksize;
        let mut padded_buf = vec![0u8; blocksize];

        for (start, end) in merged_ranges {
            let data = self.http.fetch_range(&url, start, end)?;

            let block_start = (start / blocksize as u64) as usize;
            let total_blocks = self.matcher.total_blocks();
            let num_blocks = data.len().div_ceil(blocksize);

            for i in 0..num_blocks {
                let block_id = block_start + i;
                if block_id >= total_blocks {
                    break;
                }

                if self.matcher.is_block_known(block_id) {
                    continue;
                }

                let block_offset = i * blocksize;
                let block_end = std::cmp::min(block_offset + blocksize, data.len());
                let block_data = &data[block_offset..block_end];

                padded_buf[..block_data.len()].copy_from_slice(block_data);
                if block_data.len() < blocksize {
                    padded_buf[block_data.len()..].fill(0);
                }

                if self.matcher.submit_blocks(&padded_buf, block_id)? {
                    let file = self.ensure_file()?;
                    let offset = (block_id * blocksize) as u64;
                    Self::write_at_offset(file, block_data, offset)?;
                    downloaded_blocks += 1;
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
            let mut buf = Vec::new();
            file.read_to_end(&mut buf)?;

            let actual_checksum = calc_sha1(&buf);
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
