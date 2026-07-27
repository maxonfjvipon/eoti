//! The command-line entry point: XMIR in, verdicts out.

use clap::Parser;
use std::path::PathBuf;
use std::process::ExitCode;

/// What the checker was asked to look at.
#[derive(Parser)]
#[command(
    name = "eoti",
    version,
    about = "Structural type inference and pre-run type checker for EO, over XMIR"
)]
struct Args {
    /// XMIR files to check, as EO's parser writes them into `1-parse`
    #[arg(value_name = "XMIR")]
    files: Vec<PathBuf>,
}

fn main() -> ExitCode {
    eprintln!(
        "the inference engine is not built yet, {} file(s) left unchecked",
        Args::parse().files.len()
    );
    ExitCode::from(2)
}
