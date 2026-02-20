//! Output generation module for LPPC.
//!
//! This module handles formatting and writing permission results to various
//! output destinations (stdout or files) in multiple formats (JSON, HCL).

pub mod formatter;
pub mod hcl;
pub mod json;

use std::fs;
use std::io::{self, Write};
use std::path::Path;

use colored::Colorize;
use thiserror::Error;

use crate::cli::OutputFormat;
use crate::mapping::PermissionResult;
use formatter::{create_formatter, PermissionSets};

/// Errors that can occur during output generation.
#[derive(Debug, Error)]
pub enum OutputError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Invalid filename: {0}")]
    InvalidFilename(String),
}

/// Sanitizes a filename to prevent path traversal and other security issues.
///
/// This function removes or replaces characters that could be dangerous in filenames:
/// - Path separators (/ and \)
/// - Path traversal sequences (..)
/// - Leading/trailing dots and spaces
///
/// Returns `None` if the resulting filename would be empty or dangerous.
fn sanitize_filename(name: &str) -> Option<String> {
    // Replace path separators and other dangerous characters
    let sanitized: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | '\0' => '_',
            _ => c,
        })
        .collect();

    // Remove leading/trailing whitespace and dots
    let trimmed = sanitized.trim().trim_matches('.');

    // Check for empty or dangerous patterns
    if trimmed.is_empty() {
        return None;
    }

    // Reject if it still contains path traversal attempts
    if trimmed.contains("..") {
        return None;
    }

    // Reject hidden files (starting with .)
    if trimmed.starts_with('.') {
        return None;
    }

    Some(trimmed.to_string())
}

/// Writes permission results to stdout or files.
///
/// The `OutputWriter` handles both stdout output (with headers separating
/// different provider groups) and directory output (separate files per group).
pub struct OutputWriter {
    format: OutputFormat,
    output_dir: Option<std::path::PathBuf>,
    no_color: bool,
}

impl OutputWriter {
    /// Creates a new output writer.
    ///
    /// # Arguments
    ///
    /// * `format` - The output format to use
    /// * `output_dir` - Optional directory for file output; if None, outputs to stdout
    /// * `no_color` - Whether to disable colored output
    pub fn new(
        format: OutputFormat,
        output_dir: Option<std::path::PathBuf>,
        no_color: bool,
    ) -> Self {
        Self {
            format,
            output_dir,
            no_color,
        }
    }

    /// Writes all permission results to output.
    ///
    /// When `output_dir` is set, creates one file per provider group.
    /// Otherwise, writes all groups to stdout with headers.
    ///
    /// # Arguments
    ///
    /// * `result` - The permission result containing resolved permissions
    ///
    /// # Returns
    ///
    /// `Ok(())` on success, or an `OutputError` if writing fails.
    pub fn write(&self, result: &PermissionResult) -> Result<(), OutputError> {
        let formatter = create_formatter(self.format);

        match &self.output_dir {
            Some(dir) => self.write_to_directory(dir, result, &*formatter),
            None => self.write_to_stdout(result, &*formatter),
        }
    }

    /// Writes permission results to stdout with headers.
    fn write_to_stdout(
        &self,
        result: &PermissionResult,
        formatter: &dyn formatter::OutputFormatter,
    ) -> Result<(), OutputError> {
        let stdout = io::stdout();
        let mut handle = stdout.lock();

        // Sort output names for consistent ordering
        let mut output_names: Vec<_> = result.groups.keys().collect();
        output_names.sort();

        for (i, output_name) in output_names.iter().enumerate() {
            if i > 0 {
                writeln!(handle)?;
            }

            let header = format!("----------- {} -----------", output_name);
            if self.no_color {
                writeln!(handle, "{}", header)?;
            } else {
                writeln!(handle, "{}", header.cyan().bold())?;
            }

            let group_perms = result.groups.get(*output_name).unwrap();
            let formatted = formatter.format(&PermissionSets {
                allow: &group_perms.allow,
                deny: &group_perms.deny,
            });
            writeln!(handle, "{}", formatted)?;
        }

        Ok(())
    }

    /// Writes permission results to files in a directory.
    fn write_to_directory(
        &self,
        dir: &Path,
        result: &PermissionResult,
        formatter: &dyn formatter::OutputFormatter,
    ) -> Result<(), OutputError> {
        // Create directory if it doesn't exist
        fs::create_dir_all(dir)?;

        for (output_name, group_perms) in &result.groups {
            // Sanitize the output name to prevent path traversal
            let safe_name = sanitize_filename(output_name).ok_or_else(|| {
                OutputError::InvalidFilename(format!(
                    "Output name '{}' contains invalid characters",
                    output_name
                ))
            })?;

            let filename = format!("{}.{}", safe_name, formatter.extension());
            let file_path = dir.join(&filename);

            // Double-check that the resulting path is still within the output directory
            let canonical_dir = dir.canonicalize().unwrap_or_else(|_| dir.to_path_buf());
            let canonical_file = file_path
                .parent()
                .and_then(|p| p.canonicalize().ok())
                .unwrap_or_else(|| file_path.parent().unwrap_or(dir).to_path_buf());

            if !canonical_file.starts_with(&canonical_dir) {
                return Err(OutputError::InvalidFilename(format!(
                    "Output path escapes output directory: {}",
                    output_name
                )));
            }

            let formatted = formatter.format(&PermissionSets {
                allow: &group_perms.allow,
                deny: &group_perms.deny,
            });
            fs::write(&file_path, formatted)?;

            log::info!("Written: {}", file_path.display());
        }

        Ok(())
    }

    /// Writes missing mappings warning to stderr.
    ///
    /// This method should be called to inform users about resources
    /// that don't have mapping files and may require manual review.
    ///
    /// # Arguments
    ///
    /// * `result` - The permission result containing missing mapping information
    pub fn write_missing_mappings(&self, result: &PermissionResult) {
        if result.missing_mappings.is_empty() {
            return;
        }

        let stderr = io::stderr();
        let mut handle = stderr.lock();

        let warning = "Warning: No mapping files found for the following resources:";
        if self.no_color {
            writeln!(handle, "\n{}", warning).ok();
        } else {
            writeln!(handle, "\n{}", warning.yellow()).ok();
        }

        for missing in &result.missing_mappings {
            writeln!(
                handle,
                "  - {}.{} (expected: {})",
                missing.block_type.as_str(),
                missing.type_name,
                missing.expected_path
            )
            .ok();
        }

        writeln!(
            handle,
            "These resources may require manual permission review.\n"
        )
        .ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mapping::{GroupPermissions, MissingMapping};
    use crate::terraform::BlockType;
    use std::collections::{HashMap, HashSet};
    use tempfile::TempDir;

    fn create_test_result() -> PermissionResult {
        let mut groups = HashMap::new();

        let mut allow1 = HashSet::new();
        allow1.insert("ec2:DescribeInstances".to_string());
        allow1.insert("ec2:RunInstances".to_string());
        groups.insert(
            "ComputeDeployer".to_string(),
            GroupPermissions {
                allow: allow1,
                deny: HashSet::new(),
            },
        );

        let mut allow2 = HashSet::new();
        allow2.insert("s3:CreateBucket".to_string());
        allow2.insert("s3:DeleteBucket".to_string());
        groups.insert(
            "StorageDeployer".to_string(),
            GroupPermissions {
                allow: allow2,
                deny: HashSet::new(),
            },
        );

        PermissionResult {
            groups,
            missing_mappings: Vec::new(),
        }
    }

    #[test]
    fn write_to_directory_creates_files() {
        let temp_dir = TempDir::new().unwrap();
        let writer = OutputWriter::new(
            OutputFormat::Json,
            Some(temp_dir.path().to_path_buf()),
            true,
        );
        let result = create_test_result();

        writer.write(&result).unwrap();

        // Check that files were created
        let compute_file = temp_dir.path().join("ComputeDeployer.json");
        let storage_file = temp_dir.path().join("StorageDeployer.json");

        assert!(compute_file.exists());
        assert!(storage_file.exists());

        // Verify content is valid JSON
        let compute_content = fs::read_to_string(&compute_file).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&compute_content).unwrap();
        assert_eq!(parsed["Version"], "2012-10-17");
    }

    #[test]
    fn write_to_directory_creates_dir_if_missing() {
        let temp_dir = TempDir::new().unwrap();
        let nested_dir = temp_dir.path().join("nested").join("output");

        assert!(!nested_dir.exists());

        let writer = OutputWriter::new(OutputFormat::HclGrouped, Some(nested_dir.clone()), true);
        let result = create_test_result();

        writer.write(&result).unwrap();

        assert!(nested_dir.exists());
    }

    #[test]
    fn write_hcl_format_creates_hcl_files() {
        let temp_dir = TempDir::new().unwrap();
        let writer =
            OutputWriter::new(OutputFormat::Hcl, Some(temp_dir.path().to_path_buf()), true);
        let result = create_test_result();

        writer.write(&result).unwrap();

        let compute_file = temp_dir.path().join("ComputeDeployer.hcl");
        assert!(compute_file.exists());

        let content = fs::read_to_string(&compute_file).unwrap();
        assert!(content.starts_with("jsonencode({"));
    }

    #[test]
    fn write_directory_files_have_no_headers() {
        let temp_dir = TempDir::new().unwrap();
        let writer = OutputWriter::new(
            OutputFormat::HclGrouped,
            Some(temp_dir.path().to_path_buf()),
            true,
        );
        let result = create_test_result();

        writer.write(&result).unwrap();

        let compute_file = temp_dir.path().join("ComputeDeployer.hcl");
        let content = fs::read_to_string(&compute_file).unwrap();

        // File should NOT contain the header
        assert!(!content.contains("-----------"));
    }

    #[test]
    fn write_missing_mappings_outputs_warning() {
        let result = PermissionResult {
            groups: HashMap::new(),
            missing_mappings: vec![MissingMapping {
                block_type: BlockType::Resource,
                type_name: "aws_unknown_resource".to_string(),
                expected_path: "mappings/aws/resource/aws_unknown_resource.yaml".to_string(),
            }],
        };

        let writer = OutputWriter::new(OutputFormat::HclGrouped, None, true);

        // This writes to stderr, which is hard to capture in tests
        // Just verify it doesn't panic
        writer.write_missing_mappings(&result);
    }

    #[test]
    fn write_missing_mappings_empty_does_nothing() {
        let result = PermissionResult {
            groups: HashMap::new(),
            missing_mappings: Vec::new(),
        };

        let writer = OutputWriter::new(OutputFormat::HclGrouped, None, true);

        // Should not panic or produce output
        writer.write_missing_mappings(&result);
    }

    #[test]
    fn empty_permissions_creates_no_files() {
        let temp_dir = TempDir::new().unwrap();
        let writer = OutputWriter::new(
            OutputFormat::Json,
            Some(temp_dir.path().to_path_buf()),
            true,
        );
        let result = PermissionResult {
            groups: HashMap::new(),
            missing_mappings: Vec::new(),
        };

        writer.write(&result).unwrap();

        let entries: Vec<_> = fs::read_dir(temp_dir.path()).unwrap().collect();
        assert!(entries.is_empty());
    }

    #[test]
    fn sanitize_filename_removes_path_separators() {
        assert_eq!(
            super::sanitize_filename("foo/bar"),
            Some("foo_bar".to_string())
        );
        assert_eq!(
            super::sanitize_filename("foo\\bar"),
            Some("foo_bar".to_string())
        );
    }

    #[test]
    fn sanitize_filename_rejects_path_traversal() {
        // Pure ".." is rejected
        assert_eq!(super::sanitize_filename(".."), None);
        // Contains ".." sequence
        assert_eq!(super::sanitize_filename("foo..bar"), None);
        // Path separators are replaced, making this safe
        assert_eq!(
            super::sanitize_filename("../etc/passwd"),
            Some("_etc_passwd".to_string())
        );
    }

    #[test]
    fn sanitize_filename_rejects_empty() {
        assert_eq!(super::sanitize_filename(""), None);
        assert_eq!(super::sanitize_filename("   "), None);
        assert_eq!(super::sanitize_filename("..."), None);
    }

    #[test]
    fn sanitize_filename_trims_leading_dots() {
        // Leading dots are trimmed, making this safe
        assert_eq!(
            super::sanitize_filename(".hidden"),
            Some("hidden".to_string())
        );
        assert_eq!(
            super::sanitize_filename("..hidden"),
            Some("hidden".to_string())
        );
    }

    #[test]
    fn sanitize_filename_accepts_valid_names() {
        assert_eq!(
            super::sanitize_filename("NetworkDeployer"),
            Some("NetworkDeployer".to_string())
        );
        assert_eq!(
            super::sanitize_filename("MyRole-Deployer"),
            Some("MyRole-Deployer".to_string())
        );
    }

    #[test]
    fn write_rejects_path_traversal_in_output_name() {
        let temp_dir = TempDir::new().unwrap();
        let writer = OutputWriter::new(
            OutputFormat::HclGrouped,
            Some(temp_dir.path().to_path_buf()),
            true,
        );

        let mut groups = HashMap::new();
        let mut allow = HashSet::new();
        allow.insert("s3:GetObject".to_string());
        groups.insert(
            "../../../etc/malicious".to_string(),
            GroupPermissions {
                allow,
                deny: HashSet::new(),
            },
        );

        let result = PermissionResult {
            groups,
            missing_mappings: Vec::new(),
        };

        let write_result = writer.write(&result);
        assert!(write_result.is_err());

        // Verify no files were written outside the directory
        let parent = temp_dir.path().parent().unwrap();
        let malicious_file = parent.join("malicious.hcl");
        assert!(!malicious_file.exists());
    }

    // --- New deny tests ---

    #[test]
    fn write_to_directory_with_deny_permissions() {
        let temp_dir = TempDir::new().unwrap();
        let writer = OutputWriter::new(
            OutputFormat::Json,
            Some(temp_dir.path().to_path_buf()),
            true,
        );

        let mut groups = HashMap::new();
        let mut allow = HashSet::new();
        allow.insert("s3:Get*".to_string());
        let mut deny = HashSet::new();
        deny.insert("s3:GetObject".to_string());
        groups.insert(
            "TestDeployer".to_string(),
            GroupPermissions { allow, deny },
        );

        let result = PermissionResult {
            groups,
            missing_mappings: Vec::new(),
        };

        writer.write(&result).unwrap();

        let file = temp_dir.path().join("TestDeployer.json");
        assert!(file.exists());

        let content = fs::read_to_string(&file).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        let statements = parsed["Statement"].as_array().unwrap();

        // Should have both Deny and Allow statements
        assert_eq!(statements.len(), 2);
        assert_eq!(statements[0]["Effect"], "Deny");
        assert_eq!(statements[1]["Effect"], "Allow");
    }

    #[test]
    fn write_deny_only_group() {
        let temp_dir = TempDir::new().unwrap();
        let writer = OutputWriter::new(
            OutputFormat::Json,
            Some(temp_dir.path().to_path_buf()),
            true,
        );

        let mut groups = HashMap::new();
        let mut deny = HashSet::new();
        deny.insert("s3:GetObject".to_string());
        groups.insert(
            "TestDeployer".to_string(),
            GroupPermissions {
                allow: HashSet::new(),
                deny,
            },
        );

        let result = PermissionResult {
            groups,
            missing_mappings: Vec::new(),
        };

        writer.write(&result).unwrap();

        let file = temp_dir.path().join("TestDeployer.json");
        assert!(file.exists());

        let content = fs::read_to_string(&file).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
        let statements = parsed["Statement"].as_array().unwrap();

        // Only Deny statement
        assert_eq!(statements.len(), 1);
        assert_eq!(statements[0]["Effect"], "Deny");
    }
}
