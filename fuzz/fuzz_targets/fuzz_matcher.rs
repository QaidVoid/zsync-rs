#![no_main]

use libfuzzer_sys::fuzz_target;
use zsync_rs::checksum::calc_md4;
use zsync_rs::control::{BlockChecksum, HashLengths};
use zsync_rs::rsum::calc_rsum_block;
use zsync_rs::{BlockMatcher, ControlFile};

fn make_control_from_data(data: &[u8], blocksize: usize, seq_matches: u8) -> ControlFile {
    let blocksize = blocksize.max(1);
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
            seq_matches,
            rsum_bytes: 4,
            checksum_bytes: 16,
        },
        urls: vec!["http://example.com/test.bin".to_string()],
        sha1: None,
        block_checksums,
    }
}

fuzz_target!(|data: &[u8]| {
    if data.len() > 3 {
        // Blocksize must be power of 2
        let blocksize = 1 << ((data[0] as usize % 7) + 1); // 2, 4, 8, 16, 32, 64, 128
        let seq_matches = (data[1] % 2) + 1; // 1 or 2
        let source_offset = (data[2] as usize) % (data.len() / 2).max(1);

        let control = make_control_from_data(&data[3..], blocksize, seq_matches);
        let mut matcher = BlockMatcher::new(&control);

        if source_offset < data.len() {
            let source = &data[source_offset..];
            let context = blocksize * seq_matches as usize;
            if source.len() >= context {
                let _ = matcher.submit_source_data(source, 0);
            }
        }
    }
});
