#![no_main]

use libfuzzer_sys::fuzz_target;
use zsync_rs::rsum::calc_rsum_block;

fuzz_target!(|data: &[u8]| {
    if !data.is_empty() {
        let blocksize = (data[0] as usize % 1024) + 1;
        if data.len() > blocksize {
            let _ = calc_rsum_block(&data[1..blocksize + 1]);
        }
    }
});
