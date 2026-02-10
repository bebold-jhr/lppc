use std::path::PathBuf;

use clap::Parser;

/// Least Privilege Policy Creator
///
/// Generates minimal AWS IAM policies based on static analysis of Terraform code.
///
/// DISCLAIMER: The generated policies provide a starting point. Manual review
/// is encouraged to add resource constraints, conditions, and account-specific
/// refinements.
#[derive(Parser, Debug)]
#[command(name = "lppc")]
#[command(version)]
#[command(about, long_about)]
pub struct Cli {
    /// Suppress colored output (useful for CI/CD pipelines)
    #[arg(short = 'n', long = "no-color")]
    pub no_color: bool,

    /// Enable verbose output for debugging
    #[arg(long = "verbose")]
    pub verbose: bool,

    /// Working directory containing Terraform files
    #[arg(short = 'd', long = "working-dir")]
    pub working_dir: Option<PathBuf>,

    /// Output directory for generated policy files
    #[arg(short = 'o', long = "output-dir")]
    pub output_dir: Option<PathBuf>,

    /// Output format: plain, json, json-grouped, hcl, hcl-grouped
    #[arg(short = 'f', long = "output-format", default_value = "plain")]
    pub output_format: OutputFormat,

    /// URL of the git repository containing mapping files
    #[arg(
        short = 'm',
        long = "mappings-url",
        default_value = "https://github.com/bebold-jhr/lppc-aws-mappings"
    )]
    pub mappings_url: String,

    /// Force refresh of the mapping repository
    #[arg(short = 'r', long = "refresh-mappings")]
    pub refresh_mappings: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, clap::ValueEnum)]
pub enum OutputFormat {
    #[default]
    Plain,
    Json,
    JsonGrouped,
    Hcl,
    HclGrouped,
}
