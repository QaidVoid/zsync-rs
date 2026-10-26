use std::io::{IsTerminal, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use clap::Parser;
use zsync_rs::ZsyncAssembly;

#[derive(Parser)]
#[command(name = "zsync")]
#[command(about = "Efficient file transfer using rsync algorithm over HTTP")]
struct Cli {
    /// URL to .zsync control file
    #[arg(value_name = "URL")]
    url: String,

    /// Output filename
    #[arg(short, long, value_name = "FILE")]
    output: Option<PathBuf>,

    /// Source file(s) to use for local matching (can be specified multiple times)
    #[arg(short = 'i', long, value_name = "FILE")]
    input: Vec<PathBuf>,

    /// Gap threshold in KiB for merging HTTP range requests (default: 256)
    #[arg(long, value_name = "KIB", default_value_t = 256)]
    range_gap: u64,

    /// Emit newline-delimited JSON progress events on stdout
    #[arg(long)]
    json: bool,
}

fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    const TB: u64 = GB * 1024;

    if bytes >= TB {
        format!("{:.2} TB", bytes as f64 / TB as f64)
    } else if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{bytes} B")
    }
}

fn format_pct(done: usize, total: usize) -> String {
    if total == 0 {
        return "100.00".to_string();
    }
    format!("{:.2}", done as f64 / total as f64 * 100.0)
}

/// How often a progress event may be emitted.
///
/// The library reports once per block, which is thousands of events for a
/// large file. Throttling here keeps terminal output readable and bounds the
/// volume a JSONL consumer has to parse. The library's own behaviour is
/// unchanged; this is presentation only.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);

/// Width of the progress bar in characters, excluding the brackets.
const BAR_WIDTH: usize = 30;

/// Where progress goes and how it is drawn.
#[derive(Clone, Copy, PartialEq)]
enum Mode {
    /// A single bar redrawn in place, for a terminal.
    Bar,
    /// One line per update, for stderr redirected to a file or pipe.
    Lines,
    Json,
}

/// Serialises progress events from the download callback and the main thread.
///
/// The library's callback is `Fn + Send + Sync` and owns its closure, so it
/// cannot hold a plain `&mut` to local state. A mutex-guarded reporter gives
/// the callback and the main thread one place to write without duplicating
/// the throttle logic.
struct Reporter {
    mode: Mode,
    started: Instant,
    last_emit: Option<Instant>,
    last_percent: i32,
    bar_drawn: bool,
    download_finished: bool,
}

impl Reporter {
    fn new(mode: Mode) -> Self {
        Self {
            mode,
            started: Instant::now(),
            last_emit: None,
            last_percent: -1,
            bar_drawn: false,
            download_finished: false,
        }
    }

    fn write_line(&self, line: &str) {
        let mut out = std::io::stdout().lock();
        // A closed consumer must not abort a running transfer.
        let _ = writeln!(out, "{line}");
        let _ = out.flush();
    }

    fn emit_start(&self, total_bytes: u64, total_blocks: usize, blocksize: usize) {
        if self.mode != Mode::Json {
            return;
        }
        self.write_line(&format!(
            "{{\"event\":\"start\",\"total_bytes\":{total_bytes},\"total_blocks\":{total_blocks},\"blocksize\":{blocksize}}}"
        ));
    }

    /// Emits a download progress event if the throttle allows it.
    ///
    /// Lines mode additionally drops an event that would repeat the previous
    /// whole percent, so a slow transfer does not print the same line twice.
    fn progress(&mut self, done: u64, total: u64) {
        if self.download_finished {
            return;
        }
        let now = Instant::now();
        if let Some(last) = self.last_emit
            && now.duration_since(last) < PROGRESS_INTERVAL
        {
            return;
        }

        let percent = whole_percent(done, total);
        if self.mode == Mode::Lines && percent == self.last_percent {
            return;
        }

        self.last_emit = Some(now);
        self.last_percent = percent;

        match self.mode {
            Mode::Json => {
                let elapsed_ms = self.started.elapsed().as_millis();
                self.write_line(&format!(
                    "{{\"event\":\"progress\",\"phase\":\"download\",\"done_bytes\":{done},\"total_bytes\":{total},\"elapsed_ms\":{elapsed_ms}}}"
                ));
            }
            Mode::Bar => self.draw_bar(done, total),
            Mode::Lines => {
                eprintln!(
                    "  {} / {} ({percent}%)",
                    format_bytes(done),
                    format_bytes(total)
                );
            }
        }
    }

    fn draw_bar(&mut self, done: u64, total: u64) {
        // `\x1b[K` clears what a longer previous draw left past the cursor.
        eprint!("\r{}\x1b[K", render_bar(done, total));
        self.bar_drawn = true;
    }

    /// Moves past a drawn bar so the next message starts on its own line.
    fn end_bar(&mut self) {
        if self.bar_drawn {
            eprintln!();
            self.bar_drawn = false;
        }
    }

    /// Emits the terminal download event, bypassing the throttle.
    ///
    /// A consumer must always see the completed count, even when the last
    /// block lands inside a throttle window. Human modes print it only when a
    /// download printed progress, and lines mode only when it would not repeat
    /// the last line.
    fn finish_download(&mut self, done: u64, total: u64) {
        if self.download_finished {
            return;
        }
        self.download_finished = true;

        match self.mode {
            Mode::Json => {
                let elapsed_ms = self.started.elapsed().as_millis();
                self.write_line(&format!(
                    "{{\"event\":\"progress\",\"phase\":\"download\",\"done_bytes\":{done},\"total_bytes\":{total},\"elapsed_ms\":{elapsed_ms},\"final\":true}}"
                ));
            }
            Mode::Bar => {
                if self.bar_drawn {
                    self.draw_bar(done, total);
                    self.end_bar();
                }
            }
            Mode::Lines => {
                let percent = whole_percent(done, total);
                if self.last_emit.is_some() && percent != self.last_percent {
                    eprintln!(
                        "  {} / {} ({percent}%)",
                        format_bytes(done),
                        format_bytes(total)
                    );
                }
            }
        }
    }

    fn emit_phase(&self, name: &str) {
        if self.mode != Mode::Json {
            return;
        }
        self.write_line(&format!("{{\"event\":\"phase\",\"phase\":\"{name}\"}}"));
    }

    fn emit_done(&self, path: &str) {
        if self.mode != Mode::Json {
            return;
        }
        self.write_line(&format!(
            "{{\"event\":\"done\",\"path\":\"{}\"}}",
            json_escape(path)
        ));
    }

    fn emit_error(&mut self, message: &str) {
        self.end_bar();
        if self.mode != Mode::Json {
            return;
        }
        self.write_line(&format!(
            "{{\"event\":\"error\",\"message\":\"{}\"}}",
            json_escape(message)
        ));
    }
}

fn whole_percent(done: u64, total: u64) -> i32 {
    done.saturating_mul(100).checked_div(total).unwrap_or(100) as i32
}

fn render_bar(done: u64, total: u64) -> String {
    let percent = whole_percent(done, total).clamp(0, 100);
    let filled = percent as usize * BAR_WIDTH / 100;
    format!(
        "  [{}{}] {percent:>3}%  {} / {}",
        "#".repeat(filled),
        " ".repeat(BAR_WIDTH - filled),
        format_bytes(done),
        format_bytes(total)
    )
}

/// Escapes a string for embedding in a JSON string literal.
///
/// Handles what a path or error message can contain: quote, backslash, and
/// control characters. Everything else passes through as UTF-8, which is
/// already valid JSON.
fn json_escape(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// The output name for `url` when none is given: its last path segment,
/// without any query, fragment or `.zsync` suffix.
fn default_output(url: &str) -> PathBuf {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let name = path.rsplit('/').next().unwrap_or(path);
    let name = name.strip_suffix(".zsync").unwrap_or(name);
    PathBuf::from(if name.is_empty() { "output" } else { name })
}

fn main() {
    let cli = Cli::parse();
    let mode = if cli.json {
        Mode::Json
    } else if std::io::stderr().is_terminal() {
        Mode::Bar
    } else {
        Mode::Lines
    };

    let output_path = cli.output.unwrap_or_else(|| default_output(&cli.url));

    let reporter = Arc::new(Mutex::new(Reporter::new(mode)));

    let mut assembly = match ZsyncAssembly::from_url(&cli.url, &output_path) {
        Ok(a) => a,
        Err(e) => {
            let msg = format!("Error initializing: {e}");
            if let Ok(mut r) = reporter.lock() {
                r.emit_error(&msg);
            }
            eprintln!("{msg}");
            std::process::exit(1);
        }
    };

    assembly.set_range_gap_threshold(cli.range_gap.saturating_mul(1024));

    {
        let (_, total_bytes) = assembly.progress();
        let (_, total_blocks) = assembly.block_stats();
        if let Ok(r) = reporter.lock() {
            r.emit_start(total_bytes, total_blocks, assembly.blocksize());
        }
    }

    for input_path in &cli.input {
        match assembly.submit_source_file(input_path) {
            Ok(blocks) => {
                let (done, total) = assembly.block_stats();
                eprintln!(
                    "Matched {blocks} blocks from {} ({}%)",
                    input_path.display(),
                    format_pct(done, total)
                );
            }
            Err(e) => eprintln!("Warning: failed to read {}: {e}", input_path.display()),
        }
    }

    // Self-referential scan: the temp file may contain duplicate target blocks
    if !cli.input.is_empty() && !assembly.is_complete() {
        match assembly.submit_self_referential() {
            Ok(0) => {}
            Ok(blocks) => {
                let (done, total) = assembly.block_stats();
                eprintln!(
                    "Matched {blocks} additional blocks from self-reference ({}%)",
                    format_pct(done, total)
                );
            }
            Err(e) => eprintln!("Warning: self-referential scan failed: {e}"),
        }
    }

    let (done_bytes, total_bytes) = assembly.progress();
    let (done_blocks, total_blocks) = assembly.block_stats();
    let need_bytes = total_bytes - done_bytes;

    eprintln!(
        "Target: {} ({} blocks) - {}% complete",
        format_bytes(total_bytes),
        total_blocks,
        format_pct(done_blocks, total_blocks)
    );
    eprintln!("Need to download: {}", format_bytes(need_bytes));

    if !assembly.is_complete() {
        eprintln!("Downloading missing blocks...");

        let callback_reporter = Arc::clone(&reporter);
        assembly.set_progress_callback(move |done, total| {
            if let Ok(mut r) = callback_reporter.lock() {
                r.progress(done, total);
            }
        });

        loop {
            match assembly.download_missing_blocks() {
                Ok(0) => break,
                Ok(_) => {}
                Err(e) => {
                    let msg = format!("Download error: {e}");
                    if let Ok(mut r) = reporter.lock() {
                        r.emit_error(&msg);
                    }
                    eprintln!("{msg}");
                    // `exit` skips destructors, and dropping removes the temp file.
                    drop(assembly);
                    std::process::exit(1);
                }
            }
        }
    }

    if let Ok(mut r) = reporter.lock() {
        let (done, total) = assembly.progress();
        r.finish_download(done, total);
        r.emit_phase("verify");
    }

    eprintln!("Verifying checksum...");
    if let Err(e) = assembly.complete() {
        let msg = format!("Error: {e}");
        if let Ok(mut r) = reporter.lock() {
            r.emit_error(&msg);
        }
        eprintln!("{msg}");
        std::process::exit(1);
    }

    if let Ok(r) = reporter.lock() {
        r.emit_done(&output_path.display().to_string());
    }
    eprintln!("Done: {}", output_path.display());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_output_is_the_last_segment_without_the_suffix() {
        let name = |url| default_output(url).display().to_string();
        assert_eq!(name("https://h/a/app.AppImage.zsync"), "app.AppImage");
        assert_eq!(name("https://h/a/app.zsync?sig=x/y&e=1"), "app");
        assert_eq!(name("https://h/a/app.zsync#part"), "app");
        assert_eq!(name("https://h/a/"), "output");
    }

    #[test]
    fn json_escape_covers_what_a_path_or_message_can_hold() {
        assert_eq!(json_escape(r#"a"b"#), r#"a\"b"#);
        assert_eq!(json_escape(r"C:\dir\f.bin"), r"C:\\dir\\f.bin");
        assert_eq!(json_escape("line\nnext\ttab\r"), r"line\nnext\ttab\r");
        assert_eq!(json_escape("\u{1}"), r"\u0001");
        assert_eq!(json_escape("ünïcode"), "ünïcode");
    }

    #[test]
    fn bar_fills_in_proportion_to_progress() {
        assert_eq!(
            render_bar(0, 1000),
            format!("  [{}]   0%  0 B / 1000 B", " ".repeat(BAR_WIDTH))
        );
        assert_eq!(
            render_bar(500, 1000),
            format!(
                "  [{}{}]  50%  500 B / 1000 B",
                "#".repeat(BAR_WIDTH / 2),
                " ".repeat(BAR_WIDTH - BAR_WIDTH / 2)
            )
        );
        assert_eq!(
            render_bar(1000, 1000),
            format!("  [{}] 100%  1000 B / 1000 B", "#".repeat(BAR_WIDTH))
        );
    }

    #[test]
    fn bar_treats_an_empty_target_as_complete() {
        assert!(render_bar(0, 0).contains("100%"));
    }
}
