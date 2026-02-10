use std::path::{Path, PathBuf};

use crate::cli::{Cli, OutputFormat};
use crate::error::LppcError;

#[derive(Debug)]
pub struct Config {
    pub no_color: bool,
    pub verbose: bool,
    pub working_dir: PathBuf,
    pub output_dir: Option<PathBuf>,
    pub output_format: OutputFormat,
    pub mappings_url: String,
    pub refresh_mappings: bool,
}

impl Config {
    pub fn from_cli(cli: Cli) -> Result<Self, LppcError> {
        let working_dir = match cli.working_dir {
            Some(path) => Self::resolve_path(&path)?,
            None => std::env::current_dir().map_err(|e| {
                LppcError::Config(format!("Cannot determine current directory: {}", e))
            })?,
        };

        // Verify the directory exists
        if !working_dir.exists() {
            return Err(LppcError::Config(format!(
                "Working directory does not exist: {}",
                working_dir.display()
            )));
        }

        // Verify it's actually a directory
        if !working_dir.is_dir() {
            return Err(LppcError::Config(format!(
                "Working directory is not a directory: {}",
                working_dir.display()
            )));
        }

        // Canonicalize to resolve symlinks and normalize path components
        let working_dir = working_dir.canonicalize().map_err(|e| {
            LppcError::Config(format!(
                "Cannot canonicalize working directory {}: {}",
                working_dir.display(),
                e
            ))
        })?;

        Ok(Self {
            no_color: cli.no_color,
            verbose: cli.verbose,
            working_dir,
            output_dir: cli.output_dir,
            output_format: cli.output_format,
            mappings_url: cli.mappings_url,
            refresh_mappings: cli.refresh_mappings,
        })
    }

    /// Resolves a path to an absolute path.
    /// - Absolute paths are returned as-is
    /// - Relative paths are resolved relative to current directory
    pub fn resolve_path(path: &Path) -> Result<PathBuf, LppcError> {
        if path.is_absolute() {
            Ok(path.to_path_buf())
        } else {
            let current_dir = std::env::current_dir().map_err(|e| {
                LppcError::Config(format!("Cannot determine current directory: {}", e))
            })?;
            Ok(current_dir.join(path))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_cli_with_defaults() {
        let cli = Cli {
            no_color: false,
            verbose: false,
            working_dir: None,
            output_dir: None,
            output_format: OutputFormat::Plain,
            mappings_url: "https://github.com/bebold-jhr/lppc-aws-test".to_string(),
            refresh_mappings: false,
        };

        let config = Config::from_cli(cli).expect("Config creation should succeed");

        assert!(!config.no_color);
        assert!(!config.verbose);
        assert!(config.working_dir.exists());
        assert!(config.output_dir.is_none());
        assert_eq!(config.output_format, OutputFormat::Plain);
        assert_eq!(
            config.mappings_url,
            "https://github.com/bebold-jhr/lppc-aws-test"
        );
        assert!(!config.refresh_mappings);
    }

    #[test]
    fn from_cli_with_custom_working_dir() {
        let temp_dir = std::env::temp_dir();
        // Canonicalize expected path since config now canonicalizes working_dir
        let expected_working_dir = temp_dir.canonicalize().unwrap();
        let cli = Cli {
            no_color: true,
            verbose: true,
            working_dir: Some(temp_dir.clone()),
            output_dir: Some(temp_dir.clone()),
            output_format: OutputFormat::Json,
            mappings_url: "https://example.com/repo".to_string(),
            refresh_mappings: true,
        };

        let config = Config::from_cli(cli).expect("Config creation should succeed");

        assert!(config.no_color);
        assert!(config.verbose);
        assert_eq!(config.working_dir, expected_working_dir);
        assert_eq!(config.output_dir, Some(temp_dir));
        assert_eq!(config.output_format, OutputFormat::Json);
        assert_eq!(config.mappings_url, "https://example.com/repo");
        assert!(config.refresh_mappings);
    }

    #[test]
    fn from_cli_all_output_formats() {
        let formats = [
            OutputFormat::Plain,
            OutputFormat::Json,
            OutputFormat::JsonGrouped,
            OutputFormat::Hcl,
            OutputFormat::HclGrouped,
        ];

        for format in formats {
            let cli = Cli {
                no_color: false,
                verbose: false,
                working_dir: None,
                output_dir: None,
                output_format: format,
                mappings_url: "https://example.com".to_string(),
                refresh_mappings: false,
            };

            let config = Config::from_cli(cli).expect("Config creation should succeed");
            assert_eq!(config.output_format, format);
        }
    }

    #[test]
    fn from_cli_nonexistent_working_dir_fails() {
        let cli = Cli {
            no_color: false,
            verbose: false,
            working_dir: Some(PathBuf::from("/nonexistent/path/that/does/not/exist")),
            output_dir: None,
            output_format: OutputFormat::Plain,
            mappings_url: "https://example.com".to_string(),
            refresh_mappings: false,
        };

        let result = Config::from_cli(cli);
        assert!(result.is_err());
        let error_message = result.unwrap_err().to_string();
        assert!(error_message.contains("does not exist"));
    }

    #[test]
    fn from_cli_file_as_working_dir_fails() {
        // Create a temporary file to use as "working directory"
        let temp_file = tempfile::NamedTempFile::new().expect("Failed to create temp file");
        let file_path = temp_file.path().to_path_buf();

        let cli = Cli {
            no_color: false,
            verbose: false,
            working_dir: Some(file_path),
            output_dir: None,
            output_format: OutputFormat::Plain,
            mappings_url: "https://example.com".to_string(),
            refresh_mappings: false,
        };

        let result = Config::from_cli(cli);
        assert!(result.is_err());
        let error_message = result.unwrap_err().to_string();
        assert!(error_message.contains("is not a directory"));
    }

    #[test]
    fn resolve_absolute_path_unchanged() {
        let absolute_path = PathBuf::from("/absolute/path/to/dir");
        let result = Config::resolve_path(&absolute_path).expect("Resolution should succeed");
        assert_eq!(result, absolute_path);
    }

    #[test]
    fn resolve_relative_path_becomes_absolute() {
        let relative_path = PathBuf::from("relative/path");
        let result = Config::resolve_path(&relative_path).expect("Resolution should succeed");

        assert!(result.is_absolute());
        assert!(result.ends_with("relative/path"));
    }

    #[test]
    fn resolve_current_dir_relative() {
        let relative_path = PathBuf::from(".");
        let current_dir = std::env::current_dir().expect("Should get current dir");
        let result = Config::resolve_path(&relative_path).expect("Resolution should succeed");

        assert!(result.is_absolute());
        assert_eq!(result, current_dir.join("."));
    }
}
