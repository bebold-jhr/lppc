use std::path::{Path, PathBuf};
use std::process::Command;

use log::{debug, info};
use thiserror::Error;
use which::which;

/// Executes terraform commands in a working directory.
pub struct TerraformRunner {
    terraform_path: PathBuf,
}

impl TerraformRunner {
    /// Creates a new runner, verifying terraform is installed.
    pub fn new() -> Result<Self, TerraformError> {
        let terraform_path = which("terraform").map_err(|_| TerraformError::NotFound)?;

        debug!("Found terraform at: {:?}", terraform_path);

        Ok(Self { terraform_path })
    }

    /// Checks if the directory contains any Terraform files (.tf extension).
    pub fn has_terraform_files(dir: &Path) -> Result<bool, TerraformError> {
        let entries = std::fs::read_dir(dir)?;

        for entry in entries {
            let entry = entry?;
            let path = entry.path();

            if path.is_file() {
                if let Some(ext) = path.extension() {
                    if ext == "tf" {
                        return Ok(true);
                    }
                }
            }
        }

        Ok(false)
    }

    /// Runs `terraform init -backend=false`.
    ///
    /// Uses -backend=false to skip backend configuration which may require
    /// custom parameters, credentials, or remote state access.
    pub fn init(&self, working_dir: &Path) -> Result<(), TerraformError> {
        info!("Running terraform init -backend=false in {:?}", working_dir);

        let output = Command::new(&self.terraform_path)
            .args(["init", "-backend=false", "-input=false"])
            .current_dir(working_dir)
            .output()
            .map_err(|e| {
                TerraformError::CommandFailed(format!("Failed to execute terraform init: {}", e))
            })?;

        if output.status.success() {
            debug!("Terraform init successful");
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            let error_message = if !stderr.is_empty() { stderr } else { stdout };
            Err(TerraformError::InitFailed(error_message.to_string()))
        }
    }

    /// Runs `terraform plan -out=<plan_file>`.
    ///
    /// Creates a plan file at the specified path.
    pub fn plan(&self, working_dir: &Path, plan_file: &Path) -> Result<(), TerraformError> {
        info!("Running terraform plan in {:?}", working_dir);

        let plan_file_str = plan_file.to_str().ok_or_else(|| {
            TerraformError::CommandFailed("Invalid plan file path (non-UTF8)".to_string())
        })?;

        let output = Command::new(&self.terraform_path)
            .args(["plan", "-input=false", "-out", plan_file_str])
            .current_dir(working_dir)
            .output()
            .map_err(|e| {
                TerraformError::CommandFailed(format!("Failed to execute terraform plan: {}", e))
            })?;

        if output.status.success() {
            debug!("Terraform plan successful, written to {:?}", plan_file);
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            let error_message = if !stderr.is_empty() { stderr } else { stdout };
            Err(TerraformError::PlanFailed(error_message.to_string()))
        }
    }

    /// Runs `terraform show -json <plan_file>`.
    ///
    /// Converts the binary plan file to JSON representation.
    pub fn show_json(
        &self,
        working_dir: &Path,
        plan_file: &Path,
    ) -> Result<String, TerraformError> {
        info!("Converting plan to JSON");

        let plan_file_str = plan_file.to_str().ok_or_else(|| {
            TerraformError::CommandFailed("Invalid plan file path (non-UTF8)".to_string())
        })?;

        let output = Command::new(&self.terraform_path)
            .args(["show", "-json", plan_file_str])
            .current_dir(working_dir)
            .output()
            .map_err(|e| {
                TerraformError::CommandFailed(format!("Failed to execute terraform show: {}", e))
            })?;

        if output.status.success() {
            let json = String::from_utf8(output.stdout).map_err(|e| {
                TerraformError::CommandFailed(format!("Invalid UTF-8 in terraform output: {}", e))
            })?;
            debug!("Terraform show -json successful ({} bytes)", json.len());
            Ok(json)
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            Err(TerraformError::ShowFailed(stderr.to_string()))
        }
    }
}

#[derive(Debug, Error)]
pub enum TerraformError {
    #[error(
        "Terraform is not installed or not found in PATH. Please install terraform: https://developer.hashicorp.com/terraform/downloads"
    )]
    NotFound,

    #[error("Terraform init failed:\n{0}")]
    InitFailed(String),

    #[error("Terraform plan failed:\n{0}")]
    PlanFailed(String),

    #[error("Terraform show failed:\n{0}")]
    ShowFailed(String),

    #[error("Failed to run terraform command: {0}")]
    CommandFailed(String),

    #[error("Failed to copy terraform files: {0}")]
    CopyFailed(String),

    #[error("Failed to parse terraform configuration: {0}")]
    ParseFailed(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn has_terraform_files_empty_dir() {
        let temp_dir = TempDir::new().unwrap();
        let result = TerraformRunner::has_terraform_files(temp_dir.path()).unwrap();
        assert!(!result);
    }

    #[test]
    fn has_terraform_files_with_tf_file() {
        let temp_dir = TempDir::new().unwrap();
        fs::write(temp_dir.path().join("main.tf"), "").unwrap();
        let result = TerraformRunner::has_terraform_files(temp_dir.path()).unwrap();
        assert!(result);
    }

    #[test]
    fn has_terraform_files_with_multiple_tf_files() {
        let temp_dir = TempDir::new().unwrap();
        fs::write(temp_dir.path().join("main.tf"), "").unwrap();
        fs::write(temp_dir.path().join("variables.tf"), "").unwrap();
        fs::write(temp_dir.path().join("outputs.tf"), "").unwrap();
        let result = TerraformRunner::has_terraform_files(temp_dir.path()).unwrap();
        assert!(result);
    }

    #[test]
    fn has_terraform_files_no_tf_extension() {
        let temp_dir = TempDir::new().unwrap();
        fs::write(temp_dir.path().join("readme.md"), "").unwrap();
        fs::write(temp_dir.path().join("config.json"), "").unwrap();
        fs::write(temp_dir.path().join("terraform.tfvars"), "").unwrap(); // .tfvars is not .tf
        let result = TerraformRunner::has_terraform_files(temp_dir.path()).unwrap();
        assert!(!result);
    }

    #[test]
    fn has_terraform_files_ignores_directories() {
        let temp_dir = TempDir::new().unwrap();
        // Create a directory with .tf name (edge case)
        fs::create_dir(temp_dir.path().join("module.tf")).unwrap();
        let result = TerraformRunner::has_terraform_files(temp_dir.path()).unwrap();
        assert!(!result);
    }

    #[test]
    fn terraform_error_not_found_message() {
        let error = TerraformError::NotFound;
        let message = error.to_string();
        assert!(message.contains("not installed"));
        assert!(message.contains("https://developer.hashicorp.com/terraform/downloads"));
    }

    #[test]
    fn terraform_error_init_failed_message() {
        let error = TerraformError::InitFailed("some error details".to_string());
        let message = error.to_string();
        assert!(message.contains("init failed"));
        assert!(message.contains("some error details"));
    }

    #[test]
    fn terraform_error_plan_failed_message() {
        let error = TerraformError::PlanFailed("plan error details".to_string());
        let message = error.to_string();
        assert!(message.contains("plan failed"));
        assert!(message.contains("plan error details"));
    }

    #[test]
    fn terraform_error_show_failed_message() {
        let error = TerraformError::ShowFailed("show error details".to_string());
        let message = error.to_string();
        assert!(message.contains("show failed"));
        assert!(message.contains("show error details"));
    }
}
