//! Permission matcher for resolving IAM actions from Terraform configurations.
//!
//! This module matches Terraform blocks to their corresponding mapping files
//! and resolves the required IAM permissions based on the attributes present
//! in each block.

use std::collections::{HashMap, HashSet};
use thiserror::Error;

use super::loader::{LoadError, MappingLoader};
use crate::terraform::{BlockType, TerraformConfig};

/// Permissions for a single provider group, separating allow and deny.
#[derive(Debug, Clone)]
pub struct GroupPermissions {
    /// IAM actions to allow
    pub allow: HashSet<String>,

    /// IAM actions to deny
    pub deny: HashSet<String>,
}

/// Result of permission matching for a Terraform configuration.
#[derive(Debug)]
pub struct PermissionResult {
    /// Map of output name to its resolved permissions.
    /// Key: Output name (e.g., "NetworkDeployer")
    /// Value: Allow and deny permission sets for that role
    pub groups: HashMap<String, GroupPermissions>,

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
    /// 3. Adds allow actions to the allow permission set
    /// 4. Adds deny actions to the deny permission set
    /// 5. Resolves conditional actions into the allow permission set
    /// 6. Tracks any blocks without mapping files
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
        let mut groups: HashMap<String, GroupPermissions> = HashMap::new();
        let mut missing_mappings: Vec<MissingMapping> = Vec::new();
        let mut seen_types: HashSet<(BlockType, String)> = HashSet::new();

        for (output_name, group) in &config.provider_groups {
            let mut group_allow_permissions: HashSet<String> = HashSet::new();
            let mut group_deny_permissions: HashSet<String> = HashSet::new();

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
                        // Add allow actions
                        let allow_count = mapping.allow.len();
                        for action in &mapping.allow {
                            group_allow_permissions.insert(action.clone());
                        }

                        // Add deny actions
                        let deny_count = mapping.deny.len();
                        for action in &mapping.deny {
                            group_deny_permissions.insert(action.clone());
                        }

                        // Resolve conditional actions into allow permissions
                        let conditional_actions =
                            mapping.conditional.resolve(&block.present_attributes);
                        let conditional_count = conditional_actions.len();
                        for action in conditional_actions {
                            group_allow_permissions.insert(action);
                        }

                        log::debug!(
                            "Resolved {} allow + {} conditional + {} deny actions for {}.{}",
                            allow_count,
                            conditional_count,
                            deny_count,
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
                                    "mappings/{}/{}/{}.yaml",
                                    provider,
                                    block.block_type.as_str(),
                                    block.type_name
                                ),
                            });
                        }
                    }
                }
            }

            if !group_allow_permissions.is_empty() || !group_deny_permissions.is_empty() {
                groups.insert(
                    output_name.clone(),
                    GroupPermissions {
                        allow: group_allow_permissions,
                        deny: group_deny_permissions,
                    },
                );
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
            groups,
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

        assert!(result.groups.is_empty());
        assert!(result.missing_mappings.is_empty());
    }

    #[test]
    fn resolve_single_block_with_mapping() {
        let temp_dir = TempDir::new().unwrap();

        // Create mapping file
        fs::create_dir_all(temp_dir.path().join("mappings/aws/resource")).unwrap();
        fs::write(
            temp_dir.path().join("mappings/aws/resource/aws_s3_bucket.yaml"),
            "allow:\n  - s3:CreateBucket\n  - s3:DeleteBucket",
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

        assert_eq!(result.groups.len(), 1);
        let group_perms = result.groups.get("TestDeployer").unwrap();
        assert!(group_perms.allow.contains("s3:CreateBucket"));
        assert!(group_perms.allow.contains("s3:DeleteBucket"));
        assert!(group_perms.deny.is_empty());
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
        assert!(result.groups.is_empty());
        // Missing mapping tracked
        assert_eq!(result.missing_mappings.len(), 1);
        assert_eq!(result.missing_mappings[0].type_name, "aws_unknown_resource");
        assert_eq!(
            result.missing_mappings[0].expected_path,
            "mappings/aws/resource/aws_unknown_resource.yaml"
        );
    }

    #[test]
    fn resolve_conditional_actions() {
        let temp_dir = TempDir::new().unwrap();

        // Create mapping with conditional
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

        let group_perms = result.groups.get("TestDeployer").unwrap();
        assert!(group_perms.allow.contains("s3:CreateBucket"));
        assert!(group_perms.allow.contains("s3:PutBucketTagging"));
    }

    #[test]
    fn resolve_conditional_not_included_when_absent() {
        let temp_dir = TempDir::new().unwrap();

        // Create mapping with conditional
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

        let group_perms = result.groups.get("TestDeployer").unwrap();
        assert!(group_perms.allow.contains("s3:CreateBucket"));
        assert!(!group_perms.allow.contains("s3:PutBucketTagging"));
    }

    #[test]
    fn resolve_nested_conditional() {
        let temp_dir = TempDir::new().unwrap();

        // Create mapping with nested conditional
        fs::create_dir_all(temp_dir.path().join("mappings/aws/resource")).unwrap();
        fs::write(
            temp_dir.path().join("mappings/aws/resource/aws_route53_zone.yaml"),
            r#"
allow:
  - route53:CreateHostedZone
conditional:
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

        let group_perms = result.groups.get("TestDeployer").unwrap();
        assert!(group_perms.allow.contains("route53:CreateHostedZone"));
        assert!(group_perms.allow.contains("route53:AssociateVPCWithHostedZone"));
    }

    #[test]
    fn resolve_multiple_blocks_same_type_deduplicates_permissions() {
        let temp_dir = TempDir::new().unwrap();

        // Create mapping file
        fs::create_dir_all(temp_dir.path().join("mappings/aws/resource")).unwrap();
        fs::write(
            temp_dir.path().join("mappings/aws/resource/aws_s3_bucket.yaml"),
            "allow:\n  - s3:CreateBucket\n  - s3:DeleteBucket",
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
        let group_perms = result.groups.get("TestDeployer").unwrap();
        assert_eq!(group_perms.allow.len(), 2);
    }

    #[test]
    fn resolve_multiple_groups() {
        let temp_dir = TempDir::new().unwrap();

        // Create mapping files
        fs::create_dir_all(temp_dir.path().join("mappings/aws/resource")).unwrap();
        fs::write(
            temp_dir.path().join("mappings/aws/resource/aws_s3_bucket.yaml"),
            "allow:\n  - s3:CreateBucket",
        )
        .unwrap();
        fs::write(
            temp_dir.path().join("mappings/aws/resource/aws_ec2_instance.yaml"),
            "allow:\n  - ec2:RunInstances",
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

        assert_eq!(result.groups.len(), 2);

        let storage_perms = result.groups.get("StorageDeployer").unwrap();
        assert!(storage_perms.allow.contains("s3:CreateBucket"));
        assert!(!storage_perms.allow.contains("ec2:RunInstances"));

        let compute_perms = result.groups.get("ComputeDeployer").unwrap();
        assert!(compute_perms.allow.contains("ec2:RunInstances"));
        assert!(!compute_perms.allow.contains("s3:CreateBucket"));
    }

    #[test]
    fn resolve_data_source() {
        let temp_dir = TempDir::new().unwrap();

        // Create mapping for data source
        fs::create_dir_all(temp_dir.path().join("mappings/aws/data")).unwrap();
        fs::write(
            temp_dir.path().join("mappings/aws/data/aws_availability_zones.yaml"),
            "allow:\n  - ec2:DescribeAvailabilityZones",
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

        let group_perms = result.groups.get("TestDeployer").unwrap();
        assert!(group_perms.allow.contains("ec2:DescribeAvailabilityZones"));
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

    // --- New deny tests ---

    #[test]
    fn resolve_deny_permissions() {
        let temp_dir = TempDir::new().unwrap();

        fs::create_dir_all(temp_dir.path().join("mappings/aws/resource")).unwrap();
        fs::write(
            temp_dir.path().join("mappings/aws/resource/aws_s3_bucket.yaml"),
            r#"
allow:
  - s3:Get*
  - s3:List*
deny:
  - s3:GetObject
"#,
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

        let group_perms = result.groups.get("TestDeployer").unwrap();
        assert!(group_perms.allow.contains("s3:Get*"));
        assert!(group_perms.allow.contains("s3:List*"));
        assert!(group_perms.deny.contains("s3:GetObject"));
        assert!(!group_perms.allow.contains("s3:GetObject"));
    }

    #[test]
    fn resolve_deny_only_mapping() {
        let temp_dir = TempDir::new().unwrap();

        fs::create_dir_all(temp_dir.path().join("mappings/aws/resource")).unwrap();
        fs::write(
            temp_dir.path().join("mappings/aws/resource/aws_s3_bucket.yaml"),
            r#"
deny:
  - s3:GetObject
"#,
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

        let group_perms = result.groups.get("TestDeployer").unwrap();
        assert!(group_perms.allow.is_empty());
        assert!(group_perms.deny.contains("s3:GetObject"));
    }

    #[test]
    fn resolve_deny_deduplication() {
        let temp_dir = TempDir::new().unwrap();

        fs::create_dir_all(temp_dir.path().join("mappings/aws/resource")).unwrap();
        fs::write(
            temp_dir.path().join("mappings/aws/resource/aws_s3_bucket.yaml"),
            r#"
deny:
  - s3:GetObject
"#,
        )
        .unwrap();

        let loader = MappingLoader::new(temp_dir.path().to_path_buf());
        let matcher = PermissionMatcher::new(&loader);

        // Two blocks of same type -> deny should be deduplicated
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

        let group_perms = result.groups.get("TestDeployer").unwrap();
        assert_eq!(group_perms.deny.len(), 1);
    }

    #[test]
    fn resolve_deny_across_multiple_groups() {
        let temp_dir = TempDir::new().unwrap();

        fs::create_dir_all(temp_dir.path().join("mappings/aws/resource")).unwrap();
        fs::write(
            temp_dir.path().join("mappings/aws/resource/aws_s3_bucket.yaml"),
            "deny:\n  - s3:GetObject",
        )
        .unwrap();
        fs::write(
            temp_dir.path().join("mappings/aws/resource/aws_ec2_instance.yaml"),
            "deny:\n  - ec2:TerminateInstances",
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

        let storage_perms = result.groups.get("StorageDeployer").unwrap();
        assert!(storage_perms.deny.contains("s3:GetObject"));
        assert!(!storage_perms.deny.contains("ec2:TerminateInstances"));

        let compute_perms = result.groups.get("ComputeDeployer").unwrap();
        assert!(compute_perms.deny.contains("ec2:TerminateInstances"));
        assert!(!compute_perms.deny.contains("s3:GetObject"));
    }

    #[test]
    fn resolve_conditional_does_not_add_to_deny() {
        let temp_dir = TempDir::new().unwrap();

        fs::create_dir_all(temp_dir.path().join("mappings/aws/resource")).unwrap();
        fs::write(
            temp_dir.path().join("mappings/aws/resource/aws_s3_bucket.yaml"),
            r#"
deny:
  - s3:GetObject
conditional:
  tags:
    - s3:PutBucketTagging
"#,
        )
        .unwrap();

        let loader = MappingLoader::new(temp_dir.path().to_path_buf());
        let matcher = PermissionMatcher::new(&loader);

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

        let group_perms = result.groups.get("TestDeployer").unwrap();
        // Conditional resolves into allow, not deny
        assert!(group_perms.allow.contains("s3:PutBucketTagging"));
        assert!(!group_perms.deny.contains("s3:PutBucketTagging"));
        // Original deny stays in deny
        assert!(group_perms.deny.contains("s3:GetObject"));
    }
}
