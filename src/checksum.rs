use md4::{Digest, Md4};
use sha1::Sha1;

pub fn calc_md4(data: &[u8]) -> [u8; 16] {
    let mut hasher = Md4::new();
    hasher.update(data);
    let result = hasher.finalize();
    let mut checksum = [0u8; 16];
    checksum.copy_from_slice(&result);
    checksum
}

pub fn calc_sha1(data: &[u8]) -> [u8; 20] {
    let mut hasher = Sha1::new();
    hasher.update(data);
    let result = hasher.finalize();
    let mut checksum = [0u8; 20];
    checksum.copy_from_slice(&result);
    checksum
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
