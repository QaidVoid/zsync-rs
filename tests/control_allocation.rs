//! A control file must not let its header dictate how much memory the
//! client reserves.
//!
//! Alone in its own file on purpose: it reads the process's peak virtual
//! size, which is monotonic and process-wide, so a sibling test in the
//! same binary would pollute the reading.

#[cfg(target_os = "linux")]
fn vm_peak_kb() -> u64 {
    let status = std::fs::read_to_string("/proc/self/status").expect("read status");
    status
        .lines()
        .find(|l| l.starts_with("VmPeak:"))
        .and_then(|l| l.split_whitespace().nth(1))
        .and_then(|v| v.parse().ok())
        .expect("VmPeak")
}

#[cfg(target_os = "linux")]
#[test]
fn a_tiny_header_cannot_reserve_gigabytes() {
    // 64 GiB over a 1 KiB blocksize is 64Mi blocks, right at the parser's
    // ceiling, in 135 bytes. Sizing the checksum buffer from that reserved
    // 1280 MB before this was fixed.
    let header = b"zsync: 0.6.2\n\
                   Blocksize: 1024\n\
                   Length: 68719476736\n\
                   Hash-Lengths: 1,4,16\n\
                   URL: http://example.invalid/f\n\
                   SHA-1: da39a3ee5e6b4b0d3255bfef95601890afd80709\n\n";

    let before = vm_peak_kb();
    let result = zsync_rs::ControlFile::parse(&header[..]);
    let after = vm_peak_kb();

    assert!(
        result.is_err(),
        "a header promising blocks it does not deliver must fail"
    );
    let grew_mb = (after.saturating_sub(before)) / 1024;
    assert!(
        grew_mb < 256,
        "parsing {} bytes grew peak memory by {grew_mb} MB",
        header.len()
    );
}
