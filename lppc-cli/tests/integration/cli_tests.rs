//! Integration tests for the lppc CLI.
//!
//! These tests require network access and share the cached repository at ~/.lppc.
//! They should be run sequentially to avoid git lock conflicts:
//!
//! ```sh
//! cargo test --test integration -- --test-threads=1
//! ```

#![allow(deprecated)] // cargo_bin is deprecated but works fine for standard builds

use assert_cmd::Command;
use predicates::prelude::*;

/// The default lppc mapping repository for testing.
const TEST_REPO_URL: &str = "https://github.com/bebold-jhr/lppc-aws-mappings";

// ============================================================================
// Help and Version tests (don't require network)
// ============================================================================

#[test]
fn test_help_contains_disclaimer() {
    Command::cargo_bin("lppc")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("DISCLAIMER"));
}

#[test]
fn test_help_short_flag() {
    // Short help (-h) shows condensed output, use --help for full DISCLAIMER
    Command::cargo_bin("lppc")
        .unwrap()
        .arg("-h")
        .assert()
        .success()
        .stdout(predicate::str::contains("Usage"))
        .stdout(predicate::str::contains("--help"));
}

#[test]
fn test_version() {
    Command::cargo_bin("lppc")
        .unwrap()
        .arg("--version")
        .assert()
        .success()
        .stdout(predicate::str::contains(env!("CARGO_PKG_VERSION")));
}

#[test]
fn test_version_short_flag() {
    Command::cargo_bin("lppc")
        .unwrap()
        .arg("-V")
        .assert()
        .success()
        .stdout(predicate::str::contains("lppc"));
}

#[test]
fn test_help_shows_all_options() {
    Command::cargo_bin("lppc")
        .unwrap()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("--no-color"))
        .stdout(predicate::str::contains("--verbose"))
        .stdout(predicate::str::contains("--working-dir"))
        .stdout(predicate::str::contains("--output-dir"))
        .stdout(predicate::str::contains("--output-format"))
        .stdout(predicate::str::contains("--mappings-url"))
        .stdout(predicate::str::contains("--refresh-mappings"));
}

#[test]
fn test_invalid_output_format_fails() {
    Command::cargo_bin("lppc")
        .unwrap()
        .args(["--output-format", "invalid"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid"));
}

// ============================================================================
// Tests that require network access (use TEST_REPO_URL)
// ============================================================================

#[test]
fn test_no_args_with_test_repo() {
    Command::cargo_bin("lppc")
        .unwrap()
        .args(["--mappings-url", TEST_REPO_URL])
        .assert()
        .success();
}

#[test]
fn test_no_color_long_flag() {
    Command::cargo_bin("lppc")
        .unwrap()
        .args(["--no-color", "--mappings-url", TEST_REPO_URL])
        .assert()
        .success();
}

#[test]
fn test_no_color_short_flag() {
    Command::cargo_bin("lppc")
        .unwrap()
        .args(["-n", "--mappings-url", TEST_REPO_URL])
        .assert()
        .success();
}

#[test]
fn test_verbose_flag() {
    Command::cargo_bin("lppc")
        .unwrap()
        .args(["--verbose", "--mappings-url", TEST_REPO_URL])
        .assert()
        .success();
}

#[test]
fn test_verbose_with_no_color() {
    Command::cargo_bin("lppc")
        .unwrap()
        .args(["--verbose", "--no-color", "--mappings-url", TEST_REPO_URL])
        .assert()
        .success();
}

#[test]
fn test_output_format_default_hcl_grouped() {
    Command::cargo_bin("lppc")
        .unwrap()
        .args(["--mappings-url", TEST_REPO_URL])
        .assert()
        .success();
}

#[test]
fn test_output_format_invalid_value_is_rejected() {
    Command::cargo_bin("lppc")
        .unwrap()
        .args(["--output-format", "plain", "--mappings-url", TEST_REPO_URL])
        .assert()
        .failure()
        .stderr(predicate::str::contains("invalid value"));
}

#[test]
fn test_output_format_json() {
    Command::cargo_bin("lppc")
        .unwrap()
        .args(["--output-format", "json", "--mappings-url", TEST_REPO_URL])
        .assert()
        .success();
}

#[test]
fn test_output_format_json_grouped() {
    Command::cargo_bin("lppc")
        .unwrap()
        .args([
            "--output-format",
            "json-grouped",
            "--mappings-url",
            TEST_REPO_URL,
        ])
        .assert()
        .success();
}

#[test]
fn test_output_format_hcl() {
    Command::cargo_bin("lppc")
        .unwrap()
        .args(["--output-format", "hcl", "--mappings-url", TEST_REPO_URL])
        .assert()
        .success();
}

#[test]
fn test_output_format_hcl_grouped() {
    Command::cargo_bin("lppc")
        .unwrap()
        .args([
            "--output-format",
            "hcl-grouped",
            "--mappings-url",
            TEST_REPO_URL,
        ])
        .assert()
        .success();
}

#[test]
fn test_output_format_short_flag() {
    Command::cargo_bin("lppc")
        .unwrap()
        .args(["-f", "json", "--mappings-url", TEST_REPO_URL])
        .assert()
        .success();
}

#[test]
fn test_working_dir_long_flag() {
    let temp_dir = std::env::temp_dir();
    Command::cargo_bin("lppc")
        .unwrap()
        .args([
            "--working-dir",
            temp_dir.to_str().unwrap(),
            "--mappings-url",
            TEST_REPO_URL,
        ])
        .assert()
        .success();
}

#[test]
fn test_working_dir_short_flag() {
    let temp_dir = std::env::temp_dir();
    Command::cargo_bin("lppc")
        .unwrap()
        .args([
            "-d",
            temp_dir.to_str().unwrap(),
            "--mappings-url",
            TEST_REPO_URL,
        ])
        .assert()
        .success();
}

#[test]
fn test_output_dir_long_flag() {
    let temp_dir = std::env::temp_dir();
    Command::cargo_bin("lppc")
        .unwrap()
        .args([
            "--output-dir",
            temp_dir.to_str().unwrap(),
            "--mappings-url",
            TEST_REPO_URL,
        ])
        .assert()
        .success();
}

#[test]
fn test_output_dir_short_flag() {
    let temp_dir = std::env::temp_dir();
    Command::cargo_bin("lppc")
        .unwrap()
        .args([
            "-o",
            temp_dir.to_str().unwrap(),
            "--mappings-url",
            TEST_REPO_URL,
        ])
        .assert()
        .success();
}

#[test]
fn test_mappings_url_long_flag() {
    Command::cargo_bin("lppc")
        .unwrap()
        .args(["--mappings-url", TEST_REPO_URL])
        .assert()
        .success();
}

#[test]
fn test_mappings_url_short_flag() {
    Command::cargo_bin("lppc")
        .unwrap()
        .args(["-m", TEST_REPO_URL])
        .assert()
        .success();
}

#[test]
fn test_refresh_mappings_long_flag() {
    Command::cargo_bin("lppc")
        .unwrap()
        .args(["--refresh-mappings", "--mappings-url", TEST_REPO_URL])
        .assert()
        .success();
}

#[test]
fn test_refresh_mappings_short_flag() {
    Command::cargo_bin("lppc")
        .unwrap()
        .args(["-r", "--mappings-url", TEST_REPO_URL])
        .assert()
        .success();
}

#[test]
fn test_all_flags_combined() {
    let temp_dir = std::env::temp_dir();
    Command::cargo_bin("lppc")
        .unwrap()
        .args([
            "--no-color",
            "--verbose",
            "--working-dir",
            temp_dir.to_str().unwrap(),
            "--output-dir",
            temp_dir.to_str().unwrap(),
            "--output-format",
            "json-grouped",
            "--mappings-url",
            TEST_REPO_URL,
            "--refresh-mappings",
        ])
        .assert()
        .success();
}

// ============================================================================
// Error handling tests
// ============================================================================

#[test]
fn test_invalid_url_fails_with_error() {
    Command::cargo_bin("lppc")
        .unwrap()
        .args(["--mappings-url", "not-a-valid-url"])
        .assert()
        .failure()
        .stderr(predicate::str::contains("Invalid repository URL"));
}

#[test]
fn test_nonexistent_repo_fails() {
    Command::cargo_bin("lppc")
        .unwrap()
        .args([
            "--mappings-url",
            "https://github.com/nonexistent-user-12345/nonexistent-repo-67890",
        ])
        .assert()
        .failure();
}

// ============================================================================
// Milestone 3: Working directory and Terraform execution tests
// ============================================================================

#[test]
fn test_nonexistent_working_dir_fails() {
    Command::cargo_bin("lppc")
        .unwrap()
        .args([
            "--working-dir",
            "/nonexistent/path/that/does/not/exist",
            "--mappings-url",
            TEST_REPO_URL,
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("does not exist"));
}

#[test]
fn test_file_as_working_dir_fails() {
    let temp_file = tempfile::NamedTempFile::new().expect("Failed to create temp file");
    Command::cargo_bin("lppc")
        .unwrap()
        .args([
            "--working-dir",
            temp_file.path().to_str().unwrap(),
            "--mappings-url",
            TEST_REPO_URL,
        ])
        .assert()
        .failure()
        .stderr(predicate::str::contains("is not a directory"));
}

#[test]
fn test_empty_working_dir_succeeds() {
    let temp_dir = tempfile::TempDir::new().expect("Failed to create temp dir");
    Command::cargo_bin("lppc")
        .unwrap()
        .args([
            "--working-dir",
            temp_dir.path().to_str().unwrap(),
            "--mappings-url",
            TEST_REPO_URL,
        ])
        .assert()
        .success();
}

#[test]
fn test_valid_terraform_succeeds() {
    let temp_dir = tempfile::TempDir::new().expect("Failed to create temp dir");
    std::fs::write(
        temp_dir.path().join("main.tf"),
        r#"
        terraform {
          required_version = ">= 1.0"
        }
        "#,
    )
    .expect("Failed to write main.tf");

    Command::cargo_bin("lppc")
        .unwrap()
        .args([
            "--working-dir",
            temp_dir.path().to_str().unwrap(),
            "--mappings-url",
            TEST_REPO_URL,
        ])
        .assert()
        .success();
}

#[test]
fn test_invalid_terraform_syntax_fails() {
    let temp_dir = tempfile::TempDir::new().expect("Failed to create temp dir");
    std::fs::write(
        temp_dir.path().join("main.tf"),
        "invalid { terraform syntax }",
    )
    .expect("Failed to write main.tf");

    Command::cargo_bin("lppc")
        .unwrap()
        .args([
            "--working-dir",
            temp_dir.path().to_str().unwrap(),
            "--mappings-url",
            TEST_REPO_URL,
        ])
        .assert()
        .failure();
}

#[test]
fn test_working_dir_not_polluted_with_plan_files() {
    let temp_dir = tempfile::TempDir::new().expect("Failed to create temp dir");
    std::fs::write(
        temp_dir.path().join("main.tf"),
        r#"
        terraform {
          required_version = ">= 1.0"
        }
        "#,
    )
    .expect("Failed to write main.tf");

    Command::cargo_bin("lppc")
        .unwrap()
        .args([
            "--working-dir",
            temp_dir.path().to_str().unwrap(),
            "--mappings-url",
            TEST_REPO_URL,
        ])
        .assert()
        .success();

    // Verify no plan files in working directory
    let entries: Vec<_> = std::fs::read_dir(temp_dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| {
            let name = e.file_name();
            let name_str = name.to_string_lossy();
            name_str.contains("tfplan") || name_str.ends_with(".json")
        })
        .collect();

    assert!(
        entries.is_empty(),
        "Working directory should not contain plan files"
    );
}

#[test]
fn test_relative_working_dir_resolves_correctly() {
    // Use "." as relative working dir (current directory)
    Command::cargo_bin("lppc")
        .unwrap()
        .args(["--working-dir", ".", "--mappings-url", TEST_REPO_URL])
        .assert()
        .success();
}
