//! Git operations for the mapping repository.
//!
//! Handles cloning and updating the mapping repository using shallow clones
//! to minimize bandwidth and disk usage.
//!
//! Uses the system `git` command for better compatibility with public repositories
//! and credential helpers.

use std::path::Path;
use std::process::Command;
use thiserror::Error;

/// Error types for git operations.
#[derive(Debug, Error)]
pub enum GitError {
    #[error("Git operation failed: {0}")]
    Git(String),

    #[error("Repository not found at {0}")]
    NotFound(String),

    #[error("Network unreachable, using cached version")]
    NetworkUnreachable,

    #[error("Git command not found. Please install git.")]
    GitNotInstalled,

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Handles git operations for the mapping repository.
pub struct GitOperations;

impl GitOperations {
    /// Checks if git is available on the system.
    fn check_git_available() -> Result<(), GitError> {
        match Command::new("git").arg("--version").output() {
            Ok(output) if output.status.success() => Ok(()),
            Ok(_) => Err(GitError::GitNotInstalled),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(GitError::GitNotInstalled),
            Err(e) => Err(GitError::Io(e)),
        }
    }

    /// Validates a branch name for security.
    ///
    /// Rejects branch names that could be interpreted as command-line
    /// arguments or contain special characters.
    fn is_valid_branch_name(name: &str) -> bool {
        if name.is_empty() || name.starts_with('-') {
            return false;
        }
        // Allow alphanumeric, hyphens, underscores, and forward slashes (for feature branches)
        name.chars()
            .all(|c| c.is_alphanumeric() || c == '-' || c == '_' || c == '/')
    }

    /// Validates a URL for security.
    ///
    /// Ensures the URL uses a safe protocol and doesn't contain
    /// potentially dangerous patterns.
    fn validate_url(url: &str) -> Result<(), GitError> {
        let url = url.trim();

        // Reject URLs that could be interpreted as arguments
        if url.starts_with('-') {
            return Err(GitError::Git(
                "Invalid URL: cannot start with '-'".to_string(),
            ));
        }

        // Only allow known safe protocols
        if !url.starts_with("https://") && !url.starts_with("http://") && !url.starts_with("git@") {
            return Err(GitError::Git(format!(
                "Unsupported URL scheme. Use https://, http://, or git@: {}",
                url
            )));
        }

        // Reject potentially dangerous protocols
        if url.contains("ext::") || url.contains("file://") {
            return Err(GitError::Git(
                "Potentially dangerous URL protocol detected".to_string(),
            ));
        }

        Ok(())
    }

    /// Clones a repository with shallow clone (depth=1).
    ///
    /// Creates the parent directories if they don't exist.
    pub fn shallow_clone(url: &str, target_path: &Path) -> Result<(), GitError> {
        Self::validate_url(url)?;
        Self::check_git_available()?;

        log::info!("Cloning mapping repository from {}...", url);
        log::debug!("Target path: {:?}", target_path);

        // Get target path as string, reject if invalid
        let target_str = target_path.to_str().ok_or_else(|| {
            GitError::Git("Target path contains invalid UTF-8 characters".to_string())
        })?;

        // Create parent directories if needed
        if let Some(parent) = target_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        // Remove existing directory if it exists but is not a valid git repo
        if target_path.exists() {
            // Security: Check if it's a symlink - don't follow it
            let metadata = std::fs::symlink_metadata(target_path)?;
            if metadata.is_symlink() {
                return Err(GitError::Git(format!(
                    "Target path is a symlink, refusing to proceed: {:?}",
                    target_path
                )));
            }
            log::debug!("Removing existing directory at {:?}", target_path);
            std::fs::remove_dir_all(target_path)?;
        }

        // Run git clone with depth=1 for shallow clone
        // Use "--" to separate options from URL argument for security
        let output = Command::new("git")
            .args([
                "clone",
                "--depth",
                "1",
                "--single-branch",
                "--",
                url,
                target_str,
            ])
            .output()?;

        if output.status.success() {
            log::info!("Successfully cloned mapping repository");
            Ok(())
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            log::debug!("Git clone failed: {}", stderr);
            Err(Self::classify_error(&stderr))
        }
    }

    /// Updates an existing repository via fetch + reset.
    ///
    /// Uses shallow fetch to minimize bandwidth.
    pub fn update(repo_path: &Path) -> Result<(), GitError> {
        Self::check_git_available()?;

        log::info!("Updating mapping repository...");
        log::debug!("Repository path: {:?}", repo_path);

        if !repo_path.exists() {
            return Err(GitError::NotFound(repo_path.display().to_string()));
        }

        // Fetch with depth=1
        let fetch_output = Command::new("git")
            .current_dir(repo_path)
            .args(["fetch", "--depth", "1", "origin"])
            .output()?;

        if !fetch_output.status.success() {
            let stderr = String::from_utf8_lossy(&fetch_output.stderr);
            log::error!("Failed to fetch updates: {}", stderr);
            return Err(Self::classify_error(&stderr));
        }

        // Get the default branch name
        let branch_output = Command::new("git")
            .current_dir(repo_path)
            .args(["symbolic-ref", "--short", "HEAD"])
            .output()?;

        let branch_name = if branch_output.status.success() {
            let name = String::from_utf8_lossy(&branch_output.stdout)
                .trim()
                .to_string();
            // Validate branch name for security
            if Self::is_valid_branch_name(&name) {
                name
            } else {
                log::warn!("Invalid branch name detected, falling back to 'main'");
                "main".to_string()
            }
        } else {
            "main".to_string()
        };

        // Reset to origin/<branch>
        let reset_output = Command::new("git")
            .current_dir(repo_path)
            .args(["reset", "--hard", &format!("origin/{}", branch_name)])
            .output()?;

        if !reset_output.status.success() {
            let stderr = String::from_utf8_lossy(&reset_output.stderr);
            log::error!("Failed to reset to origin: {}", stderr);
            return Err(GitError::Git(stderr.to_string()));
        }

        // Get the current commit hash for logging
        let rev_output = Command::new("git")
            .current_dir(repo_path)
            .args(["rev-parse", "--short", "HEAD"])
            .output()?;

        if rev_output.status.success() {
            let commit_hash = String::from_utf8_lossy(&rev_output.stdout)
                .trim()
                .to_string();
            log::info!("Successfully updated mapping repository to {}", commit_hash);
        } else {
            log::info!("Successfully updated mapping repository");
        }

        log::debug!("Branch: {}", branch_name);

        Ok(())
    }

    /// Checks if the remote is reachable by attempting to connect.
    pub fn is_remote_reachable(url: &str) -> bool {
        log::debug!("Checking if remote is reachable: {}", url);

        // Validate URL before using it
        if Self::validate_url(url).is_err() {
            log::debug!("Invalid URL format, treating as unreachable");
            return false;
        }

        // Use git ls-remote to check connectivity (lightweight check)
        // Use "--" to separate options from URL argument for security
        match Command::new("git")
            .args(["ls-remote", "--exit-code", "-h", "--", url])
            .output()
        {
            Ok(output) => {
                let is_reachable = output.status.success();
                if is_reachable {
                    log::debug!("Remote is reachable");
                } else {
                    log::debug!(
                        "Remote is not reachable: {}",
                        String::from_utf8_lossy(&output.stderr)
                    );
                }
                is_reachable
            }
            Err(e) => {
                log::debug!("Failed to check remote: {}", e);
                false
            }
        }
    }

    /// Classifies an error message into our error types.
    fn classify_error(error_message: &str) -> GitError {
        let lower = error_message.to_lowercase();

        if lower.contains("could not resolve")
            || lower.contains("failed to resolve")
            || lower.contains("network")
            || lower.contains("connection")
            || lower.contains("timed out")
            || lower.contains("unreachable")
            || lower.contains("no address")
            || lower.contains("dns")
            || lower.contains("unable to access")
        {
            GitError::NetworkUnreachable
        } else {
            GitError::Git(error_message.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify_error_network_resolve() {
        let error = GitOperations::classify_error("fatal: could not resolve host: github.com");
        assert!(matches!(error, GitError::NetworkUnreachable));
    }

    #[test]
    fn test_classify_error_network_unable_to_access() {
        let error =
            GitOperations::classify_error("fatal: unable to access 'https://github.com/...'");
        assert!(matches!(error, GitError::NetworkUnreachable));
    }

    #[test]
    fn test_classify_error_generic() {
        let error = GitOperations::classify_error("some other error");
        assert!(matches!(error, GitError::Git(_)));
    }

    #[test]
    fn test_update_nonexistent_repo() {
        let temp_dir =
            std::env::temp_dir().join(format!("lppc_test_git_update_{}", std::process::id()));
        let result = GitOperations::update(&temp_dir.join("nonexistent"));
        assert!(matches!(result, Err(GitError::NotFound(_))));
    }

    #[test]
    fn test_check_git_available() {
        // This test will pass if git is installed on the system
        let result = GitOperations::check_git_available();
        // Don't assert success - git might not be installed in all test environments
        // Just make sure it doesn't panic
        let _ = result;
    }

    // Security tests for URL validation

    #[test]
    fn test_validate_url_https() {
        assert!(GitOperations::validate_url("https://github.com/user/repo").is_ok());
    }

    #[test]
    fn test_validate_url_http() {
        assert!(GitOperations::validate_url("http://github.com/user/repo").is_ok());
    }

    #[test]
    fn test_validate_url_ssh() {
        assert!(GitOperations::validate_url("git@github.com:user/repo.git").is_ok());
    }

    #[test]
    fn test_validate_url_rejects_dash_prefix() {
        let result = GitOperations::validate_url("--upload-pack=evil");
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("cannot start with '-'")
        );
    }

    #[test]
    fn test_validate_url_rejects_ext_protocol() {
        let result = GitOperations::validate_url("ext::sh -c 'evil'%");
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_url_rejects_file_protocol() {
        let result = GitOperations::validate_url("file:///etc/passwd");
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_url_rejects_unknown_protocol() {
        let result = GitOperations::validate_url("ftp://example.com/repo");
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("Unsupported URL scheme")
        );
    }

    // Security tests for branch name validation

    #[test]
    fn test_valid_branch_names() {
        assert!(GitOperations::is_valid_branch_name("main"));
        assert!(GitOperations::is_valid_branch_name("feature/new-thing"));
        assert!(GitOperations::is_valid_branch_name("release_1_0"));
    }

    #[test]
    fn test_invalid_branch_name_empty() {
        assert!(!GitOperations::is_valid_branch_name(""));
    }

    #[test]
    fn test_invalid_branch_name_dash_prefix() {
        assert!(!GitOperations::is_valid_branch_name("--help"));
        assert!(!GitOperations::is_valid_branch_name("-branch"));
    }

    #[test]
    fn test_invalid_branch_name_special_chars() {
        assert!(!GitOperations::is_valid_branch_name("branch;rm -rf"));
        assert!(!GitOperations::is_valid_branch_name("branch$(evil)"));
    }
}
