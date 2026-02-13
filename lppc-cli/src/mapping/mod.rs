//! Mapping repository management and permission resolution.
//!
//! This module handles the lifecycle of the external mapping repository that contains
//! YAML files mapping Terraform resources to AWS IAM permissions. It handles cloning,
//! caching, updating, and offline fallback scenarios.
//!
//! The module also provides functionality for loading YAML mapping files and resolving
//! IAM permissions based on Terraform block configurations.

pub mod cache;
pub mod loader;
pub mod matcher;
pub mod repository;
pub mod schema;
pub mod yaml_parser;

pub use loader::MappingLoader;
pub use matcher::{GroupPermissions, MissingMapping, PermissionMatcher, PermissionResult};

use std::path::PathBuf;
use thiserror::Error;

use cache::{CacheError, CacheManager};
use repository::{GitError, GitOperations};

/// Error types for mapping repository operations.
#[derive(Debug, Error)]
pub enum MappingError {
    #[error("Cache error: {0}")]
    Cache(#[from] CacheError),

    #[error("Git error: {0}")]
    Git(#[from] GitError),

    #[error("Mapping repository not available: {0}")]
    NotAvailable(String),
}

/// Represents the local mapping repository state.
pub struct MappingRepository {
    /// Path to the cached repository (e.g., ~/.lppc/bebold-jhr/lppc-aws-test)
    pub local_path: PathBuf,
    /// Original URL of the repository
    pub url: String,
    /// Whether the repository was refreshed in this run
    pub was_refreshed: bool,
}

impl MappingRepository {
    /// Ensures the mapping repository is available and up-to-date.
    ///
    /// # Logic
    ///
    /// 1. If `force_refresh` is true, always update
    /// 2. If not cached, clone the repository
    /// 3. If cached but older than 24 hours, update
    /// 4. If network unavailable but cached, use cache with warning
    /// 5. If network unavailable and not cached, return error
    ///
    /// # Arguments
    ///
    /// * `url` - The URL of the mapping repository
    /// * `force_refresh` - If true, forces an immediate update regardless of cache age
    ///
    /// # Returns
    ///
    /// Returns a `MappingRepository` with the local path to the cached repository,
    /// or an error if the repository is not available and cannot be cloned.
    pub fn ensure_available(url: &str, force_refresh: bool) -> Result<Self, MappingError> {
        let cache = CacheManager::new()?;
        let local_path = cache.get_repo_path(url)?;
        let is_cached = cache.is_cached(url);

        log::debug!("Repository URL: {}", url);
        log::debug!("Local path: {:?}", local_path);
        log::debug!("Is cached: {}", is_cached);

        // Determine if we need to update
        let needs_update = if force_refresh {
            log::debug!("Force refresh requested");
            true
        } else if !is_cached {
            log::debug!("Repository not cached, clone required");
            true
        } else {
            let needs_it = cache.needs_refresh(url)?;
            if needs_it {
                log::debug!("Cache expired (older than 24 hours), update needed");
            } else {
                log::debug!("Cache is fresh (updated within 24 hours)");
            }
            needs_it
        };

        let was_refreshed = if needs_update {
            match Self::try_update_or_clone(&local_path, url, is_cached) {
                Ok(()) => {
                    cache.update_timestamp(url)?;
                    true
                }
                Err(MappingError::Git(GitError::NetworkUnreachable)) if is_cached => {
                    // Network failed but we have cache - use it
                    log::warn!(
                        "Cannot reach remote repository, using cached version. \
                        Run with --verbose for more details."
                    );
                    false
                }
                Err(e) => {
                    if !is_cached {
                        return Err(MappingError::NotAvailable(format!(
                            "Cannot clone mapping repository and no cached version exists: {}",
                            e
                        )));
                    }
                    return Err(e);
                }
            }
        } else {
            log::debug!("Using cached mapping repository (last updated within 24 hours)");
            false
        };

        Ok(Self {
            local_path,
            url: url.to_string(),
            was_refreshed,
        })
    }

    /// Attempts to update or clone the repository.
    fn try_update_or_clone(
        local_path: &std::path::Path,
        url: &str,
        is_cached: bool,
    ) -> Result<(), MappingError> {
        if is_cached {
            log::info!("Updating mapping repository...");
            GitOperations::update(local_path)?;
        } else {
            log::info!("Cloning mapping repository...");
            GitOperations::shallow_clone(url, local_path)?;
        }
        Ok(())
    }

    /// Returns the path to the aws mappings directory within the repository.
    ///
    /// This is where the YAML mapping files for AWS resources are located.
    pub fn aws_mappings_path(&self) -> PathBuf {
        self.local_path.join("aws")
    }

    /// Returns the path to a specific mapping file.
    ///
    /// # Arguments
    ///
    /// * `provider` - The provider name (e.g., "aws")
    /// * `block_type` - The block type (e.g., "resource", "data", "ephemeral", "action")
    /// * `resource_type` - The resource type (e.g., "aws_s3_bucket")
    ///
    /// # Returns
    ///
    /// The full path to the mapping file (e.g., `~/.lppc/user/repo/aws/resource/aws_s3_bucket.yaml`)
    pub fn mapping_file_path(
        &self,
        provider: &str,
        block_type: &str,
        resource_type: &str,
    ) -> PathBuf {
        self.local_path
            .join(provider)
            .join(block_type)
            .join(format!("{}.yaml", resource_type))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mapping_file_path() {
        let repo = MappingRepository {
            local_path: PathBuf::from("/home/user/.lppc/bebold-jhr/lppc-aws-test"),
            url: "https://github.com/bebold-jhr/lppc-aws-test".to_string(),
            was_refreshed: false,
        };

        let path = repo.mapping_file_path("aws", "resource", "aws_s3_bucket");
        assert_eq!(
            path,
            PathBuf::from(
                "/home/user/.lppc/bebold-jhr/lppc-aws-test/aws/resource/aws_s3_bucket.yaml"
            )
        );
    }

    #[test]
    fn test_aws_mappings_path() {
        let repo = MappingRepository {
            local_path: PathBuf::from("/home/user/.lppc/bebold-jhr/lppc-aws-test"),
            url: "https://github.com/bebold-jhr/lppc-aws-test".to_string(),
            was_refreshed: false,
        };

        let path = repo.aws_mappings_path();
        assert_eq!(
            path,
            PathBuf::from("/home/user/.lppc/bebold-jhr/lppc-aws-test/aws")
        );
    }

    // Integration tests for ensure_available would require network access
    // or a mock git server, and are better suited for integration tests
}
