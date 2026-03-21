use std::io::{BufRead, Read};

use crate::rsum::Rsum;

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
    pub fn parse<R: Read>(reader: R) -> Result<Self, ParseError> {
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

        loop {
            line.clear();
            let bytes_read = reader.read_line(&mut line)?;
            if bytes_read == 0 {
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
        let mut checksums = Vec::with_capacity(num_blocks);
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
