use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use hcl::{Block, Body, Expression, Traversal, TraversalOperator};
use log::{debug, warn};
use thiserror::Error;
use walkdir::WalkDir;

use super::model::{
    BlockType, ModuleContext, ProviderGroup, ProviderMappings, TerraformBlock, TerraformConfig,
};
use super::module_detector::ModulesManifest;
use super::provider::AwsProvider;

/// Maximum size for .tf files (10 MB) - prevents memory exhaustion from extremely large files.
const MAX_TF_FILE_SIZE: u64 = 10 * 1024 * 1024;

/// Parses HCL files directly without running terraform plan.
///
/// This parser extracts providers, resources, and data sources from .tf files
/// and groups them by provider role_arn using the alias-based naming strategy.
pub struct HclParser;

impl HclParser {
    /// Parses all .tf files in a directory and returns the configuration.
    ///
    /// This method parses the root module and recursively parses any submodules
    /// found in `.terraform/modules/` after `terraform init`. Provider mappings
    /// from module calls are properly resolved so resources in submodules are
    /// assigned to the correct provider group.
    pub fn parse_directory(dir: &Path) -> Result<TerraformConfig, HclParseError> {
        // Load modules manifest if available (after terraform init)
        let manifest = ModulesManifest::load(dir);

        // Log discovered remote modules
        if let Some(ref m) = manifest {
            Self::log_discovered_modules(m);
        }

        // Parse recursively starting from root module
        let root_context = ModuleContext::root();
        let (all_providers, all_blocks) =
            Self::extract_blocks_recursive(dir, &root_context, "", manifest.as_ref())?;

        debug!(
            "Parsed {} providers and {} blocks from HCL files",
            all_providers.len(),
            all_blocks.len()
        );

        // Group by role_arn and derive names
        let provider_groups = Self::group_by_role(&all_providers, all_blocks);

        Ok(TerraformConfig {
            provider_groups,
            unmapped_blocks: Vec::new(), // HCL parsing doesn't produce unmapped blocks
        })
    }

    /// Logs discovered remote modules for verbose output.
    fn log_discovered_modules(manifest: &ModulesManifest) {
        let remote_modules = manifest.remote_modules();

        if remote_modules.is_empty() {
            debug!("No remote modules found in modules.json");
            return;
        }

        debug!(
            "Discovered {} remote module(s) from modules.json:",
            remote_modules.len()
        );

        for module in remote_modules {
            debug!(
                "  - {} ({})",
                module.key,
                module.source_type.description()
            );
        }
    }

    /// Recursively extracts blocks from a directory, tracking module context.
    ///
    /// This method:
    /// 1. Parses all .tf files in the given directory (not recursive walk)
    /// 2. Collects providers, resources, and module calls
    /// 3. For each module call, looks up the module directory in the manifest
    /// 4. Recursively parses each module with its provider context
    fn extract_blocks_recursive(
        dir: &Path,
        context: &ModuleContext,
        module_key: &str,
        manifest: Option<&ModulesManifest>,
    ) -> Result<(Vec<ParsedProvider>, Vec<TerraformBlock>), HclParseError> {
        let mut all_providers = Vec::new();
        let mut all_blocks = Vec::new();
        let mut module_calls = Vec::new();

        // Parse all .tf files in this directory (non-recursive)
        let tf_files = Self::collect_tf_files_in_dir(dir)?;

        for file_path in tf_files {
            // Skip oversized files to prevent memory exhaustion
            if let Ok(metadata) = std::fs::metadata(&file_path) {
                if metadata.len() > MAX_TF_FILE_SIZE {
                    warn!(
                        "Skipping oversized .tf file ({} bytes): {:?}",
                        metadata.len(),
                        file_path
                    );
                    continue;
                }
            }

            let content = std::fs::read_to_string(&file_path)
                .map_err(|e| HclParseError::Io(format!("{}: {}", file_path.display(), e)))?;

            let body: Body = hcl::from_str(&content)
                .map_err(|e| HclParseError::Hcl(format!("{}: {}", file_path.display(), e)))?;

            // Extract providers, blocks, and module calls
            let (providers, blocks, calls) =
                Self::extract_from_body_with_context(&body, context)?;

            // Only collect providers from root module
            if module_key.is_empty() {
                all_providers.extend(providers);
            }

            all_blocks.extend(blocks);
            module_calls.extend(calls);
        }

        // After parsing the directory, recursively parse module directories
        if let Some(manifest) = manifest {
            for call in module_calls {
                let child_key = ModulesManifest::build_child_key(module_key, &call.name);
                let child_context = context.child(&call.name, &call.provider_mappings);

                if let Some(module_dir) = manifest.find_module_dir(&child_key) {
                    // Verify the module directory exists and is a directory
                    if module_dir.is_dir() {
                        // Check if this is a remote module for enhanced logging
                        let is_remote = manifest
                            .find_entry(&child_key)
                            .map(|e| e.source_type.is_remote())
                            .unwrap_or(false);

                        debug!(
                            "Parsing module '{}' at {:?} with context {:?}",
                            child_key, module_dir, child_context.address_prefix
                        );

                        let (_, child_blocks) = Self::extract_blocks_recursive(
                            &module_dir,
                            &child_context,
                            &child_key,
                            Some(manifest),
                        )?;

                        if is_remote {
                            debug!(
                                "Extracted {} block(s) from remote module '{}'",
                                child_blocks.len(),
                                child_key
                            );
                        }

                        all_blocks.extend(child_blocks);
                    } else {
                        warn!(
                            "Module directory not found for '{}': {:?}",
                            child_key, module_dir
                        );
                    }
                } else {
                    debug!(
                        "Module '{}' not found in manifest, may be external or not initialized",
                        child_key
                    );
                }
            }
        }

        Ok((all_providers, all_blocks))
    }

    /// Collects all .tf files directly in a directory (not recursive).
    fn collect_tf_files_in_dir(dir: &Path) -> Result<Vec<PathBuf>, HclParseError> {
        let mut files = Vec::new();

        let entries = std::fs::read_dir(dir)
            .map_err(|e| HclParseError::Io(format!("{}: {}", dir.display(), e)))?;

        for entry in entries {
            let entry =
                entry.map_err(|e| HclParseError::Io(format!("{}: {}", dir.display(), e)))?;
            let path = entry.path();

            if path.is_file() && path.extension().is_some_and(|ext| ext == "tf") {
                files.push(path);
            }
        }

        Ok(files)
    }

    /// Legacy method for parsing without module context.
    /// Walks directory recursively (skipping .terraform).
    #[allow(dead_code)]
    fn parse_directory_flat(dir: &Path) -> Result<TerraformConfig, HclParseError> {
        let mut all_providers = Vec::new();
        let mut all_blocks = Vec::new();

        // Walk directory and parse all .tf files
        // - follow_links(false): Security - prevent symlink attacks
        // - Log errors instead of silently ignoring them for auditability
        for entry in WalkDir::new(dir)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| match e {
                Ok(entry) => Some(entry),
                Err(err) => {
                    warn!("Skipping directory entry: {}", err);
                    None
                }
            })
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "tf"))
        {
            // Skip .terraform directory
            if entry
                .path()
                .components()
                .any(|c| c.as_os_str() == ".terraform")
            {
                continue;
            }

            // Skip oversized files to prevent memory exhaustion
            if let Ok(metadata) = entry.metadata() {
                if metadata.len() > MAX_TF_FILE_SIZE {
                    warn!(
                        "Skipping oversized .tf file ({} bytes): {:?}",
                        metadata.len(),
                        entry.path()
                    );
                    continue;
                }
            }

            let content = std::fs::read_to_string(entry.path())
                .map_err(|e| HclParseError::Io(format!("{}: {}", entry.path().display(), e)))?;

            let body: Body = hcl::from_str(&content)
                .map_err(|e| HclParseError::Hcl(format!("{}: {}", entry.path().display(), e)))?;

            // Extract providers and blocks from this file
            let (providers, blocks) = Self::extract_from_body(&body, "")?;
            all_providers.extend(providers);
            all_blocks.extend(blocks);
        }

        debug!(
            "Parsed {} providers and {} blocks from HCL files",
            all_providers.len(),
            all_blocks.len()
        );

        // Group by role_arn and derive names
        let provider_groups = Self::group_by_role(&all_providers, all_blocks);

        Ok(TerraformConfig {
            provider_groups,
            unmapped_blocks: Vec::new(), // HCL parsing doesn't produce unmapped blocks
        })
    }

    /// Extracts providers and resource/data blocks from an HCL body.
    pub fn extract_from_body(
        body: &Body,
        address_prefix: &str,
    ) -> Result<(Vec<ParsedProvider>, Vec<TerraformBlock>), HclParseError> {
        let context = ModuleContext::root();
        let (providers, blocks, _) = Self::extract_from_body_with_context(body, &context)?;

        // Apply address prefix if provided (for backwards compatibility)
        let blocks = if address_prefix.is_empty() {
            blocks
        } else {
            blocks
                .into_iter()
                .map(|mut b| {
                    b.address = format!("{}.{}", address_prefix, b.address);
                    b
                })
                .collect()
        };

        Ok((providers, blocks))
    }

    /// Extracts providers, resource/data blocks, and module calls from an HCL body.
    ///
    /// Uses the provided ModuleContext to resolve provider keys for resources
    /// in submodules to their root provider keys.
    fn extract_from_body_with_context(
        body: &Body,
        context: &ModuleContext,
    ) -> Result<(Vec<ParsedProvider>, Vec<TerraformBlock>, Vec<ParsedModuleCall>), HclParseError>
    {
        let mut providers = Vec::new();
        let mut blocks = Vec::new();
        let mut module_calls = Vec::new();

        for block in body.blocks() {
            match block.identifier.as_str() {
                "provider" => {
                    if let Some(provider) = Self::parse_provider_block(block)? {
                        providers.push(provider);
                    }
                }
                "resource" => {
                    if let Some(tf_block) =
                        Self::parse_resource_block_with_context(block, BlockType::Resource, context)?
                    {
                        blocks.push(tf_block);
                    }
                }
                "data" => {
                    if let Some(tf_block) =
                        Self::parse_resource_block_with_context(block, BlockType::Data, context)?
                    {
                        blocks.push(tf_block);
                    }
                }
                "ephemeral" => {
                    if let Some(tf_block) =
                        Self::parse_resource_block_with_context(block, BlockType::Ephemeral, context)?
                    {
                        blocks.push(tf_block);
                    }
                }
                "action" => {
                    if let Some(tf_block) =
                        Self::parse_resource_block_with_context(block, BlockType::Action, context)?
                    {
                        blocks.push(tf_block);
                    }
                }
                "module" => {
                    // Parse module call for provider mappings
                    if let Some(module_call) = Self::parse_module_call(block) {
                        module_calls.push(module_call);
                    }
                }
                _ => {} // Ignore other block types (variable, output, locals, terraform, etc.)
            }
        }

        Ok((providers, blocks, module_calls))
    }

    /// Parses a module call block to extract name and provider mappings.
    fn parse_module_call(block: &Block) -> Option<ParsedModuleCall> {
        let name = block.labels.first()?.as_str().to_string();
        let provider_mappings = Self::parse_module_providers(block.body());

        Some(ParsedModuleCall {
            name,
            provider_mappings,
        })
    }

    /// Parses a provider block, extracting alias and role_arn.
    fn parse_provider_block(block: &Block) -> Result<Option<ParsedProvider>, HclParseError> {
        // provider "aws" { ... }
        let labels: Vec<&str> = block.labels.iter().map(|l| l.as_str()).collect();

        if labels.first() != Some(&"aws") {
            return Ok(None); // Only process AWS providers
        }

        let alias = Self::get_string_attr(block.body(), "alias");
        let role_arn = Self::get_assume_role_arn(block.body());

        let config_key = match &alias {
            Some(a) => format!("aws.{}", a),
            None => "aws".to_string(),
        };

        debug!(
            "Parsed provider: config_key={}, alias={:?}, role_arn={:?}",
            config_key, alias, role_arn
        );

        Ok(Some(ParsedProvider {
            config_key,
            alias,
            role_arn,
        }))
    }

    /// Extracts role_arn from assume_role block.
    fn get_assume_role_arn(body: &Body) -> Option<String> {
        for block in body.blocks() {
            if block.identifier.as_str() == "assume_role" {
                if let Some(role_arn) = Self::get_expression_as_string(block.body(), "role_arn") {
                    return Some(role_arn);
                }
            }
        }
        None
    }

    /// Gets a string attribute value, returning None if it contains interpolation.
    fn get_string_attr(body: &Body, name: &str) -> Option<String> {
        body.attributes()
            .find(|a| a.key.as_str() == name)
            .and_then(|a| {
                if let Expression::String(s) = &a.expr {
                    Some(s.clone())
                } else {
                    None
                }
            })
    }

    /// Gets an expression as its string representation (preserves interpolation syntax).
    fn get_expression_as_string(body: &Body, name: &str) -> Option<String> {
        body.attributes()
            .find(|a| a.key.as_str() == name)
            .map(|a| Self::expression_to_string(&a.expr))
    }

    /// Converts an HCL expression to its string representation.
    /// For templates with interpolation, returns the full template string.
    fn expression_to_string(expr: &Expression) -> String {
        match expr {
            Expression::String(s) => s.clone(),
            Expression::TemplateExpr(t) => {
                // Template expressions contain interpolation like ${var.foo}
                // We serialize them back to their original form
                format!("{}", t)
            }
            _ => format!("{:?}", expr), // Fallback for complex expressions
        }
    }

    /// Parses a resource or data block with module context for provider resolution.
    ///
    /// This method resolves the module-local provider key to the root provider key
    /// using the provided context, ensuring resources in submodules are assigned
    /// to the correct provider group.
    fn parse_resource_block_with_context(
        block: &Block,
        block_type: BlockType,
        context: &ModuleContext,
    ) -> Result<Option<TerraformBlock>, HclParseError> {
        let labels: Vec<&str> = block.labels.iter().map(|l| l.as_str()).collect();

        if labels.len() < 2 {
            return Ok(None);
        }

        let type_name = labels[0];
        let name = labels[1];

        // Only process AWS resources
        if !type_name.starts_with("aws_") {
            return Ok(None);
        }

        // Get the module-local provider key from explicit `provider` attribute or default to "aws"
        let local_provider_key =
            Self::get_provider_attr(block.body()).unwrap_or_else(|| "aws".to_string());

        // Resolve to root provider key using context
        let provider_config_key = context.resolve_to_root(&local_provider_key);

        // Collect present attributes
        let present_attributes = Self::collect_attributes(block.body());

        // Build address based on block type
        let type_prefix = match block_type {
            BlockType::Resource => format!("{}.{}", type_name, name),
            BlockType::Data => format!("data.{}.{}", type_name, name),
            BlockType::Ephemeral => format!("ephemeral.{}.{}", type_name, name),
            BlockType::Action => format!("action.{}.{}", type_name, name),
        };

        // Include module address prefix in the resource address
        let address = if context.address_prefix.is_empty() {
            type_prefix
        } else {
            format!("{}.{}", context.address_prefix, type_prefix)
        };

        Ok(Some(TerraformBlock {
            block_type,
            type_name: type_name.to_string(),
            name: name.to_string(),
            provider_config_key,
            present_attributes,
            address,
        }))
    }

    /// Gets the provider attribute from a block body.
    /// Handles both string literals (e.g., "aws.dns") and references (e.g., aws.dns).
    fn get_provider_attr(body: &Body) -> Option<String> {
        body.attributes()
            .find(|a| a.key.as_str() == "provider")
            .map(|a| match &a.expr {
                Expression::String(s) => s.clone(),
                Expression::Variable(var) => var.to_string(),
                Expression::Traversal(traversal) => Self::traversal_to_provider_key(traversal),
                _ => Self::expression_to_string(&a.expr),
            })
    }

    /// Converts an HCL Traversal to a provider key string (e.g., "aws.dns").
    ///
    /// In Terraform, provider references look like `aws.dns` which parses as a
    /// Traversal with a variable base and GetAttr operators.
    fn traversal_to_provider_key(traversal: &Traversal) -> String {
        let mut parts = Vec::new();

        // Get the base variable name
        parts.push(traversal.expr.to_string());

        // Get the attribute names from the operators
        for op in traversal.operators.iter() {
            match op {
                TraversalOperator::GetAttr(ident) => {
                    parts.push(ident.to_string());
                }
                TraversalOperator::Index(_)
                | TraversalOperator::LegacyIndex(_)
                | TraversalOperator::AttrSplat
                | TraversalOperator::FullSplat => {
                    // These operators aren't typically used in provider references
                    // but we handle them gracefully by ignoring them
                }
            }
        }

        parts.join(".")
    }

    /// Parses the providers block from a module call.
    ///
    /// In HCL:
    /// ```hcl
    /// module "example" {
    ///   providers = {
    ///     aws         = aws.production
    ///     aws.replica = aws.dr_region
    ///   }
    /// }
    /// ```
    pub fn parse_module_providers(body: &Body) -> ProviderMappings {
        let mut mappings = ProviderMappings::default();

        // Find the "providers" attribute
        let providers_attr = body.attributes().find(|a| a.key.as_str() == "providers");

        let Some(attr) = providers_attr else {
            return mappings;
        };

        // Parse the object expression
        if let Expression::Object(obj) = &attr.expr {
            for (key, value) in obj.iter() {
                let local_key = Self::object_key_to_provider_key(key);
                let parent_key = Self::expression_to_provider_key(value);

                if let (Some(local), Some(parent)) = (local_key, parent_key) {
                    mappings.insert(local, parent);
                }
            }
        }

        mappings
    }

    /// Converts an object key to a provider key string.
    /// Handles both simple identifiers (aws) and expressions/traversals (aws.replica).
    fn object_key_to_provider_key(key: &hcl::expr::ObjectKey) -> Option<String> {
        match key {
            hcl::expr::ObjectKey::Identifier(ident) => Some(ident.to_string()),
            hcl::expr::ObjectKey::Expression(expr) => Self::expression_to_provider_key(expr),
            // ObjectKey is non-exhaustive, handle future variants gracefully
            _ => None,
        }
    }

    /// Converts an expression to a provider key string.
    /// Handles variables (aws) and traversals (aws.production).
    fn expression_to_provider_key(expr: &Expression) -> Option<String> {
        match expr {
            Expression::Variable(var) => Some(var.to_string()),
            Expression::Traversal(traversal) => Some(Self::traversal_to_provider_key(traversal)),
            _ => None,
        }
    }

    /// Collects attribute names from a block body.
    fn collect_attributes(body: &Body) -> HashSet<Vec<String>> {
        let mut paths = HashSet::new();
        Self::collect_attrs_recursive(body, &mut Vec::new(), &mut paths);
        paths
    }

    fn collect_attrs_recursive(
        body: &Body,
        current_path: &mut Vec<String>,
        paths: &mut HashSet<Vec<String>>,
    ) {
        for attr in body.attributes() {
            let mut path = current_path.clone();
            path.push(attr.key.to_string());
            paths.insert(path);
        }

        for block in body.blocks() {
            let mut path = current_path.clone();
            path.push(block.identifier.to_string());
            paths.insert(path.clone());
            Self::collect_attrs_recursive(block.body(), &mut path, paths);
        }
    }

    /// Groups blocks by role_arn and derives output names.
    ///
    /// The naming strategy is:
    /// - Group providers by their `role_arn` string **as-is** (exact string match)
    /// - For each group:
    ///   - If any provider has no alias (default provider) -> "DefaultDeployer"
    ///   - Otherwise -> "{AlphabeticallyFirstAlias}Deployer"
    fn group_by_role(
        providers: &[ParsedProvider],
        blocks: Vec<TerraformBlock>,
    ) -> HashMap<String, ProviderGroup> {
        // Build role_arn -> providers map
        let mut role_to_providers: HashMap<Option<String>, Vec<&ParsedProvider>> = HashMap::new();
        for provider in providers {
            role_to_providers
                .entry(provider.role_arn.clone())
                .or_default()
                .push(provider);
        }

        // Build config_key -> role_arn map
        let mut key_to_role: HashMap<String, Option<String>> = HashMap::new();
        for provider in providers {
            key_to_role.insert(provider.config_key.clone(), provider.role_arn.clone());
        }

        // Derive output names for each role group
        let mut role_to_name: HashMap<Option<String>, String> = HashMap::new();
        for (role_arn, providers) in &role_to_providers {
            let name = Self::derive_group_name(providers);
            role_to_name.insert(role_arn.clone(), name);
        }

        // Group blocks by their provider's role
        let mut groups: HashMap<String, ProviderGroup> = HashMap::new();

        for block in blocks {
            let role_arn = key_to_role
                .get(&block.provider_config_key)
                .cloned()
                .unwrap_or(None);

            let output_name = role_to_name
                .get(&role_arn)
                .cloned()
                .unwrap_or_else(|| "DefaultDeployer".to_string());

            let group = groups
                .entry(output_name.clone())
                .or_insert_with(|| ProviderGroup {
                    output_name: output_name.clone(),
                    role_arn: role_arn.clone(),
                    blocks: Vec::new(),
                });
            group.blocks.push(block);
        }

        // Handle case where providers exist but no blocks reference them
        // (ensures we have at least one group per unique role)
        for (role_arn, name) in &role_to_name {
            if !groups.contains_key(name) {
                debug!(
                    "Provider group {} has no blocks (role_arn: {:?})",
                    name, role_arn
                );
            }
        }

        // If no providers defined but we have blocks, they go to DefaultDeployer
        if providers.is_empty() && !groups.is_empty() {
            warn!("No AWS providers defined, using DefaultDeployer for all blocks");
        }

        groups
    }

    /// Derives the output name for a group of providers sharing the same role.
    ///
    /// Rules:
    /// 1. If any provider in the group has no alias (is default) -> "DefaultDeployer"
    /// 2. Otherwise -> "{AlphabeticallyFirstAlias}Deployer" (converted to PascalCase)
    pub fn derive_group_name(providers: &[&ParsedProvider]) -> String {
        let aliases: Vec<Option<&str>> = providers.iter().map(|p| p.alias.as_deref()).collect();

        // If any provider has no alias (is the default), use "Default"
        if aliases.iter().any(|a| a.is_none()) {
            return "DefaultDeployer".to_string();
        }

        // Otherwise, use alphabetically first alias converted to PascalCase
        let mut sorted_aliases: Vec<&str> = aliases.iter().filter_map(|a| *a).collect();
        sorted_aliases.sort();

        match sorted_aliases.first() {
            Some(alias) => {
                let pascal_alias = AwsProvider::to_pascal_case(alias);
                // Case-insensitive check for "deployer" suffix
                if pascal_alias.to_lowercase().ends_with("deployer") {
                    let prefix_len = pascal_alias.len() - 8;
                    format!("{}Deployer", &pascal_alias[..prefix_len])
                } else {
                    format!("{}Deployer", pascal_alias)
                }
            }
            None => "DefaultDeployer".to_string(),
        }
    }
}

/// Parsed provider information (intermediate representation).
#[derive(Debug)]
pub struct ParsedProvider {
    /// Provider config key (e.g., "aws", "aws.dns")
    pub config_key: String,

    /// Provider alias (None for default provider)
    pub alias: Option<String>,

    /// Role ARN from assume_role block (may contain interpolation)
    pub role_arn: Option<String>,
}

/// Parsed module call information.
#[derive(Debug)]
pub struct ParsedModuleCall {
    /// Module name (label from module block)
    pub name: String,

    /// Provider mappings for this module call
    pub provider_mappings: ProviderMappings,
}

#[derive(Debug, Error)]
pub enum HclParseError {
    #[error("IO error: {0}")]
    Io(String),
    #[error("HCL parse error: {0}")]
    Hcl(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_provider_with_literal_arn() {
        let hcl = r#"
            provider "aws" {
              alias = "DnsAccount"
              assume_role {
                role_arn = "arn:aws:iam::987654321012:role/DnsLookupRole"
              }
            }
        "#;

        let body: Body = hcl::from_str(hcl).unwrap();
        let (providers, _) = HclParser::extract_from_body(&body, "").unwrap();

        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].alias, Some("DnsAccount".to_string()));
        assert_eq!(
            providers[0].role_arn,
            Some("arn:aws:iam::987654321012:role/DnsLookupRole".to_string())
        );
        assert_eq!(providers[0].config_key, "aws.DnsAccount");
    }

    #[test]
    fn parse_provider_with_interpolated_arn() {
        let hcl = r#"
            provider "aws" {
              assume_role {
                role_arn = "arn:aws:iam::${var.account_id}:role/MyRole"
              }
            }
        "#;

        let body: Body = hcl::from_str(hcl).unwrap();
        let (providers, _) = HclParser::extract_from_body(&body, "").unwrap();

        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].alias, None);
        // The interpolation syntax should be preserved
        assert!(
            providers[0]
                .role_arn
                .as_ref()
                .unwrap()
                .contains("${var.account_id}")
        );
    }

    #[test]
    fn parse_provider_without_assume_role() {
        let hcl = r#"
            provider "aws" {
              region = "us-east-1"
            }
        "#;

        let body: Body = hcl::from_str(hcl).unwrap();
        let (providers, _) = HclParser::extract_from_body(&body, "").unwrap();

        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].alias, None);
        assert_eq!(providers[0].role_arn, None);
        assert_eq!(providers[0].config_key, "aws");
    }

    #[test]
    fn ignores_non_aws_providers() {
        let hcl = r#"
            provider "google" {
              project = "my-project"
            }
        "#;

        let body: Body = hcl::from_str(hcl).unwrap();
        let (providers, _) = HclParser::extract_from_body(&body, "").unwrap();

        assert!(providers.is_empty());
    }

    #[test]
    fn derive_name_single_alias() {
        let providers = vec![ParsedProvider {
            config_key: "aws.dns".to_string(),
            alias: Some("DnsAccount".to_string()),
            role_arn: Some("arn:aws:iam::123:role/Role".to_string()),
        }];

        let refs: Vec<&ParsedProvider> = providers.iter().collect();
        let name = HclParser::derive_group_name(&refs);
        assert_eq!(name, "DnsAccountDeployer");
    }

    #[test]
    fn derive_name_with_default_provider() {
        let providers = vec![
            ParsedProvider {
                config_key: "aws".to_string(),
                alias: None,
                role_arn: Some("same_arn".to_string()),
            },
            ParsedProvider {
                config_key: "aws.west".to_string(),
                alias: Some("west".to_string()),
                role_arn: Some("same_arn".to_string()),
            },
        ];

        let refs: Vec<&ParsedProvider> = providers.iter().collect();
        let name = HclParser::derive_group_name(&refs);
        assert_eq!(name, "DefaultDeployer");
    }

    #[test]
    fn derive_name_multiple_aliases_alphabetical() {
        let providers = vec![
            ParsedProvider {
                config_key: "aws.west".to_string(),
                alias: Some("west".to_string()),
                role_arn: Some("same_arn".to_string()),
            },
            ParsedProvider {
                config_key: "aws.east".to_string(),
                alias: Some("east".to_string()),
                role_arn: Some("same_arn".to_string()),
            },
        ];

        let refs: Vec<&ParsedProvider> = providers.iter().collect();
        let name = HclParser::derive_group_name(&refs);
        assert_eq!(name, "EastDeployer"); // "east" < "west" alphabetically
    }

    #[test]
    fn derive_name_no_providers() {
        let providers: Vec<&ParsedProvider> = vec![];
        let name = HclParser::derive_group_name(&providers);
        assert_eq!(name, "DefaultDeployer");
    }

    #[test]
    fn parse_resource_block() {
        let hcl = r#"
            resource "aws_s3_bucket" "main" {
              bucket = "my-bucket"
              tags = {
                Environment = "prod"
              }
            }
        "#;

        let body: Body = hcl::from_str(hcl).unwrap();
        let (_, blocks) = HclParser::extract_from_body(&body, "").unwrap();

        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].type_name, "aws_s3_bucket");
        assert_eq!(blocks[0].name, "main");
        assert_eq!(blocks[0].block_type, BlockType::Resource);
        assert!(
            blocks[0]
                .present_attributes
                .contains(&vec!["bucket".to_string()])
        );
        assert!(
            blocks[0]
                .present_attributes
                .contains(&vec!["tags".to_string()])
        );
        assert_eq!(blocks[0].address, "aws_s3_bucket.main");
    }

    #[test]
    fn parse_data_block() {
        let hcl = r#"
            data "aws_availability_zones" "available" {
              state = "available"
            }
        "#;

        let body: Body = hcl::from_str(hcl).unwrap();
        let (_, blocks) = HclParser::extract_from_body(&body, "").unwrap();

        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].type_name, "aws_availability_zones");
        assert_eq!(blocks[0].block_type, BlockType::Data);
        assert_eq!(blocks[0].address, "data.aws_availability_zones.available");
    }

    #[test]
    fn parse_resource_with_explicit_provider() {
        let hcl = r#"
            resource "aws_route53_zone" "main" {
              provider = aws.dns
              name     = "example.com"
            }
        "#;

        let body: Body = hcl::from_str(hcl).unwrap();
        let (_, blocks) = HclParser::extract_from_body(&body, "").unwrap();

        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].provider_config_key, "aws.dns");
    }

    #[test]
    fn ignores_non_aws_resources() {
        let hcl = r#"
            resource "google_storage_bucket" "main" {
              name = "my-bucket"
            }
        "#;

        let body: Body = hcl::from_str(hcl).unwrap();
        let (_, blocks) = HclParser::extract_from_body(&body, "").unwrap();

        assert!(blocks.is_empty());
    }

    #[test]
    fn groups_providers_by_role_arn_string() {
        let providers = vec![
            ParsedProvider {
                config_key: "aws".to_string(),
                alias: None,
                role_arn: Some("arn:aws:iam::${var.account_id}:role/MyRole".to_string()),
            },
            ParsedProvider {
                config_key: "aws.global".to_string(),
                alias: Some("global".to_string()),
                role_arn: Some("arn:aws:iam::${var.account_id}:role/MyRole".to_string()),
            },
        ];

        let blocks = vec![
            TerraformBlock {
                block_type: BlockType::Resource,
                type_name: "aws_s3_bucket".to_string(),
                name: "one".to_string(),
                provider_config_key: "aws".to_string(),
                present_attributes: HashSet::new(),
                address: "aws_s3_bucket.one".to_string(),
            },
            TerraformBlock {
                block_type: BlockType::Resource,
                type_name: "aws_s3_bucket".to_string(),
                name: "two".to_string(),
                provider_config_key: "aws.global".to_string(),
                present_attributes: HashSet::new(),
                address: "aws_s3_bucket.two".to_string(),
            },
        ];

        let groups = HclParser::group_by_role(&providers, blocks);

        // Both blocks should be in the same group (same role_arn string)
        assert_eq!(groups.len(), 1);
        assert!(groups.contains_key("DefaultDeployer"));
        assert_eq!(groups["DefaultDeployer"].blocks.len(), 2);
    }

    #[test]
    fn different_role_arns_create_different_groups() {
        let providers = vec![
            ParsedProvider {
                config_key: "aws".to_string(),
                alias: None,
                role_arn: Some("arn:aws:iam::123456789012:role/NetworkRole".to_string()),
            },
            ParsedProvider {
                config_key: "aws.dns".to_string(),
                alias: Some("dns".to_string()),
                role_arn: Some("arn:aws:iam::987654321012:role/DnsRole".to_string()),
            },
        ];

        let blocks = vec![
            TerraformBlock {
                block_type: BlockType::Resource,
                type_name: "aws_vpc".to_string(),
                name: "main".to_string(),
                provider_config_key: "aws".to_string(),
                present_attributes: HashSet::new(),
                address: "aws_vpc.main".to_string(),
            },
            TerraformBlock {
                block_type: BlockType::Resource,
                type_name: "aws_route53_zone".to_string(),
                name: "main".to_string(),
                provider_config_key: "aws.dns".to_string(),
                present_attributes: HashSet::new(),
                address: "aws_route53_zone.main".to_string(),
            },
        ];

        let groups = HclParser::group_by_role(&providers, blocks);

        assert_eq!(groups.len(), 2);
        assert!(groups.contains_key("DefaultDeployer"));
        assert!(groups.contains_key("DnsDeployer"));
        assert_eq!(groups["DefaultDeployer"].blocks.len(), 1);
        assert_eq!(groups["DnsDeployer"].blocks.len(), 1);
    }

    #[test]
    fn parse_nested_attributes() {
        let hcl = r#"
            resource "aws_route53_zone" "private" {
              name = "example.com"

              vpc {
                vpc_id = "vpc-123"
              }

              tags = {
                Environment = "prod"
              }
            }
        "#;

        let body: Body = hcl::from_str(hcl).unwrap();
        let (_, blocks) = HclParser::extract_from_body(&body, "").unwrap();

        assert_eq!(blocks.len(), 1);
        let block = &blocks[0];

        // Check top-level attributes
        assert!(block.present_attributes.contains(&vec!["name".to_string()]));
        assert!(block.present_attributes.contains(&vec!["tags".to_string()]));

        // Check nested vpc block
        assert!(block.present_attributes.contains(&vec!["vpc".to_string()]));
        assert!(
            block
                .present_attributes
                .contains(&vec!["vpc".to_string(), "vpc_id".to_string()])
        );
    }

    #[test]
    fn parse_simple_provider_mapping() {
        let hcl = r#"
            module "test" {
              source = "./test"
              providers = {
                aws = aws.production
              }
            }
        "#;

        let body: Body = hcl::from_str(hcl).unwrap();
        let module_block = body.blocks().next().unwrap();
        let mappings = HclParser::parse_module_providers(module_block.body());

        assert_eq!(mappings.resolve("aws"), "aws.production");
        assert_eq!(mappings.resolve("aws.other"), "aws.other"); // Unmapped passes through
    }

    #[test]
    fn parse_multiple_provider_mappings() {
        let hcl = r#"
            module "multi" {
              source = "./multi"
              providers = {
                aws.primary   = aws.us_east
                aws.secondary = aws.eu_west
                aws           = aws.default_region
              }
            }
        "#;

        let body: Body = hcl::from_str(hcl).unwrap();
        let module_block = body.blocks().next().unwrap();
        let mappings = HclParser::parse_module_providers(module_block.body());

        assert_eq!(mappings.resolve("aws.primary"), "aws.us_east");
        assert_eq!(mappings.resolve("aws.secondary"), "aws.eu_west");
        assert_eq!(mappings.resolve("aws"), "aws.default_region");
    }

    #[test]
    fn parse_module_without_providers_block() {
        let hcl = r#"
            module "simple" {
              source = "./simple"
              name   = "test"
            }
        "#;

        let body: Body = hcl::from_str(hcl).unwrap();
        let module_block = body.blocks().next().unwrap();
        let mappings = HclParser::parse_module_providers(module_block.body());

        assert!(!mappings.has_mappings());
        assert_eq!(mappings.resolve("aws"), "aws"); // Pass through
    }

    #[test]
    fn parse_provider_mapping_with_variable_key() {
        // Test that simple identifiers work as keys
        let hcl = r#"
            module "test" {
              source = "./test"
              providers = {
                aws = aws.test
              }
            }
        "#;

        let body: Body = hcl::from_str(hcl).unwrap();
        let module_block = body.blocks().next().unwrap();
        let mappings = HclParser::parse_module_providers(module_block.body());

        assert!(mappings.has_mappings());
        assert_eq!(mappings.resolve("aws"), "aws.test");
    }
}
