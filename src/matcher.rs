use std::collections::HashMap;

use crate::checksum::calc_md4;
use crate::control::{ControlFile, HashLengths};
use crate::rsum::{Rsum, calc_rsum_block};

#[derive(Debug, thiserror::Error)]
pub enum MatchError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone)]
struct TargetBlock {
    id: usize,
    rsum: Rsum,
    checksum: Vec<u8>,
}

pub struct BlockMatcher {
    blocksize: usize,
    hash_lengths: HashLengths,
    rsum_has_a: bool,
    targets: Vec<TargetBlock>,
    rsum_hash: HashMap<u32, Vec<usize>>,
    known_blocks: Vec<bool>,
}

impl BlockMatcher {
    pub fn new(control: &ControlFile) -> Self {
        let targets: Vec<TargetBlock> = control
            .block_checksums
            .iter()
            .enumerate()
            .map(|(id, bc)| TargetBlock {
                id,
                rsum: bc.rsum,
                checksum: bc.checksum.clone(),
            })
            .collect();

        let mut rsum_hash: HashMap<u32, Vec<usize>> = HashMap::new();
        for target in &targets {
            let hash = Self::hash_rsum(&target.rsum, control.hash_lengths.seq_matches > 1);
            rsum_hash.entry(hash).or_default().push(target.id);
        }

        let known_blocks = vec![false; targets.len()];

        let rsum_has_a = control.hash_lengths.rsum_bytes >= 3;

        Self {
            blocksize: control.blocksize,
            hash_lengths: control.hash_lengths,
            rsum_has_a,
            targets,
            rsum_hash,
            known_blocks,
        }
    }

    fn hash_rsum(rsum: &Rsum, seq_matches: bool) -> u32 {
        if seq_matches {
            ((rsum.b as u32) << 16) | (rsum.a as u32)
        } else {
            rsum.b as u32
        }
    }

    pub fn submit_blocks(&mut self, data: &[u8], block_start: usize) -> Result<bool, MatchError> {
        let blocksize = self.blocksize;
        let num_blocks = data.len() / blocksize;

        for i in 0..num_blocks {
            let block_data = &data[i * blocksize..(i + 1) * blocksize];
            let block_id = block_start + i;

            if block_id >= self.targets.len() {
                break;
            }

            let checksum = calc_md4(block_data);
            if checksum[..self.hash_lengths.checksum_bytes as usize]
                == self.targets[block_id].checksum[..]
            {
                self.known_blocks[block_id] = true;
            } else {
                return Ok(false);
            }
        }

        Ok(true)
    }

    pub fn submit_source_data(&mut self, data: &[u8], _offset: u64) -> usize {
        let blocksize = self.blocksize;
        let context = blocksize * self.hash_lengths.seq_matches as usize;
        let mut got_blocks = 0;

        if data.len() < blocksize {
            return 0;
        }

        let mut r0 = calc_rsum_block(&data[0..blocksize]);
        let mut r1 = if self.hash_lengths.seq_matches > 1 {
            calc_rsum_block(&data[blocksize..blocksize * 2])
        } else {
            Rsum { a: 0, b: 0 }
        };

        let x_limit = data.len() - context;
        let mut x = 0usize;

        while x <= x_limit {
            let mut matched = false;
            let hash = Self::hash_rsum(&r0, self.hash_lengths.seq_matches > 1);

            if let Some(candidate_ids) = self.rsum_hash.get(&hash) {
                for &block_id in candidate_ids {
                    if self.known_blocks[block_id] {
                        continue;
                    }

                    let target = &self.targets[block_id];
                    let rsum_match = if self.rsum_has_a {
                        target.rsum.a == r0.a && target.rsum.b == r0.b
                    } else {
                        target.rsum.b == r0.b
                    };
                    if !rsum_match {
                        continue;
                    }

                    let block_data = &data[x..x + blocksize];
                    let checksum = calc_md4(block_data);

                    if checksum[..self.hash_lengths.checksum_bytes as usize] == target.checksum[..]
                    {
                        self.known_blocks[block_id] = true;
                        got_blocks += 1;
                        matched = true;
                        break;
                    }
                }
            }

            if matched {
                x += blocksize;
                if x <= x_limit {
                    r0 = calc_rsum_block(&data[x..x + blocksize]);
                    if self.hash_lengths.seq_matches > 1 && x + blocksize * 2 <= data.len() {
                        r1 = calc_rsum_block(&data[x + blocksize..x + blocksize * 2]);
                    }
                }
            } else {
                if x + blocksize >= data.len() {
                    break;
                }
                let oc = data[x];
                let nc = data[x + blocksize];
                Self::update_rsum(&mut r0, oc, nc, blocksize);

                if self.hash_lengths.seq_matches > 1 && x + blocksize * 2 < data.len() {
                    let nc2 = data[x + blocksize * 2];
                    Self::update_rsum(&mut r1, nc, nc2, blocksize);
                }

                x += 1;
            }
        }

        got_blocks
    }

    fn update_rsum(rsum: &mut Rsum, old_byte: u8, new_byte: u8, blocksize: usize) {
        let blockshift = (blocksize - 1).count_ones() as u8;
        rsum.a = rsum
            .a
            .wrapping_add(u16::from(new_byte))
            .wrapping_sub(u16::from(old_byte));
        rsum.b = rsum
            .b
            .wrapping_add(rsum.a)
            .wrapping_sub(u16::from(old_byte) << blockshift);
    }

    pub fn needed_block_ranges(&self) -> Vec<(usize, usize)> {
        let mut ranges = Vec::new();
        let mut start: Option<usize> = None;

        for (i, &known) in self.known_blocks.iter().enumerate() {
            if !known && start.is_none() {
                start = Some(i);
            } else if known && start.is_some() {
                ranges.push((start.unwrap(), i));
                start = None;
            }
        }

        if let Some(s) = start {
            ranges.push((s, self.known_blocks.len()));
        }

        ranges
    }

    pub fn blocks_todo(&self) -> usize {
        self.known_blocks.iter().filter(|&&k| !k).count()
    }

    pub fn is_complete(&self) -> bool {
        self.known_blocks.iter().all(|&k| k)
    }

    pub fn total_blocks(&self) -> usize {
        self.targets.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control::{BlockChecksum, ControlFile, HashLengths};

    fn make_control(data: &[u8], blocksize: usize) -> ControlFile {
        let num_blocks = data.len().div_ceil(blocksize);
        let mut block_checksums = Vec::with_capacity(num_blocks);

        for i in 0..num_blocks {
            let start = i * blocksize;
            let end = std::cmp::min(start + blocksize, data.len());
            let mut block = data[start..end].to_vec();
            block.resize(blocksize, 0);

            let rsum = calc_rsum_block(&block);
            let checksum = calc_md4(&block);

            block_checksums.push(BlockChecksum {
                rsum,
                checksum: checksum.to_vec(),
            });
        }

        ControlFile {
            version: "0.6.2".to_string(),
            filename: Some("test.bin".to_string()),
            mtime: None,
            blocksize,
            length: data.len() as u64,
            hash_lengths: HashLengths {
                seq_matches: 1,
                rsum_bytes: 4,
                checksum_bytes: 16,
            },
            urls: vec!["http://example.com/test.bin".to_string()],
            sha1: None,
            block_checksums,
        }
    }

    #[test]
    fn test_matcher_new() {
        let data = vec![1u8, 2, 3, 4, 5, 6, 7, 8];
        let control = make_control(&data, 4);
        let matcher = BlockMatcher::new(&control);
        assert_eq!(matcher.blocks_todo(), 2);
    }

    #[test]
    fn test_submit_source_data() {
        let data = vec![1u8, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12];
        let control = make_control(&data, 4);
        let mut matcher = BlockMatcher::new(&control);

        let got = matcher.submit_source_data(&data, 0);
        assert_eq!(got, 3);
        assert!(matcher.is_complete());
    }

    #[test]
    fn test_needed_block_ranges() {
        let data = vec![1u8, 2, 3, 4, 5, 6, 7, 8];
        let control = make_control(&data, 4);
        let matcher = BlockMatcher::new(&control);
        let ranges = matcher.needed_block_ranges();
        assert_eq!(ranges, vec![(0, 2)]);
    }
}
