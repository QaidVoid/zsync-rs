use std::io::{BufRead, Read, Seek, SeekFrom, Write};

use crate::checksum::{calc_md4, calc_sha1_stream};
use crate::rsum::{Rsum, calc_rsum_block};

#[derive(Debug, thiserror::Error)]
pub enum GenerateError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("file is empty")]
    EmptyFile,
}

#[derive(Debug, thiserror::Error)]
pub enum WriteError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Invalid header: {0}")]
    InvalidHeader(String),
    #[error("Missing required field: {0}")]
    MissingField(String),
    #[error("Invalid blocksize: {0}")]
    InvalidBlocksize(String),
    #[error("Invalid hash lengths: {0}")]
    InvalidHashLengths(String),
    #[error("Invalid length: {0}")]
    InvalidLength(String),
    #[error("Unexpected end of file")]
    UnexpectedEof,
    #[error("Header section exceeds {0} bytes")]
    HeaderTooLarge(u64),
}

#[derive(Debug, Clone, Copy)]
pub struct BlockChecksum {
    pub rsum: Rsum,
    pub checksum: [u8; 16],
}

#[derive(Debug, Clone)]
pub struct ControlFile {
    pub version: String,
    pub filename: Option<String>,
    pub mtime: Option<String>,
    pub blocksize: usize,
    pub length: u64,
    pub hash_lengths: HashLengths,
    pub urls: Vec<String>,
    pub sha1: Option<String>,
    pub block_checksums: Vec<BlockChecksum>,
}

#[derive(Debug, Clone, Copy)]
pub struct HashLengths {
    pub seq_matches: u8,
    pub rsum_bytes: u8,
    pub checksum_bytes: u8,
}

impl Default for HashLengths {
    fn default() -> Self {
        Self {
            seq_matches: 1,
            rsum_bytes: 4,
            checksum_bytes: 16,
        }
    }
}

impl ControlFile {
    /// Parse a control file.
    ///
    /// The header section is read through a byte cap. It arrives from the
    /// network, and both a single unterminated line and an endless run of
    /// `URL:` lines would otherwise grow without bound, letting the origin
    /// choose how much memory the client spends before any of it is
    /// validated.
    pub fn parse<R: Read>(reader: R) -> Result<Self, ParseError> {
        /// Generous: real headers are a few hundred bytes, and even a long
        /// mirror list is far below this.
        const MAX_HEADER_BYTES: u64 = 1 << 20;

        let mut reader = std::io::BufReader::new(reader);
        let mut line = String::new();

        let mut version = String::new();
        let mut filename = None;
        let mut mtime = None;
        let mut blocksize = None;
        let mut length = None;
        let mut hash_lengths = HashLengths::default();
        let mut urls = Vec::new();
        let mut sha1 = None;

        let mut header = (&mut reader).take(MAX_HEADER_BYTES);
        loop {
            line.clear();
            let bytes_read = header.read_line(&mut line)?;
            if bytes_read == 0 {
                if header.limit() == 0 {
                    return Err(ParseError::HeaderTooLarge(MAX_HEADER_BYTES));
                }
                return Err(ParseError::UnexpectedEof);
            }

            let trimmed = line.trim_end_matches(['\n', '\r', ' ']);
            if trimmed.is_empty() {
                break;
            }

            let Some((key, value)) = trimmed.split_once(':') else {
                return Err(ParseError::InvalidHeader(trimmed.to_string()));
            };

            let value = value.trim_start_matches(' ');

            match key {
                "zsync" => {
                    version = value.to_string();
                }
                "Filename" => {
                    filename = Some(value.to_string());
                }
                "MTime" => {
                    mtime = Some(value.to_string());
                }
                "Blocksize" => {
                    let bs: usize = value
                        .parse()
                        .map_err(|_| ParseError::InvalidBlocksize(value.to_string()))?;
                    if bs == 0 || (bs & (bs - 1)) != 0 {
                        return Err(ParseError::InvalidBlocksize(value.to_string()));
                    }
                    blocksize = Some(bs);
                }
                "Length" => {
                    length = Some(
                        value
                            .parse()
                            .map_err(|_| ParseError::InvalidLength(value.to_string()))?,
                    );
                }
                "URL" => {
                    urls.push(value.to_string());
                }
                "Hash-Lengths" => {
                    let parts: Vec<&str> = value.split(',').collect();
                    if parts.len() != 3 {
                        return Err(ParseError::InvalidHashLengths(value.to_string()));
                    }
                    let seq_matches: u8 = parts[0]
                        .parse()
                        .map_err(|_| ParseError::InvalidHashLengths(value.to_string()))?;
                    let rsum_bytes: u8 = parts[1]
                        .parse()
                        .map_err(|_| ParseError::InvalidHashLengths(value.to_string()))?;
                    let checksum_bytes: u8 = parts[2]
                        .parse()
                        .map_err(|_| ParseError::InvalidHashLengths(value.to_string()))?;

                    if !(1..=2).contains(&seq_matches)
                        || !(1..=4).contains(&rsum_bytes)
                        || !(3..=16).contains(&checksum_bytes)
                    {
                        return Err(ParseError::InvalidHashLengths(value.to_string()));
                    }

                    hash_lengths = HashLengths {
                        seq_matches,
                        rsum_bytes,
                        checksum_bytes,
                    };
                }
                "SHA-1" => {
                    if value.len() != 40 {
                        return Err(ParseError::InvalidHeader(
                            "SHA-1 digest wrong length".to_string(),
                        ));
                    }
                    sha1 = Some(value.to_string());
                }
                _ => {}
            }
        }

        let blocksize =
            blocksize.ok_or_else(|| ParseError::MissingField("Blocksize".to_string()))?;
        let length: u64 = length.ok_or_else(|| ParseError::MissingField("Length".to_string()))?;

        let num_blocks = length.div_ceil(blocksize as u64) as usize;

        // Sanity check: avoid massive allocations from malformed input.
        // Each block needs (rsum_bytes + checksum_bytes) of data following the header.
        const MAX_BLOCKS: usize = 64 * 1024 * 1024;
        if num_blocks > MAX_BLOCKS {
            return Err(ParseError::InvalidLength(format!(
                "too many blocks: {num_blocks}"
            )));
        }

        let block_checksums = Self::read_block_checksums(&mut reader, num_blocks, hash_lengths)?;

        Ok(Self {
            version,
            filename,
            mtime,
            blocksize,
            length,
            hash_lengths,
            urls,
            sha1,
            block_checksums,
        })
    }

    fn read_block_checksums<R: BufRead>(
        reader: &mut R,
        num_blocks: usize,
        hash_lengths: HashLengths,
    ) -> Result<Vec<BlockChecksum>, ParseError> {
        // Reserve for what has arrived, not for what was claimed. The count
        // is derived from a header a few bytes long, so sizing the buffer
        // from it lets a tiny response reserve gigabytes before a single
        // checksum is read. Growth is geometric, so a genuinely large file
        // costs a handful of reallocations and nothing else.
        const MAX_PREALLOC: usize = 1 << 16;
        let mut checksums = Vec::with_capacity(num_blocks.min(MAX_PREALLOC));
        let entry_size = (hash_lengths.rsum_bytes + hash_lengths.checksum_bytes) as usize;
        let mut buf = vec![0u8; entry_size];

        for _ in 0..num_blocks {
            reader.read_exact(&mut buf)?;

            let rsum_bytes = hash_lengths.rsum_bytes as usize;
            let (rsum_a, rsum_b) = match rsum_bytes {
                1 => (0u16, u16::from(buf[0])),
                2 => (0u16, u16::from_be_bytes([buf[0], buf[1]])),
                3 => (u16::from(buf[0]), u16::from_be_bytes([buf[1], buf[2]])),
                4 => (
                    u16::from_be_bytes([buf[0], buf[1]]),
                    u16::from_be_bytes([buf[2], buf[3]]),
                ),
                _ => (0, 0),
            };

            let mut checksum = [0u8; 16];
            checksum[..hash_lengths.checksum_bytes as usize]
                .copy_from_slice(&buf[rsum_bytes..entry_size]);

            checksums.push(BlockChecksum {
                rsum: Rsum {
                    a: rsum_a,
                    b: rsum_b,
                },
                checksum,
            });
        }

        Ok(checksums)
    }

    pub fn num_blocks(&self) -> usize {
        self.block_checksums.len()
    }

    /// Generate a control file by scanning an input file.
    /// Blocksize is auto-calculated if `None`.
    pub fn generate<R: Read + Seek>(
        reader: &mut R,
        filename: &str,
        url: &str,
        blocksize: Option<usize>,
    ) -> Result<Self, GenerateError> {
        let file_length = reader.seek(SeekFrom::End(0))?;
        if file_length == 0 {
            return Err(GenerateError::EmptyFile);
        }
        reader.seek(SeekFrom::Start(0))?;

        let blocksize = blocksize.unwrap_or_else(|| auto_blocksize(file_length));
        let hash_lengths = calculate_hash_lengths(file_length, blocksize);
        let num_blocks = file_length.div_ceil(blocksize as u64) as usize;

        let mut block_checksums = Vec::with_capacity(num_blocks);
        let mut buf = vec![0u8; blocksize];

        for i in 0..num_blocks {
            let is_last = i == num_blocks - 1;
            let block_len = if is_last {
                let rem = (file_length % blocksize as u64) as usize;
                if rem == 0 { blocksize } else { rem }
            } else {
                blocksize
            };

            reader.read_exact(&mut buf[..block_len])?;
            if block_len < blocksize {
                buf[block_len..].fill(0);
            }

            let rsum = calc_rsum_block(&buf);
            let checksum = calc_md4(&buf);
            block_checksums.push(BlockChecksum { rsum, checksum });
        }

        reader.seek(SeekFrom::Start(0))?;
        let sha1_bytes = calc_sha1_stream(reader)?;
        let sha1 = sha1_bytes
            .iter()
            .fold(String::with_capacity(40), |mut s, b| {
                use std::fmt::Write;
                let _ = write!(s, "{b:02x}");
                s
            });

        Ok(Self {
            version: "0.6.2".to_string(),
            filename: Some(filename.to_string()),
            mtime: None,
            blocksize,
            length: file_length,
            hash_lengths,
            urls: vec![url.to_string()],
            sha1: Some(sha1),
            block_checksums,
        })
    }

    /// Write the control file to a writer.
    pub fn write<W: Write>(&self, writer: &mut W) -> Result<(), WriteError> {
        writeln!(writer, "zsync: {}", self.version)?;
        if let Some(ref filename) = self.filename {
            writeln!(writer, "Filename: {filename}")?;
        }
        if let Some(ref mtime) = self.mtime {
            writeln!(writer, "MTime: {mtime}")?;
        }
        writeln!(writer, "Blocksize: {}", self.blocksize)?;
        writeln!(writer, "Length: {}", self.length)?;
        writeln!(
            writer,
            "Hash-Lengths: {},{},{}",
            self.hash_lengths.seq_matches,
            self.hash_lengths.rsum_bytes,
            self.hash_lengths.checksum_bytes
        )?;
        for url in &self.urls {
            writeln!(writer, "URL: {url}")?;
        }
        if let Some(ref sha1) = self.sha1 {
            writeln!(writer, "SHA-1: {sha1}")?;
        }
        writeln!(writer)?;

        let rsum_bytes = self.hash_lengths.rsum_bytes as usize;
        let checksum_bytes = self.hash_lengths.checksum_bytes as usize;

        for block in &self.block_checksums {
            let rsum_be = rsum_to_bytes(block.rsum, rsum_bytes);
            writer.write_all(&rsum_be)?;
            writer.write_all(&block.checksum[..checksum_bytes])?;
        }

        Ok(())
    }
}

fn rsum_to_bytes(rsum: Rsum, rsum_bytes: usize) -> Vec<u8> {
    match rsum_bytes {
        1 => vec![rsum.b as u8],
        2 => rsum.b.to_be_bytes().to_vec(),
        3 => {
            let mut v = Vec::with_capacity(3);
            v.push(rsum.a as u8);
            v.extend_from_slice(&rsum.b.to_be_bytes());
            v
        }
        4 => {
            let mut v = Vec::with_capacity(4);
            v.extend_from_slice(&rsum.a.to_be_bytes());
            v.extend_from_slice(&rsum.b.to_be_bytes());
            v
        }
        _ => vec![0; rsum_bytes],
    }
}

fn auto_blocksize(file_length: u64) -> usize {
    if file_length < 100_000_000 {
        2048
    } else {
        4096
    }
}

/// Calculate optimal hash lengths based on file size and blocksize.
fn calculate_hash_lengths(file_length: u64, blocksize: usize) -> HashLengths {
    let len = file_length as f64;
    let bs = blocksize as f64;
    let seq_matches: u8 = if file_length > blocksize as u64 { 2 } else { 1 };
    let sm = f64::from(seq_matches);

    let rsum_bytes = ((len.ln() + bs.ln()) / 2_f64.ln() - 8.6) / sm / 8.0;
    let rsum_bytes = (rsum_bytes.ceil() as i32).clamp(2, 4);

    let num_blocks = 1.0 + len / bs;
    let calc1 = ((20.0 + len.log2() + num_blocks.log2()) / sm / 8.0).ceil();
    let calc2 = (7.9 + (20.0 + num_blocks.log2())) / 8.0;
    let checksum_bytes = (calc1.max(calc2) as i32).clamp(4, 16);

    HashLengths {
        seq_matches,
        rsum_bytes: rsum_bytes as u8,
        checksum_bytes: checksum_bytes as u8,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stream that never ends and never contains a newline, which is
    /// what a hostile origin serves to make the client read forever.
    struct Endless;

    impl std::io::Read for Endless {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            buf.fill(b'A');
            Ok(buf.len())
        }
    }

    #[test]
    fn an_endless_header_is_refused_rather_than_buffered() {
        // The cap is what makes this terminate at all. Which error comes
        // out depends on where the cut lands: a line with no separator is
        // reported as a bad header, a truncated section as an oversized
        // one. Either is fine; reading forever is not.
        let err = ControlFile::parse(Endless).expect_err("must not read forever");
        assert!(
            matches!(
                err,
                ParseError::HeaderTooLarge(_) | ParseError::InvalidHeader(_)
            ),
            "expected the header cap to stop parsing, got: {err}"
        );
    }

    #[test]
    fn an_endless_run_of_headers_is_refused() {
        // Individually valid lines, without end. The cap has to bound the
        // section, not just a single line.
        struct ManyUrls;
        impl std::io::Read for ManyUrls {
            fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
                let line = b"URL: http://example.invalid/f\n";
                let mut written = 0;
                while written + line.len() <= buf.len() {
                    buf[written..written + line.len()].copy_from_slice(line);
                    written += line.len();
                }
                Ok(written.max(1))
            }
        }
        let err = ControlFile::parse(ManyUrls).expect_err("must not read forever");
        assert!(
            matches!(err, ParseError::HeaderTooLarge(_)),
            "expected a header cap error, got: {err}"
        );
    }

    #[test]
    fn test_parse_minimal() {
        let mut data = Vec::new();
        data.extend_from_slice(
            b"zsync: 0.6.2\nBlocksize: 2048\nLength: 2048\nHash-Lengths: 1,4,16\n\n",
        );
        data.extend_from_slice(&[0u8; 20]);
        let result = ControlFile::parse(&data[..]);
        assert!(result.is_ok());
        let cf = result.unwrap();
        assert_eq!(cf.blocksize, 2048);
        assert_eq!(cf.length, 2048);
    }

    #[test]
    fn test_parse_missing_blocksize() {
        let data = b"zsync: 0.6.2\nLength: 4096\n\n";
        let result = ControlFile::parse(&data[..]);
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_invalid_blocksize() {
        let data = b"zsync: 0.6.2\nBlocksize: 1000\nLength: 4096\n\n";
        let result = ControlFile::parse(&data[..]);
        assert!(result.is_err());
    }

    #[test]
    fn test_generate_write_roundtrip() {
        let file_data = vec![42u8; 4096];
        let mut cursor = std::io::Cursor::new(&file_data);

        let cf = ControlFile::generate(&mut cursor, "test.bin", "test.bin", Some(2048)).unwrap();
        assert_eq!(cf.blocksize, 2048);
        assert_eq!(cf.length, 4096);
        assert_eq!(cf.block_checksums.len(), 2);
        assert!(cf.sha1.is_some());

        let mut buf = Vec::new();
        cf.write(&mut buf).unwrap();
        let parsed = ControlFile::parse(&buf[..]).unwrap();

        assert_eq!(parsed.blocksize, cf.blocksize);
        assert_eq!(parsed.length, cf.length);
        assert_eq!(parsed.sha1, cf.sha1);
        assert_eq!(parsed.block_checksums.len(), cf.block_checksums.len());
        let rlen = cf.hash_lengths.rsum_bytes as usize;
        let clen = cf.hash_lengths.checksum_bytes as usize;
        for (a, b) in parsed.block_checksums.iter().zip(&cf.block_checksums) {
            let a_bytes = rsum_to_bytes(a.rsum, rlen);
            let b_bytes = rsum_to_bytes(b.rsum, rlen);
            assert_eq!(a_bytes, b_bytes);
            assert_eq!(a.checksum[..clen], b.checksum[..clen]);
        }
    }

    #[test]
    fn test_generate_empty_file() {
        let file_data: Vec<u8> = vec![];
        let mut cursor = std::io::Cursor::new(&file_data);
        let result = ControlFile::generate(&mut cursor, "empty", "empty", None);
        assert!(result.is_err());
    }

    #[test]
    fn test_generate_partial_last_block() {
        // File not aligned to blocksize
        let file_data = vec![0xABu8; 3000];
        let mut cursor = std::io::Cursor::new(&file_data);

        let cf = ControlFile::generate(&mut cursor, "test.bin", "test.bin", Some(2048)).unwrap();
        assert_eq!(cf.length, 3000);
        assert_eq!(cf.block_checksums.len(), 2);
    }

    #[test]
    fn test_auto_blocksize_small() {
        assert_eq!(auto_blocksize(1024), 2048);
        assert_eq!(auto_blocksize(99_999_999), 2048);
    }

    #[test]
    fn test_auto_blocksize_large() {
        assert_eq!(auto_blocksize(100_000_000), 4096);
        assert_eq!(auto_blocksize(500_000_000), 4096);
    }

    #[test]
    fn test_rsum_to_bytes() {
        let rsum = Rsum {
            a: 0x1234,
            b: 0x5678,
        };
        assert_eq!(rsum_to_bytes(rsum, 4), vec![0x12, 0x34, 0x56, 0x78]);
        assert_eq!(rsum_to_bytes(rsum, 2), vec![0x56, 0x78]);
        assert_eq!(rsum_to_bytes(rsum, 1), vec![0x78]);
    }
}
