use crate::checksum::calc_md4;
use crate::control::{ControlFile, HashLengths};
use crate::rsum::{Rsum, calc_rsum_block};

#[derive(Debug, thiserror::Error)]
pub enum MatchError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

const HASH_EMPTY: u32 = u32::MAX;
const BITHASH_BITS: u32 = 3;

#[derive(Debug, Clone, Copy)]
struct TargetBlock {
    rsum: Rsum,
    checksum: [u8; 16],
}

/// Read-only scan state shared across threads.
struct ScanState<'a> {
    targets: &'a [TargetBlock],
    hash_table: &'a [u32],
    hash_next: &'a [u32],
    bithash: &'a [u8],
    blocksize: usize,
    blockshift: u8,
    seq_matches: usize,
    checksum_bytes: usize,
    rsum_a_mask: u16,
    hash_func_shift: u32,
    hash_mask: u32,
    bithash_mask: u32,
}

impl ScanState<'_> {
    #[inline(always)]
    fn calc_hash_rolling(&self, r0: &Rsum, r1: &Rsum) -> u32 {
        let mut h = r0.b as u32;
        if self.seq_matches > 1 {
            h ^= (r1.b as u32) << self.hash_func_shift;
        } else {
            h ^= ((r0.a & self.rsum_a_mask) as u32) << self.hash_func_shift;
        }
        h
    }

    #[inline(always)]
    fn rsum_match(&self, target: &Rsum, rolling: &Rsum) -> bool {
        target.a == (rolling.a & self.rsum_a_mask) && target.b == rolling.b
    }

    /// Scan a chunk of data for matching blocks. Pure read-only, no mutations.
    /// `base_offset` is the absolute byte offset of `data[0]` within the source file.
    /// Returns vec of (target_block_id, absolute_source_offset).
    fn scan_chunk(&self, data: &[u8], base_offset: usize) -> Vec<(usize, usize)> {
        let blocksize = self.blocksize;
        let blockshift = self.blockshift;
        let seq_matches = self.seq_matches;
        let checksum_bytes = self.checksum_bytes;
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

            if let Some(hint_id) = next_match_id.take()
                && seq_matches > 1
                && hint_id < self.targets.len()
            {
                let target = &self.targets[hint_id];
                if self.rsum_match(&target.rsum, &r0) {
                    let checksum = calc_md4(&data[x..x + blocksize]);
                    if checksum[..checksum_bytes] == target.checksum[..checksum_bytes] {
                        matched_blocks.push((hint_id, base_offset + x));
                        blocks_matched = 1;
                        if hint_id + 1 < self.targets.len() {
                            next_match_id = Some(hint_id + 1);
                        }
                    }
                }
            }

            while blocks_matched == 0 && x < x_limit {
                let hash = self.calc_hash_rolling(&r0, &r1);

                let bh = (hash & self.bithash_mask) as usize;
                if self.bithash[bh >> 3] & (1 << (bh & 7)) != 0 {
                    let mut block_idx = self.hash_table[(hash & self.hash_mask) as usize];

                    while block_idx != HASH_EMPTY {
                        let block_id = block_idx as usize;
                        block_idx = self.hash_next[block_id];

                        let target = &self.targets[block_id];
                        if !self.rsum_match(&target.rsum, &r0) {
                            continue;
                        }

                        if seq_matches > 1 && block_id + 1 < self.targets.len() {
                            let next_target = &self.targets[block_id + 1];
                            if !self.rsum_match(&next_target.rsum, &r1) {
                                continue;
                            }

                            let checksum = calc_md4(&data[x..x + blocksize]);
                            if checksum[..checksum_bytes] != target.checksum[..checksum_bytes] {
                                continue;
                            }

                            let next_checksum = calc_md4(&data[x + blocksize..x + blocksize * 2]);
                            if next_checksum[..checksum_bytes]
                                == next_target.checksum[..checksum_bytes]
                            {
                                matched_blocks.push((block_id, base_offset + x));
                                matched_blocks.push((block_id + 1, base_offset + x + blocksize));
                                blocks_matched = seq_matches;

                                if block_id + 2 < self.targets.len() {
                                    next_match_id = Some(block_id + 2);
                                }
                                break;
                            }
                        } else {
                            let checksum = calc_md4(&data[x..x + blocksize]);
                            if checksum[..checksum_bytes] == target.checksum[..checksum_bytes] {
                                matched_blocks.push((block_id, base_offset + x));
                                blocks_matched = 1;
                                break;
                            }
                        }
                    }
                }

                if blocks_matched == 0 {
                    let oc = data[x];
                    let nc = data[x + blocksize];
                    r0.a = r0.a.wrapping_add(u16::from(nc)).wrapping_sub(u16::from(oc));
                    r0.b =
                        r0.b.wrapping_add(r0.a)
                            .wrapping_sub(u16::from(oc) << blockshift);

                    if seq_matches > 1 {
                        let nc2 = data[x + blocksize * 2];
                        r1.a =
                            r1.a.wrapping_add(u16::from(nc2))
                                .wrapping_sub(u16::from(nc));
                        r1.b =
                            r1.b.wrapping_add(r1.a)
                                .wrapping_sub(u16::from(nc) << blockshift);
                    }

                    x += 1;
                }
            }

            if blocks_matched > 0 {
                x += blocksize * blocks_matched;

                if x >= x_limit {
                    // Can't calculate rsums for remaining data
                } else {
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
}

pub struct BlockMatcher {
    blocksize: usize,
    blockshift: u8,
    hash_lengths: HashLengths,
    rsum_a_mask: u16,
    hash_func_shift: u32,
    targets: Vec<TargetBlock>,
    known_blocks: Vec<bool>,
    hash_table: Vec<u32>,
    hash_next: Vec<u32>,
    hash_mask: u32,
    bithash: Vec<u8>,
    bithash_mask: u32,
}

impl BlockMatcher {
    pub fn new(control: &ControlFile) -> Self {
        let num_blocks = control.block_checksums.len();
        let seq_matches = control.hash_lengths.seq_matches as u32;
        let rsum_bytes = control.hash_lengths.rsum_bytes as u32;

        let rsum_a_mask: u16 = match rsum_bytes {
            0..=2 => 0,
            3 => 0x00ff,
            _ => 0xffff,
        };

        let targets: Vec<TargetBlock> = control
            .block_checksums
            .iter()
            .map(|bc| TargetBlock {
                rsum: Rsum {
                    a: bc.rsum.a & rsum_a_mask,
                    b: bc.rsum.b,
                },
                checksum: bc.checksum,
            })
            .collect();

        let rsum_bits = rsum_bytes * 8;
        let avail_bits = if seq_matches > 1 {
            rsum_bits.min(16) * 2
        } else {
            rsum_bits
        };

        let mut hash_bits = avail_bits;
        while hash_bits > 5 && (1u32 << (hash_bits - 1)) > num_blocks as u32 {
            hash_bits -= 1;
        }
        let hash_mask = (1u32 << hash_bits) - 1;

        let bithash_bits_total = (hash_bits + BITHASH_BITS).min(avail_bits);
        let bithash_mask = (1u32 << bithash_bits_total) - 1;

        let hash_func_shift = if seq_matches > 1 && avail_bits < 24 {
            bithash_bits_total.saturating_sub(avail_bits / 2)
        } else {
            bithash_bits_total.saturating_sub(avail_bits - 16)
        };

        let blockshift = control.blocksize.trailing_zeros() as u8;

        let mut matcher = Self {
            blocksize: control.blocksize,
            blockshift,
            hash_lengths: control.hash_lengths,
            rsum_a_mask,
            hash_func_shift,
            targets,
            known_blocks: vec![false; num_blocks],
            hash_table: vec![HASH_EMPTY; (hash_mask + 1) as usize],
            hash_next: vec![HASH_EMPTY; num_blocks],
            hash_mask,
            bithash: vec![0u8; ((bithash_mask + 1) >> 3) as usize + 1],
            bithash_mask,
        };

        for id in (0..num_blocks).rev() {
            let h = matcher.calc_hash(id);
            let bucket = (h & hash_mask) as usize;
            matcher.hash_next[id] = matcher.hash_table[bucket];
            matcher.hash_table[bucket] = id as u32;
            let bh = (h & bithash_mask) as usize;
            matcher.bithash[bh >> 3] |= 1 << (bh & 7);
        }

        matcher
    }

    fn calc_hash(&self, block_id: usize) -> u32 {
        let mut h = self.targets[block_id].rsum.b as u32;
        if self.hash_lengths.seq_matches > 1 {
            let next_b = if block_id + 1 < self.targets.len() {
                self.targets[block_id + 1].rsum.b as u32
            } else {
                0
            };
            h ^= next_b << self.hash_func_shift;
        } else {
            h ^= (self.targets[block_id].rsum.a as u32) << self.hash_func_shift;
        }
        h
    }

    fn remove_block_from_hash(&mut self, id: usize) {
        let h = self.calc_hash(id);
        let bucket = (h & self.hash_mask) as usize;

        let mut prev = HASH_EMPTY;
        let mut curr = self.hash_table[bucket];

        while curr != HASH_EMPTY {
            if curr as usize == id {
                if prev == HASH_EMPTY {
                    self.hash_table[bucket] = self.hash_next[id];
                } else {
                    self.hash_next[prev as usize] = self.hash_next[id];
                }
                return;
            }
            prev = curr;
            curr = self.hash_next[curr as usize];
        }
    }

    fn scan_state(&self) -> ScanState<'_> {
        ScanState {
            targets: &self.targets,
            hash_table: &self.hash_table,
            hash_next: &self.hash_next,
            bithash: &self.bithash,
            blocksize: self.blocksize,
            blockshift: self.blockshift,
            seq_matches: self.hash_lengths.seq_matches as usize,
            checksum_bytes: self.hash_lengths.checksum_bytes as usize,
            rsum_a_mask: self.rsum_a_mask,
            hash_func_shift: self.hash_func_shift,
            hash_mask: self.hash_mask,
            bithash_mask: self.bithash_mask,
        }
    }

    pub fn submit_blocks(&mut self, data: &[u8], block_start: usize) -> Result<bool, MatchError> {
        let blocksize = self.blocksize;
        let checksum_bytes = self.hash_lengths.checksum_bytes as usize;
        let num_blocks = data.len() / blocksize;

        for i in 0..num_blocks {
            let block_data = &data[i * blocksize..(i + 1) * blocksize];
            let block_id = block_start + i;

            if block_id >= self.targets.len() {
                break;
            }

            let checksum = calc_md4(block_data);
            if checksum[..checksum_bytes] == self.targets[block_id].checksum[..checksum_bytes] {
                self.known_blocks[block_id] = true;
            } else {
                return Ok(false);
            }
        }

        Ok(true)
    }

    pub fn submit_source_data(&mut self, data: &[u8], offset: u64) -> Vec<(usize, usize)> {
        let context = self.blocksize * self.hash_lengths.seq_matches as usize;
        if data.len() < context {
            return Vec::new();
        }

        let num_threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1);

        let min_per_thread = 16 * 1024 * 1024; // 16 MB per thread minimum
        let scannable = data.len() - context;

        let candidates = if num_threads > 1 && scannable >= min_per_thread * 2 {
            let state = self.scan_state();
            let actual_threads = num_threads.min(scannable / min_per_thread);
            let chunk_size = scannable / actual_threads;

            std::thread::scope(|s| {
                let handles: Vec<_> = (0..actual_threads)
                    .map(|i| {
                        let start = i * chunk_size;
                        let end = if i == actual_threads - 1 {
                            data.len()
                        } else {
                            (i + 1) * chunk_size + context
                        };
                        let chunk = &data[start..end];
                        let state = &state;
                        let base = offset as usize + start;
                        s.spawn(move || state.scan_chunk(chunk, base))
                    })
                    .collect();

                let mut all: Vec<(usize, usize)> = Vec::new();
                for h in handles {
                    all.extend(h.join().unwrap());
                }
                all
            })
        } else {
            let state = self.scan_state();
            state.scan_chunk(data, offset as usize)
        };

        // Deduplicate: first match per block_id wins
        let mut seen = vec![false; self.targets.len()];
        let mut matched_blocks = Vec::new();
        for (block_id, offset) in candidates {
            if !seen[block_id] {
                seen[block_id] = true;
                self.known_blocks[block_id] = true;
                self.remove_block_from_hash(block_id);
                matched_blocks.push((block_id, offset));
            }
        }

        matched_blocks
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

    pub fn is_block_known(&self, block_id: usize) -> bool {
        block_id < self.known_blocks.len() && self.known_blocks[block_id]
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

            block_checksums.push(BlockChecksum { rsum, checksum });
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
