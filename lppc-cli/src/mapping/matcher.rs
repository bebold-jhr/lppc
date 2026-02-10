//! Permission matcher for resolving IAM actions from Terraform configurations.
//!
//! This module matches Terraform blocks to their corresponding mapping files
//! and resolves the required IAM permissions based on the attributes present
//! in each block.

use std::collections::{HashMap, HashSet};
use thiserror::Error;

use super::loader::{LoadError, MappingLoader};
use crate::terraform::{BlockType, TerraformConfig};

/// Result of permission matching for a Terraform configuration.
#[derive(Debug)]
pub struct PermissionResult {
    /// Map of output name to set of permissions.
    /// Key: Output name (e.g., "NetworkDeployer")
    /// Value: Set of IAM actions required for that role
    pub permissions: HashMap<String, HashSet<String>>,

    /// Blocks that had no mapping file available.
    /// These require manual permission review.
    pub missing_mappings: Vec<MissingMapping>,
}

/// Represents a Terraform block with no corresponding mapping file.
#[derive(Debug, Clone)]
pub struct MissingMapping {
    /// The block type (resource, data, ephemeral, action)
    pub block_type: BlockType,

    /// The type name (e.g., "aws_s3_bucket")
    pub type_name: String,

    /// The expected path where the mapping file should be
    pub expected_path: String,
}

/// Errors that can occur during permission matching.
#[derive(Debug, Error)]
pub enum MatchError {
    #[error("Load error: {0}")]
    Load(#[from] LoadError),
}

/// Matches Terraform blocks to IAM permissions.
pub struct PermissionMatcher<'a> {
    loader: &'a MappingLoader,
}

impl<'a> PermissionMatcher<'a> {
    /// Creates a new permission matcher with the given loader.
    pub fn new(loader: &'a MappingLoader) -> Self {
        Self { loader }
    }

    /// Resolves permissions for all blocks in the Terraform configuration.
    ///
    /// This method:
    /// 1. Iterates through all provider groups
    /// 2. For each block, loads the corresponding mapping file
    /// 3. Adds required actions to the permission set
    /// 4. Resolves optional actions based on present attributes
    /// 5. Tracks any blocks without mapping files
    ///
    /// # Arguments
    ///
    /// * `config` - The parsed Terraform configuration
    ///
    /// # Returns
    ///
    /// A `PermissionResult` containing the resolved permissions per provider group
    /// and a list of blocks with missing mappings.
    pub fn resolve(&self, config: &TerraformConfig) -> Result<PermissionResult, MatchError> {
        let mut permissions: HashMap<String, HashSet<String>> = HashMap::new();
        let mut missing_mappings: Vec<MissingMapping> = Vec::new();
        let mut seen_types: HashSet<(BlockType, String)> = HashSet::new();

        for (output_name, group) in &config.provider_groups {
            let mut group_permissions: HashSet<String> = HashSet::new();

            for block in &group.blocks {
                let type_key = (block.block_type, block.type_name.clone());

                // Load mapping (only report missing once per type)
                let provider =
                    MappingLoader::extract_provider(&block.type_name).unwrap_or("unknown");

                match self
                    .loader
                    .load(provider, block.block_type, &block.type_name)?
                {
                    Some(mapping) => {
                        // Add required actions
                        let required_count = mapping.actions.len();
                        for action in &mapping.actions {
                            group_permissions.insert(action.clone());
                        }

                        // Resolve optional actions based on present attributes
                        let optional_actions = mapping.optional.resolve(&block.present_attributes);
                        let optional_count = optional_actions.len();
                        for action in optional_actions {
                            group_permissions.insert(action);
                        }

                        log::debug!(
                            "Resolved {} required + {} optional actions for {}.{}",
                            required_count,
                            optional_count,
                            block.block_type.as_str(),
                            block.type_name
                        );
                    }
                    None => {
                        // Track missing mapping (once per type)
                        if !seen_types.contains(&type_key) {
                            seen_types.insert(type_key);
                            missing_mappings.push(MissingMapping {
                                block_type: block.block_type,
                                type_name: block.type_name.clone(),
                                expected_path: format!(
                                    "{}/{}/{}.yaml",
                                    provider,
                                    block.block_type.as_str(),
                                    block.type_name
                                ),
                            });
                        }
                    }
                }
            }

            if !group_permissions.is_empty() {
                permissions.insert(output_name.clone(), group_permissions);
            }
        }

        // Handle unmapped blocks (no provider found)
        for block in &config.unmapped_blocks {
            log::warn!(
                "Block {}.{} could not be mapped to a provider",
                block.block_type.as_str(),
                block.type_name
            );
        }

        Ok(PermissionResult {
            permissions,
            missing_mappings,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terraform::{ProviderGroup, TerraformBlock};
    use std::fs;
    use tempfile::TempDir;

    fn create_test_block(
        block_type: BlockType,
        type_name: &str,
        present_attributes: HashSet<Vec<String>>,
    ) -> TerraformBlock {
        TerraformBlock {
            block_type,
            type_name: type_name.to_string(),
            name: "test".to_string(),
            provider_config_key: "aws".to_string(),
            present_attributes,
            address: format!("{}.{}.test", block_type.as_str(), type_name),
        }
    }

    fn create_test_config(groups: HashMap<String, ProviderGroup>) -> TerraformConfig {
        TerraformConfig {
            provider_groups: groups,
            unmapped_blocks: Vec::new(),
        }
    }

    #[test]
    fn resolve_empty_config() {
        let temp_dir = TempDir::new().unwrap();
        let loader = MappingLoader::new(temp_dir.path().to_path_buf());
        let matcher = PermissionMatcher::new(&loader);

        let config = create_test_config(HashMap::new());
        let result = matcher.resolve(&config).unwrap();

        assert!(result.permissions.is_empty());
        assert!(result.missing_mappings.is_empty());
    }

    #[test]
    fn resolve_single_block_with_mapping() {
        let temp_dir = TempDir::new().unwrap();

        // Create mapping file
        fs::create_dir_all(temp_dir.path().join("aws/resource")).unwrap();
        fs::write(
            temp_dir.path().join("aws/resource/aws_s3_bucket.yaml"),
            "actions:\n  - s3:CreateBucket\n  - s3:DeleteBucket",
        )
        .unwrap();

        let loader = MappingLoader::new(temp_dir.path().to_path_buf());
        let matcher = PermissionMatcher::new(&loader);

        let block = create_test_block(BlockType::Resource, "aws_s3_bucket", HashSet::new());
        let mut groups = HashMap::new();
        groups.insert(
            "TestDeployer".to_string(),
            ProviderGroup {
                output_name: "TestDeployer".to_string(),
                role_arn: Some("arn:aws:iam::123456789012:role/Test".to_string()),
                blocks: vec![block],
            },
        );

        let config = create_test_config(groups);
        let result = matcher.resolve(&config).unwrap();

        assert_eq!(result.permissions.len(), 1);
        let perms = result.permissions.get("TestDeployer").unwrap();
        assert!(perms.contains("s3:CreateBucket"));
        assert!(perms.contains("s3:DeleteBucket"));
        assert!(result.missing_mappings.is_empty());
    }

    #[test]
    fn resolve_block_without_mapping() {
        let temp_dir = TempDir::new().unwrap();
        let loader = MappingLoader::new(temp_dir.path().to_path_buf());
        let matcher = PermissionMatcher::new(&loader);

        let block = create_test_block(BlockType::Resource, "aws_unknown_resource", HashSet::new());
        let mut groups = HashMap::new();
        groups.insert(
            "TestDeployer".to_string(),
            ProviderGroup {
                output_name: "TestDeployer".to_string(),
                role_arn: Some("arn:aws:iam::123456789012:role/Test".to_string()),
                blocks: vec![block],
            },
        );

        let config = create_test_config(groups);
        let result = matcher.resolve(&config).unwrap();

        // No permissions resolved (mapping not found)
        assert!(result.permissions.is_empty());
        // Missing mapping tracked
        assert_eq!(result.missing_mappings.len(), 1);
        assert_eq!(result.missing_mappings[0].type_name, "aws_unknown_resource");
        assert_eq!(
            result.missing_mappings[0].expected_path,
            "aws/resource/aws_unknown_resource.yaml"
        );
    }

    #[test]
    fn resolve_optional_actions() {
        let temp_dir = TempDir::new().unwrap();

        // Create mapping with optional
        fs::create_dir_all(temp_dir.path().join("aws/resource")).unwrap();
        fs::write(
            temp_dir.path().join("aws/resource/aws_s3_bucket.yaml"),
            r#"
actions:
  - s3:CreateBucket
optional:
  tags:
    - s3:PutBucketTagging
"#,
        )
        .unwrap();

        let loader = MappingLoader::new(temp_dir.path().to_path_buf());
        let matcher = PermissionMatcher::new(&loader);

        // Block with tags attribute present
        let mut present = HashSet::new();
        present.insert(vec!["tags".to_string()]);
        let block = create_test_block(BlockType::Resource, "aws_s3_bucket", present);

        let mut groups = HashMap::new();
        groups.insert(
            "TestDeployer".to_string(),
            ProviderGroup {
                output_name: "TestDeployer".to_string(),
                role_arn: Some("arn:aws:iam::123456789012:role/Test".to_string()),
                blocks: vec![block],
            },
        );

        let config = create_test_config(groups);
        let result = matcher.resolve(&config).unwrap();

        let perms = result.permissions.get("TestDeployer").unwrap();
        assert!(perms.contains("s3:CreateBucket"));
        assert!(perms.contains("s3:PutBucketTagging"));
    }

    #[test]
    fn resolve_optional_not_included_when_absent() {
        let temp_dir = TempDir::new().unwrap();

        // Create mapping with optional
        fs::create_dir_all(temp_dir.path().join("aws/resource")).unwrap();
        fs::write(
            temp_dir.path().join("aws/resource/aws_s3_bucket.yaml"),
            r#"
actions:
  - s3:CreateBucket
optional:
  tags:
    - s3:PutBucketTagging
"#,
        )
        .unwrap();

        let loader = MappingLoader::new(temp_dir.path().to_path_buf());
        let matcher = PermissionMatcher::new(&loader);

        // Block without tags attribute
        let block = create_test_block(BlockType::Resource, "aws_s3_bucket", HashSet::new());

        let mut groups = HashMap::new();
        groups.insert(
            "TestDeployer".to_string(),
            ProviderGroup {
                output_name: "TestDeployer".to_string(),
                role_arn: Some("arn:aws:iam::123456789012:role/Test".to_string()),
                blocks: vec![block],
            },
        );

        let config = create_test_config(groups);
        let result = matcher.resolve(&config).unwrap();

        let perms = result.permissions.get("TestDeployer").unwrap();
        assert!(perms.contains("s3:CreateBucket"));
        assert!(!perms.contains("s3:PutBucketTagging"));
    }

    #[test]
    fn resolve_nested_optional() {
        let temp_dir = TempDir::new().unwrap();

        // Create mapping with nested optional
        fs::create_dir_all(temp_dir.path().join("aws/resource")).unwrap();
        fs::write(
            temp_dir.path().join("aws/resource/aws_route53_zone.yaml"),
            r#"
actions:
  - route53:CreateHostedZone
optional:
  vpc:
    vpc_id:
      - route53:AssociateVPCWithHostedZone
"#,
        )
        .unwrap();

        let loader = MappingLoader::new(temp_dir.path().to_path_buf());
        let matcher = PermissionMatcher::new(&loader);

        // Block with vpc.vpc_id present
        let mut present = HashSet::new();
        present.insert(vec!["vpc".to_string()]);
        present.insert(vec!["vpc".to_string(), "vpc_id".to_string()]);
        let block = create_test_block(BlockType::Resource, "aws_route53_zone", present);

        let mut groups = HashMap::new();
        groups.insert(
            "TestDeployer".to_string(),
            ProviderGroup {
                output_name: "TestDeployer".to_string(),
                role_arn: Some("arn:aws:iam::123456789012:role/Test".to_string()),
                blocks: vec![block],
            },
        );

        let config = create_test_config(groups);
        let result = matcher.resolve(&config).unwrap();

        let perms = result.permissions.get("TestDeployer").unwrap();
        assert!(perms.contains("route53:CreateHostedZone"));
        assert!(perms.contains("route53:AssociateVPCWithHostedZone"));
    }

    #[test]
    fn resolve_multiple_blocks_same_type_deduplicates_permissions() {
        let temp_dir = TempDir::new().unwrap();

        // Create mapping file
        fs::create_dir_all(temp_dir.path().join("aws/resource")).unwrap();
        fs::write(
            temp_dir.path().join("aws/resource/aws_s3_bucket.yaml"),
            "actions:\n  - s3:CreateBucket\n  - s3:DeleteBucket",
        )
        .unwrap();

        let loader = MappingLoader::new(temp_dir.path().to_path_buf());
        let matcher = PermissionMatcher::new(&loader);

        // Two blocks of same type
        let block1 = create_test_block(BlockType::Resource, "aws_s3_bucket", HashSet::new());
        let block2 = create_test_block(BlockType::Resource, "aws_s3_bucket", HashSet::new());

        let mut groups = HashMap::new();
        groups.insert(
            "TestDeployer".to_string(),
            ProviderGroup {
                output_name: "TestDeployer".to_string(),
                role_arn: Some("arn:aws:iam::123456789012:role/Test".to_string()),
                blocks: vec![block1, block2],
            },
        );

        let config = create_test_config(groups);
        let result = matcher.resolve(&config).unwrap();

        // Should have exactly 2 permissions (deduplicated)
        let perms = result.permissions.get("TestDeployer").unwrap();
        assert_eq!(perms.len(), 2);
    }

    #[test]
    fn resolve_multiple_groups() {
        let temp_dir = TempDir::new().unwrap();

        // Create mapping files
        fs::create_dir_all(temp_dir.path().join("aws/resource")).unwrap();
        fs::write(
            temp_dir.path().join("aws/resource/aws_s3_bucket.yaml"),
            "actions:\n  - s3:CreateBucket",
        )
        .unwrap();
        fs::write(
            temp_dir.path().join("aws/resource/aws_ec2_instance.yaml"),
            "actions:\n  - ec2:RunInstances",
        )
        .unwrap();

        let loader = MappingLoader::new(temp_dir.path().to_path_buf());
        let matcher = PermissionMatcher::new(&loader);

        let block1 = create_test_block(BlockType::Resource, "aws_s3_bucket", HashSet::new());
        let block2 = create_test_block(BlockType::Resource, "aws_ec2_instance", HashSet::new());

        let mut groups = HashMap::new();
        groups.insert(
            "StorageDeployer".to_string(),
            ProviderGroup {
                output_name: "StorageDeployer".to_string(),
                role_arn: Some("arn:aws:iam::123456789012:role/Storage".to_string()),
                blocks: vec![block1],
            },
        );
        groups.insert(
            "ComputeDeployer".to_string(),
            ProviderGroup {
                output_name: "ComputeDeployer".to_string(),
                role_arn: Some("arn:aws:iam::123456789012:role/Compute".to_string()),
                blocks: vec![block2],
            },
        );

        let config = create_test_config(groups);
        let result = matcher.resolve(&config).unwrap();

        assert_eq!(result.permissions.len(), 2);

        let storage_perms = result.permissions.get("StorageDeployer").unwrap();
        assert!(storage_perms.contains("s3:CreateBucket"));
        assert!(!storage_perms.contains("ec2:RunInstances"));

        let compute_perms = result.permissions.get("ComputeDeployer").unwrap();
        assert!(compute_perms.contains("ec2:RunInstances"));
        assert!(!compute_perms.contains("s3:CreateBucket"));
    }

    #[test]
    fn resolve_data_source() {
        let temp_dir = TempDir::new().unwrap();

        // Create mapping for data source
        fs::create_dir_all(temp_dir.path().join("aws/data")).unwrap();
        fs::write(
            temp_dir.path().join("aws/data/aws_availability_zones.yaml"),
            "actions:\n  - ec2:DescribeAvailabilityZones",
        )
        .unwrap();

        let loader = MappingLoader::new(temp_dir.path().to_path_buf());
        let matcher = PermissionMatcher::new(&loader);

        let block = create_test_block(BlockType::Data, "aws_availability_zones", HashSet::new());

        let mut groups = HashMap::new();
        groups.insert(
            "TestDeployer".to_string(),
            ProviderGroup {
                output_name: "TestDeployer".to_string(),
                role_arn: Some("arn:aws:iam::123456789012:role/Test".to_string()),
                blocks: vec![block],
            },
        );

        let config = create_test_config(groups);
        let result = matcher.resolve(&config).unwrap();

        let perms = result.permissions.get("TestDeployer").unwrap();
        assert!(perms.contains("ec2:DescribeAvailabilityZones"));
    }

    #[test]
    fn missing_mapping_tracked_once_per_type() {
        let temp_dir = TempDir::new().unwrap();
        let loader = MappingLoader::new(temp_dir.path().to_path_buf());
        let matcher = PermissionMatcher::new(&loader);

        // Multiple blocks of same unknown type
        let block1 = create_test_block(BlockType::Resource, "aws_unknown", HashSet::new());
        let block2 = create_test_block(BlockType::Resource, "aws_unknown", HashSet::new());

        let mut groups = HashMap::new();
        groups.insert(
            "TestDeployer".to_string(),
            ProviderGroup {
                output_name: "TestDeployer".to_string(),
                role_arn: Some("arn:aws:iam::123456789012:role/Test".to_string()),
                blocks: vec![block1, block2],
            },
        );

        let config = create_test_config(groups);
        let result = matcher.resolve(&config).unwrap();

        // Should only track missing mapping once
        assert_eq!(result.missing_mappings.len(), 1);
    }

    #[test]
    fn missing_mapping_tracked_per_block_type() {
        let temp_dir = TempDir::new().unwrap();
        let loader = MappingLoader::new(temp_dir.path().to_path_buf());
        let matcher = PermissionMatcher::new(&loader);

        // Same type name but different block types
        let block1 = create_test_block(BlockType::Resource, "aws_unknown", HashSet::new());
        let block2 = create_test_block(BlockType::Data, "aws_unknown", HashSet::new());

        let mut groups = HashMap::new();
        groups.insert(
            "TestDeployer".to_string(),
            ProviderGroup {
                output_name: "TestDeployer".to_string(),
                role_arn: Some("arn:aws:iam::123456789012:role/Test".to_string()),
                blocks: vec![block1, block2],
            },
        );

        let config = create_test_config(groups);
        let result = matcher.resolve(&config).unwrap();

        // Should track both as separate missing mappings
        assert_eq!(result.missing_mappings.len(), 2);
    }
}
