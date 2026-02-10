use serde::Deserialize;
use std::collections::HashMap;

use super::model::BlockType;

/// Root structure of terraform show -json output
#[derive(Debug, Deserialize)]
pub struct TerraformPlan {
    #[allow(dead_code)]
    pub format_version: String,
    pub terraform_version: String,
    pub configuration: Configuration,
}

/// Configuration section containing provider configs and root module
#[derive(Debug, Deserialize)]
pub struct Configuration {
    #[serde(default)]
    pub provider_config: HashMap<String, ProviderConfig>,
    pub root_module: Module,
}

/// Provider configuration
#[derive(Debug, Deserialize)]
pub struct ProviderConfig {
    pub name: String,
    #[serde(default)]
    pub alias: Option<String>,
    #[allow(dead_code)]
    #[serde(default)]
    pub full_name: Option<String>,
    #[serde(default)]
    pub expressions: HashMap<String, serde_json::Value>,
}

impl ProviderConfig {
    /// Extracts role_arn from assume_role expression
    pub fn get_role_arn(&self) -> Option<String> {
        // assume_role is an array of objects, each potentially containing role_arn
        let assume_role = self.expressions.get("assume_role")?;

        if let Some(arr) = assume_role.as_array() {
            // Find the last assume_role block (for role chaining)
            for item in arr.iter().rev() {
                if let Some(role_arn_expr) = item.get("role_arn") {
                    if let Some(constant) = role_arn_expr.get("constant_value") {
                        if let Some(arn) = constant.as_str() {
                            return Some(arn.to_string());
                        }
                    }
                    // Also check for references (we can't resolve these, but note them)
                    if role_arn_expr.get("references").is_some() {
                        // Role ARN is a reference, not a constant - we can't resolve it
                        return None;
                    }
                }
            }
        }

        None
    }

    /// Extracts region from expressions
    pub fn get_region(&self) -> Option<String> {
        let region = self.expressions.get("region")?;
        region
            .get("constant_value")?
            .as_str()
            .map(|s| s.to_string())
    }
}

/// Module structure (root or child)
#[derive(Debug, Deserialize)]
pub struct Module {
    #[serde(default)]
    pub resources: Vec<ResourceConfig>,
    #[serde(default)]
    pub module_calls: HashMap<String, ModuleCall>,
}

/// Module call (reference to child module)
#[derive(Debug, Deserialize)]
pub struct ModuleCall {
    #[allow(dead_code)]
    pub source: String,
    #[serde(default)]
    pub module: Option<Module>,
}

/// Resource or data source configuration
#[derive(Debug, Deserialize)]
pub struct ResourceConfig {
    pub address: String,
    pub mode: String, // "managed" for resources, "data" for data sources
    #[serde(rename = "type")]
    pub resource_type: String,
    pub name: String,
    pub provider_config_key: String,
    #[serde(default)]
    pub expressions: HashMap<String, serde_json::Value>,
}

impl ResourceConfig {
    /// Checks if this is an AWS resource
    pub fn is_aws(&self) -> bool {
        self.resource_type.starts_with("aws_")
    }

    /// Gets the block type based on mode
    pub fn block_type(&self) -> Option<BlockType> {
        match self.mode.as_str() {
            "managed" => Some(BlockType::Resource),
            "data" => Some(BlockType::Data),
            // Note: ephemeral and action may have different modes in future terraform versions
            _ => None,
        }
    }

    /// Collects all attribute paths present in expressions
    /// This is used for optional permission mapping
    pub fn collect_attribute_paths(&self) -> std::collections::HashSet<Vec<String>> {
        let mut paths = std::collections::HashSet::new();
        Self::collect_paths_recursive(&self.expressions, &mut Vec::new(), &mut paths);
        paths
    }

    fn collect_paths_recursive(
        value: &HashMap<String, serde_json::Value>,
        current_path: &mut Vec<String>,
        paths: &mut std::collections::HashSet<Vec<String>>,
    ) {
        for (key, val) in value {
            // Skip internal terraform keys
            if key.starts_with("__") {
                continue;
            }

            let mut new_path = current_path.clone();
            new_path.push(key.clone());
            paths.insert(new_path.clone());

            // Recurse into nested objects (block expressions)
            // In terraform JSON, nested blocks appear as arrays of objects
            if let Some(arr) = val.as_array() {
                for item in arr {
                    if let Some(obj) = item.as_object() {
                        let nested: HashMap<String, serde_json::Value> =
                            obj.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
                        Self::collect_paths_recursive(&nested, &mut new_path, paths);
                    }
                }
            } else if let Some(obj) = val.as_object() {
                // Check for nested expressions within constant_value or similar
                if let Some(nested_obj) = obj.get("constant_value").and_then(|v| v.as_object()) {
                    let nested: HashMap<String, serde_json::Value> = nested_obj
                        .iter()
                        .map(|(k, v)| (k.clone(), v.clone()))
                        .collect();
                    Self::collect_paths_recursive(&nested, &mut new_path, paths);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_provider_config_with_assume_role() {
        let json = r#"{
            "name": "aws",
            "expressions": {
                "assume_role": [{
                    "role_arn": {
                        "constant_value": "arn:aws:iam::111111111111:role/TestRole"
                    }
                }],
                "region": {
                    "constant_value": "us-west-2"
                }
            }
        }"#;

        let config: ProviderConfig = serde_json::from_str(json).unwrap();
        assert_eq!(
            config.get_role_arn(),
            Some("arn:aws:iam::111111111111:role/TestRole".to_string())
        );
        assert_eq!(config.get_region(), Some("us-west-2".to_string()));
    }

    #[test]
    fn parse_provider_config_without_assume_role() {
        let json = r#"{
            "name": "aws",
            "expressions": {
                "region": {
                    "constant_value": "eu-central-1"
                }
            }
        }"#;

        let config: ProviderConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.get_role_arn(), None);
        assert_eq!(config.get_region(), Some("eu-central-1".to_string()));
    }

    #[test]
    fn parse_provider_config_with_role_arn_reference() {
        let json = r#"{
            "name": "aws",
            "expressions": {
                "assume_role": [{
                    "role_arn": {
                        "references": ["var.role_arn"]
                    }
                }]
            }
        }"#;

        let config: ProviderConfig = serde_json::from_str(json).unwrap();
        // References cannot be resolved, so this returns None
        assert_eq!(config.get_role_arn(), None);
    }

    #[test]
    fn parse_provider_config_with_role_chaining() {
        // When there are multiple assume_role blocks (role chaining),
        // the last one determines the final role
        let json = r#"{
            "name": "aws",
            "expressions": {
                "assume_role": [
                    {
                        "role_arn": {
                            "constant_value": "arn:aws:iam::111111111111:role/FirstRole"
                        }
                    },
                    {
                        "role_arn": {
                            "constant_value": "arn:aws:iam::222222222222:role/SecondRole"
                        }
                    }
                ]
            }
        }"#;

        let config: ProviderConfig = serde_json::from_str(json).unwrap();
        // Should return the last role in the chain
        assert_eq!(
            config.get_role_arn(),
            Some("arn:aws:iam::222222222222:role/SecondRole".to_string())
        );
    }

    #[test]
    fn parse_provider_config_with_alias() {
        let json = r#"{
            "name": "aws",
            "alias": "secondary",
            "full_name": "registry.terraform.io/hashicorp/aws",
            "expressions": {}
        }"#;

        let config: ProviderConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.name, "aws");
        assert_eq!(config.alias, Some("secondary".to_string()));
        assert_eq!(
            config.full_name,
            Some("registry.terraform.io/hashicorp/aws".to_string())
        );
    }

    #[test]
    fn resource_config_is_aws() {
        let aws_resource: ResourceConfig = serde_json::from_str(
            r#"{
                "address": "aws_s3_bucket.example",
                "mode": "managed",
                "type": "aws_s3_bucket",
                "name": "example",
                "provider_config_key": "aws"
            }"#,
        )
        .unwrap();

        let google_resource: ResourceConfig = serde_json::from_str(
            r#"{
                "address": "google_storage_bucket.example",
                "mode": "managed",
                "type": "google_storage_bucket",
                "name": "example",
                "provider_config_key": "google"
            }"#,
        )
        .unwrap();

        assert!(aws_resource.is_aws());
        assert!(!google_resource.is_aws());
    }

    #[test]
    fn resource_config_block_type() {
        let managed: ResourceConfig = serde_json::from_str(
            r#"{
                "address": "aws_s3_bucket.example",
                "mode": "managed",
                "type": "aws_s3_bucket",
                "name": "example",
                "provider_config_key": "aws"
            }"#,
        )
        .unwrap();

        let data: ResourceConfig = serde_json::from_str(
            r#"{
                "address": "data.aws_availability_zones.available",
                "mode": "data",
                "type": "aws_availability_zones",
                "name": "available",
                "provider_config_key": "aws"
            }"#,
        )
        .unwrap();

        assert_eq!(managed.block_type(), Some(BlockType::Resource));
        assert_eq!(data.block_type(), Some(BlockType::Data));
    }

    #[test]
    fn resource_config_unknown_mode_returns_none() {
        let unknown: ResourceConfig = serde_json::from_str(
            r#"{
                "address": "unknown_resource.example",
                "mode": "unknown_mode",
                "type": "unknown_resource",
                "name": "example",
                "provider_config_key": "aws"
            }"#,
        )
        .unwrap();

        assert_eq!(unknown.block_type(), None);
    }

    #[test]
    fn collect_attribute_paths_simple() {
        let json = r#"{
            "address": "aws_s3_bucket.example",
            "mode": "managed",
            "type": "aws_s3_bucket",
            "name": "example",
            "provider_config_key": "aws",
            "expressions": {
                "bucket": { "constant_value": "my-bucket" },
                "acl": { "constant_value": "private" }
            }
        }"#;

        let resource: ResourceConfig = serde_json::from_str(json).unwrap();
        let paths = resource.collect_attribute_paths();

        assert!(paths.contains(&vec!["bucket".to_string()]));
        assert!(paths.contains(&vec!["acl".to_string()]));
        assert_eq!(paths.len(), 2);
    }

    #[test]
    fn collect_attribute_paths_with_nested_blocks() {
        let json = r#"{
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
                "tags": { "constant_value": { "Environment": "test" } }
            }
        }"#;

        let resource: ResourceConfig = serde_json::from_str(json).unwrap();
        let paths = resource.collect_attribute_paths();

        assert!(paths.contains(&vec!["name".to_string()]));
        assert!(paths.contains(&vec!["vpc".to_string()]));
        assert!(paths.contains(&vec!["vpc".to_string(), "vpc_id".to_string()]));
        assert!(paths.contains(&vec!["tags".to_string()]));
    }

    #[test]
    fn collect_attribute_paths_skips_internal_keys() {
        let json = r#"{
            "address": "aws_s3_bucket.example",
            "mode": "managed",
            "type": "aws_s3_bucket",
            "name": "example",
            "provider_config_key": "aws",
            "expressions": {
                "__internal": { "constant_value": "should be skipped" },
                "bucket": { "constant_value": "my-bucket" }
            }
        }"#;

        let resource: ResourceConfig = serde_json::from_str(json).unwrap();
        let paths = resource.collect_attribute_paths();

        assert!(paths.contains(&vec!["bucket".to_string()]));
        assert!(!paths.contains(&vec!["__internal".to_string()]));
        assert_eq!(paths.len(), 1);
    }

    #[test]
    fn parse_terraform_plan_minimal() {
        let json = r#"{
            "format_version": "1.0",
            "terraform_version": "1.5.0",
            "configuration": {
                "root_module": {
                    "resources": []
                }
            }
        }"#;

        let plan: TerraformPlan = serde_json::from_str(json).unwrap();
        assert_eq!(plan.format_version, "1.0");
        assert_eq!(plan.terraform_version, "1.5.0");
        assert!(plan.configuration.provider_config.is_empty());
        assert!(plan.configuration.root_module.resources.is_empty());
    }

    #[test]
    fn parse_module_with_child_modules() {
        let json = r#"{
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
        }"#;

        let module: Module = serde_json::from_str(json).unwrap();
        assert!(module.resources.is_empty());
        assert_eq!(module.module_calls.len(), 1);

        let vpc_call = module.module_calls.get("vpc").unwrap();
        assert_eq!(vpc_call.source, "./modules/vpc");

        let vpc_module = vpc_call.module.as_ref().unwrap();
        assert_eq!(vpc_module.resources.len(), 1);
        assert_eq!(vpc_module.resources[0].resource_type, "aws_vpc");
    }
}
