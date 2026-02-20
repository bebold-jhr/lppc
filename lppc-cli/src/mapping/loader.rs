//! Mapping loader with in-memory caching.
//!
//! This module handles loading YAML mapping files from the cached repository
//! and maintains an in-memory cache to avoid repeated file I/O when multiple
//! blocks of the same type are processed.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use thiserror::Error;

use super::schema::ActionMapping;
use super::yaml_parser;
use crate::terraform::BlockType;

/// Maximum YAML file size (1 MB) to prevent resource exhaustion attacks.
const MAX_YAML_FILE_SIZE: u64 = 1024 * 1024;

/// Errors that can occur during mapping loading.
#[derive(Debug, Error)]
pub enum LoadError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Parse error in {0}: {1}")]
    Parse(PathBuf, String),

    #[error("File too large: {0}")]
    FileTooLarge(PathBuf),
}

/// Validates that a path component contains only safe characters.
/// Prevents path traversal attacks by rejecting components with `.`, `/`, `\`, etc.
fn is_valid_path_component(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('-')
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Loads and caches mapping files from the repository.
///
/// The loader maintains an in-memory cache to avoid repeated file I/O when
/// multiple Terraform blocks of the same type need to be processed.
pub struct MappingLoader {
    /// Base path to the mapping repository
    repo_path: PathBuf,

    /// In-memory cache of loaded mappings
    /// Key: "{provider}/{block_type}/{type_name}" e.g., "aws/resource/aws_s3_bucket"
    /// Value: Some(mapping) if file exists, None if not found
    cache: Mutex<HashMap<String, Option<ActionMapping>>>,
}

impl MappingLoader {
    /// Creates a new loader for the given repository path.
    ///
    /// # Arguments
    ///
    /// * `repo_path` - Path to the root of the mapping repository
    pub fn new(repo_path: PathBuf) -> Self {
        Self {
            repo_path,
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// Loads a mapping file for a given block.
    ///
    /// Results are cached in memory, so subsequent calls for the same block type
    /// will return the cached value without file I/O.
    ///
    /// # Arguments
    ///
    /// * `provider` - Provider name (e.g., "aws")
    /// * `block_type` - Block type (resource, data, ephemeral, action)
    /// * `type_name` - Type name (e.g., "aws_s3_bucket")
    ///
    /// # Returns
    ///
    /// * `Ok(Some(mapping))` - Mapping file exists and was parsed successfully
    /// * `Ok(None)` - Mapping file does not exist or has invalid name
    /// * `Err(_)` - IO or parse error
    pub fn load(
        &self,
        provider: &str,
        block_type: BlockType,
        type_name: &str,
    ) -> Result<Option<ActionMapping>, LoadError> {
        // Validate inputs to prevent path traversal attacks
        if !is_valid_path_component(provider) || !is_valid_path_component(type_name) {
            log::warn!(
                "Invalid provider or type_name (possible path traversal): {}/{}",
                provider,
                type_name
            );
            return Ok(None);
        }

        let cache_key = format!("{}/{}/{}", provider, block_type.as_str(), type_name);

        // Check cache first
        {
            let cache = self.cache.lock().unwrap();
            if let Some(cached) = cache.get(&cache_key) {
                log::debug!("Cache hit for {}", cache_key);
                return Ok(cached.clone());
            }
        }

        // Not in cache, load from file
        let file_path = self
            .repo_path
            .join("mappings")
            .join(provider)
            .join(block_type.as_str())
            .join(format!("{}.yaml", type_name));

        let mapping = if file_path.exists() {
            log::debug!("Loading mapping from {:?}", file_path);

            // Check file size before reading to prevent resource exhaustion
            let metadata = std::fs::metadata(&file_path)?;
            if metadata.len() > MAX_YAML_FILE_SIZE {
                return Err(LoadError::FileTooLarge(file_path));
            }

            let content = std::fs::read_to_string(&file_path)?;
            let mapping = yaml_parser::parse_mapping(&content)
                .map_err(|e| LoadError::Parse(file_path.clone(), e.to_string()))?;
            Some(mapping)
        } else {
            log::debug!("No mapping file found for {}", cache_key);
            None
        };

        // Store in cache
        {
            let mut cache = self.cache.lock().unwrap();
            cache.insert(cache_key, mapping.clone());
        }

        Ok(mapping)
    }

    /// Extracts the provider name from a type name.
    ///
    /// # Arguments
    ///
    /// * `type_name` - The full type name (e.g., "aws_s3_bucket")
    ///
    /// # Returns
    ///
    /// The provider prefix (e.g., "aws"), or `None` if no underscore is found.
    ///
    /// # Example
    ///
    /// ```ignore
    /// assert_eq!(MappingLoader::extract_provider("aws_s3_bucket"), Some("aws"));
    /// assert_eq!(MappingLoader::extract_provider("google_compute_instance"), Some("google"));
    /// assert_eq!(MappingLoader::extract_provider("invalid"), None);
    /// ```
    pub fn extract_provider(type_name: &str) -> Option<&str> {
        type_name.split('_').next().filter(|s| !s.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn extract_provider_from_aws_resource() {
        assert_eq!(
            MappingLoader::extract_provider("aws_s3_bucket"),
            Some("aws")
        );
    }

    #[test]
    fn extract_provider_from_google_resource() {
        assert_eq!(
            MappingLoader::extract_provider("google_compute_instance"),
            Some("google")
        );
    }

    #[test]
    fn extract_provider_no_underscore() {
        assert_eq!(MappingLoader::extract_provider("invalid"), Some("invalid"));
    }

    #[test]
    fn extract_provider_empty_string() {
        assert_eq!(MappingLoader::extract_provider(""), None);
    }

    #[test]
    fn extract_provider_starts_with_underscore() {
        assert_eq!(MappingLoader::extract_provider("_resource"), None);
    }

    #[test]
    fn loader_returns_none_for_missing_file() {
        let temp_dir = TempDir::new().unwrap();
        let loader = MappingLoader::new(temp_dir.path().to_path_buf());

        let result = loader
            .load("aws", BlockType::Resource, "aws_nonexistent")
            .unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn loader_loads_existing_mapping() {
        let temp_dir = TempDir::new().unwrap();

        // Create mapping file
        fs::create_dir_all(temp_dir.path().join("mappings/aws/resource")).unwrap();
        fs::write(
            temp_dir.path().join("mappings/aws/resource/aws_s3_bucket.yaml"),
            "allow:\n  - s3:CreateBucket\n  - s3:DeleteBucket",
        )
        .unwrap();

        let loader = MappingLoader::new(temp_dir.path().to_path_buf());

        let result = loader
            .load("aws", BlockType::Resource, "aws_s3_bucket")
            .unwrap();
        assert!(result.is_some());

        let mapping = result.unwrap();
        assert_eq!(mapping.allow.len(), 2);
        assert!(mapping.allow.contains(&"s3:CreateBucket".to_string()));
        assert!(mapping.allow.contains(&"s3:DeleteBucket".to_string()));
    }

    #[test]
    fn loader_caches_mappings() {
        let temp_dir = TempDir::new().unwrap();

        // Create mapping file
        fs::create_dir_all(temp_dir.path().join("mappings/aws/resource")).unwrap();
        fs::write(
            temp_dir.path().join("mappings/aws/resource/aws_s3_bucket.yaml"),
            "allow:\n  - s3:CreateBucket",
        )
        .unwrap();

        let loader = MappingLoader::new(temp_dir.path().to_path_buf());

        // Load twice
        let first = loader
            .load("aws", BlockType::Resource, "aws_s3_bucket")
            .unwrap();
        let second = loader
            .load("aws", BlockType::Resource, "aws_s3_bucket")
            .unwrap();

        // Both should return Some with same data
        assert!(first.is_some());
        assert!(second.is_some());
        assert_eq!(first.unwrap().allow, second.unwrap().allow);
    }

    #[test]
    fn loader_caches_missing_files() {
        let temp_dir = TempDir::new().unwrap();
        let loader = MappingLoader::new(temp_dir.path().to_path_buf());

        // Load twice - both should be None
        let first = loader
            .load("aws", BlockType::Resource, "aws_nonexistent")
            .unwrap();
        let second = loader
            .load("aws", BlockType::Resource, "aws_nonexistent")
            .unwrap();

        assert!(first.is_none());
        assert!(second.is_none());
    }

    #[test]
    fn loader_handles_data_block_type() {
        let temp_dir = TempDir::new().unwrap();

        // Create mapping file for data source
        fs::create_dir_all(temp_dir.path().join("mappings/aws/data")).unwrap();
        fs::write(
            temp_dir.path().join("mappings/aws/data/aws_availability_zones.yaml"),
            "allow:\n  - ec2:DescribeAvailabilityZones",
        )
        .unwrap();

        let loader = MappingLoader::new(temp_dir.path().to_path_buf());

        let result = loader
            .load("aws", BlockType::Data, "aws_availability_zones")
            .unwrap();
        assert!(result.is_some());
    }

    #[test]
    fn loader_handles_conditional_actions() {
        let temp_dir = TempDir::new().unwrap();

        // Create mapping file with conditional
        fs::create_dir_all(temp_dir.path().join("mappings/aws/resource")).unwrap();
        fs::write(
            temp_dir.path().join("mappings/aws/resource/aws_s3_bucket.yaml"),
            r#"
allow:
  - s3:CreateBucket
conditional:
  tags:
    - s3:PutBucketTagging
"#,
        )
        .unwrap();

        let loader = MappingLoader::new(temp_dir.path().to_path_buf());

        let result = loader
            .load("aws", BlockType::Resource, "aws_s3_bucket")
            .unwrap();
        assert!(result.is_some());

        let mapping = result.unwrap();
        assert!(!mapping.conditional.is_none());
    }

    #[test]
    fn loader_returns_parse_error_for_invalid_yaml() {
        let temp_dir = TempDir::new().unwrap();

        // Create invalid YAML file
        fs::create_dir_all(temp_dir.path().join("mappings/aws/resource")).unwrap();
        fs::write(
            temp_dir.path().join("mappings/aws/resource/aws_s3_bucket.yaml"),
            "{{invalid yaml",
        )
        .unwrap();

        let loader = MappingLoader::new(temp_dir.path().to_path_buf());

        let result = loader.load("aws", BlockType::Resource, "aws_s3_bucket");
        assert!(matches!(result, Err(LoadError::Parse(_, _))));
    }

    // Security tests

    #[test]
    fn is_valid_path_component_rejects_path_traversal() {
        assert!(!is_valid_path_component(".."));
        assert!(!is_valid_path_component("../etc"));
        assert!(!is_valid_path_component("foo/bar"));
        assert!(!is_valid_path_component("foo\\bar"));
        assert!(!is_valid_path_component(".hidden"));
        assert!(!is_valid_path_component(""));
        assert!(!is_valid_path_component("-dash-prefix"));
    }

    #[test]
    fn is_valid_path_component_accepts_valid_names() {
        assert!(is_valid_path_component("aws"));
        assert!(is_valid_path_component("aws_s3_bucket"));
        assert!(is_valid_path_component("google_compute_instance"));
        assert!(is_valid_path_component("my-resource-123"));
    }

    #[test]
    fn loader_rejects_path_traversal_in_provider() {
        let temp_dir = TempDir::new().unwrap();
        let loader = MappingLoader::new(temp_dir.path().to_path_buf());

        // Attempt path traversal via provider
        let result = loader
            .load("../etc", BlockType::Resource, "passwd")
            .unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn loader_rejects_path_traversal_in_type_name() {
        let temp_dir = TempDir::new().unwrap();
        let loader = MappingLoader::new(temp_dir.path().to_path_buf());

        // Attempt path traversal via type_name
        let result = loader
            .load("aws", BlockType::Resource, "../../etc/passwd")
            .unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn loader_rejects_hidden_files() {
        let temp_dir = TempDir::new().unwrap();
        let loader = MappingLoader::new(temp_dir.path().to_path_buf());

        let result = loader
            .load(".hidden", BlockType::Resource, "aws_s3_bucket")
            .unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn loader_returns_error_for_oversized_file() {
        let temp_dir = TempDir::new().unwrap();

        // Create a file larger than MAX_YAML_FILE_SIZE
        fs::create_dir_all(temp_dir.path().join("mappings/aws/resource")).unwrap();
        let large_content = "a".repeat(2 * 1024 * 1024); // 2 MB
        fs::write(
            temp_dir.path().join("mappings/aws/resource/aws_large.yaml"),
            large_content,
        )
        .unwrap();

        let loader = MappingLoader::new(temp_dir.path().to_path_buf());

        let result = loader.load("aws", BlockType::Resource, "aws_large");
        assert!(matches!(result, Err(LoadError::FileTooLarge(_))));
    }
}
