//! The `zsync` binary must emit usable progress, in both modes.
//!
//! These drive the real binary against a local HTTP server, so they cover
//! the wiring that unit tests inside the binary cannot: that the library's
//! per-block callback reaches the terminal, and that `--json` puts one
//! parseable object per line on stdout and nothing else.

use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener};
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

/// An HTTP origin serving a data file and its control file.
///
/// The transfer under test needs 206 responses with exactly the requested
/// bytes. A server that ignores `Range` and returns the whole body would
/// make the download appear to succeed while the client reads the wrong
/// offsets, so this one is explicit about it.
///
/// The listener is non-blocking and polls a shutdown flag, so a test can
/// stop the server and join its thread instead of leaving it blocked in
/// `accept` for the life of the test binary.
fn serve(
    body: Vec<u8>,
    control: Vec<u8>,
    addr: SocketAddr,
    chunk_delay: Duration,
) -> (Arc<AtomicBool>, thread::JoinHandle<()>) {
    let listener = TcpListener::bind(addr).expect("rebind");
    listener
        .set_nonblocking(true)
        .expect("set listener non-blocking");
    let shutdown = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&shutdown);

    let handle = thread::spawn(move || {
        while !flag.load(Ordering::Relaxed) {
            match listener.accept() {
                Ok((mut stream, _)) => {
                    stream
                        .set_nonblocking(false)
                        .expect("set connection blocking");
                    let mut reader = BufReader::new(stream.try_clone().expect("clone"));
                    let mut request_line = String::new();
                    if reader.read_line(&mut request_line).is_err() {
                        continue;
                    }

                    let path = request_line
                        .split_whitespace()
                        .nth(1)
                        .unwrap_or("/")
                        .to_string();
                    let payload: &[u8] = if path.ends_with(".zsync") {
                        &control
                    } else {
                        &body
                    };

                    let mut range: Option<(usize, usize)> = None;
                    loop {
                        let mut line = String::new();
                        if reader.read_line(&mut line).unwrap_or(0) == 0 {
                            break;
                        }
                        let trimmed = line.trim_end();
                        if trimmed.is_empty() {
                            break;
                        }
                        let lower = trimmed.to_ascii_lowercase();
                        if let Some(value) = lower.strip_prefix("range: ")
                            && let Some(spec) = value.strip_prefix("bytes=")
                        {
                            let mut parts = spec.split('-');
                            let start = parts.next().unwrap_or("0").parse().unwrap_or(0);
                            let end = parts
                                .next()
                                .and_then(|e| e.parse().ok())
                                .unwrap_or(payload.len() - 1);
                            range = Some((start, end.min(payload.len() - 1)));
                        }
                    }

                    let (start, end) = range.unwrap_or((0, payload.len() - 1));
                    let slice = &payload[start..=end];
                    let header = if range.is_some() {
                        format!(
                            "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {start}-{end}/{}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n",
                            slice.len(),
                            payload.len()
                        )
                    } else {
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n",
                            slice.len()
                        )
                    };
                    let _ = stream.write_all(header.as_bytes());

                    // Dribble the body out so the transfer spans several
                    // throttle windows. A body delivered in one write would
                    // complete inside a single window and let a missing
                    // callback still pass, because the forced final event
                    // does not depend on the callback firing at all.
                    const CHUNK: usize = 32 * 1024;
                    for part in slice.chunks(CHUNK) {
                        if stream.write_all(part).is_err() {
                            break;
                        }
                        let _ = stream.flush();
                        thread::sleep(chunk_delay);
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    thread::sleep(std::time::Duration::from_millis(2));
                }
                Err(_) => break,
            }
        }
    });

    (shutdown, handle)
}

/// Writes `body` into `dir` and generates its control file, returning the
/// control file's bytes.
fn control_bytes(dir: &Path, body: &[u8], data_url: &str) -> Vec<u8> {
    let data_path = dir.join("file.bin");
    std::fs::write(&data_path, body).expect("write data");

    let control = dir.join("file.zsync");
    let status = Command::new(env!("CARGO_BIN_EXE_zsyncmake"))
        .arg("-u")
        .arg(data_url)
        .arg("-o")
        .arg(&control)
        .arg(&data_path)
        .status()
        .expect("run zsyncmake");
    assert!(status.success(), "zsyncmake failed");
    std::fs::read(&control).expect("read control")
}

/// Prepares a fixture and starts an origin serving both halves of it.
///
/// The control file embeds the data URL, and the port is only known after
/// binding, so a probe bind picks the port first and the real listener takes
/// it back once the control file names it.
fn origin(dir: &Path, body: &[u8]) -> (String, Arc<AtomicBool>, thread::JoinHandle<()>) {
    let probe = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = probe.local_addr().expect("addr");
    drop(probe);

    let data_url = format!("http://{addr}/file.bin");
    let control = control_bytes(dir, body, &data_url);
    let control_url = format!("http://{addr}/file.zsync");

    // ~20 chunks at 25 ms keeps a transfer near half a second, which is
    // several 100 ms throttle windows.
    let (shutdown, handle) = serve(body.to_vec(), control, addr, Duration::from_millis(25));
    (control_url, shutdown, handle)
}

fn zsync(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_zsync"))
        .args(args)
        .output()
        .expect("run zsync")
}

fn temp_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("zsync-{tag}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    dir
}

#[test]
fn human_mode_prints_progress_between_start_and_done() {
    let body: Vec<u8> = (0..400_000u32).map(|i| (i % 251) as u8).collect();
    let dir = temp_dir("human");
    let (control_url, shutdown, server) = origin(&dir, &body);

    let out_path = dir.join("out.bin");
    let out = zsync(&[&control_url, "-o", out_path.to_str().expect("path")]);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(out.status.success(), "zsync failed: {stderr}");

    // Progress rows are the indented "X / Y (N%)" lines.
    let progress: Vec<&str> = stderr
        .lines()
        .filter(|l| l.starts_with("  ") && l.contains(" / ") && l.contains('%'))
        .collect();
    assert!(
        !progress.is_empty(),
        "no progress lines in stderr:\n{stderr}"
    );
    assert!(
        progress.len() > 1,
        "only one progress line, so nothing was reported during the transfer: {progress:?}"
    );
    assert!(
        progress.last().expect("last").contains("(100%)"),
        "final progress line was not 100%: {progress:?}"
    );

    // The start and done markers still bracket the run.
    assert!(stderr.contains("Downloading missing blocks"));
    assert!(stderr.contains("Done:"));

    shutdown.store(true, Ordering::Relaxed);
    server.join().ok();
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn json_mode_emits_only_parseable_jsonl_on_stdout() {
    let body: Vec<u8> = (0..400_000u32).map(|i| (i % 97) as u8).collect();
    let dir = temp_dir("json");
    let (control_url, shutdown, server) = origin(&dir, &body);

    let out_path = dir.join("out.bin");
    let out = zsync(&[
        &control_url,
        "-o",
        out_path.to_str().expect("path"),
        "--json",
    ]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert!(out.status.success(), "zsync failed: {stderr}");

    let lines: Vec<&str> = stdout.lines().collect();
    assert!(!lines.is_empty(), "no jsonl on stdout");

    // Every line must be a JSON object. A stray human line breaks consumers.
    for line in &lines {
        assert!(
            line.starts_with('{') && line.ends_with('}'),
            "non-JSON line on stdout: {line:?}"
        );
        assert!(
            line.contains("\"event\":"),
            "JSON line without an event field: {line:?}"
        );
    }

    assert!(lines[0].contains("\"event\":\"start\""), "{}", lines[0]);
    assert!(
        lines.last().expect("last").contains("\"event\":\"done\""),
        "{}",
        lines.last().expect("last")
    );

    // A real transfer must produce intermediate progress, not just the
    // forced final event. Without the callback wired up this count is 1.
    let progress_events: Vec<&&str> = lines
        .iter()
        .filter(|l| l.contains("\"event\":\"progress\""))
        .collect();
    assert!(
        progress_events.len() > 1,
        "only {} progress event(s), so the callback never fired: {progress_events:?}",
        progress_events.len()
    );

    // The terminal progress event must report the full byte count.
    let final_progress = lines
        .iter()
        .find(|l| l.contains("\"final\":true"))
        .expect("no final progress event");
    assert!(
        final_progress.contains("\"done_bytes\":400000"),
        "final event did not carry the total: {final_progress}"
    );

    // Human output belongs on stderr even in json mode.
    assert!(stderr.contains("Done:"));

    shutdown.store(true, Ordering::Relaxed);
    server.join().ok();
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn json_mode_reports_errors_as_json() {
    let dir = temp_dir("err");

    let out_path = dir.join("out.bin");
    let out = zsync(&[
        "http://127.0.0.1:1/missing.zsync",
        "-o",
        out_path.to_str().expect("path"),
        "--json",
    ]);
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(!out.status.success(), "expected failure");
    let line = stdout.lines().next().expect("no jsonl on stdout");
    assert!(
        line.contains("\"event\":\"error\""),
        "expected an error event, got {line:?}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

// Windows forbids `"` in file names. The escaping itself is covered on every
// platform by the binary's unit tests.
#[cfg(unix)]
#[test]
fn json_escape_handles_quotes_in_paths() {
    // Paths reach the done event verbatim, so a quote in one must not
    // produce invalid JSON.
    let body: Vec<u8> = (0..200_000u32).map(|i| (i % 13) as u8).collect();
    let dir = temp_dir("esc");
    let (control_url, shutdown, server) = origin(&dir, &body);

    let weird = dir.join("out\"quoted\".bin");
    let out = zsync(&[&control_url, "-o", weird.to_str().expect("path"), "--json"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "zsync failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let done = stdout
        .lines()
        .find(|l| l.contains("\"event\":\"done\""))
        .expect("no done event");
    assert!(
        done.contains("\\\""),
        "quote in path was not escaped: {done}"
    );

    shutdown.store(true, Ordering::Relaxed);
    server.join().ok();
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_complete_seed_reports_progress_without_downloading() {
    let body: Vec<u8> = (0..200_000u32).map(|i| (i % 31) as u8).collect();
    let dir = temp_dir("seed");
    let (control_url, shutdown, server) = origin(&dir, &body);

    // The seed is the target itself, so nothing needs downloading.
    let seed = dir.join("file.bin");
    let out_path = dir.join("out.bin");
    let out = zsync(&[
        &control_url,
        "-i",
        seed.to_str().expect("path"),
        "-o",
        out_path.to_str().expect("path"),
        "--json",
    ]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "zsync failed: {stderr}");

    assert!(stdout.contains("\"event\":\"start\""));
    let final_progress = stdout
        .lines()
        .find(|l| l.contains("\"phase\":\"download\"") && l.contains("\"final\":true"))
        .expect("no final download event");
    assert!(
        final_progress.contains("\"done_bytes\":200000"),
        "complete seed did not report full progress: {final_progress}"
    );
    assert!(stdout.contains("\"event\":\"done\""));

    shutdown.store(true, Ordering::Relaxed);
    server.join().ok();
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn human_mode_prints_no_progress_when_nothing_downloads() {
    let body: Vec<u8> = (0..200_000u32).map(|i| (i % 29) as u8).collect();
    let dir = temp_dir("seed-human");
    let (control_url, shutdown, server) = origin(&dir, &body);

    let seed = dir.join("file.bin");
    let out_path = dir.join("out.bin");
    let out = zsync(&[
        &control_url,
        "-i",
        seed.to_str().expect("path"),
        "-o",
        out_path.to_str().expect("path"),
    ]);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "zsync failed: {stderr}");

    // The seed scan reports progress; the download that follows must not.
    let after_scan = stderr
        .split_once("Need to download")
        .map(|(_, rest)| rest)
        .expect("no download summary");
    assert!(
        !after_scan
            .lines()
            .any(|l| l.starts_with("  ") && l.contains(" / ")),
        "progress line printed without a download:\n{stderr}"
    );

    shutdown.store(true, Ordering::Relaxed);
    server.join().ok();
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_failed_download_removes_the_temp_file() {
    let body: Vec<u8> = (0..200_000u32).map(|i| (i % 37) as u8).collect();
    let dir = temp_dir("fail-tmp");

    // The control file points its data at a closed port, so the download
    // fails after the seed has already written blocks to the temp file.
    let probe = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = probe.local_addr().expect("addr");
    drop(probe);
    let control = control_bytes(&dir, &body, "http://127.0.0.1:1/file.bin");
    let (shutdown, server) = serve(body.clone(), control, addr, Duration::ZERO);

    let seed = dir.join("seed.bin");
    std::fs::write(&seed, &body[..100_000]).expect("write seed");
    let out_path = dir.join("out.bin");
    let out = zsync(&[
        &format!("http://{addr}/file.zsync"),
        "-i",
        seed.to_str().expect("path"),
        "-o",
        out_path.to_str().expect("path"),
        "--json",
    ]);
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(!out.status.success(), "expected failure");
    assert!(
        stdout
            .lines()
            .last()
            .expect("no jsonl")
            .contains("\"event\":\"error\""),
        "last event was not an error: {stdout}"
    );
    assert!(
        !dir.join("out.bin.zsync-tmp").exists(),
        "temp file left behind after a failed download"
    );

    shutdown.store(true, Ordering::Relaxed);
    server.join().ok();
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn json_mode_reports_seed_scan_progress_before_the_download() {
    // The last 64 KiB differ from the seed, so a download still follows the
    // scan.
    let len = 2_200_000u32;
    let body: Vec<u8> = (0..len).map(|i| (i % 241) as u8).collect();
    let dir = temp_dir("scan-json");
    let (control_url, shutdown, server) = origin(&dir, &body);

    let mut seed_bytes = body.clone();
    let tail = seed_bytes.len() - 64 * 1024;
    seed_bytes[tail..].fill(0xEE);
    let seed = dir.join("seed.bin");
    std::fs::write(&seed, &seed_bytes).expect("write seed");

    let out_path = dir.join("out.bin");
    let out = zsync(&[
        &control_url,
        "-i",
        seed.to_str().expect("path"),
        "-o",
        out_path.to_str().expect("path"),
        "--json",
    ]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "zsync failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let lines: Vec<&str> = stdout.lines().collect();
    let scan: Vec<usize> = (0..lines.len())
        .filter(|&i| lines[i].contains("\"phase\":\"scan\""))
        .collect();
    let first_download = lines
        .iter()
        .position(|l| l.contains("\"phase\":\"download\""))
        .expect("no download events");

    assert!(
        scan.len() > 1,
        "expected progress during the scan: {stdout}"
    );
    assert!(
        scan.iter().all(|&i| i < first_download),
        "scan events must come before the download: {stdout}"
    );
    assert!(
        scan.iter().all(|&i| lines[i].contains("seed.bin")),
        "scan events must name the seed: {stdout}"
    );
    let last_scan = lines[*scan.last().expect("scan")];
    assert!(
        last_scan.contains("\"final\":true")
            && last_scan.contains(&format!("\"done_bytes\":{len}")),
        "the scan must end with the whole seed: {last_scan}"
    );

    shutdown.store(true, Ordering::Relaxed);
    server.join().ok();
    std::fs::remove_dir_all(&dir).ok();
}
