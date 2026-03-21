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
            Ok(blocks) => eprintln!("Found {blocks} matching blocks in source file"),
            Err(e) => eprintln!("Warning: failed to read source file: {e}"),
        }
    }

    let (done, total) = assembly.progress();
    eprintln!("Need to download {} of {} bytes", total - done, total);

    if !assembly.is_complete() {
        eprintln!("Downloading missing blocks...");
        loop {
            match assembly.download_missing_blocks() {
                Ok(0) => break,
                Ok(n) => eprintln!("Downloaded {n} blocks"),
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
