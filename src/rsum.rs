//! The rolling checksum that lets a scan test every byte offset cheaply.
//!
//! This is the rsync weak checksum: two 16-bit sums over a block, both
//! wrapping. Sliding the block one byte along updates them from the byte that
//! leaves and the byte that enters, without revisiting the rest, which is what
//! makes testing every offset of a source file affordable. A match here is
//! only a candidate; [`crate::checksum::calc_md4`] confirms it.

/// The rolling checksum of one block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rsum {
    /// The sum of the block's bytes, modulo 2^16.
    pub a: u16,
    /// The sum of the running values of `a` after each byte, modulo 2^16.
    pub b: u16,
}

/// Computes the rolling checksum of `data` from scratch.
pub fn calc_rsum_block(data: &[u8]) -> Rsum {
    let mut a: u16 = 0;
    let mut b: u16 = 0;

    for &byte in data {
        a = a.wrapping_add(u16::from(byte));
        b = b.wrapping_add(a);
    }

    Rsum { a, b }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rsum_empty() {
        let rsum = calc_rsum_block(&[]);
        assert_eq!(rsum.a, 0);
        assert_eq!(rsum.b, 0);
    }

    #[test]
    fn test_rsum_single_byte() {
        let rsum = calc_rsum_block(&[1]);
        assert_eq!(rsum.a, 1);
        assert_eq!(rsum.b, 1);
    }

    #[test]
    fn test_rsum_basic() {
        let data: Vec<u8> = (0..=255).collect();
        let rsum = calc_rsum_block(&data);
        assert!(rsum.a > 0);
        assert!(rsum.b > 0);
    }
}
