//! The strong hashes zsync relies on.
//!
//! MD4 is the per-block checksum that confirms a candidate block once its
//! rolling checksum matches. SHA-1 covers the whole target and is the final
//! check that an assembled file is the one the control file describes.
//! Neither is used for security: they detect accidental mismatches, and the
//! format fixes both choices.

use std::io::Read;

use md4::{Digest, Md4};
use sha1::Sha1;

const SHA1_BUFFER_SIZE: usize = 64 * 1024;

/// The MD4 digest of `data`.
pub fn calc_md4(data: &[u8]) -> [u8; 16] {
    let mut hasher = Md4::new();
    hasher.update(data);
    let result = hasher.finalize();
    let mut checksum = [0u8; 16];
    checksum.copy_from_slice(&result);
    checksum
}

/// The SHA-1 digest of `data`.
pub fn calc_sha1(data: &[u8]) -> [u8; 20] {
    let mut hasher = Sha1::new();
    hasher.update(data);
    let result = hasher.finalize();
    let mut checksum = [0u8; 20];
    checksum.copy_from_slice(&result);
    checksum
}

/// The SHA-1 digest of everything `reader` yields until end of file.
///
/// The input is read in 64 KiB chunks, so a file of any size hashes in
/// constant memory.
///
/// # Errors
///
/// Returns the first error `reader` reports.
pub fn calc_sha1_stream<R: Read>(reader: &mut R) -> std::io::Result<[u8; 20]> {
    let mut hasher = Sha1::new();
    let mut buf = [0u8; SHA1_BUFFER_SIZE];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let result = hasher.finalize();
    let mut checksum = [0u8; 20];
    checksum.copy_from_slice(&result);
    Ok(checksum)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_md4_empty() {
        let result = calc_md4(&[]);
        assert_eq!(
            result,
            [
                0x31, 0xd6, 0xcf, 0xe0, 0xd1, 0x6a, 0xe9, 0x31, 0xb7, 0x3c, 0x59, 0xd7, 0xe0, 0xc0,
                0x89, 0xc0
            ]
        );
    }

    #[test]
    fn test_sha1_empty() {
        let result = calc_sha1(&[]);
        assert_eq!(
            result,
            [
                0xda, 0x39, 0xa3, 0xee, 0x5e, 0x6b, 0x4b, 0x0d, 0x32, 0x55, 0xbf, 0xef, 0x95, 0x60,
                0x18, 0x90, 0xaf, 0xd8, 0x07, 0x09
            ]
        );
    }
}
