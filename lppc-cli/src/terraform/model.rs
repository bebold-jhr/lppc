use std::collections::{HashMap, HashSet};

/// Represents provider mappings for a module call.
///
/// In HCL: `providers = { aws.local = aws.parent }`
/// Maps module-local provider keys to parent provider keys.
#[derive(Debug, Clone, Default)]
pub struct ProviderMappings {
    /// Map from module-local key to parent key
    /// e.g., "aws" -> "aws.test" or "aws.primary" -> "aws.us_east"
    mappings: HashMap<String, String>,
}

impl ProviderMappings {
    /// Creates a new ProviderMappings with the given mappings.
    pub fn new(mappings: HashMap<String, String>) -> Self {
        Self { mappings }
    }

    /// Resolves a module-local provider key to the parent's provider key.
    /// Returns the original key if no mapping exists (implicit inheritance).
    pub fn resolve<'a>(&'a self, local_key: &'a str) -> &'a str {
        self.mappings
            .get(local_key)
            .map(|s| s.as_str())
            .unwrap_or(local_key)
    }

    /// Returns true if this module has explicit provider mappings.
    pub fn has_mappings(&self) -> bool {
        !self.mappings.is_empty()
    }

    /// Inserts a mapping from local key to parent key.
    pub fn insert(&mut self, local_key: String, parent_key: String) {
        self.mappings.insert(local_key, parent_key);
    }

    /// Returns an iterator over the mappings.
    pub fn iter(&self) -> impl Iterator<Item = (&String, &String)> {
        self.mappings.iter()
    }
}

/// Context for parsing a module, including provider mappings from ancestors.
#[derive(Debug, Clone)]
pub struct ModuleContext {
    /// Address prefix for resources (e.g., "module.infra.module.networking")
    pub address_prefix: String,

    /// Cumulative provider key mapping from this module's context to root.
    /// Maps module-local keys to root provider keys.
    provider_key_to_root: HashMap<String, String>,
}

impl ModuleContext {
    /// Creates context for the root module (no mappings needed).
    pub fn root() -> Self {
        Self {
            address_prefix: String::new(),
            provider_key_to_root: HashMap::new(),
        }
    }

    /// Creates context for a child module with the given provider mappings.
    pub fn child(&self, module_name: &str, mappings: &ProviderMappings) -> Self {
        let address_prefix = if self.address_prefix.is_empty() {
            format!("module.{}", module_name)
        } else {
            format!("{}.module.{}", self.address_prefix, module_name)
        };

        // Build cumulative mapping: local_key -> root_key
        let mut provider_key_to_root = HashMap::new();

        for (local_key, parent_key) in mappings.iter() {
            // Resolve parent_key through our own mappings to get root key
            let root_key = self.resolve_to_root(parent_key);
            provider_key_to_root.insert(local_key.clone(), root_key);
        }

        Self {
            address_prefix,
            provider_key_to_root,
        }
    }

    /// Resolves a local provider key to the root provider key.
    pub fn resolve_to_root(&self, local_key: &str) -> String {
        self.provider_key_to_root
            .get(local_key)
            .cloned()
            .unwrap_or_else(|| local_key.to_string())
    }
}

/// Represents a parsed Terraform configuration
#[derive(Debug)]
pub struct TerraformConfig {
    /// Map of role identifier to the set of terraform blocks using that role
    /// Key: Output name (e.g., "123456789012NetworkDeployer")
    /// Value: ProviderGroup containing all blocks for that role
    pub provider_groups: HashMap<String, ProviderGroup>,

    /// Blocks that couldn't be mapped to a provider (for warnings)
    pub unmapped_blocks: Vec<TerraformBlock>,
}

/// A group of blocks that share the same assumed role
#[derive(Debug)]
pub struct ProviderGroup {
    /// The output name for this group (derived from role ARN)
    pub output_name: String,

    /// The role ARN (for reference)
    pub role_arn: Option<String>,

    /// All terraform blocks using this provider/role
    pub blocks: Vec<TerraformBlock>,
}

/// Represents a single terraform block (resource, data, ephemeral, or action)
#[derive(Debug, Clone)]
pub struct TerraformBlock {
    /// Block type: "resource", "data", "ephemeral", "action"
    pub block_type: BlockType,

    /// Type name, e.g., "aws_s3_bucket", "aws_availability_zones"
    pub type_name: String,

    /// Resource name/label, e.g., "this", "main"
    pub name: String,

    /// Provider config key (e.g., "aws", "aws.secondary")
    pub provider_config_key: String,

    /// Nested attributes present in this block (for optional permission mapping)
    /// Represented as paths, e.g., [["vpc", "vpc_id"], ["tags"]]
    pub present_attributes: HashSet<Vec<String>>,

    /// Full address (e.g., "module.vpc.aws_subnet.main")
    pub address: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BlockType {
    Resource,
    Data,
    Ephemeral,
    Action,
}

impl BlockType {
    pub fn as_str(&self) -> &'static str {
        match self {
            BlockType::Resource => "resource",
            BlockType::Data => "data",
            BlockType::Ephemeral => "ephemeral",
            BlockType::Action => "action",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_type_as_str_returns_correct_string() {
        assert_eq!(BlockType::Resource.as_str(), "resource");
        assert_eq!(BlockType::Data.as_str(), "data");
        assert_eq!(BlockType::Ephemeral.as_str(), "ephemeral");
        assert_eq!(BlockType::Action.as_str(), "action");
    }

    #[test]
    fn block_type_equality_works() {
        assert_eq!(BlockType::Resource, BlockType::Resource);
        assert_ne!(BlockType::Resource, BlockType::Data);
    }

    #[test]
    fn terraform_block_can_be_cloned() {
        let block = TerraformBlock {
            block_type: BlockType::Resource,
            type_name: "aws_s3_bucket".to_string(),
            name: "example".to_string(),
            provider_config_key: "aws".to_string(),
            present_attributes: HashSet::new(),
            address: "aws_s3_bucket.example".to_string(),
        };

        let cloned = block.clone();
        assert_eq!(cloned.type_name, block.type_name);
        assert_eq!(cloned.address, block.address);
    }

    #[test]
    fn provider_mappings_resolve_returns_mapped_key() {
        let mut mappings = ProviderMappings::default();
        mappings.insert("aws".to_string(), "aws.production".to_string());

        assert_eq!(mappings.resolve("aws"), "aws.production");
    }

    #[test]
    fn provider_mappings_resolve_returns_original_for_unmapped() {
        let mut mappings = ProviderMappings::default();
        mappings.insert("aws".to_string(), "aws.production".to_string());

        // Unmapped key passes through unchanged
        assert_eq!(mappings.resolve("aws.other"), "aws.other");
    }

    #[test]
    fn provider_mappings_has_mappings_returns_correct_value() {
        let empty = ProviderMappings::default();
        assert!(!empty.has_mappings());

        let mut with_mapping = ProviderMappings::default();
        with_mapping.insert("aws".to_string(), "aws.test".to_string());
        assert!(with_mapping.has_mappings());
    }

    #[test]
    fn module_context_root_has_empty_prefix() {
        let root = ModuleContext::root();
        assert_eq!(root.address_prefix, "");
    }

    #[test]
    fn module_context_child_builds_address_prefix() {
        let root = ModuleContext::root();
        let child = root.child("infra", &ProviderMappings::default());
        assert_eq!(child.address_prefix, "module.infra");

        let grandchild = child.child("networking", &ProviderMappings::default());
        assert_eq!(grandchild.address_prefix, "module.infra.module.networking");
    }

    #[test]
    fn module_context_resolve_to_root_single_level() {
        let mut mappings = ProviderMappings::default();
        mappings.insert("aws".to_string(), "aws.prod".to_string());

        let root = ModuleContext::root();
        let child = root.child("mymodule", &mappings);

        assert_eq!(child.resolve_to_root("aws"), "aws.prod");
        assert_eq!(child.resolve_to_root("aws.other"), "aws.other"); // Unmapped
    }

    #[test]
    fn module_context_resolve_to_root_nested_modules() {
        // Root has aws.prod
        // Level1 maps: aws -> aws.prod
        // Level2 maps: aws.inner -> aws

        let root = ModuleContext::root();

        let mut level1_mappings = ProviderMappings::default();
        level1_mappings.insert("aws".to_string(), "aws.prod".to_string());
        let level1 = root.child("level1", &level1_mappings);

        let mut level2_mappings = ProviderMappings::default();
        level2_mappings.insert("aws.inner".to_string(), "aws".to_string());
        let level2 = level1.child("level2", &level2_mappings);

        // level2's aws.inner -> level1's aws -> root's aws.prod
        assert_eq!(level2.resolve_to_root("aws.inner"), "aws.prod");
    }

    #[test]
    fn module_context_multiple_mappings() {
        let mut mappings = ProviderMappings::default();
        mappings.insert("aws.primary".to_string(), "aws.us_east".to_string());
        mappings.insert("aws.secondary".to_string(), "aws.eu_west".to_string());
        mappings.insert("aws".to_string(), "aws.default_region".to_string());

        let root = ModuleContext::root();
        let child = root.child("multi", &mappings);

        assert_eq!(child.resolve_to_root("aws.primary"), "aws.us_east");
        assert_eq!(child.resolve_to_root("aws.secondary"), "aws.eu_west");
        assert_eq!(child.resolve_to_root("aws"), "aws.default_region");
    }
}
