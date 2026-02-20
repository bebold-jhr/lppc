use std::fs;
use std::path::{Path, PathBuf};

use log::debug;
use tempfile::TempDir;
use walkdir::WalkDir;

use super::hcl_parser::HclParser;
use super::model::TerraformConfig;
use super::module_detector::{
    detect_module_sources, find_common_ancestor, resolve_external_modules,
};
use super::runner::{TerraformError, TerraformRunner};

/// Result of executing terraform commands (legacy, for backwards compatibility).
#[deprecated(
    since = "0.2.0",
    note = "Use PlanExecutor::execute_hcl() which returns TerraformConfig directly"
)]
pub struct PlanResult {
    /// The JSON representation of the plan.
    pub json: String,

    /// Temp directory holding the copied terraform files and plan output.
    /// Using underscore prefix to indicate it's intentionally unused but needed for RAII.
    _temp_dir: TempDir,
}

/// A directory to be copied to the temp location.
#[derive(Debug, Clone)]
struct DirectoryCopy {
    /// Source path (absolute).
    source: PathBuf,
    /// Destination path relative to temp root.
    dest_relative: PathBuf,
}

/// Plan for copying files to temp directory.
#[derive(Debug)]
struct CopyPlan {
    /// Common ancestor of working dir and all external modules.
    #[allow(dead_code)]
    common_ancestor: PathBuf,
    /// Working directory path relative to common ancestor.
    working_dir_relative: PathBuf,
    /// Directories to copy (working dir + external modules).
    directories_to_copy: Vec<DirectoryCopy>,
}

impl CopyPlan {
    /// Returns the directory where terraform should be executed.
    fn terraform_execution_dir(&self, temp_root: &Path) -> PathBuf {
        temp_root.join(&self.working_dir_relative)
    }
}

/// Executes the terraform workflow and parses HCL files directly.
///
/// All operations are performed in an isolated temporary directory to avoid
/// modifying the user's working directory.
///
/// This executor uses direct HCL parsing instead of `terraform plan`, which means:
/// - No AWS credentials required
/// - No backend configuration required
/// - Works offline (except for initial module downloads)
/// - Faster execution
pub struct PlanExecutor {
    runner: TerraformRunner,
}

impl PlanExecutor {
    /// Creates a new executor, verifying terraform is installed.
    pub fn new() -> Result<Self, TerraformError> {
        Ok(Self {
            runner: TerraformRunner::new()?,
        })
    }

    /// Executes terraform init and parses HCL files directly.
    ///
    /// All operations are performed in an isolated temporary directory.
    /// The user's working directory is never modified.
    ///
    /// This method supports external local modules by:
    /// 1. Detecting module sources from terraform configuration
    /// 2. Identifying external modules (outside the working directory)
    /// 3. Copying both the working directory and external modules with preserved structure
    /// 4. Running `terraform init -backend=false` to download registry/git modules
    /// 5. Parsing all HCL files directly (including downloaded modules)
    ///
    /// Returns `None` if no Terraform files are found in the working directory.
    pub fn execute(&self, working_dir: &Path) -> Result<Option<TerraformConfig>, TerraformError> {
        // Check for .tf files first
        if !TerraformRunner::has_terraform_files(working_dir)? {
            debug!("No Terraform files found in working directory");
            return Ok(None);
        }

        // Detect module sources from terraform configuration
        let module_sources = detect_module_sources(working_dir)?;
        debug!("Detected {} module sources", module_sources.len());

        // Resolve external local modules
        let external_modules = resolve_external_modules(working_dir, &module_sources)?;
        debug!("Found {} external local modules", external_modules.len());

        // Plan the copy structure
        let copy_plan = self.plan_copy_structure(working_dir, &external_modules)?;

        // Create isolated temp directory
        let temp_dir = TempDir::with_prefix("lppc-").map_err(TerraformError::Io)?;

        debug!(
            "Created isolated execution directory: {:?}",
            temp_dir.path()
        );

        // Execute the copy plan
        self.execute_copy_plan(&copy_plan, temp_dir.path())?;

        // Get the execution directory for terraform commands
        let execution_dir = copy_plan.terraform_execution_dir(temp_dir.path());

        // Remove state files from the working directory copy
        Self::clean_terraform_state(&execution_dir)?;

        // Log the temp directory structure in verbose mode
        log_directory_tree(temp_dir.path(), "Prepared temp directory structure");

        // Run terraform init in the execution directory (downloads modules, no backend)
        self.runner.init(&execution_dir)?;

        // Parse HCL files directly (no terraform plan!)
        let config = HclParser::parse_directory(&execution_dir)
            .map_err(|e| TerraformError::ParseFailed(e.to_string()))?;

        debug!(
            "Parsed {} provider groups from HCL files",
            config.provider_groups.len()
        );

        Ok(Some(config))
    }

    /// Legacy method that returns JSON plan output.
    ///
    /// This method is deprecated. Use `execute()` which returns `TerraformConfig` directly.
    #[deprecated(
        since = "0.2.0",
        note = "Use execute() which returns TerraformConfig directly without requiring terraform plan"
    )]
    #[allow(deprecated)]
    pub fn execute_json(&self, working_dir: &Path) -> Result<Option<PlanResult>, TerraformError> {
        // Check for .tf files first
        if !TerraformRunner::has_terraform_files(working_dir)? {
            debug!("No Terraform files found in working directory");
            return Ok(None);
        }

        // Detect module sources from terraform configuration
        let module_sources = detect_module_sources(working_dir)?;
        debug!("Detected {} module sources", module_sources.len());

        // Resolve external local modules
        let external_modules = resolve_external_modules(working_dir, &module_sources)?;
        debug!("Found {} external local modules", external_modules.len());

        // Plan the copy structure
        let copy_plan = self.plan_copy_structure(working_dir, &external_modules)?;

        // Create isolated temp directory
        let temp_dir = TempDir::with_prefix("lppc-").map_err(TerraformError::Io)?;

        debug!(
            "Created isolated execution directory: {:?}",
            temp_dir.path()
        );

        // Execute the copy plan
        self.execute_copy_plan(&copy_plan, temp_dir.path())?;

        // Get the execution directory for terraform commands
        let execution_dir = copy_plan.terraform_execution_dir(temp_dir.path());

        // Remove state files from the working directory copy
        Self::clean_terraform_state(&execution_dir)?;

        // Log the temp directory structure in verbose mode
        log_directory_tree(temp_dir.path(), "Prepared temp directory structure");

        let plan_file = temp_dir.path().join("tfplan");

        // Run terraform init in the execution directory
        self.runner.init(&execution_dir)?;

        // Run terraform plan
        self.runner.plan(&execution_dir, &plan_file)?;

        // Run terraform show -json
        let json = self.runner.show_json(&execution_dir, &plan_file)?;

        Ok(Some(PlanResult {
            json,
            _temp_dir: temp_dir,
        }))
    }

    /// Plans the directory structure for copying to temp.
    ///
    /// If there are no external modules, the working directory is copied directly
    /// to the temp root. If there are external modules, we find the common ancestor
    /// and preserve the relative structure.
    fn plan_copy_structure(
        &self,
        working_dir: &Path,
        external_modules: &[PathBuf],
    ) -> Result<CopyPlan, TerraformError> {
        let working_dir_abs = working_dir.canonicalize().map_err(TerraformError::Io)?;

        if external_modules.is_empty() {
            // Simple case: no external modules, copy working dir to temp root
            debug!("No external modules, using simple copy");
            return Ok(CopyPlan {
                common_ancestor: working_dir_abs.clone(),
                working_dir_relative: PathBuf::new(),
                directories_to_copy: vec![DirectoryCopy {
                    source: working_dir_abs,
                    dest_relative: PathBuf::new(),
                }],
            });
        }

        // Find common ancestor of working dir and all external modules
        let mut all_paths = vec![working_dir_abs.clone()];
        all_paths.extend(external_modules.iter().cloned());

        let common_ancestor = find_common_ancestor(&all_paths);
        debug!("Common ancestor: {:?}", common_ancestor);

        // Calculate relative paths from common ancestor
        let working_dir_relative = working_dir_abs
            .strip_prefix(&common_ancestor)
            .map_err(|e| {
                TerraformError::CopyFailed(format!("Failed to compute relative path: {}", e))
            })?
            .to_path_buf();

        // Build list of directories to copy
        let mut directories_to_copy = Vec::new();

        // Add working directory
        directories_to_copy.push(DirectoryCopy {
            source: working_dir_abs,
            dest_relative: working_dir_relative.clone(),
        });

        // Add external modules
        for module_path in external_modules {
            let relative = module_path.strip_prefix(&common_ancestor).map_err(|e| {
                TerraformError::CopyFailed(format!("Failed to compute relative path: {}", e))
            })?;

            directories_to_copy.push(DirectoryCopy {
                source: module_path.clone(),
                dest_relative: relative.to_path_buf(),
            });
        }

        debug!(
            "Copy plan: {} directories, working dir at {:?}",
            directories_to_copy.len(),
            working_dir_relative
        );

        Ok(CopyPlan {
            common_ancestor,
            working_dir_relative,
            directories_to_copy,
        })
    }

    /// Executes the copy plan, copying all directories to the temp location.
    fn execute_copy_plan(&self, plan: &CopyPlan, temp_root: &Path) -> Result<(), TerraformError> {
        for dir_copy in &plan.directories_to_copy {
            let dest = temp_root.join(&dir_copy.dest_relative);
            debug!("Copying {:?} to {:?}", dir_copy.source, dest);
            Self::copy_terraform_files(&dir_copy.source, &dest)?;
        }
        Ok(())
    }

    /// Copies all files from the source directory to the destination.
    ///
    /// Preserves directory structure for local module references.
    /// Skips the `.terraform/` directory entirely during copy.
    fn copy_terraform_files(src: &Path, dest: &Path) -> Result<(), TerraformError> {
        debug!("Copying terraform files from {:?} to {:?}", src, dest);

        for entry in WalkDir::new(src).into_iter().filter_map(|e| e.ok()) {
            let source_path = entry.path();
            let relative_path = source_path.strip_prefix(src).map_err(|e| {
                TerraformError::CopyFailed(format!("Failed to compute relative path: {}", e))
            })?;

            // Skip the .terraform directory entirely during copy
            if relative_path.starts_with(".terraform") {
                continue;
            }

            let dest_path = dest.join(relative_path);

            if entry.file_type().is_dir() {
                fs::create_dir_all(&dest_path)?;
            } else if entry.file_type().is_file() {
                // Ensure parent directory exists
                if let Some(parent) = dest_path.parent() {
                    fs::create_dir_all(parent)?;
                }
                fs::copy(source_path, &dest_path)?;
            }
            // Skip symlinks - they could point outside the working directory
        }

        Ok(())
    }

    /// Removes state files from the destination directory.
    ///
    /// This ensures we start with a clean slate for terraform init.
    /// The `.terraform.lock.hcl` file is preserved for consistent provider versions.
    fn clean_terraform_state(dir: &Path) -> Result<(), TerraformError> {
        // Remove .terraform/ directory if it exists (shouldn't after copy, but defensive)
        let terraform_dir = dir.join(".terraform");
        if terraform_dir.exists() {
            debug!("Removing .terraform/ directory from temp location");
            fs::remove_dir_all(&terraform_dir)?;
        }

        // Remove state files
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let file_name = entry.file_name();
            let name = file_name.to_string_lossy();

            // Remove terraform.tfstate and terraform.tfstate.backup
            if name == "terraform.tfstate" || name.starts_with("terraform.tfstate.") {
                debug!("Removing state file: {:?}", entry.path());
                fs::remove_file(entry.path())?;
            }
        }

        // Note: .terraform.lock.hcl is intentionally preserved

        Ok(())
    }
}

/// Logs the directory structure in a tree-like format.
///
/// Only produces output when debug logging is enabled (verbose mode).
fn log_directory_tree(root: &Path, label: &str) {
    debug!("{}:", label);
    log_tree_recursive(root, "");
}

/// Recursively logs the directory tree structure.
fn log_tree_recursive(current: &Path, prefix: &str) {
    let entries: Vec<_> = match fs::read_dir(current) {
        Ok(entries) => entries.filter_map(|e| e.ok()).collect(),
        Err(_) => return,
    };

    let mut entries: Vec<_> = entries;
    entries.sort_by_key(|e| e.file_name());

    let count = entries.len();
    for (i, entry) in entries.iter().enumerate() {
        let is_last = i == count - 1;
        let connector = if is_last { "└── " } else { "├── " };
        let name = entry.file_name();

        debug!("{}{}{}", prefix, connector, name.to_string_lossy());

        if entry.path().is_dir() {
            let new_prefix = if is_last {
                format!("{}    ", prefix)
            } else {
                format!("{}│   ", prefix)
            };
            log_tree_recursive(&entry.path(), &new_prefix);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn execute_returns_none_for_empty_directory() {
        // Skip if terraform is not installed
        if which::which("terraform").is_err() {
            eprintln!("Skipping test: terraform not installed");
            return;
        }

        let temp_dir = TempDir::new().unwrap();
        let executor = PlanExecutor::new().unwrap();

        let result = executor.execute(temp_dir.path()).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn execute_returns_none_for_non_tf_files() {
        // Skip if terraform is not installed
        if which::which("terraform").is_err() {
            eprintln!("Skipping test: terraform not installed");
            return;
        }

        let temp_dir = TempDir::new().unwrap();
        fs::write(temp_dir.path().join("readme.md"), "# Test").unwrap();
        fs::write(temp_dir.path().join("config.json"), "{}").unwrap();

        let executor = PlanExecutor::new().unwrap();

        let result = executor.execute(temp_dir.path()).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn execute_succeeds_with_valid_terraform() {
        // Skip if terraform is not installed
        if which::which("terraform").is_err() {
            eprintln!("Skipping test: terraform not installed");
            return;
        }

        let temp_dir = TempDir::new().unwrap();
        fs::write(
            temp_dir.path().join("main.tf"),
            r#"
            terraform {
              required_version = ">= 1.0"
            }
            "#,
        )
        .unwrap();

        let executor = PlanExecutor::new().unwrap();

        let result = executor.execute(temp_dir.path()).unwrap();
        assert!(result.is_some());

        let config = result.unwrap();
        // With no resources, provider_groups should be empty
        // This is a minimal terraform config with just version constraint
        assert!(config.unmapped_blocks.is_empty());
    }

    #[test]
    fn execute_succeeds_with_aws_resource() {
        // Skip if terraform is not installed
        if which::which("terraform").is_err() {
            eprintln!("Skipping test: terraform not installed");
            return;
        }

        let temp_dir = TempDir::new().unwrap();
        fs::write(
            temp_dir.path().join("main.tf"),
            r#"
            terraform {
              required_version = ">= 1.0"
            }

            provider "aws" {
              region = "us-east-1"
            }

            resource "aws_s3_bucket" "example" {
              bucket = "my-test-bucket"
            }
            "#,
        )
        .unwrap();

        let executor = PlanExecutor::new().unwrap();

        let result = executor.execute(temp_dir.path()).unwrap();
        assert!(result.is_some());

        let config = result.unwrap();
        // Should have one provider group (DefaultDeployer since no assume_role)
        assert_eq!(config.provider_groups.len(), 1);
        assert!(config.provider_groups.contains_key("DefaultDeployer"));

        let group = config.provider_groups.get("DefaultDeployer").unwrap();
        assert_eq!(group.blocks.len(), 1);
        assert_eq!(group.blocks[0].type_name, "aws_s3_bucket");
    }

    #[test]
    fn execute_fails_with_invalid_terraform_syntax() {
        // Skip if terraform is not installed
        if which::which("terraform").is_err() {
            eprintln!("Skipping test: terraform not installed");
            return;
        }

        let temp_dir = TempDir::new().unwrap();
        fs::write(
            temp_dir.path().join("main.tf"),
            "invalid { terraform syntax }",
        )
        .unwrap();

        let executor = PlanExecutor::new().unwrap();

        let result = executor.execute(temp_dir.path());
        assert!(result.is_err());
    }

    #[test]
    fn temp_directory_does_not_pollute_working_dir() {
        // Skip if terraform is not installed
        if which::which("terraform").is_err() {
            eprintln!("Skipping test: terraform not installed");
            return;
        }

        let temp_dir = TempDir::new().unwrap();
        fs::write(
            temp_dir.path().join("main.tf"),
            r#"
            terraform {
              required_version = ">= 1.0"
            }
            "#,
        )
        .unwrap();

        let executor = PlanExecutor::new().unwrap();
        let _ = executor.execute(temp_dir.path());

        // Check that no tfplan or json files were created in the working directory
        let entries: Vec<_> = fs::read_dir(temp_dir.path())
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
            "Working directory should not contain plan files, found: {:?}",
            entries
        );
    }

    #[test]
    fn working_directory_not_modified_no_terraform_dir_created() {
        // Skip if terraform is not installed
        if which::which("terraform").is_err() {
            eprintln!("Skipping test: terraform not installed");
            return;
        }

        let temp_dir = TempDir::new().unwrap();
        fs::write(
            temp_dir.path().join("main.tf"),
            r#"
            terraform {
              required_version = ">= 1.0"
            }
            "#,
        )
        .unwrap();

        // Record initial files
        let initial_entries: Vec<_> = fs::read_dir(temp_dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name())
            .collect();

        let executor = PlanExecutor::new().unwrap();
        let _ = executor.execute(temp_dir.path());

        // Verify no new files/directories were created
        let final_entries: Vec<_> = fs::read_dir(temp_dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name())
            .collect();

        assert_eq!(
            initial_entries, final_entries,
            "Working directory should not be modified"
        );
        assert!(
            !temp_dir.path().join(".terraform").exists(),
            ".terraform should not exist in working directory"
        );
    }

    #[test]
    fn copy_preserves_directory_structure() {
        let src = TempDir::new().unwrap();
        let dest = TempDir::new().unwrap();

        // Create nested structure
        fs::create_dir_all(src.path().join("modules/vpc")).unwrap();
        fs::write(src.path().join("main.tf"), "# main").unwrap();
        fs::write(src.path().join("modules/vpc/main.tf"), "# vpc").unwrap();

        PlanExecutor::copy_terraform_files(src.path(), dest.path()).unwrap();

        assert!(dest.path().join("main.tf").exists());
        assert!(dest.path().join("modules/vpc/main.tf").exists());
    }

    #[test]
    fn copy_skips_terraform_directory() {
        let src = TempDir::new().unwrap();
        let dest = TempDir::new().unwrap();

        // Create .terraform directory
        fs::create_dir_all(src.path().join(".terraform/providers")).unwrap();
        fs::write(src.path().join(".terraform/terraform.tfstate"), "state").unwrap();
        fs::write(src.path().join("main.tf"), "# main").unwrap();

        PlanExecutor::copy_terraform_files(src.path(), dest.path()).unwrap();

        assert!(dest.path().join("main.tf").exists());
        assert!(!dest.path().join(".terraform").exists());
    }

    #[test]
    fn copy_preserves_lock_file() {
        let src = TempDir::new().unwrap();
        let dest = TempDir::new().unwrap();

        fs::write(src.path().join(".terraform.lock.hcl"), "lock content").unwrap();
        fs::write(src.path().join("main.tf"), "# main").unwrap();

        PlanExecutor::copy_terraform_files(src.path(), dest.path()).unwrap();

        assert!(dest.path().join(".terraform.lock.hcl").exists());
        let content = fs::read_to_string(dest.path().join(".terraform.lock.hcl")).unwrap();
        assert_eq!(content, "lock content");
    }

    #[test]
    fn copy_preserves_json_and_yaml_files() {
        let src = TempDir::new().unwrap();
        let dest = TempDir::new().unwrap();

        fs::write(src.path().join("main.tf"), "# main").unwrap();
        fs::write(
            src.path().join("policy.json"),
            r#"{"Version": "2012-10-17"}"#,
        )
        .unwrap();
        fs::write(src.path().join("config.yaml"), "key: value").unwrap();
        fs::write(src.path().join("data.yaml"), "data: test").unwrap();

        PlanExecutor::copy_terraform_files(src.path(), dest.path()).unwrap();

        assert!(dest.path().join("main.tf").exists());
        assert!(dest.path().join("policy.json").exists());
        assert!(dest.path().join("config.yaml").exists());
        assert!(dest.path().join("data.yaml").exists());
    }

    #[test]
    fn clean_removes_state_files() {
        let dir = TempDir::new().unwrap();

        fs::write(dir.path().join("terraform.tfstate"), "state").unwrap();
        fs::write(dir.path().join("terraform.tfstate.backup"), "backup").unwrap();
        fs::write(
            dir.path().join("terraform.tfstate.12345.backup"),
            "old backup",
        )
        .unwrap();
        fs::write(dir.path().join("main.tf"), "# main").unwrap();
        fs::write(dir.path().join(".terraform.lock.hcl"), "lock").unwrap();

        PlanExecutor::clean_terraform_state(dir.path()).unwrap();

        assert!(!dir.path().join("terraform.tfstate").exists());
        assert!(!dir.path().join("terraform.tfstate.backup").exists());
        assert!(!dir.path().join("terraform.tfstate.12345.backup").exists());
        assert!(dir.path().join("main.tf").exists());
        assert!(dir.path().join(".terraform.lock.hcl").exists());
    }

    #[test]
    fn clean_removes_terraform_directory_if_present() {
        let dir = TempDir::new().unwrap();

        fs::create_dir_all(dir.path().join(".terraform/providers")).unwrap();
        fs::write(dir.path().join(".terraform/state"), "state").unwrap();
        fs::write(dir.path().join("main.tf"), "# main").unwrap();

        PlanExecutor::clean_terraform_state(dir.path()).unwrap();

        assert!(!dir.path().join(".terraform").exists());
        assert!(dir.path().join("main.tf").exists());
    }

    #[test]
    fn works_with_existing_terraform_directory_in_source() {
        // Skip if terraform is not installed
        if which::which("terraform").is_err() {
            eprintln!("Skipping test: terraform not installed");
            return;
        }

        let temp_dir = TempDir::new().unwrap();

        // Simulate a directory that was previously initialized with a backend
        fs::create_dir_all(temp_dir.path().join(".terraform")).unwrap();
        fs::write(
            temp_dir.path().join(".terraform/terraform.tfstate"),
            r#"{"backend": {"type": "http"}}"#,
        )
        .unwrap();

        fs::write(
            temp_dir.path().join("main.tf"),
            r#"
            terraform {
              required_version = ">= 1.0"
            }
            "#,
        )
        .unwrap();

        let executor = PlanExecutor::new().unwrap();
        let result = executor.execute(temp_dir.path());

        // Should succeed without backend configuration errors
        assert!(result.is_ok());
        assert!(result.unwrap().is_some());

        // Original .terraform should be untouched
        assert!(
            temp_dir
                .path()
                .join(".terraform/terraform.tfstate")
                .exists()
        );
    }

    #[test]
    fn local_modules_inside_working_dir_work() {
        // Skip if terraform is not installed
        if which::which("terraform").is_err() {
            eprintln!("Skipping test: terraform not installed");
            return;
        }

        let temp_dir = TempDir::new().unwrap();

        // Create a local module
        fs::create_dir_all(temp_dir.path().join("modules/test")).unwrap();
        fs::write(
            temp_dir.path().join("modules/test/main.tf"),
            r#"
            variable "name" {
              type = string
            }
            output "result" {
              value = var.name
            }
            "#,
        )
        .unwrap();

        // Create main config that uses the local module
        fs::write(
            temp_dir.path().join("main.tf"),
            r#"
            terraform {
              required_version = ">= 1.0"
            }

            module "test" {
              source = "./modules/test"
              name   = "hello"
            }
            "#,
        )
        .unwrap();

        let executor = PlanExecutor::new().unwrap();
        let result = executor.execute(temp_dir.path());

        assert!(result.is_ok());
        assert!(result.unwrap().is_some());
    }

    #[test]
    fn plan_copy_structure_simple_no_external_modules() {
        let temp_dir = TempDir::new().unwrap();
        fs::write(temp_dir.path().join("main.tf"), "# test").unwrap();

        let executor = PlanExecutor::new().unwrap();
        let plan = executor.plan_copy_structure(temp_dir.path(), &[]).unwrap();

        // With no external modules, working dir should be at root of temp
        assert_eq!(plan.working_dir_relative, PathBuf::new());
        assert_eq!(plan.directories_to_copy.len(), 1);
    }

    #[test]
    fn plan_copy_structure_with_external_modules() {
        let project_dir = TempDir::new().unwrap();

        // Create structure: project/modules/budgets and project/envs/dev
        fs::create_dir_all(project_dir.path().join("modules/budgets")).unwrap();
        fs::create_dir_all(project_dir.path().join("envs/dev")).unwrap();
        fs::write(project_dir.path().join("modules/budgets/main.tf"), "").unwrap();
        fs::write(project_dir.path().join("envs/dev/main.tf"), "").unwrap();

        let working_dir = project_dir.path().join("envs/dev");
        let external_modules = vec![
            project_dir
                .path()
                .join("modules/budgets")
                .canonicalize()
                .unwrap(),
        ];

        let executor = PlanExecutor::new().unwrap();
        let plan = executor
            .plan_copy_structure(&working_dir, &external_modules)
            .unwrap();

        // Working dir should be at envs/dev relative to common ancestor
        assert_eq!(plan.working_dir_relative, PathBuf::from("envs/dev"));
        // Should copy both working dir and external module
        assert_eq!(plan.directories_to_copy.len(), 2);
    }

    #[test]
    fn external_module_copied_with_structure() {
        // Skip if terraform is not installed
        if which::which("terraform").is_err() {
            eprintln!("Skipping test: terraform not installed");
            return;
        }

        let project_dir = TempDir::new().unwrap();

        // Create structure: project/modules/budgets and project/envs/dev
        fs::create_dir_all(project_dir.path().join("modules/budgets")).unwrap();
        fs::create_dir_all(project_dir.path().join("envs/dev")).unwrap();

        fs::write(
            project_dir.path().join("modules/budgets/main.tf"),
            r#"
            variable "budget_limit" { type = number }
            "#,
        )
        .unwrap();

        fs::write(
            project_dir.path().join("envs/dev/main.tf"),
            r#"
            terraform { required_version = ">= 1.0" }

            module "budgets" {
              source       = "../../modules/budgets"
              budget_limit = 100
            }
            "#,
        )
        .unwrap();

        let working_dir = project_dir.path().join("envs/dev");
        let executor = PlanExecutor::new().unwrap();
        let result = executor.execute(&working_dir);

        assert!(result.is_ok());
        assert!(result.unwrap().is_some());
    }

    #[test]
    fn works_with_modules_json() {
        // Skip if terraform is not installed
        if which::which("terraform").is_err() {
            eprintln!("Skipping test: terraform not installed");
            return;
        }

        let project_dir = TempDir::new().unwrap();

        // Create structure
        fs::create_dir_all(project_dir.path().join("modules/budgets")).unwrap();
        fs::create_dir_all(project_dir.path().join("envs/dev/.terraform/modules")).unwrap();

        // Create modules.json
        fs::write(
            project_dir
                .path()
                .join("envs/dev/.terraform/modules/modules.json"),
            r#"{"Modules":[{"Key":"budgets","Source":"../../modules/budgets","Dir":"../../modules/budgets"}]}"#,
        )
        .unwrap();

        fs::write(
            project_dir.path().join("modules/budgets/main.tf"),
            r#"variable "limit" { type = number }"#,
        )
        .unwrap();

        fs::write(
            project_dir.path().join("envs/dev/main.tf"),
            r#"
            terraform { required_version = ">= 1.0" }
            module "budgets" {
              source = "../../modules/budgets"
              limit  = 100
            }
            "#,
        )
        .unwrap();

        let working_dir = project_dir.path().join("envs/dev");
        let executor = PlanExecutor::new().unwrap();
        let result = executor.execute(&working_dir);

        assert!(result.is_ok());
    }

    #[test]
    fn falls_back_to_regex_without_modules_json() {
        // Skip if terraform is not installed
        if which::which("terraform").is_err() {
            eprintln!("Skipping test: terraform not installed");
            return;
        }

        let project_dir = TempDir::new().unwrap();

        // No .terraform directory - simulate CI/CD environment
        fs::create_dir_all(project_dir.path().join("modules/budgets")).unwrap();
        fs::create_dir_all(project_dir.path().join("envs/dev")).unwrap();

        fs::write(
            project_dir.path().join("modules/budgets/main.tf"),
            r#"variable "limit" { type = number }"#,
        )
        .unwrap();
        fs::write(
            project_dir.path().join("envs/dev/main.tf"),
            r#"
            terraform { required_version = ">= 1.0" }
            module "budgets" {
              source = "../../modules/budgets"
              limit  = 100
            }
            "#,
        )
        .unwrap();

        let working_dir = project_dir.path().join("envs/dev");
        let executor = PlanExecutor::new().unwrap();
        let result = executor.execute(&working_dir);

        assert!(result.is_ok());
    }

    #[test]
    fn multiple_external_modules_work() {
        // Skip if terraform is not installed
        if which::which("terraform").is_err() {
            eprintln!("Skipping test: terraform not installed");
            return;
        }

        let project_dir = TempDir::new().unwrap();

        // Create multiple external modules
        fs::create_dir_all(project_dir.path().join("shared/module_a")).unwrap();
        fs::create_dir_all(project_dir.path().join("shared/module_b")).unwrap();
        fs::create_dir_all(project_dir.path().join("envs/dev")).unwrap();

        fs::write(
            project_dir.path().join("shared/module_a/main.tf"),
            r#"variable "name_a" { type = string }"#,
        )
        .unwrap();
        fs::write(
            project_dir.path().join("shared/module_b/main.tf"),
            r#"variable "name_b" { type = string }"#,
        )
        .unwrap();
        fs::write(
            project_dir.path().join("envs/dev/main.tf"),
            r#"
            terraform { required_version = ">= 1.0" }
            module "a" {
              source = "../../shared/module_a"
              name_a = "hello"
            }
            module "b" {
              source = "../../shared/module_b"
              name_b = "world"
            }
            "#,
        )
        .unwrap();

        let working_dir = project_dir.path().join("envs/dev");
        let executor = PlanExecutor::new().unwrap();
        let result = executor.execute(&working_dir);

        assert!(result.is_ok());
        assert!(result.unwrap().is_some());
    }

    #[test]
    fn module_with_provider_mapping_assigns_resources_correctly() {
        // Skip if terraform is not installed
        if which::which("terraform").is_err() {
            eprintln!("Skipping test: terraform not installed");
            return;
        }

        let temp_dir = TempDir::new().unwrap();

        // Create module directory with a resource
        fs::create_dir_all(temp_dir.path().join("modules/billing")).unwrap();
        fs::write(
            temp_dir.path().join("modules/billing/main.tf"),
            r#"
            resource "aws_budgets_budget" "monthly" {
              name         = "monthly-budget"
              budget_type  = "COST"
              limit_amount = "100"
              limit_unit   = "USD"
              time_unit    = "MONTHLY"
            }
            "#,
        )
        .unwrap();

        // Create root module with aliased provider and module call with mapping
        fs::write(
            temp_dir.path().join("main.tf"),
            r#"
            terraform { required_version = ">= 1.0" }

            provider "aws" {
              alias = "billing"
              region = "us-east-1"
              assume_role {
                role_arn = "arn:aws:iam::123456789012:role/BillingRole"
              }
            }

            module "billing" {
              source = "./modules/billing"
              providers = {
                aws = aws.billing
              }
            }
            "#,
        )
        .unwrap();

        let executor = PlanExecutor::new().unwrap();
        let result = executor.execute(temp_dir.path()).unwrap();
        assert!(result.is_some());

        let config = result.unwrap();

        // The budget resource should be in the BillingDeployer group
        assert!(
            config.provider_groups.contains_key("BillingDeployer"),
            "Expected BillingDeployer group. Found groups: {:?}",
            config.provider_groups.keys().collect::<Vec<_>>()
        );

        let group = config.provider_groups.get("BillingDeployer").unwrap();
        assert!(
            group.blocks.iter().any(|b| b.type_name == "aws_budgets_budget"),
            "Expected aws_budgets_budget in BillingDeployer group. Found: {:?}",
            group.blocks.iter().map(|b| &b.type_name).collect::<Vec<_>>()
        );

        // Verify the resource address includes module prefix
        let budget_block = group
            .blocks
            .iter()
            .find(|b| b.type_name == "aws_budgets_budget")
            .unwrap();
        assert_eq!(
            budget_block.address, "module.billing.aws_budgets_budget.monthly",
            "Expected module prefix in address"
        );
    }

    #[test]
    fn module_without_provider_mapping_uses_default() {
        // Skip if terraform is not installed
        if which::which("terraform").is_err() {
            eprintln!("Skipping test: terraform not installed");
            return;
        }

        let temp_dir = TempDir::new().unwrap();

        // Create module directory with a resource
        fs::create_dir_all(temp_dir.path().join("modules/storage")).unwrap();
        fs::write(
            temp_dir.path().join("modules/storage/main.tf"),
            r#"
            resource "aws_s3_bucket" "data" {
              bucket = "data-bucket"
            }
            "#,
        )
        .unwrap();

        // Create root module with default provider (no alias) and module without providers block
        fs::write(
            temp_dir.path().join("main.tf"),
            r#"
            terraform { required_version = ">= 1.0" }

            provider "aws" {
              region = "us-east-1"
              assume_role {
                role_arn = "arn:aws:iam::123456789012:role/DefaultRole"
              }
            }

            module "storage" {
              source = "./modules/storage"
              # No providers block - inherits default aws provider
            }
            "#,
        )
        .unwrap();

        let executor = PlanExecutor::new().unwrap();
        let result = executor.execute(temp_dir.path()).unwrap();
        assert!(result.is_some());

        let config = result.unwrap();

        // The bucket should be in DefaultDeployer group (no alias on root provider)
        assert!(
            config.provider_groups.contains_key("DefaultDeployer"),
            "Expected DefaultDeployer group. Found groups: {:?}",
            config.provider_groups.keys().collect::<Vec<_>>()
        );

        let group = config.provider_groups.get("DefaultDeployer").unwrap();
        assert!(
            group.blocks.iter().any(|b| b.type_name == "aws_s3_bucket"),
            "Expected aws_s3_bucket in DefaultDeployer group"
        );
    }

    #[test]
    fn multiple_provider_mappings_route_to_correct_groups() {
        // Skip if terraform is not installed
        if which::which("terraform").is_err() {
            eprintln!("Skipping test: terraform not installed");
            return;
        }

        let temp_dir = TempDir::new().unwrap();

        // Create multi-region module with resources using different providers
        fs::create_dir_all(temp_dir.path().join("modules/multi")).unwrap();
        fs::write(
            temp_dir.path().join("modules/multi/main.tf"),
            r#"
            resource "aws_s3_bucket" "primary" {
              provider = aws.primary
              bucket   = "primary-bucket"
            }

            resource "aws_s3_bucket" "secondary" {
              provider = aws.secondary
              bucket   = "secondary-bucket"
            }
            "#,
        )
        .unwrap();

        // Create root module with multiple providers
        fs::write(
            temp_dir.path().join("main.tf"),
            r#"
            terraform { required_version = ">= 1.0" }

            provider "aws" {
              alias  = "us_east"
              region = "us-east-1"
              assume_role {
                role_arn = "arn:aws:iam::111111111111:role/UsEastRole"
              }
            }

            provider "aws" {
              alias  = "eu_west"
              region = "eu-west-1"
              assume_role {
                role_arn = "arn:aws:iam::222222222222:role/EuWestRole"
              }
            }

            module "multi" {
              source = "./modules/multi"
              providers = {
                aws.primary   = aws.us_east
                aws.secondary = aws.eu_west
              }
            }
            "#,
        )
        .unwrap();

        let executor = PlanExecutor::new().unwrap();
        let result = executor.execute(temp_dir.path()).unwrap();
        assert!(result.is_some());

        let config = result.unwrap();

        // Should have two groups (one per provider with different role_arn)
        assert!(
            config.provider_groups.len() >= 2,
            "Expected at least 2 provider groups. Found: {:?}",
            config.provider_groups.keys().collect::<Vec<_>>()
        );

        // Primary bucket should be in us_east group
        let us_east_group = config
            .provider_groups
            .values()
            .find(|g| g.role_arn.as_ref().is_some_and(|r| r.contains("UsEastRole")));
        assert!(us_east_group.is_some(), "Expected a group with UsEastRole");

        if let Some(group) = us_east_group {
            assert!(
                group.blocks.iter().any(|b| b.name == "primary"),
                "Expected 'primary' bucket in UsEastRole group"
            );
        }

        // Secondary bucket should be in eu_west group
        let eu_west_group = config
            .provider_groups
            .values()
            .find(|g| g.role_arn.as_ref().is_some_and(|r| r.contains("EuWestRole")));
        assert!(eu_west_group.is_some(), "Expected a group with EuWestRole");

        if let Some(group) = eu_west_group {
            assert!(
                group.blocks.iter().any(|b| b.name == "secondary"),
                "Expected 'secondary' bucket in EuWestRole group"
            );
        }
    }
}
