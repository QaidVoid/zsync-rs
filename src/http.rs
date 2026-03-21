use std::io::Read;

use crate::control::ControlFile;

#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    #[error("HTTP error: {0}")]
    Http(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Invalid URL: {0}")]
    InvalidUrl(String),
    #[error("No URLs available")]
    NoUrls,
}

pub struct HttpClient {
    agent: ureq::Agent,
}

impl Default for HttpClient {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpClient {
    pub fn new() -> Self {
        Self {
            agent: ureq::Agent::config_builder()
                .https_only(false)
                .build()
                .new_agent(),
        }
    }

    pub fn fetch_control_file(&self, url: &str) -> Result<ControlFile, HttpError> {
        let response = self
            .agent
            .get(url)
            .call()
            .map_err(|e| HttpError::Http(e.to_string()))?;

        let mut reader = response.into_body().into_reader();
        ControlFile::parse(&mut reader).map_err(|e| HttpError::Http(e.to_string()))
    }

    pub fn fetch_range(&self, url: &str, start: u64, end: u64) -> Result<Vec<u8>, HttpError> {
        let range_header = format!("bytes={}-{}", start, end);

        let response = self
            .agent
            .get(url)
            .header("Range", &range_header)
            .call()
            .map_err(|e| HttpError::Http(e.to_string()))?;

        let status = response.status();
        if status != 206 && status != 200 {
            return Err(HttpError::Http(format!(
                "Expected 206 Partial Content, got {}",
                status
            )));
        }

        let mut buf = Vec::new();
        response.into_body().into_reader().read_to_end(&mut buf)?;

        Ok(buf)
    }

    pub fn fetch_ranges(
        &self,
        url: &str,
        ranges: &[(u64, u64)],
        blocksize: usize,
    ) -> Result<Vec<(u64, Vec<u8>)>, HttpError> {
        let mut results = Vec::new();

        for &(start, end) in ranges {
            let data = self.fetch_range(url, start, end)?;
            let aligned_start = (start / blocksize as u64) * blocksize as u64;
            results.push((aligned_start, data));
        }

        Ok(results)
    }
}

/// Merge byte ranges to minimize HTTP requests.
/// If merging all ranges into one wastes less than the needed data itself, use a single request.
/// Otherwise merge ranges with gaps smaller than 1 MB.
pub fn merge_byte_ranges(ranges: &[(u64, u64)]) -> Vec<(u64, u64)> {
    if ranges.len() <= 1 {
        return ranges.to_vec();
    }

    let total_needed: u64 = ranges.iter().map(|(s, e)| e - s + 1).sum();
    let total_span = ranges.last().unwrap().1 - ranges.first().unwrap().0 + 1;

    // If a single request wastes less than the needed data, just use one request
    if total_span <= total_needed * 2 {
        return vec![(ranges.first().unwrap().0, ranges.last().unwrap().1)];
    }

    // Otherwise merge ranges with small gaps
    let mut merged = vec![ranges[0]];
    for &(start, end) in &ranges[1..] {
        let last = merged.last_mut().unwrap();
        let gap = start.saturating_sub(last.1 + 1);
        if gap < 1024 * 1024 {
            last.1 = end;
        } else {
            merged.push((start, end));
        }
    }
    merged
}

pub fn byte_ranges_from_block_ranges(
    block_ranges: &[(usize, usize)],
    blocksize: usize,
    file_length: u64,
) -> Vec<(u64, u64)> {
    block_ranges
        .iter()
        .map(|&(start_block, end_block)| {
            let start = start_block as u64 * blocksize as u64;
            let end =
                ((end_block as u64 * blocksize as u64).saturating_sub(1)).min(file_length - 1);
            (start, end)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_byte_ranges_from_block_ranges() {
        let block_ranges = vec![(0, 2), (4, 6)];
        let byte_ranges = byte_ranges_from_block_ranges(&block_ranges, 1024, 10000);
        assert_eq!(byte_ranges, vec![(0, 2047), (4096, 6143)]);
    }

    #[test]
    fn test_byte_ranges_clamped_to_file_length() {
        let block_ranges = vec![(9, 10)];
        let byte_ranges = byte_ranges_from_block_ranges(&block_ranges, 1024, 9500);
        assert_eq!(byte_ranges, vec![(9216, 9499)]);
    }
}
