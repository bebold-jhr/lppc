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

/// Result of looking up a mapping for a Terraform type.
///
/// Represents the three possible outcomes:
/// - `Found`: A `.yaml` mapping file exists with IAM permissions
/// - `Skipped`: A `.skip` file exists, marking the type as intentionally needing no permissions
/// - `NotFound`: Neither file exists — the type is unmapped
#[derive(Debug, Clone)]
pub enum MappingLookup {
    Found(ActionMapping),
    Skipped,
    NotFound,
}

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

    /// In-memory cache of mapping lookup results
    /// Key: "{provider}/{block_type}/{type_name}" e.g., "resource/aws_s3_bucket"
    /// Value: Found(mapping), Skipped, or NotFound
    cache: Mutex<HashMap<String, MappingLookup>>,
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

    /// Loads a mapping for a given block.
    ///
    /// Checks for a `.yaml` mapping file first, then a `.skip` file.
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
    /// * `Ok(MappingLookup::Found(mapping))` - YAML mapping file exists and was parsed
    /// * `Ok(MappingLookup::Skipped)` - A `.skip` file exists (type needs no permissions)
    /// * `Ok(MappingLookup::NotFound)` - Neither file exists or name is invalid
    /// * `Err(_)` - IO or parse error
    pub fn load(
        &self,
        provider: &str,
        block_type: BlockType,
        type_name: &str,
    ) -> Result<MappingLookup, LoadError> {
        // Validate inputs to prevent path traversal attacks
        if !is_valid_path_component(provider) || !is_valid_path_component(type_name) {
            log::warn!(
                "Invalid provider or type_name (possible path traversal): {}/{}",
                provider,
                type_name
            );
            return Ok(MappingLookup::NotFound);
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

        // Not in cache — check for .yaml file first
        let block_type_dir = self
            .repo_path
            .join("mappings")
            .join(block_type.as_str());

        let yaml_path = block_type_dir.join(format!("{}.yaml", type_name));

        let lookup = if yaml_path.exists() {
            log::debug!("Loading mapping from {:?}", yaml_path);

            // Check file size before reading to prevent resource exhaustion
            let metadata = std::fs::metadata(&yaml_path)?;
            if metadata.len() > MAX_YAML_FILE_SIZE {
                return Err(LoadError::FileTooLarge(yaml_path));
            }

            let content = std::fs::read_to_string(&yaml_path)?;
            let mapping = yaml_parser::parse_mapping(&content)
                .map_err(|e| LoadError::Parse(yaml_path.clone(), e.to_string()))?;
            MappingLookup::Found(mapping)
        } else {
            // No .yaml file — check for .skip file
            let skip_path = block_type_dir.join(format!("{}.skip", type_name));

            if skip_path.exists() {
                log::debug!("Skip file found for {}", cache_key);
                MappingLookup::Skipped
            } else {
                log::debug!("No mapping file found for {}", cache_key);
                MappingLookup::NotFound
            }
        };

        // Store in cache
        {
            let mut cache = self.cache.lock().unwrap();
            cache.insert(cache_key, lookup.clone());
        }

        Ok(lookup)
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
    fn loader_returns_not_found_for_missing_file() {
        let temp_dir = TempDir::new().unwrap();
        let loader = MappingLoader::new(temp_dir.path().to_path_buf());

        let result = loader
            .load("aws", BlockType::Resource, "aws_nonexistent")
            .unwrap();
        assert!(matches!(result, MappingLookup::NotFound));
    }

    #[test]
    fn loader_loads_existing_mapping() {
        let temp_dir = TempDir::new().unwrap();

        // Create mapping file
        fs::create_dir_all(temp_dir.path().join("mappings/resource")).unwrap();
        fs::write(
            temp_dir.path().join("mappings/resource/aws_s3_bucket.yaml"),
            "allow:\n  - s3:CreateBucket\n  - s3:DeleteBucket",
        )
        .unwrap();

        let loader = MappingLoader::new(temp_dir.path().to_path_buf());

        let result = loader
            .load("aws", BlockType::Resource, "aws_s3_bucket")
            .unwrap();

        match result {
            MappingLookup::Found(mapping) => {
                assert_eq!(mapping.allow.len(), 2);
                assert!(mapping.allow.contains(&"s3:CreateBucket".to_string()));
                assert!(mapping.allow.contains(&"s3:DeleteBucket".to_string()));
            }
            _ => panic!("Expected MappingLookup::Found"),
        }
    }

    #[test]
    fn loader_caches_found_mappings() {
        let temp_dir = TempDir::new().unwrap();

        // Create mapping file
        fs::create_dir_all(temp_dir.path().join("mappings/resource")).unwrap();
        fs::write(
            temp_dir.path().join("mappings/resource/aws_s3_bucket.yaml"),
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

        // Both should be Found with same data
        match (first, second) {
            (MappingLookup::Found(a), MappingLookup::Found(b)) => {
                assert_eq!(a.allow, b.allow);
            }
            _ => panic!("Expected both to be MappingLookup::Found"),
        }
    }

    #[test]
    fn loader_caches_not_found() {
        let temp_dir = TempDir::new().unwrap();
        let loader = MappingLoader::new(temp_dir.path().to_path_buf());

        // Load twice - both should be NotFound
        let first = loader
            .load("aws", BlockType::Resource, "aws_nonexistent")
            .unwrap();
        let second = loader
            .load("aws", BlockType::Resource, "aws_nonexistent")
            .unwrap();

        assert!(matches!(first, MappingLookup::NotFound));
        assert!(matches!(second, MappingLookup::NotFound));
    }

    #[test]
    fn loader_handles_data_block_type() {
        let temp_dir = TempDir::new().unwrap();

        // Create mapping file for data source
        fs::create_dir_all(temp_dir.path().join("mappings/data")).unwrap();
        fs::write(
            temp_dir.path().join("mappings/data/aws_availability_zones.yaml"),
            "allow:\n  - ec2:DescribeAvailabilityZones",
        )
        .unwrap();

        let loader = MappingLoader::new(temp_dir.path().to_path_buf());

        let result = loader
            .load("aws", BlockType::Data, "aws_availability_zones")
            .unwrap();
        assert!(matches!(result, MappingLookup::Found(_)));
    }

    #[test]
    fn loader_handles_conditional_actions() {
        let temp_dir = TempDir::new().unwrap();

        // Create mapping file with conditional
        fs::create_dir_all(temp_dir.path().join("mappings/resource")).unwrap();
        fs::write(
            temp_dir.path().join("mappings/resource/aws_s3_bucket.yaml"),
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

        match result {
            MappingLookup::Found(mapping) => {
                assert!(!mapping.conditional.is_none());
            }
            _ => panic!("Expected MappingLookup::Found"),
        }
    }

    #[test]
    fn loader_returns_parse_error_for_invalid_yaml() {
        let temp_dir = TempDir::new().unwrap();

        // Create invalid YAML file
        fs::create_dir_all(temp_dir.path().join("mappings/resource")).unwrap();
        fs::write(
            temp_dir.path().join("mappings/resource/aws_s3_bucket.yaml"),
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
        assert!(matches!(result, MappingLookup::NotFound));
    }

    #[test]
    fn loader_rejects_path_traversal_in_type_name() {
        let temp_dir = TempDir::new().unwrap();
        let loader = MappingLoader::new(temp_dir.path().to_path_buf());

        // Attempt path traversal via type_name
        let result = loader
            .load("aws", BlockType::Resource, "../../etc/passwd")
            .unwrap();
        assert!(matches!(result, MappingLookup::NotFound));
    }

    #[test]
    fn loader_rejects_hidden_files() {
        let temp_dir = TempDir::new().unwrap();
        let loader = MappingLoader::new(temp_dir.path().to_path_buf());

        let result = loader
            .load(".hidden", BlockType::Resource, "aws_s3_bucket")
            .unwrap();
        assert!(matches!(result, MappingLookup::NotFound));
    }

    #[test]
    fn loader_returns_error_for_oversized_file() {
        let temp_dir = TempDir::new().unwrap();

        // Create a file larger than MAX_YAML_FILE_SIZE
        fs::create_dir_all(temp_dir.path().join("mappings/resource")).unwrap();
        let large_content = "a".repeat(2 * 1024 * 1024); // 2 MB
        fs::write(
            temp_dir.path().join("mappings/resource/aws_large.yaml"),
            large_content,
        )
        .unwrap();

        let loader = MappingLoader::new(temp_dir.path().to_path_buf());

        let result = loader.load("aws", BlockType::Resource, "aws_large");
        assert!(matches!(result, Err(LoadError::FileTooLarge(_))));
    }

    // --- Skip file tests ---

    #[test]
    fn loader_returns_skipped_for_skip_file() {
        let temp_dir = TempDir::new().unwrap();

        // Create a .skip file (no .yaml)
        fs::create_dir_all(temp_dir.path().join("mappings/data")).unwrap();
        fs::write(
            temp_dir.path().join("mappings/data/aws_arn.skip"),
            "",
        )
        .unwrap();

        let loader = MappingLoader::new(temp_dir.path().to_path_buf());

        let result = loader
            .load("aws", BlockType::Data, "aws_arn")
            .unwrap();
        assert!(matches!(result, MappingLookup::Skipped));
    }

    #[test]
    fn loader_caches_skipped_files() {
        let temp_dir = TempDir::new().unwrap();

        fs::create_dir_all(temp_dir.path().join("mappings/data")).unwrap();
        fs::write(
            temp_dir.path().join("mappings/data/aws_arn.skip"),
            "",
        )
        .unwrap();

        let loader = MappingLoader::new(temp_dir.path().to_path_buf());

        let first = loader.load("aws", BlockType::Data, "aws_arn").unwrap();
        let second = loader.load("aws", BlockType::Data, "aws_arn").unwrap();

        assert!(matches!(first, MappingLookup::Skipped));
        assert!(matches!(second, MappingLookup::Skipped));
    }

    #[test]
    fn loader_yaml_takes_priority_over_skip() {
        let temp_dir = TempDir::new().unwrap();

        // Create both .yaml and .skip files — .yaml should win
        fs::create_dir_all(temp_dir.path().join("mappings/resource")).unwrap();
        fs::write(
            temp_dir.path().join("mappings/resource/aws_s3_bucket.yaml"),
            "allow:\n  - s3:CreateBucket",
        )
        .unwrap();
        fs::write(
            temp_dir.path().join("mappings/resource/aws_s3_bucket.skip"),
            "",
        )
        .unwrap();

        let loader = MappingLoader::new(temp_dir.path().to_path_buf());

        let result = loader
            .load("aws", BlockType::Resource, "aws_s3_bucket")
            .unwrap();

        match result {
            MappingLookup::Found(mapping) => {
                assert!(mapping.allow.contains(&"s3:CreateBucket".to_string()));
            }
            _ => panic!("Expected MappingLookup::Found (yaml should take priority over skip)"),
        }
    }

    #[test]
    fn loader_rejects_path_traversal_in_skip_file() {
        let temp_dir = TempDir::new().unwrap();

        // Even if a .skip file existed at a traversal path, the validator should reject it
        let loader = MappingLoader::new(temp_dir.path().to_path_buf());

        let result = loader
            .load("aws", BlockType::Data, "../etc/passwd")
            .unwrap();
        assert!(matches!(result, MappingLookup::NotFound));
    }
}
