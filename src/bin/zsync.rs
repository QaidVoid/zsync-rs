use std::path::PathBuf;

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

    /// Source file to use for local matching
    #[arg(short = 'i', long, value_name = "FILE")]
    input: Option<PathBuf>,
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

fn main() {
    let cli = Cli::parse();

    let output_path = cli.output.unwrap_or_else(|| {
        PathBuf::from(
            cli.url
                .trim_end_matches(".zsync")
                .rsplit('/')
                .next()
                .unwrap_or("output"),
        )
    });

    let mut assembly = match ZsyncAssembly::from_url(&cli.url, &output_path) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("Error initializing: {e}");
            std::process::exit(1);
        }
    };

    if let Some(ref input_path) = cli.input {
        match assembly.submit_source_file(input_path) {
            Ok(blocks) => {
                let (done, total) = assembly.block_stats();
                let pct = if total > 0 {
                    (done as f64 / total as f64 * 100.0) as u32
                } else {
                    0
                };
                eprintln!("Matched {blocks} blocks from source ({pct}%)");
            }
            Err(e) => eprintln!("Warning: failed to read source file: {e}"),
        }
    }

    let (done_bytes, total_bytes) = assembly.progress();
    let (done_blocks, total_blocks) = assembly.block_stats();
    let need_bytes = total_bytes - done_bytes;
    let pct_done = if total_blocks > 0 {
        (done_blocks as f64 / total_blocks as f64 * 100.0) as u32
    } else {
        100
    };

    eprintln!(
        "Target: {} ({} blocks) - {}% complete",
        format_bytes(total_bytes),
        total_blocks,
        pct_done
    );
    eprintln!("Need to download: {}", format_bytes(need_bytes));

    if !assembly.is_complete() {
        eprintln!("Downloading missing blocks...");
        loop {
            match assembly.download_missing_blocks() {
                Ok(0) => break,
                Ok(n) => {
                    let (done, total) = assembly.block_stats();
                    let pct = if total > 0 {
                        (done as f64 / total as f64 * 100.0) as u32
                    } else {
                        100
                    };
                    eprintln!("Downloaded {n} blocks ({pct}%)");
                }
                Err(e) => {
                    eprintln!("Download error: {e}");
                    std::process::exit(1);
                }
            }
        }
    }

    eprintln!("Verifying checksum...");
    if let Err(e) = assembly.complete() {
        eprintln!("Error: {e}");
        std::process::exit(1);
    }

    eprintln!("Done: {}", output_path.display());
}
