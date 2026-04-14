use std::fs::File;
use std::io::BufWriter;
use std::path::PathBuf;

use clap::Parser;
use zsync_rs::ControlFile;

#[derive(Parser)]
#[command(name = "zsyncmake")]
#[command(about = "Generate zsync control files for efficient delta transfers")]
struct Cli {
    /// Input file to generate zsync data for
    #[arg(value_name = "FILE")]
    file: PathBuf,

    /// URL for the file (default: filename)
    #[arg(short = 'u', value_name = "URL")]
    url: Option<String>,

    /// Output .zsync file (default: <input>.zsync)
    #[arg(short = 'o', value_name = "FILE")]
    output: Option<PathBuf>,

    /// Block size (default: auto-calculated)
    #[arg(short = 'b', value_name = "BLOCKSIZE")]
    blocksize: Option<usize>,
}

fn main() {
    let cli = Cli::parse();

    let filename = cli
        .file
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string();

    let url = cli.url.unwrap_or_else(|| filename.clone());
    let output_path = cli
        .output
        .unwrap_or_else(|| PathBuf::from(format!("{filename}.zsync")));

    let mut input = match File::open(&cli.file) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("Error opening {}: {e}", cli.file.display());
            std::process::exit(1);
        }
    };

    eprintln!("Reading {}...", cli.file.display());

    let control = match ControlFile::generate(&mut input, &filename, &url, cli.blocksize) {
        Ok(cf) => cf,
        Err(e) => {
            eprintln!("Error generating control file: {e}");
            std::process::exit(1);
        }
    };

    let output = match File::create(&output_path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("Error creating {}: {e}", output_path.display());
            std::process::exit(1);
        }
    };

    if let Err(e) = control.write(&mut BufWriter::new(output)) {
        eprintln!("Error writing control file: {e}");
        std::process::exit(1);
    }

    eprintln!(
        "Wrote {} ({} blocks, blocksize {})",
        output_path.display(),
        control.num_blocks(),
        control.blocksize
    );
}
