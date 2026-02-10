use log::{debug, warn};
use std::collections::HashMap;

use super::json_types::{Module, TerraformPlan};
use super::model::{ProviderGroup, TerraformBlock, TerraformConfig};
use super::provider::{AwsProvider, ProviderRegistry};

/// Parses terraform JSON into our internal model
pub struct TerraformParser;

impl TerraformParser {
    /// Parses the JSON string from `terraform show -json`
    pub fn parse(json: &str) -> Result<TerraformConfig, ParseError> {
        let plan: TerraformPlan =
            serde_json::from_str(json).map_err(|e| ParseError::Json(e.to_string()))?;

        debug!("Parsed terraform plan version {}", plan.terraform_version);

        // Extract providers
        let providers = Self::extract_providers(&plan.configuration.provider_config);
        debug!("Found {} AWS providers", providers.len());

        // Extract all resources (including from modules)
        let blocks = Self::extract_blocks(&plan.configuration.root_module, "");
        debug!("Found {} AWS blocks", blocks.len());

        // Group blocks by provider
        let (provider_groups, unmapped_blocks) = Self::group_blocks(blocks, &providers);

        Ok(TerraformConfig {
            provider_groups,
            unmapped_blocks,
        })
    }

    /// Extracts AWS provider configurations
    fn extract_providers(
        provider_configs: &HashMap<String, super::json_types::ProviderConfig>,
    ) -> ProviderRegistry {
        let mut registry = ProviderRegistry::default();

        for (config_key, config) in provider_configs {
            // Only process AWS providers
            if config.name != "aws" {
                continue;
            }

            let provider = AwsProvider {
                config_key: config_key.clone(),
                alias: config.alias.clone(),
                role_arn: config.get_role_arn(),
                region: config.get_region(),
            };

            debug!(
                "Found AWS provider '{}' with role: {:?}",
                config_key, provider.role_arn
            );

            registry.add(provider);
        }

        registry
    }

    /// Recursively extracts all resource blocks from a module
    fn extract_blocks(module: &Module, address_prefix: &str) -> Vec<TerraformBlock> {
        let mut blocks = Vec::new();

        // Extract resources from this module
        for resource in &module.resources {
            if !resource.is_aws() {
                continue;
            }

            let block_type = match resource.block_type() {
                Some(bt) => bt,
                None => {
                    warn!(
                        "Unknown block mode '{}' for {}",
                        resource.mode, resource.address
                    );
                    continue;
                }
            };

            let address = if address_prefix.is_empty() {
                resource.address.clone()
            } else {
                format!("{}.{}", address_prefix, resource.address)
            };

            blocks.push(TerraformBlock {
                block_type,
                type_name: resource.resource_type.clone(),
                name: resource.name.clone(),
                provider_config_key: resource.provider_config_key.clone(),
                present_attributes: resource.collect_attribute_paths(),
                address,
            });
        }

        // Recursively extract from child modules
        for (module_name, module_call) in &module.module_calls {
            if let Some(child_module) = &module_call.module {
                let child_prefix = if address_prefix.is_empty() {
                    format!("module.{}", module_name)
                } else {
                    format!("{}.module.{}", address_prefix, module_name)
                };

                let child_blocks = Self::extract_blocks(child_module, &child_prefix);
                blocks.extend(child_blocks);
            }
        }

        blocks
    }

    /// Groups blocks by their provider's output name
    fn group_blocks(
        blocks: Vec<TerraformBlock>,
        providers: &ProviderRegistry,
    ) -> (HashMap<String, ProviderGroup>, Vec<TerraformBlock>) {
        let mut groups: HashMap<String, ProviderGroup> = HashMap::new();
        let mut unmapped = Vec::new();

        // Build mapping from config_key to output name
        let key_to_output: HashMap<String, String> = providers
            .group_by_output_name()
            .into_iter()
            .flat_map(|(output_name, keys)| {
                keys.into_iter().map(move |key| (key, output_name.clone()))
            })
            .collect();

        for block in blocks {
            // Try to find the provider for this block
            let output_name = key_to_output
                .get(&block.provider_config_key)
                .or_else(|| key_to_output.get("aws")) // Fall back to default
                .cloned();

            match output_name {
                Some(name) => {
                    let group = groups.entry(name.clone()).or_insert_with(|| {
                        let provider = providers
                            .get(&block.provider_config_key)
                            .or_else(|| providers.get_default());

                        ProviderGroup {
                            output_name: name,
                            role_arn: provider.and_then(|p| p.role_arn.clone()),
                            blocks: Vec::new(),
                        }
                    });
                    group.blocks.push(block);
                }
                None => {
                    // No provider defined at all
                    warn!("Block {} has no matching provider config", block.address);
                    unmapped.push(block);
                }
            }
        }

        (groups, unmapped)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ParseError {
    #[error("JSON parse error: {0}")]
    Json(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terraform::model::BlockType;

    #[test]
    fn parse_minimal_plan() {
        let json = r#"{
            "format_version": "1.0",
            "terraform_version": "1.5.0",
            "configuration": {
                "root_module": {
                    "resources": []
                }
            }
        }"#;

        let config = TerraformParser::parse(json).unwrap();
        assert!(config.provider_groups.is_empty());
        assert!(config.unmapped_blocks.is_empty());
    }

    #[test]
    fn parse_invalid_json_returns_error() {
        let json = "{ invalid json }";
        let result = TerraformParser::parse(json);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ParseError::Json(_)));
    }

    #[test]
    fn parse_plan_with_single_provider_and_resource() {
        let json = r#"{
            "format_version": "1.0",
            "terraform_version": "1.5.0",
            "configuration": {
                "provider_config": {
                    "aws": {
                        "name": "aws",
                        "expressions": {
                            "assume_role": [{
                                "role_arn": {
                                    "constant_value": "arn:aws:iam::123456789012:role/MainRole"
                                }
                            }]
                        }
                    }
                },
                "root_module": {
                    "resources": [
                        {
                            "address": "aws_s3_bucket.main",
                            "mode": "managed",
                            "type": "aws_s3_bucket",
                            "name": "main",
                            "provider_config_key": "aws",
                            "expressions": {}
                        }
                    ],
                    "module_calls": {}
                }
            }
        }"#;

        let config = TerraformParser::parse(json).unwrap();

        assert_eq!(config.provider_groups.len(), 1);
        assert!(config.unmapped_blocks.is_empty());

        // Output name is derived from alias (None -> DefaultDeployer), not role ARN
        let group = config.provider_groups.get("DefaultDeployer").unwrap();
        assert_eq!(group.blocks.len(), 1);
        assert_eq!(group.blocks[0].type_name, "aws_s3_bucket");
        assert_eq!(group.blocks[0].address, "aws_s3_bucket.main");
        assert_eq!(group.blocks[0].block_type, BlockType::Resource);
    }

    #[test]
    fn parse_plan_with_multiple_providers() {
        let json = r#"{
            "format_version": "1.0",
            "terraform_version": "1.5.0",
            "configuration": {
                "provider_config": {
                    "aws": {
                        "name": "aws",
                        "expressions": {
                            "assume_role": [{
                                "role_arn": { "constant_value": "arn:aws:iam::123456789012:role/MainRole" }
                            }]
                        }
                    },
                    "aws.secondary": {
                        "name": "aws",
                        "alias": "secondary",
                        "expressions": {
                            "assume_role": [{
                                "role_arn": { "constant_value": "arn:aws:iam::987654321012:role/SecondaryRole" }
                            }]
                        }
                    }
                },
                "root_module": {
                    "resources": [
                        {
                            "address": "aws_s3_bucket.main",
                            "mode": "managed",
                            "type": "aws_s3_bucket",
                            "name": "main",
                            "provider_config_key": "aws",
                            "expressions": {}
                        },
                        {
                            "address": "aws_s3_bucket.secondary",
                            "mode": "managed",
                            "type": "aws_s3_bucket",
                            "name": "secondary",
                            "provider_config_key": "aws.secondary",
                            "expressions": {}
                        }
                    ],
                    "module_calls": {}
                }
            }
        }"#;

        let config = TerraformParser::parse(json).unwrap();

        // Output names derived from aliases: None -> DefaultDeployer, "secondary" -> SecondaryDeployer
        assert_eq!(config.provider_groups.len(), 2);
        assert!(config.provider_groups.contains_key("DefaultDeployer"));
        assert!(config.provider_groups.contains_key("SecondaryDeployer"));

        let main_group = config.provider_groups.get("DefaultDeployer").unwrap();
        assert_eq!(main_group.blocks.len(), 1);
        assert_eq!(main_group.blocks[0].name, "main");

        let secondary_group = config.provider_groups.get("SecondaryDeployer").unwrap();
        assert_eq!(secondary_group.blocks.len(), 1);
        assert_eq!(secondary_group.blocks[0].name, "secondary");
    }

    #[test]
    fn parse_plan_with_module() {
        let json = r#"{
            "format_version": "1.0",
            "terraform_version": "1.5.0",
            "configuration": {
                "provider_config": {
                    "aws": {
                        "name": "aws",
                        "expressions": {
                            "assume_role": [{
                                "role_arn": { "constant_value": "arn:aws:iam::123456789012:role/NetworkDeployer" }
                            }]
                        }
                    }
                },
                "root_module": {
                    "resources": [],
                    "module_calls": {
                        "vpc": {
                            "source": "./modules/vpc",
                            "module": {
                                "resources": [
                                    {
                                        "address": "aws_vpc.this",
                                        "mode": "managed",
                                        "type": "aws_vpc",
                                        "name": "this",
                                        "provider_config_key": "aws",
                                        "expressions": {}
                                    }
                                ],
                                "module_calls": {}
                            }
                        }
                    }
                }
            }
        }"#;

        let config = TerraformParser::parse(json).unwrap();

        // Output name derived from alias (None -> DefaultDeployer)
        let group = config.provider_groups.get("DefaultDeployer").unwrap();
        assert!(group.blocks.iter().any(|b| b.type_name == "aws_vpc"));
        assert!(
            group
                .blocks
                .iter()
                .any(|b| b.address == "module.vpc.aws_vpc.this")
        );
    }

    #[test]
    fn parse_plan_with_nested_modules() {
        let json = r#"{
            "format_version": "1.0",
            "terraform_version": "1.5.0",
            "configuration": {
                "provider_config": {
                    "aws": {
                        "name": "aws",
                        "expressions": {}
                    }
                },
                "root_module": {
                    "resources": [],
                    "module_calls": {
                        "network": {
                            "source": "./modules/network",
                            "module": {
                                "resources": [],
                                "module_calls": {
                                    "vpc": {
                                        "source": "./vpc",
                                        "module": {
                                            "resources": [
                                                {
                                                    "address": "aws_vpc.main",
                                                    "mode": "managed",
                                                    "type": "aws_vpc",
                                                    "name": "main",
                                                    "provider_config_key": "aws",
                                                    "expressions": {}
                                                }
                                            ],
                                            "module_calls": {}
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }"#;

        let config = TerraformParser::parse(json).unwrap();

        let group = config.provider_groups.get("DefaultDeployer").unwrap();
        assert!(
            group
                .blocks
                .iter()
                .any(|b| b.address == "module.network.module.vpc.aws_vpc.main")
        );
    }

    #[test]
    fn parse_plan_with_data_source() {
        let json = r#"{
            "format_version": "1.0",
            "terraform_version": "1.5.0",
            "configuration": {
                "provider_config": {
                    "aws": {
                        "name": "aws",
                        "expressions": {}
                    }
                },
                "root_module": {
                    "resources": [
                        {
                            "address": "data.aws_availability_zones.available",
                            "mode": "data",
                            "type": "aws_availability_zones",
                            "name": "available",
                            "provider_config_key": "aws",
                            "expressions": {}
                        }
                    ],
                    "module_calls": {}
                }
            }
        }"#;

        let config = TerraformParser::parse(json).unwrap();

        let group = config.provider_groups.get("DefaultDeployer").unwrap();
        assert_eq!(group.blocks.len(), 1);
        assert_eq!(group.blocks[0].block_type, BlockType::Data);
        assert_eq!(group.blocks[0].type_name, "aws_availability_zones");
    }

    #[test]
    fn parse_plan_with_non_aws_resources_ignored() {
        let json = r#"{
            "format_version": "1.0",
            "terraform_version": "1.5.0",
            "configuration": {
                "provider_config": {
                    "aws": {
                        "name": "aws",
                        "expressions": {}
                    },
                    "google": {
                        "name": "google",
                        "expressions": {}
                    }
                },
                "root_module": {
                    "resources": [
                        {
                            "address": "aws_s3_bucket.main",
                            "mode": "managed",
                            "type": "aws_s3_bucket",
                            "name": "main",
                            "provider_config_key": "aws",
                            "expressions": {}
                        },
                        {
                            "address": "google_storage_bucket.main",
                            "mode": "managed",
                            "type": "google_storage_bucket",
                            "name": "main",
                            "provider_config_key": "google",
                            "expressions": {}
                        }
                    ],
                    "module_calls": {}
                }
            }
        }"#;

        let config = TerraformParser::parse(json).unwrap();

        // Only AWS resources should be in groups
        let group = config.provider_groups.get("DefaultDeployer").unwrap();
        assert_eq!(group.blocks.len(), 1);
        assert_eq!(group.blocks[0].type_name, "aws_s3_bucket");
    }

    #[test]
    fn parse_plan_provider_without_assume_role_uses_default() {
        let json = r#"{
            "format_version": "1.0",
            "terraform_version": "1.5.0",
            "configuration": {
                "provider_config": {
                    "aws": {
                        "name": "aws",
                        "expressions": {
                            "region": { "constant_value": "us-east-1" }
                        }
                    }
                },
                "root_module": {
                    "resources": [
                        {
                            "address": "aws_s3_bucket.main",
                            "mode": "managed",
                            "type": "aws_s3_bucket",
                            "name": "main",
                            "provider_config_key": "aws",
                            "expressions": {}
                        }
                    ],
                    "module_calls": {}
                }
            }
        }"#;

        let config = TerraformParser::parse(json).unwrap();

        assert!(config.provider_groups.contains_key("DefaultDeployer"));
        let group = config.provider_groups.get("DefaultDeployer").unwrap();
        assert_eq!(group.role_arn, None);
        assert_eq!(group.blocks.len(), 1);
    }

    #[test]
    fn parse_plan_resource_with_attributes() {
        let json = r#"{
            "format_version": "1.0",
            "terraform_version": "1.5.0",
            "configuration": {
                "provider_config": {
                    "aws": {
                        "name": "aws",
                        "expressions": {}
                    }
                },
                "root_module": {
                    "resources": [
                        {
                            "address": "aws_route53_zone.private",
                            "mode": "managed",
                            "type": "aws_route53_zone",
                            "name": "private",
                            "provider_config_key": "aws",
                            "expressions": {
                                "name": { "constant_value": "example.com" },
                                "vpc": [{
                                    "vpc_id": { "constant_value": "vpc-123" }
                                }],
                                "tags": { "constant_value": { "Env": "test" } }
                            }
                        }
                    ],
                    "module_calls": {}
                }
            }
        }"#;

        let config = TerraformParser::parse(json).unwrap();

        let group = config.provider_groups.get("DefaultDeployer").unwrap();
        let block = &group.blocks[0];

        assert!(block.present_attributes.contains(&vec!["name".to_string()]));
        assert!(block.present_attributes.contains(&vec!["vpc".to_string()]));
        assert!(
            block
                .present_attributes
                .contains(&vec!["vpc".to_string(), "vpc_id".to_string()])
        );
        assert!(block.present_attributes.contains(&vec!["tags".to_string()]));
    }

    #[test]
    fn parse_plan_same_role_different_aliases_grouped_together() {
        let json = r#"{
            "format_version": "1.0",
            "terraform_version": "1.5.0",
            "configuration": {
                "provider_config": {
                    "aws": {
                        "name": "aws",
                        "expressions": {
                            "assume_role": [{
                                "role_arn": { "constant_value": "arn:aws:iam::123456789012:role/NetworkDeployer" }
                            }],
                            "region": { "constant_value": "us-east-1" }
                        }
                    },
                    "aws.west": {
                        "name": "aws",
                        "alias": "west",
                        "expressions": {
                            "assume_role": [{
                                "role_arn": { "constant_value": "arn:aws:iam::123456789012:role/NetworkDeployer" }
                            }],
                            "region": { "constant_value": "us-west-2" }
                        }
                    }
                },
                "root_module": {
                    "resources": [
                        {
                            "address": "aws_s3_bucket.east",
                            "mode": "managed",
                            "type": "aws_s3_bucket",
                            "name": "east",
                            "provider_config_key": "aws",
                            "expressions": {}
                        },
                        {
                            "address": "aws_s3_bucket.west",
                            "mode": "managed",
                            "type": "aws_s3_bucket",
                            "name": "west",
                            "provider_config_key": "aws.west",
                            "expressions": {}
                        }
                    ],
                    "module_calls": {}
                }
            }
        }"#;

        let config = TerraformParser::parse(json).unwrap();

        // Both resources should be in the same group since they use the same role ARN
        // Output name is derived from the first alias alphabetically (None < "west")
        assert_eq!(config.provider_groups.len(), 1);
        let group = config.provider_groups.get("DefaultDeployer").unwrap();
        assert_eq!(group.blocks.len(), 2);
    }

    #[test]
    fn parse_plan_block_uses_default_provider_when_specific_not_found() {
        let json = r#"{
            "format_version": "1.0",
            "terraform_version": "1.5.0",
            "configuration": {
                "provider_config": {
                    "aws": {
                        "name": "aws",
                        "expressions": {
                            "assume_role": [{
                                "role_arn": { "constant_value": "arn:aws:iam::123456789012:role/MainRole" }
                            }]
                        }
                    }
                },
                "root_module": {
                    "resources": [
                        {
                            "address": "aws_s3_bucket.main",
                            "mode": "managed",
                            "type": "aws_s3_bucket",
                            "name": "main",
                            "provider_config_key": "module.something:aws",
                            "expressions": {}
                        }
                    ],
                    "module_calls": {}
                }
            }
        }"#;

        let config = TerraformParser::parse(json).unwrap();

        // Block should fall back to the default provider's output name (derived from alias)
        assert_eq!(config.provider_groups.len(), 1);
        let group = config.provider_groups.get("DefaultDeployer").unwrap();
        assert_eq!(group.blocks.len(), 1);
    }
}
