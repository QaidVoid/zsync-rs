use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use crate::checksum::calc_sha1;
use crate::control::ControlFile;
use crate::http::{HttpClient, byte_ranges_from_block_ranges};
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
    matcher: BlockMatcher,
    http: HttpClient,
    output_path: std::path::PathBuf,
    temp_path: std::path::PathBuf,
    file: Option<File>,
}

impl ZsyncAssembly {
    pub fn new(control: ControlFile, output_path: &Path) -> Result<Self, AssemblyError> {
        let matcher = BlockMatcher::new(&control);
        let http = HttpClient::new();
        let temp_path = output_path.with_extension("zsync-tmp");

        Ok(Self {
            control,
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
        Self::new(control, output_path)
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

    pub fn submit_source_file(&mut self, path: &Path) -> Result<usize, AssemblyError> {
        let mut file = File::open(path)?;
        let mut buf = Vec::new();
        file.read_to_end(&mut buf)?;

        let blocksize = self.control.blocksize;
        let context = blocksize * 2;
        let mut total_blocks = 0;

        if buf.len() < context {
            return Ok(0);
        }

        let offset = 0u64;
        total_blocks += self.matcher.submit_source_data(&buf, offset);

        Ok(total_blocks)
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
        let url = self
            .control
            .urls
            .first()
            .ok_or(AssemblyError::NoUrls)?
            .clone();

        let block_ranges = self.matcher.needed_block_ranges();
        if block_ranges.is_empty() {
            return Ok(0);
        }

        let byte_ranges = byte_ranges_from_block_ranges(
            &block_ranges,
            self.control.blocksize,
            self.control.length,
        );
        let mut downloaded_blocks = 0;

        for (start, end) in byte_ranges {
            let data = self.http.fetch_range(&url, start, end)?;

            let blocksize = self.control.blocksize;
            let block_start = (start / blocksize as u64) as usize;
            let num_blocks = data.len() / blocksize;

            for i in 0..num_blocks {
                let block_id = block_start + i;
                let block_offset = i * blocksize;
                let block_data = &data[block_offset..block_offset + blocksize];

                if self.matcher.submit_blocks(block_data, block_id)? {
                    let file = self.ensure_file()?;
                    let write_offset = block_id * blocksize;
                    file.seek(SeekFrom::Start(write_offset as u64))?;
                    file.write_all(block_data)?;
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hex_encode() {
        assert_eq!(hex_encode(&[0x00, 0xff, 0x10]), "00ff10");
    }
}
