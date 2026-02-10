//! Cache management for the mapping repository.
//!
//! Handles the local cache directory (~/.lppc), URL parsing for various git formats,
//! and timestamp tracking for cache expiry.

use sha2::{Digest, Sha256};
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};
use thiserror::Error;

/// Cache expiry duration in hours.
const CACHE_EXPIRY_HOURS: u64 = 24;

/// Error types for cache operations.
#[derive(Debug, Error)]
pub enum CacheError {
    #[error("Cannot determine home directory")]
    NoHomeDirectory,

    #[error("Invalid repository URL: {0}")]
    InvalidUrl(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

/// Manages the local cache directory for mapping repositories.
pub struct CacheManager {
    /// Base cache directory (typically ~/.lppc)
    base_dir: PathBuf,
}

impl CacheManager {
    /// Creates a new cache manager, initializing ~/.lppc if needed.
    pub fn new() -> Result<Self, CacheError> {
        let home_dir = dirs::home_dir().ok_or(CacheError::NoHomeDirectory)?;
        let base_dir = home_dir.join(".lppc");

        // Create the cache directory if it doesn't exist
        if !base_dir.exists() {
            fs::create_dir_all(&base_dir)?;
            log::debug!("Created cache directory: {:?}", base_dir);
        }

        Ok(Self { base_dir })
    }

    /// Creates a cache manager with a custom base directory (for testing).
    #[cfg(test)]
    pub fn with_base_dir(base_dir: PathBuf) -> Result<Self, CacheError> {
        if !base_dir.exists() {
            fs::create_dir_all(&base_dir)?;
        }
        Ok(Self { base_dir })
    }

    /// Returns the local path for a given repository URL.
    ///
    /// Example: `https://github.com/bebold-jhr/lppc-aws-test` -> `~/.lppc/bebold-jhr/lppc-aws-test`
    pub fn get_repo_path(&self, url: &str) -> Result<PathBuf, CacheError> {
        let repo_path = Self::parse_repo_path(url)?;
        Ok(self.base_dir.join(repo_path))
    }

    /// Checks if the repository exists in cache.
    pub fn is_cached(&self, url: &str) -> bool {
        match self.get_repo_path(url) {
            Ok(path) => path.exists() && path.join(".git").exists(),
            Err(_) => false,
        }
    }

    /// Checks if the cache needs refresh (older than 24 hours or no timestamp file).
    pub fn needs_refresh(&self, url: &str) -> Result<bool, CacheError> {
        let timestamp_path = self.timestamp_file_path(url);

        if !timestamp_path.exists() {
            return Ok(true);
        }

        let metadata = fs::metadata(&timestamp_path)?;
        let modified = metadata.modified()?;
        let expiry_duration = Duration::from_secs(CACHE_EXPIRY_HOURS * 60 * 60);

        let is_expired = SystemTime::now()
            .duration_since(modified)
            .map(|age| age > expiry_duration)
            .unwrap_or(true);

        Ok(is_expired)
    }

    /// Updates the last refresh timestamp by touching the timestamp file.
    pub fn update_timestamp(&self, url: &str) -> Result<(), CacheError> {
        let timestamp_path = self.timestamp_file_path(url);

        // Write current timestamp to file
        let now = chrono::Utc::now().to_rfc3339();
        fs::write(&timestamp_path, now)?;

        log::debug!("Updated timestamp file: {:?}", timestamp_path);
        Ok(())
    }

    /// Gets the timestamp file path for a repository.
    ///
    /// Uses a hash of the URL to create a unique filename.
    fn timestamp_file_path(&self, url: &str) -> PathBuf {
        let hash = Self::hash_url(url);
        self.base_dir.join(format!(".last_update_{}", hash))
    }

    /// Creates a short hash of the URL for the timestamp filename.
    fn hash_url(url: &str) -> String {
        let mut hasher = Sha256::new();
        hasher.update(url.as_bytes());
        let result = hasher.finalize();
        // Use first 8 characters of the hex hash
        hex::encode(&result[..4])
    }

    /// Parses repository URL to extract username/repo-name.
    ///
    /// Supports:
    /// - `https://github.com/user/repo`
    /// - `https://github.com/user/repo.git`
    /// - `git@github.com:user/repo.git`
    pub fn parse_repo_path(url: &str) -> Result<String, CacheError> {
        let url = url.trim();

        // Handle HTTPS URLs
        if url.starts_with("https://") || url.starts_with("http://") {
            return Self::parse_https_url(url);
        }

        // Handle SSH URLs (git@github.com:user/repo.git)
        if url.starts_with("git@") {
            return Self::parse_ssh_url(url);
        }

        Err(CacheError::InvalidUrl(format!(
            "URL must start with 'https://', 'http://', or 'git@': {}",
            url
        )))
    }

    /// Validates a path component (user or repo name) for security.
    ///
    /// Rejects path traversal attempts and other dangerous patterns.
    fn validate_path_component(component: &str, url: &str) -> Result<(), CacheError> {
        if component.is_empty() {
            return Err(CacheError::InvalidUrl(format!(
                "Empty path component in URL: {}",
                url
            )));
        }

        // Reject path traversal attempts
        if component.contains("..") {
            return Err(CacheError::InvalidUrl(format!(
                "Path traversal detected in URL: {}",
                url
            )));
        }

        // Reject slashes and backslashes within components
        if component.contains('/') || component.contains('\\') {
            return Err(CacheError::InvalidUrl(format!(
                "Invalid characters in URL path component: {}",
                url
            )));
        }

        // Reject hidden directories (starting with .)
        if component.starts_with('.') {
            return Err(CacheError::InvalidUrl(format!(
                "Path component cannot start with '.': {}",
                url
            )));
        }

        // Reject components that look like command-line arguments
        if component.starts_with('-') {
            return Err(CacheError::InvalidUrl(format!(
                "Path component cannot start with '-': {}",
                url
            )));
        }

        Ok(())
    }

    /// Parses HTTPS git URLs.
    fn parse_https_url(url: &str) -> Result<String, CacheError> {
        // Remove protocol prefix
        let without_protocol = url
            .strip_prefix("https://")
            .or_else(|| url.strip_prefix("http://"))
            .unwrap_or(url);

        // Split by '/' and get the parts after the host
        let parts: Vec<&str> = without_protocol.split('/').collect();

        // We expect at least: host/user/repo
        if parts.len() < 3 {
            return Err(CacheError::InvalidUrl(format!(
                "URL must contain user and repository: {}",
                url
            )));
        }

        let user = parts[1];
        let repo = parts[2].trim_end_matches(".git");

        // Validate both components for security
        Self::validate_path_component(user, url)?;
        Self::validate_path_component(repo, url)?;

        Ok(format!("{}/{}", user, repo))
    }

    /// Parses SSH git URLs (git@host:user/repo.git).
    fn parse_ssh_url(url: &str) -> Result<String, CacheError> {
        // Format: git@github.com:user/repo.git
        let without_prefix = url
            .strip_prefix("git@")
            .ok_or_else(|| CacheError::InvalidUrl(format!("Invalid SSH URL format: {}", url)))?;

        // Find the colon that separates host from path
        let colon_pos = without_prefix.find(':').ok_or_else(|| {
            CacheError::InvalidUrl(format!("SSH URL missing ':' separator: {}", url))
        })?;

        let path = &without_prefix[colon_pos + 1..];
        let path = path.trim_end_matches(".git");

        // Split by '/' to get user/repo
        let parts: Vec<&str> = path.split('/').collect();
        if parts.len() != 2 {
            return Err(CacheError::InvalidUrl(format!(
                "SSH URL must contain user/repo path: {}",
                url
            )));
        }

        let user = parts[0];
        let repo = parts[1];

        // Validate both components for security
        Self::validate_path_component(user, url)?;
        Self::validate_path_component(repo, url)?;

        Ok(format!("{}/{}", user, repo))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_parse_https_url() {
        let result = CacheManager::parse_repo_path("https://github.com/bebold-jhr/lppc-aws-test");
        assert_eq!(result.unwrap(), "bebold-jhr/lppc-aws-test");
    }

    #[test]
    fn test_parse_https_url_with_git_suffix() {
        let result =
            CacheManager::parse_repo_path("https://github.com/bebold-jhr/lppc-aws-test.git");
        assert_eq!(result.unwrap(), "bebold-jhr/lppc-aws-test");
    }

    #[test]
    fn test_parse_ssh_url() {
        let result = CacheManager::parse_repo_path("git@github.com:bebold-jhr/lppc-aws-test.git");
        assert_eq!(result.unwrap(), "bebold-jhr/lppc-aws-test");
    }

    #[test]
    fn test_parse_ssh_url_without_git_suffix() {
        let result = CacheManager::parse_repo_path("git@github.com:bebold-jhr/lppc-aws-test");
        assert_eq!(result.unwrap(), "bebold-jhr/lppc-aws-test");
    }

    #[test]
    fn test_invalid_url_no_protocol() {
        let result = CacheManager::parse_repo_path("not-a-valid-url");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, CacheError::InvalidUrl(_)));
    }

    #[test]
    fn test_invalid_url_missing_repo() {
        let result = CacheManager::parse_repo_path("https://github.com/user");
        assert!(result.is_err());
    }

    #[test]
    fn test_invalid_ssh_url_missing_colon() {
        let result = CacheManager::parse_repo_path("git@github.com/user/repo");
        assert!(result.is_err());
    }

    #[test]
    fn test_hash_url_consistency() {
        let hash1 = CacheManager::hash_url("https://github.com/user/repo");
        let hash2 = CacheManager::hash_url("https://github.com/user/repo");
        assert_eq!(hash1, hash2);
    }

    #[test]
    fn test_hash_url_different_urls() {
        let hash1 = CacheManager::hash_url("https://github.com/user/repo1");
        let hash2 = CacheManager::hash_url("https://github.com/user/repo2");
        assert_ne!(hash1, hash2);
    }

    #[test]
    fn test_cache_manager_creates_directory() {
        let temp_dir = std::env::temp_dir().join(format!("lppc_test_{}", std::process::id()));
        let base_dir = temp_dir.join(".lppc");

        // Clean up from previous runs
        let _ = fs::remove_dir_all(&temp_dir);

        let manager = CacheManager::with_base_dir(base_dir.clone()).unwrap();
        assert!(base_dir.exists());

        // Cleanup
        let _ = fs::remove_dir_all(&temp_dir);
        drop(manager);
    }

    #[test]
    fn test_get_repo_path() {
        let temp_dir = std::env::temp_dir().join(format!("lppc_test_repo_{}", std::process::id()));
        let base_dir = temp_dir.join(".lppc");
        let _ = fs::remove_dir_all(&temp_dir);

        let manager = CacheManager::with_base_dir(base_dir.clone()).unwrap();
        let path = manager
            .get_repo_path("https://github.com/bebold-jhr/lppc-aws-test")
            .unwrap();

        assert_eq!(path, base_dir.join("bebold-jhr/lppc-aws-test"));

        // Cleanup
        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_is_cached_false_when_not_exists() {
        let temp_dir = std::env::temp_dir().join(format!("lppc_test_cache_{}", std::process::id()));
        let base_dir = temp_dir.join(".lppc");
        let _ = fs::remove_dir_all(&temp_dir);

        let manager = CacheManager::with_base_dir(base_dir).unwrap();
        assert!(!manager.is_cached("https://github.com/user/nonexistent"));

        // Cleanup
        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_is_cached_true_when_exists_with_git() {
        let temp_dir =
            std::env::temp_dir().join(format!("lppc_test_cached_{}", std::process::id()));
        let base_dir = temp_dir.join(".lppc");
        let _ = fs::remove_dir_all(&temp_dir);

        let manager = CacheManager::with_base_dir(base_dir.clone()).unwrap();

        // Create fake cached repo with .git directory
        let repo_path = base_dir.join("user/repo");
        fs::create_dir_all(repo_path.join(".git")).unwrap();

        assert!(manager.is_cached("https://github.com/user/repo"));

        // Cleanup
        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_needs_refresh_true_when_no_timestamp() {
        let temp_dir =
            std::env::temp_dir().join(format!("lppc_test_refresh_{}", std::process::id()));
        let base_dir = temp_dir.join(".lppc");
        let _ = fs::remove_dir_all(&temp_dir);

        let manager = CacheManager::with_base_dir(base_dir).unwrap();
        assert!(
            manager
                .needs_refresh("https://github.com/user/repo")
                .unwrap()
        );

        // Cleanup
        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_timestamp_update_and_fresh_check() {
        let temp_dir = std::env::temp_dir().join(format!("lppc_test_ts_{}", std::process::id()));
        let base_dir = temp_dir.join(".lppc");
        let _ = fs::remove_dir_all(&temp_dir);

        let manager = CacheManager::with_base_dir(base_dir).unwrap();
        let url = "https://github.com/user/repo";

        // Update timestamp
        manager.update_timestamp(url).unwrap();

        // Should not need refresh immediately after update
        assert!(!manager.needs_refresh(url).unwrap());

        // Cleanup
        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_timestamp_file_path_unique_per_url() {
        let temp_dir =
            std::env::temp_dir().join(format!("lppc_test_ts_unique_{}", std::process::id()));
        let base_dir = temp_dir.join(".lppc");
        let _ = fs::remove_dir_all(&temp_dir);

        let manager = CacheManager::with_base_dir(base_dir).unwrap();

        let path1 = manager.timestamp_file_path("https://github.com/user/repo1");
        let path2 = manager.timestamp_file_path("https://github.com/user/repo2");

        assert_ne!(path1, path2);

        // Cleanup
        let _ = fs::remove_dir_all(&temp_dir);
    }

    // Security tests for path traversal prevention

    #[test]
    fn test_rejects_path_traversal_in_user() {
        let result = CacheManager::parse_repo_path("https://github.com/../etc/passwd");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("traversal"));
    }

    #[test]
    fn test_rejects_path_traversal_in_repo() {
        let result = CacheManager::parse_repo_path("https://github.com/user/../../../etc");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("traversal"));
    }

    #[test]
    fn test_rejects_hidden_user_directory() {
        let result = CacheManager::parse_repo_path("https://github.com/.hidden/repo");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("start with '.'"));
    }

    #[test]
    fn test_rejects_hidden_repo_directory() {
        let result = CacheManager::parse_repo_path("https://github.com/user/.hidden");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("start with '.'"));
    }

    #[test]
    fn test_rejects_user_starting_with_dash() {
        let result = CacheManager::parse_repo_path("https://github.com/-user/repo");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("start with '-'"));
    }

    #[test]
    fn test_rejects_repo_starting_with_dash() {
        let result = CacheManager::parse_repo_path("https://github.com/user/-repo");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("start with '-'"));
    }

    #[test]
    fn test_rejects_ssh_path_traversal() {
        // Test with a user/repo format that contains path traversal
        let result = CacheManager::parse_repo_path("git@github.com:..%2F..%2Fetc/passwd.git");
        assert!(
            result.is_err(),
            "Expected error for encoded path traversal URL"
        );

        // Also test direct path traversal in user component
        let result2 = CacheManager::parse_repo_path("git@github.com:../repo.git");
        assert!(
            result2.is_err(),
            "Expected error for path traversal in user"
        );
    }
}
