#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rsum {
    pub a: u16,
    pub b: u16,
}

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
