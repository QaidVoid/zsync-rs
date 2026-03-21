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
    rsum_a_mask: u16,
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
        let seq_matches = control.hash_lengths.seq_matches > 1;

        if seq_matches && targets.len() > 1 {
            for i in 0..targets.len() - 1 {
                let hash = Self::hash_rsum_pair(&targets[i].rsum, &targets[i + 1].rsum);
                rsum_hash.entry(hash).or_default().push(i);
            }
        } else {
            for target in &targets {
                let hash = Self::hash_rsum_single(&target.rsum);
                rsum_hash.entry(hash).or_default().push(target.id);
            }
        }

        let known_blocks = vec![false; targets.len()];

        let rsum_a_mask = match control.hash_lengths.rsum_bytes {
            0..=2 => 0u16,
            3 => 0x00ff,
            _ => 0xffff,
        };

        Self {
            blocksize: control.blocksize,
            hash_lengths: control.hash_lengths,
            rsum_a_mask,
            targets,
            rsum_hash,
            known_blocks,
        }
    }

    fn rsum_match(&self, target: &Rsum, rolling: &Rsum) -> bool {
        target.a == (rolling.a & self.rsum_a_mask) && target.b == rolling.b
    }

    fn hash_rsum_single(rsum: &Rsum) -> u32 {
        rsum.b as u32
    }

    fn hash_rsum_pair(r0: &Rsum, r1: &Rsum) -> u32 {
        ((r0.b as u32) << 16) | (r1.b as u32)
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

    pub fn submit_source_data(&mut self, data: &[u8], _offset: u64) -> Vec<(usize, usize)> {
        let blocksize = self.blocksize;
        let seq_matches = self.hash_lengths.seq_matches as usize;
        let context = blocksize * seq_matches;
        let mut matched_blocks = Vec::new();

        if data.len() < context {
            return matched_blocks;
        }

        let x_limit = data.len() - context;
        let mut x = 0usize;
        let mut next_match_id: Option<usize> = None;

        let mut r0 = calc_rsum_block(&data[0..blocksize]);
        let mut r1 = if seq_matches > 1 {
            calc_rsum_block(&data[blocksize..blocksize * 2])
        } else {
            Rsum { a: 0, b: 0 }
        };

        while x < x_limit {
            let mut blocks_matched = 0usize;

            // Sequential chaining: after a pair match, try the next block individually.
            // This matches the C zsync `next_match` optimization.
            if let Some(hint_id) = next_match_id.take()
                && seq_matches > 1
                && hint_id < self.targets.len()
                && !self.known_blocks[hint_id]
            {
                let target = &self.targets[hint_id];
                if self.rsum_match(&target.rsum, &r0) {
                    let block_data = &data[x..x + blocksize];
                    let checksum = calc_md4(block_data);
                    if checksum[..self.hash_lengths.checksum_bytes as usize]
                        == target.checksum[..]
                    {
                        self.known_blocks[hint_id] = true;
                        matched_blocks.push((hint_id, x));
                        blocks_matched = 1;
                        if hint_id + 1 < self.targets.len() {
                            next_match_id = Some(hint_id + 1);
                        }
                    }
                }
            }

            // Inner loop: advance byte-by-byte, looking up rolling checksum in hash table
            while blocks_matched == 0 && x < x_limit {
                let hash = if seq_matches > 1 {
                    Self::hash_rsum_pair(&r0, &r1)
                } else {
                    Self::hash_rsum_single(&r0)
                };

                if let Some(candidate_ids) = self.rsum_hash.get(&hash) {
                    for &block_id in candidate_ids {
                        if self.known_blocks[block_id] {
                            continue;
                        }

                        let target = &self.targets[block_id];
                        if !self.rsum_match(&target.rsum, &r0) {
                            continue;
                        }

                        if seq_matches > 1 && block_id + 1 < self.targets.len() {
                            let next_target = &self.targets[block_id + 1];
                            if !self.rsum_match(&next_target.rsum, &r1) {
                                continue;
                            }

                            let block_data = &data[x..x + blocksize];
                            let checksum = calc_md4(block_data);
                            if checksum[..self.hash_lengths.checksum_bytes as usize]
                                != target.checksum[..]
                            {
                                continue;
                            }

                            let next_block_data = &data[x + blocksize..x + blocksize * 2];
                            let next_checksum = calc_md4(next_block_data);
                            if next_checksum[..self.hash_lengths.checksum_bytes as usize]
                                == next_target.checksum[..]
                            {
                                self.known_blocks[block_id] = true;
                                self.known_blocks[block_id + 1] = true;
                                matched_blocks.push((block_id, x));
                                matched_blocks.push((block_id + 1, x + blocksize));
                                blocks_matched = seq_matches;

                                if block_id + 2 < self.targets.len() {
                                    next_match_id = Some(block_id + 2);
                                }
                                break;
                            }
                        } else {
                            let block_data = &data[x..x + blocksize];
                            let checksum = calc_md4(block_data);
                            if checksum[..self.hash_lengths.checksum_bytes as usize]
                                == target.checksum[..]
                            {
                                self.known_blocks[block_id] = true;
                                matched_blocks.push((block_id, x));
                                blocks_matched = 1;
                                break;
                            }
                        }
                    }
                }

                if blocks_matched == 0 {
                    let oc = data[x];
                    let nc = data[x + blocksize];
                    Self::update_rsum(&mut r0, oc, nc, blocksize);

                    if seq_matches > 1 {
                        let nc2 = data[x + blocksize * 2];
                        Self::update_rsum(&mut r1, nc, nc2, blocksize);
                    }

                    x += 1;
                }
            }

            if blocks_matched > 0 {
                x += blocksize * blocks_matched;

                if x >= x_limit {
                    // Can't calculate rsums for remaining data
                } else {
                    // Reuse r1 as r0 when advancing by 1 block (sequential match)
                    if seq_matches > 1 && blocks_matched == 1 {
                        r0 = r1;
                    } else {
                        r0 = calc_rsum_block(&data[x..x + blocksize]);
                    }
                    if seq_matches > 1 {
                        r1 = calc_rsum_block(&data[x + blocksize..x + blocksize * 2]);
                    }
                }
            }
        }

        matched_blocks
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

        // Pad with context bytes (blocksize * seq_matches) like submit_source_file does
        let mut padded = data.clone();
        padded.resize(data.len() + 4, 0);
        let got = matcher.submit_source_data(&padded, 0);
        assert_eq!(got.len(), 3);
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
