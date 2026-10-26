//! The `zsyncmake` binary must not report success for output it failed to
//! write.

#[cfg(target_os = "linux")]
#[test]
fn a_failed_write_to_a_full_disk_is_reported() {
    let dir = std::env::temp_dir().join(format!("zsyncmake-full-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    let input = dir.join("small.bin");
    // Small enough that the whole control file fits in the write buffer, so
    // the only write happens when the buffer is flushed.
    std::fs::write(&input, vec![7u8; 10_000]).expect("write input");

    let out = std::process::Command::new(env!("CARGO_BIN_EXE_zsyncmake"))
        .arg("-o")
        .arg("/dev/full")
        .arg(&input)
        .output()
        .expect("run zsyncmake");
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(!out.status.success(), "reported success: {stderr}");
    assert!(stderr.contains("Error writing control file"), "{stderr}");

    std::fs::remove_dir_all(&dir).ok();
}
