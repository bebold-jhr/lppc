use clap::Parser;
use std::path::PathBuf;

/// Interactive CLI tool for creating LPPC mapping files.
///
/// DISCLAIMER: Generated mappings require manual review before use.
/// This tool assists in creating mapping files but does not guarantee
/// correctness or completeness of the generated content.
#[derive(Parser, Debug)]
#[command(name = "lppc-mapping-creator")]
#[command(version)]
#[command(about, long_about = None)]
#[command(after_help = "DISCLAIMER: Generated mappings require manual review before use.")]
pub struct Args {
    /// Path to the mappings repository (absolute or relative)
    pub working_dir: PathBuf,

    /// Enable verbose logging output
    #[arg(long)]
    pub verbose: bool,
}
