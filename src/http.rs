//! HTTP transport: control files, byte ranges, and planning range requests.
//!
//! [`HttpClient`] fetches a control file and the byte ranges of a target. The
//! free functions turn the block ranges a matcher still needs into byte
//! ranges, and merge ranges separated by small gaps, since one request that
//! re-downloads a few known bytes is usually cheaper than another round trip.

use std::io::Read;
use std::time::Duration;

use crate::control::ControlFile;

/// Why a fetch failed.
#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    /// The request failed, the server answered with an unexpected status, or
    /// a fetched control file did not parse. The message says which.
    #[error("HTTP error: {0}")]
    Http(String),
    /// Reading a response body failed.
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    /// A URL could not be used. Not currently returned by this crate.
    #[error("Invalid URL: {0}")]
    InvalidUrl(String),
    /// There was no URL to fetch from. Not currently returned by this crate.
    #[error("No URLs available")]
    NoUrls,
}

/// The body of one range response, capped at the length requested.
///
/// Returned by [`HttpClient::fetch_range_reader`].
pub struct HttpRangeReader {
    reader: Box<dyn std::io::Read + Send + Sync>,
}

impl std::io::Read for HttpRangeReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.reader.read(buf)
    }
}

/// Cap on establishing a connection, covering the TCP handshake and TLS
/// setup but never the transfer that follows.
///
/// ureq applies no connect timeout of its own, which leaves an address that
/// silently drops SYNs costing the kernel's full retry schedule, around two
/// minutes each. A mirror redirect that lands on a host with several such
/// addresses stalls for as long as it takes to exhaust them all, and the
/// error raised at the end is an unhelpful synthetic "connection refused".
/// ureq divides this budget across the resolved addresses, so the cap is
/// generous enough that a distant mirror still gets several seconds.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);

/// Cap on the DNS lookup, for the same reason as [`CONNECT_TIMEOUT`].
///
/// Deliberately the longer of the two. ureq attributes a timeout to whichever
/// phase would have expired first, so an equal pair reports a connect failure
/// as a resolve one and points whoever reads the error at the wrong thing.
pub const RESOLVE_TIMEOUT: Duration = Duration::from_secs(30);

/// HTTP transport for fetching control files and byte ranges.
pub struct HttpClient {
    agent: ureq::Agent,
}

impl Default for HttpClient {
    fn default() -> Self {
        Self::new()
    }
}

impl HttpClient {
    /// A client permitting plain HTTP, with a bound on reaching a host.
    ///
    /// zsync is routinely served over HTTP and across mirror redirects, so
    /// the scheme policy is permissive on purpose. An embedder with a
    /// stricter one, such as one that must refuse a downgrade to HTTP or
    /// pin its own trust roots, should build an agent and pass it to
    /// [`HttpClient::with_agent`] rather than relying on these.
    ///
    /// [`CONNECT_TIMEOUT`] and [`RESOLVE_TIMEOUT`] apply. Neither bounds the
    /// transfer itself, so a mirror that trickles bytes for an hour is left
    /// alone; they only cap how long a host that never answers can cost.
    pub fn new() -> Self {
        Self {
            agent: ureq::Agent::config_builder()
                .https_only(false)
                .timeout_connect(Some(CONNECT_TIMEOUT))
                .timeout_resolve(Some(RESOLVE_TIMEOUT))
                .build()
                .new_agent(),
        }
    }

    /// A client using a caller-supplied agent.
    ///
    /// The agent carries the whole transport policy: TLS roots, whether a
    /// redirect may downgrade to HTTP, timeouts and proxies. Supplying one
    /// keeps that decision with the embedder, which is the only place that
    /// knows what the transferred bytes are trusted for.
    pub fn with_agent(agent: ureq::Agent) -> Self {
        Self { agent }
    }

    /// Fetches and parses the control file at `url`.
    ///
    /// # Errors
    ///
    /// Returns [`HttpError::Http`] when the request fails or the response
    /// does not parse as a control file.
    pub fn fetch_control_file(&self, url: &str) -> Result<ControlFile, HttpError> {
        let response = self
            .agent
            .get(url)
            .call()
            .map_err(|e| HttpError::Http(e.to_string()))?;

        let mut reader = response.into_body().into_reader();
        ControlFile::parse(&mut reader).map_err(|e| HttpError::Http(e.to_string()))
    }

    /// A reader over `start..=end` of `url`.
    ///
    /// The reader is capped at the requested length. Without that cap the
    /// origin decides how much the client reads: a response longer than
    /// the range is consumed in full, and callers that accumulate it, such
    /// as block assembly, grow a buffer to match. Bytes past the range
    /// were never asked for and are of no use, so they are not read.
    ///
    /// A `200 OK` is accepted only for a range starting at zero. A server
    /// that answers `200` has ignored `Range` and sends the file from its
    /// first byte, which is right for such a range and wrong for any other.
    ///
    /// # Errors
    ///
    /// Returns [`HttpError::Http`] when the request fails, when the status is
    /// neither 200 nor 206, or when it is 200 for a range that does not start
    /// at zero.
    pub fn fetch_range_reader(
        &self,
        url: &str,
        start: u64,
        end: u64,
    ) -> Result<HttpRangeReader, HttpError> {
        let range_header = format!("bytes={}-{}", start, end);
        let requested = end.saturating_sub(start).saturating_add(1);

        let response = self
            .agent
            .get(url)
            .header("Range", &range_header)
            .call()
            .map_err(|e| HttpError::Http(e.to_string()))?;

        let status = response.status();
        if status == 200 && start > 0 {
            return Err(HttpError::Http(
                "server ignored the Range header, so it cannot serve partial downloads".to_string(),
            ));
        }
        if status != 206 && status != 200 {
            return Err(HttpError::Http(format!(
                "Expected 206 Partial Content, got {}",
                status
            )));
        }

        Ok(HttpRangeReader {
            reader: Box::new(response.into_body().into_reader().take(requested)),
        })
    }

    /// Fetches `start..=end` of `url` into memory.
    ///
    /// The same cap and status handling as [`Self::fetch_range_reader`]
    /// apply.
    ///
    /// # Errors
    ///
    /// Returns [`HttpError::Http`] when the request fails and
    /// [`HttpError::Io`] when reading the body fails.
    pub fn fetch_range(&self, url: &str, start: u64, end: u64) -> Result<Vec<u8>, HttpError> {
        let mut reader = self.fetch_range_reader(url, start, end)?;
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf)?;
        Ok(buf)
    }

    /// Fetches each inclusive range in `ranges`, one request at a time.
    ///
    /// Each body is paired with its start offset rounded down to a multiple
    /// of `blocksize`. The ranges are expected to start on a block boundary,
    /// as those from [`byte_ranges_from_block_ranges`] do; otherwise the
    /// offset names a point before the body's first byte.
    ///
    /// # Errors
    ///
    /// Stops at and returns the first error from [`Self::fetch_range`].
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

/// Default gap threshold for range merging (256 KiB, same as zsync2).
pub const DEFAULT_RANGE_GAP_THRESHOLD: u64 = 256 * 1024;

/// Merge byte ranges to minimize HTTP requests.
///
/// `ranges` are inclusive and sorted by start, without overlap. Two neighbours
/// separated by at most `gap_threshold` bytes become one range that also
/// covers the gap, so a threshold of zero merges only ranges that touch.
pub fn merge_byte_ranges(ranges: &[(u64, u64)], gap_threshold: u64) -> Vec<(u64, u64)> {
    if ranges.len() <= 1 {
        return ranges.to_vec();
    }

    let mut merged = vec![ranges[0]];
    for &(start, end) in &ranges[1..] {
        let last = merged.last_mut().unwrap();
        let gap = start.saturating_sub(last.1 + 1);
        if gap <= gap_threshold {
            last.1 = end;
        } else {
            merged.push((start, end));
        }
    }
    merged
}

/// Converts block ranges into the byte ranges that hold them.
///
/// Each block range is half-open, from its first block to one past its last,
/// as [`BlockMatcher::needed_block_ranges`](crate::BlockMatcher::needed_block_ranges)
/// returns them. Each byte range is inclusive, ready for a `Range` header, and
/// the last one is clamped to `file_length` so a short final block is not
/// requested past the end of the file.
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

    /// A server that answers any request with far more data than asked
    /// for, which is what a hostile or broken origin does.
    fn overlong_server(body_len: usize) -> String {
        use std::io::Write;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            if let Ok((mut sock, _)) = listener.accept() {
                let mut req = [0u8; 1024];
                let _ = std::io::Read::read(&mut sock, &mut req);
                let hdr = format!(
                    "HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 0-1023/{}\r\nContent-Length: {}\r\n\r\n",
                    body_len, body_len
                );
                let _ = sock.write_all(hdr.as_bytes());
                let chunk = vec![0u8; 64 * 1024];
                let mut sent = 0;
                while sent < body_len {
                    let n = chunk.len().min(body_len - sent);
                    if sock.write_all(&chunk[..n]).is_err() {
                        break;
                    }
                    sent += n;
                }
            }
        });
        format!("http://{}/f", addr)
    }

    /// A server that ignores `Range` and answers every request with `200`
    /// and the whole body, as a plain static file server without range
    /// support does.
    fn range_ignoring_server(body_len: usize) -> String {
        use std::io::Write;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for mut sock in listener.incoming().flatten() {
                let mut req = [0u8; 1024];
                let _ = std::io::Read::read(&mut sock, &mut req);
                let hdr = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {body_len}\r\nConnection: close\r\n\r\n"
                );
                let _ = sock.write_all(hdr.as_bytes());
                let _ = sock.write_all(&vec![0u8; body_len]);
            }
        });
        format!("http://{}/f", addr)
    }

    #[test]
    fn a_full_response_to_a_later_range_is_refused() {
        let url = range_ignoring_server(4096);
        let client = HttpClient::new();

        // From byte zero the whole body is the right data.
        assert_eq!(
            client.fetch_range(&url, 0, 1023).expect("fetch").len(),
            1024
        );

        // From anywhere else it is the wrong data, so it must not be read.
        let err = client
            .fetch_range(&url, 2048, 3071)
            .expect_err("a 200 for a later range must be refused");
        assert!(err.to_string().contains("Range"), "unexpected error: {err}");
    }

    #[test]
    fn a_range_response_is_capped_at_what_was_requested() {
        // 1 KiB asked for, 8 MiB offered. Accepting the surplus lets any
        // origin decide how much memory the client spends.
        let url = overlong_server(8 * 1024 * 1024);
        let client = HttpClient::new();
        let data = client.fetch_range(&url, 0, 1023).expect("fetch");
        assert_eq!(
            data.len(),
            1024,
            "a range response must be capped at the requested length"
        );
    }

    #[test]
    fn the_default_client_bounds_reaching_a_host_but_not_the_transfer() {
        let timeouts = HttpClient::new().agent.config().timeouts();

        // Unbounded, a host that drops SYNs costs the kernel's full retry
        // schedule per resolved address, minutes at a time.
        assert_eq!(timeouts.connect, Some(CONNECT_TIMEOUT));
        assert_eq!(timeouts.resolve, Some(RESOLVE_TIMEOUT));

        // Deliberately unbounded: mirrors serving a multi-hundred-megabyte
        // image are often slow, and a cap here would abort a healthy but
        // sluggish transfer.
        assert_eq!(timeouts.global, None);
        assert_eq!(timeouts.per_call, None);
        assert_eq!(timeouts.recv_body, None);
    }

    #[test]
    fn a_supplied_agent_carries_its_own_policy() {
        // The point of `with_agent` is that the embedder's policy reaches
        // the transfer. A client built with an HTTPS-only agent must refuse
        // a plain-HTTP URL, where the default client would allow it.
        let strict = HttpClient::with_agent(
            ureq::Agent::config_builder()
                .https_only(true)
                .build()
                .new_agent(),
        );
        let err = strict
            .fetch_control_file("http://127.0.0.1:1/nothing.zsync")
            .expect_err("an HTTPS-only agent must refuse a plain-HTTP URL");
        let msg = err.to_string();
        assert!(
            msg.to_lowercase().contains("http"),
            "expected a scheme rejection, got: {msg}"
        );
    }

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
