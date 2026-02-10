use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use log::debug;
use regex::Regex;
use serde::Deserialize;
use walkdir::WalkDir;

use super::runner::TerraformError;

/// Represents the source type of a terraform module with detailed parsing.
///
/// This enum classifies module sources and extracts structured information
/// from Terraform Registry, Git, and local module sources.
#[derive(Debug, Clone, PartialEq)]
pub enum ModuleSourceType {
    /// Root module (Key is empty string in modules.json)
    Root,
    /// Local filesystem path
    Local { path: String },
    /// Terraform Registry module
    Registry {
        namespace: String,
        name: String,
        provider: String,
        /// Registry host (None = public registry.terraform.io)
        registry: Option<String>,
        /// Submodule path after "//" (e.g., "modules/filter")
        subdir: Option<String>,
    },
    /// Git repository
    Git {
        url: String,
        /// Git reference (branch, tag, commit)
        ref_spec: Option<String>,
        /// Subdirectory within the repository
        subdir: Option<String>,
    },
}

impl ModuleSourceType {
    /// Parses a module source string into its type with extracted components.
    ///
    /// Handles all Terraform module source formats:
    /// - Empty string -> Root
    /// - Local paths (./foo, ../foo, /absolute) -> Local
    /// - Git URLs (git::https://..., git::ssh://..., github.com/...) -> Git
    /// - Registry sources (namespace/name/provider, registry.terraform.io/...) -> Registry
    pub fn parse(source: &str) -> Self {
        if source.is_empty() {
            return Self::Root;
        }

        // Git sources with explicit prefix
        if source.starts_with("git::") {
            return Self::parse_git_source(source);
        }

        // GitHub shorthand (github.com/org/repo)
        if source.starts_with("github.com/") {
            return Self::parse_github_shorthand(source);
        }

        // Local paths
        if source.starts_with("./") || source.starts_with("../") || source.starts_with('/') {
            return Self::Local {
                path: source.to_string(),
            };
        }

        // Registry sources (namespace/name/provider or registry.terraform.io/...)
        Self::parse_registry_source(source)
    }

    /// Parses a git:: prefixed source string.
    fn parse_git_source(source: &str) -> Self {
        // Remove "git::" prefix
        let without_prefix = &source[5..];

        // Split by "?" to get URL and query params
        let (url_part, ref_spec) = match without_prefix.split_once('?') {
            Some((url, query)) => {
                let ref_val = query
                    .split('&')
                    .find_map(|param| param.strip_prefix("ref="))
                    .map(String::from);
                (url, ref_val)
            }
            None => (without_prefix, None),
        };

        // Find subdir delimiter "//" that's not part of a URL scheme.
        let (url, subdir) = Self::split_url_and_subdir(url_part);

        Self::Git { url, ref_spec, subdir }
    }

    /// Splits a URL and subdirectory, handling the "//" delimiter.
    ///
    /// URL schemes like "https://" have "://" pattern, so we skip those.
    fn split_url_and_subdir(url_part: &str) -> (String, Option<String>) {
        let mut search_start = 0;

        while let Some(pos) = url_part[search_start..].find("//") {
            let absolute_pos = search_start + pos;

            // Check if this "//" is part of a URL scheme (preceded by ":")
            if absolute_pos > 0 && url_part.as_bytes()[absolute_pos - 1] == b':' {
                // Skip this "://" and continue searching
                search_start = absolute_pos + 2;
                continue;
            }

            // Found the subdir delimiter
            return (
                url_part[..absolute_pos].to_string(),
                Some(url_part[absolute_pos + 2..].to_string()),
            );
        }

        // No subdir delimiter found
        (url_part.to_string(), None)
    }

    /// Parses GitHub shorthand (github.com/org/repo) to Git source.
    fn parse_github_shorthand(source: &str) -> Self {
        let url = format!("https://{}.git", source);
        Self::Git {
            url,
            ref_spec: None,
            subdir: None,
        }
    }

    /// Parses a Terraform Registry source string.
    fn parse_registry_source(source: &str) -> Self {
        // First, extract subdir if present (after "//")
        let (source_without_subdir, subdir) = Self::split_registry_subdir(source);

        // Handle full registry URL: registry.terraform.io/namespace/name/provider
        // or app.terraform.io/org/name/provider
        let (registry, path) = if source_without_subdir.contains("terraform.io/") {
            let parts: Vec<&str> = source_without_subdir.splitn(2, "terraform.io/").collect();
            if parts.len() == 2 {
                let registry_host = format!("{}terraform.io", parts[0]);
                (Some(registry_host), parts[1])
            } else {
                (None, source_without_subdir)
            }
        } else {
            (None, source_without_subdir)
        };

        // Parse namespace/name/provider
        let components: Vec<&str> = path.split('/').collect();
        if components.len() >= 3 {
            Self::Registry {
                namespace: components[0].to_string(),
                name: components[1].to_string(),
                provider: components[2].to_string(),
                registry,
                subdir,
            }
        } else {
            // Fallback to local if parsing fails (shouldn't happen with valid sources)
            Self::Local {
                path: source.to_string(),
            }
        }
    }

    /// Splits registry source and subdir at "//".
    fn split_registry_subdir(source: &str) -> (&str, Option<String>) {
        if let Some(pos) = source.find("//") {
            let base = &source[..pos];
            let subdir = &source[pos + 2..];
            if !subdir.is_empty() {
                return (base, Some(subdir.to_string()));
            }
        }
        (source, None)
    }

    /// Returns true if this is a remote module (Git or Registry).
    ///
    /// Remote modules are downloaded by `terraform init` and their code
    /// is stored in `.terraform/modules/`.
    pub fn is_remote(&self) -> bool {
        matches!(self, Self::Git { .. } | Self::Registry { .. })
    }

    /// Returns true if this is a local module path.
    pub fn is_local(&self) -> bool {
        matches!(self, Self::Local { .. })
    }

    /// Returns a human-readable description of the source type for logging.
    pub fn description(&self) -> String {
        match self {
            Self::Root => "root".to_string(),
            Self::Local { path } => format!("local: {}", path),
            Self::Registry {
                namespace,
                name,
                provider,
                registry,
                subdir,
            } => {
                let host = registry
                    .as_ref()
                    .map(|h| format!("{}/", h))
                    .unwrap_or_default();
                let subdir_str = subdir
                    .as_ref()
                    .map(|s| format!("//{}", s))
                    .unwrap_or_default();
                format!("registry: {}{}/{}/{}{}", host, namespace, name, provider, subdir_str)
            }
            Self::Git { url, ref_spec, subdir } => {
                let ref_info = ref_spec.as_deref().unwrap_or("default");
                let subdir_str = subdir
                    .as_ref()
                    .map(|s| format!(" //{}", s))
                    .unwrap_or_default();
                format!("git: {} (ref: {}){}", url, ref_info, subdir_str)
            }
        }
    }
}

/// Represents a detected module source from terraform configuration.
///
/// This is used for detecting external local modules that need to be copied
/// to the sandbox directory.
#[derive(Debug, Clone, PartialEq)]
pub struct ModuleSource {
    /// The raw source string from terraform config.
    pub source: String,
    /// Classification of the source type.
    pub source_type: ModuleSourceType,
}

impl ModuleSource {
    /// Creates a ModuleSource from a raw source string.
    pub fn from_source_string(source: &str) -> Self {
        let source_type = ModuleSourceType::parse(source);
        Self {
            source: source.to_string(),
            source_type,
        }
    }

    /// Returns true if this is a local module source.
    pub fn is_local(&self) -> bool {
        self.source_type.is_local()
    }

    /// Determines if this module is external to the given working directory.
    ///
    /// A module is external if its resolved absolute path is not under the working directory.
    pub fn is_external_to(&self, working_dir: &Path) -> bool {
        match &self.source_type {
            ModuleSourceType::Local { path } => {
                let resolved = working_dir.join(path);
                match (resolved.canonicalize(), working_dir.canonicalize()) {
                    (Ok(resolved_abs), Ok(working_dir_abs)) => {
                        !resolved_abs.starts_with(&working_dir_abs)
                    }
                    // If we can't resolve the path, assume it's external (safer)
                    _ => true,
                }
            }
            // Git/Registry modules are not "external local" - they're downloaded by terraform
            _ => false,
        }
    }

    /// Resolves the absolute path of a local module relative to the working directory.
    ///
    /// Returns None if this is not a local module or if the path doesn't exist.
    pub fn resolve_path(&self, working_dir: &Path) -> Option<PathBuf> {
        match &self.source_type {
            ModuleSourceType::Local { path } => {
                let resolved = working_dir.join(path);
                resolved.canonicalize().ok()
            }
            _ => None,
        }
    }
}

/// Structure for parsing .terraform/modules/modules.json
#[derive(Debug, Deserialize)]
pub struct ModulesJson {
    #[serde(rename = "Modules")]
    pub modules: Vec<RawModuleEntry>,
}

/// Raw entry from modules.json before source type parsing
#[derive(Debug, Deserialize, Clone)]
pub struct RawModuleEntry {
    #[serde(rename = "Key")]
    pub key: String,
    #[serde(rename = "Source")]
    pub source: String,
    #[serde(rename = "Dir")]
    pub dir: String,
}

/// Parsed module entry from modules.json with source type classification.
#[derive(Debug, Clone)]
pub struct ModuleEntry {
    /// Module key in the terraform configuration (e.g., "vpc", "vpc.subnets")
    pub key: String,
    /// Original source string (kept for debugging/logging)
    #[allow(dead_code)]
    pub source: String,
    /// Parsed source type with extracted components
    pub source_type: ModuleSourceType,
    /// Directory where module code is located (relative to working dir)
    pub dir: String,
}

/// Manifest of modules from .terraform/modules/modules.json.
///
/// Provides lookup of module directories by their key in the module hierarchy,
/// and classification of module sources (local, git, registry).
#[derive(Debug)]
pub struct ModulesManifest {
    /// The working directory where modules.json was loaded from.
    working_dir: PathBuf,
    /// Parsed module entries with source type classification.
    entries: Vec<ModuleEntry>,
}

impl ModulesManifest {
    /// Loads the modules manifest from the working directory.
    ///
    /// Returns None if the file doesn't exist or cannot be parsed.
    /// The source strings are parsed to extract detailed type information
    /// (namespace, provider, git ref, subdirectory, etc.).
    pub fn load(working_dir: &Path) -> Option<Self> {
        let path = working_dir.join(".terraform/modules/modules.json");
        let content = fs::read_to_string(&path).ok()?;
        let parsed: ModulesJson = serde_json::from_str(&content).ok()?;

        // Convert raw entries to parsed entries with source type
        let entries = parsed
            .modules
            .into_iter()
            .map(|raw| ModuleEntry {
                key: raw.key,
                source_type: ModuleSourceType::parse(&raw.source),
                source: raw.source,
                dir: raw.dir,
            })
            .collect();

        Some(Self {
            working_dir: working_dir.to_path_buf(),
            entries,
        })
    }

    /// Finds the directory for a module by its key.
    ///
    /// The key identifies the module in the call hierarchy:
    /// - "" = root module
    /// - "budgets" = module.budgets called from root
    /// - "budgets.nested" = module.nested called from within module.budgets
    ///
    /// Returns the absolute path to the module directory, or None if not found.
    ///
    /// Note: Security is provided by the sandbox - all files are copied to an isolated
    /// temp directory before parsing. The `canonicalize()` call validates the path exists
    /// and resolves symlinks/traversals to an absolute path.
    pub fn find_module_dir(&self, module_key: &str) -> Option<PathBuf> {
        let entry = self.entries.iter().find(|m| m.key == module_key)?;
        let joined_path = self.working_dir.join(&entry.dir);

        // Canonicalize to resolve symlinks and ../ sequences, and validate path exists
        // Security note: We operate in an isolated temp directory sandbox, so path
        // traversal within the sandbox is safe - we only parse files we copied there.
        joined_path.canonicalize().ok()
    }

    /// Returns all module entries.
    #[allow(dead_code)]
    pub fn entries(&self) -> &[ModuleEntry] {
        &self.entries
    }

    /// Returns all remote module entries (Git and Registry sources).
    ///
    /// These are modules that were downloaded by `terraform init` and are
    /// stored in `.terraform/modules/`.
    pub fn remote_modules(&self) -> Vec<&ModuleEntry> {
        self.entries
            .iter()
            .filter(|e| e.source_type.is_remote())
            .collect()
    }

    /// Returns the count of remote modules.
    #[allow(dead_code)]
    pub fn remote_module_count(&self) -> usize {
        self.entries.iter().filter(|e| e.source_type.is_remote()).count()
    }

    /// Finds a module entry by its key.
    pub fn find_entry(&self, module_key: &str) -> Option<&ModuleEntry> {
        self.entries.iter().find(|m| m.key == module_key)
    }

    /// Builds a module key from a parent key and child module name.
    ///
    /// For example:
    /// - parent_key="" (root), module_name="budgets" -> "budgets"
    /// - parent_key="budgets", module_name="nested" -> "budgets.nested"
    pub fn build_child_key(parent_key: &str, module_name: &str) -> String {
        if parent_key.is_empty() {
            module_name.to_string()
        } else {
            format!("{}.{}", parent_key, module_name)
        }
    }
}

/// Detects module sources from the working directory.
///
/// Uses a two-phase approach:
/// 1. Primary: Parse `.terraform/modules/modules.json` if it exists
/// 2. Fallback: Parse `.tf` files with regex if modules.json doesn't exist
pub fn detect_module_sources(working_dir: &Path) -> Result<Vec<ModuleSource>, TerraformError> {
    // Try primary method first
    if let Some(sources) = parse_modules_json(working_dir) {
        debug!(
            "Detected {} module sources from modules.json",
            sources.len()
        );
        return Ok(sources);
    }

    // Fallback to regex parsing
    debug!("modules.json not found, falling back to regex parsing of .tf files");
    parse_tf_files_for_modules(working_dir)
}

/// Parses .terraform/modules/modules.json to extract module sources.
///
/// Returns None if the file doesn't exist or cannot be parsed.
fn parse_modules_json(working_dir: &Path) -> Option<Vec<ModuleSource>> {
    let path = working_dir.join(".terraform/modules/modules.json");
    let content = fs::read_to_string(&path).ok()?;
    let parsed: ModulesJson = serde_json::from_str(&content).ok()?;

    let sources: Vec<ModuleSource> = parsed
        .modules
        .into_iter()
        .filter(|m| !m.source.is_empty())
        .map(|m| ModuleSource::from_source_string(&m.source))
        .collect();

    Some(sources)
}

/// Maximum size of a .tf file to process with regex (10 MB).
/// Files larger than this are skipped to prevent ReDoS attacks.
const MAX_TF_FILE_SIZE: u64 = 10 * 1024 * 1024;

/// Parses .tf files with regex to extract module sources.
///
/// This is the fallback method when modules.json doesn't exist (e.g., in CI/CD).
fn parse_tf_files_for_modules(working_dir: &Path) -> Result<Vec<ModuleSource>, TerraformError> {
    let mut sources = Vec::new();
    let mut seen_sources = HashSet::new();

    // Pattern to match module source declarations
    // Handles: source = "../../modules/foo"
    //          source = "./local/module"
    // The (?s) flag makes . match newlines so we can handle multi-line module blocks
    let pattern = Regex::new(r#"(?s)module\s+"[^"]+"\s*\{[^}]*?source\s*=\s*"([^"]+)""#)
        .expect("valid regex");

    for entry in WalkDir::new(working_dir)
        .into_iter()
        .filter_map(|e| e.ok())
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

        // Check file size to prevent ReDoS on maliciously large files
        if let Ok(metadata) = entry.metadata() {
            if metadata.len() > MAX_TF_FILE_SIZE {
                debug!(
                    "Skipping oversized .tf file ({} bytes): {:?}",
                    metadata.len(),
                    entry.path()
                );
                continue;
            }
        }

        let content = fs::read_to_string(entry.path()).map_err(TerraformError::Io)?;

        for cap in pattern.captures_iter(&content) {
            if let Some(source_match) = cap.get(1) {
                let source_str = source_match.as_str();
                // Deduplicate sources
                if seen_sources.insert(source_str.to_string()) {
                    sources.push(ModuleSource::from_source_string(source_str));
                }
            }
        }
    }

    debug!(
        "Detected {} module sources from .tf files via regex",
        sources.len()
    );
    Ok(sources)
}

/// Resolves external local modules from the detected sources.
///
/// Returns a list of absolute paths to external modules that need to be copied.
pub fn resolve_external_modules(
    working_dir: &Path,
    sources: &[ModuleSource],
) -> Result<Vec<PathBuf>, TerraformError> {
    let mut external_paths = Vec::new();
    let mut seen_paths = HashSet::new();

    let working_dir_abs = working_dir.canonicalize().map_err(TerraformError::Io)?;

    for source in sources {
        if source.is_local() && source.is_external_to(&working_dir_abs) {
            if let Some(resolved_path) = source.resolve_path(&working_dir_abs) {
                // Deduplicate paths
                if seen_paths.insert(resolved_path.clone()) {
                    debug!("Found external module: {:?}", resolved_path);
                    external_paths.push(resolved_path);
                }
            }
        }
    }

    Ok(external_paths)
}

/// Finds the common ancestor directory of multiple paths.
///
/// This is used to determine the minimal directory structure that needs
/// to be copied to preserve relative paths between the working directory
/// and external modules.
pub fn find_common_ancestor(paths: &[PathBuf]) -> PathBuf {
    if paths.is_empty() {
        return PathBuf::new();
    }

    if paths.len() == 1 {
        return paths[0].clone();
    }

    let mut ancestor = paths[0].clone();

    for path in &paths[1..] {
        while !path.starts_with(&ancestor) {
            if !ancestor.pop() {
                // Reached root
                return PathBuf::from("/");
            }
        }
    }

    ancestor
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // ==================== ModuleSourceType parsing tests ====================

    #[test]
    fn parse_empty_source_is_root() {
        let parsed = ModuleSourceType::parse("");
        assert!(matches!(parsed, ModuleSourceType::Root));
        assert!(!parsed.is_remote());
        assert!(!parsed.is_local());
    }

    #[test]
    fn parse_registry_source_standard() {
        let source = "terraform-aws-modules/vpc/aws";
        let parsed = ModuleSourceType::parse(source);

        match &parsed {
            ModuleSourceType::Registry {
                namespace,
                name,
                provider,
                registry,
                subdir,
            } => {
                assert_eq!(namespace, "terraform-aws-modules");
                assert_eq!(name, "vpc");
                assert_eq!(provider, "aws");
                assert!(registry.is_none());
                assert!(subdir.is_none());
            }
            _ => panic!("Expected Registry, got {:?}", parsed),
        }
        assert!(parsed.is_remote());
    }

    #[test]
    fn parse_registry_source_with_host() {
        let source = "registry.terraform.io/terraform-aws-modules/vpc/aws";
        let parsed = ModuleSourceType::parse(source);

        match &parsed {
            ModuleSourceType::Registry {
                namespace,
                name,
                provider,
                registry,
                subdir,
            } => {
                assert_eq!(namespace, "terraform-aws-modules");
                assert_eq!(name, "vpc");
                assert_eq!(provider, "aws");
                assert_eq!(registry.as_deref(), Some("registry.terraform.io"));
                assert!(subdir.is_none());
            }
            _ => panic!("Expected Registry, got {:?}", parsed),
        }
    }

    #[test]
    fn parse_private_registry_source() {
        let source = "app.terraform.io/my-org/vpc/aws";
        let parsed = ModuleSourceType::parse(source);

        match &parsed {
            ModuleSourceType::Registry {
                namespace,
                registry,
                ..
            } => {
                assert_eq!(namespace, "my-org");
                assert_eq!(registry.as_deref(), Some("app.terraform.io"));
            }
            _ => panic!("Expected Registry, got {:?}", parsed),
        }
    }

    #[test]
    fn parse_registry_source_with_subdir() {
        let source = "be-bold/account-lookup/aws//modules/filter";
        let parsed = ModuleSourceType::parse(source);

        match &parsed {
            ModuleSourceType::Registry {
                namespace,
                name,
                provider,
                registry,
                subdir,
            } => {
                assert_eq!(namespace, "be-bold");
                assert_eq!(name, "account-lookup");
                assert_eq!(provider, "aws");
                assert!(registry.is_none());
                assert_eq!(subdir.as_deref(), Some("modules/filter"));
            }
            _ => panic!("Expected Registry, got {:?}", parsed),
        }
    }

    #[test]
    fn parse_registry_source_with_host_and_subdir() {
        let source = "registry.terraform.io/be-bold/account-lookup/aws//modules/filter";
        let parsed = ModuleSourceType::parse(source);

        match &parsed {
            ModuleSourceType::Registry {
                namespace,
                name,
                provider,
                registry,
                subdir,
            } => {
                assert_eq!(namespace, "be-bold");
                assert_eq!(name, "account-lookup");
                assert_eq!(provider, "aws");
                assert_eq!(registry.as_deref(), Some("registry.terraform.io"));
                assert_eq!(subdir.as_deref(), Some("modules/filter"));
            }
            _ => panic!("Expected Registry, got {:?}", parsed),
        }
    }

    #[test]
    fn parse_git_https_source() {
        let source = "git::https://github.com/org/terraform-aws-vpc.git";
        let parsed = ModuleSourceType::parse(source);

        match &parsed {
            ModuleSourceType::Git {
                url,
                ref_spec,
                subdir,
            } => {
                assert_eq!(url, "https://github.com/org/terraform-aws-vpc.git");
                assert!(ref_spec.is_none());
                assert!(subdir.is_none());
            }
            _ => panic!("Expected Git, got {:?}", parsed),
        }
        assert!(parsed.is_remote());
    }

    #[test]
    fn parse_git_ssh_source() {
        let source = "git::ssh://git@github.com/org/terraform-aws-vpc.git";
        let parsed = ModuleSourceType::parse(source);

        match &parsed {
            ModuleSourceType::Git { url, .. } => {
                assert_eq!(url, "ssh://git@github.com/org/terraform-aws-vpc.git");
            }
            _ => panic!("Expected Git, got {:?}", parsed),
        }
    }

    #[test]
    fn parse_git_source_with_ref() {
        let source = "git::https://github.com/org/repo.git?ref=v1.2.3";
        let parsed = ModuleSourceType::parse(source);

        match &parsed {
            ModuleSourceType::Git { ref_spec, .. } => {
                assert_eq!(ref_spec.as_deref(), Some("v1.2.3"));
            }
            _ => panic!("Expected Git, got {:?}", parsed),
        }
    }

    #[test]
    fn parse_git_source_with_subdir() {
        let source = "git::https://github.com/org/repo.git//modules/vpc";
        let parsed = ModuleSourceType::parse(source);

        match &parsed {
            ModuleSourceType::Git {
                url,
                ref_spec,
                subdir,
            } => {
                assert_eq!(url, "https://github.com/org/repo.git");
                assert!(ref_spec.is_none());
                assert_eq!(subdir.as_deref(), Some("modules/vpc"));
            }
            _ => panic!("Expected Git, got {:?}", parsed),
        }
    }

    #[test]
    fn parse_git_source_with_ref_and_subdir() {
        let source = "git::https://github.com/org/repo.git//modules/vpc?ref=v1.0.0";
        let parsed = ModuleSourceType::parse(source);

        match &parsed {
            ModuleSourceType::Git {
                url,
                ref_spec,
                subdir,
            } => {
                assert_eq!(url, "https://github.com/org/repo.git");
                assert_eq!(ref_spec.as_deref(), Some("v1.0.0"));
                assert_eq!(subdir.as_deref(), Some("modules/vpc"));
            }
            _ => panic!("Expected Git, got {:?}", parsed),
        }
    }

    #[test]
    fn parse_github_shorthand() {
        let source = "github.com/hashicorp/example";
        let parsed = ModuleSourceType::parse(source);

        match &parsed {
            ModuleSourceType::Git { url, .. } => {
                assert_eq!(url, "https://github.com/hashicorp/example.git");
            }
            _ => panic!("Expected Git, got {:?}", parsed),
        }
    }

    #[test]
    fn parse_local_relative_path() {
        let source = "./modules/vpc";
        let parsed = ModuleSourceType::parse(source);

        match &parsed {
            ModuleSourceType::Local { path } => {
                assert_eq!(path, "./modules/vpc");
            }
            _ => panic!("Expected Local, got {:?}", parsed),
        }
        assert!(!parsed.is_remote());
        assert!(parsed.is_local());
    }

    #[test]
    fn parse_local_parent_path() {
        let source = "../../shared/networking";
        let parsed = ModuleSourceType::parse(source);

        assert!(matches!(parsed, ModuleSourceType::Local { .. }));
    }

    #[test]
    fn parse_local_absolute_path() {
        let source = "/opt/terraform/modules/vpc";
        let parsed = ModuleSourceType::parse(source);

        match &parsed {
            ModuleSourceType::Local { path } => {
                assert_eq!(path, "/opt/terraform/modules/vpc");
            }
            _ => panic!("Expected Local, got {:?}", parsed),
        }
    }

    #[test]
    fn is_remote_returns_true_for_git() {
        let source_type = ModuleSourceType::Git {
            url: "https://example.com".to_string(),
            ref_spec: None,
            subdir: None,
        };
        assert!(source_type.is_remote());
    }

    #[test]
    fn is_remote_returns_true_for_registry() {
        let source_type = ModuleSourceType::Registry {
            namespace: "hashicorp".to_string(),
            name: "vpc".to_string(),
            provider: "aws".to_string(),
            registry: None,
            subdir: None,
        };
        assert!(source_type.is_remote());
    }

    #[test]
    fn is_remote_returns_false_for_local() {
        let source_type = ModuleSourceType::Local {
            path: "./modules".to_string(),
        };
        assert!(!source_type.is_remote());
    }

    #[test]
    fn is_remote_returns_false_for_root() {
        let source_type = ModuleSourceType::Root;
        assert!(!source_type.is_remote());
    }

    #[test]
    fn source_type_description_for_registry() {
        let source_type = ModuleSourceType::Registry {
            namespace: "terraform-aws-modules".to_string(),
            name: "vpc".to_string(),
            provider: "aws".to_string(),
            registry: None,
            subdir: None,
        };
        assert_eq!(
            source_type.description(),
            "registry: terraform-aws-modules/vpc/aws"
        );
    }

    #[test]
    fn source_type_description_for_git_with_ref() {
        let source_type = ModuleSourceType::Git {
            url: "https://github.com/org/repo.git".to_string(),
            ref_spec: Some("v1.0.0".to_string()),
            subdir: None,
        };
        assert_eq!(
            source_type.description(),
            "git: https://github.com/org/repo.git (ref: v1.0.0)"
        );
    }

    // ==================== ModuleSource tests ====================

    #[test]
    fn classify_local_relative_path() {
        let source = ModuleSource::from_source_string("./modules/vpc");
        assert!(matches!(source.source_type, ModuleSourceType::Local { .. }));
        assert!(source.is_local());
    }

    #[test]
    fn classify_local_parent_path() {
        let source = ModuleSource::from_source_string("../../modules/budgets");
        assert!(matches!(source.source_type, ModuleSourceType::Local { .. }));
        assert!(source.is_local());
    }

    #[test]
    fn classify_local_absolute_path() {
        let source = ModuleSource::from_source_string("/absolute/path/to/module");
        assert!(matches!(source.source_type, ModuleSourceType::Local { .. }));
        assert!(source.is_local());
    }

    #[test]
    fn classify_git_source() {
        let source = ModuleSource::from_source_string("git::https://github.com/org/vpc.git");
        assert!(matches!(source.source_type, ModuleSourceType::Git { .. }));
        assert!(!source.is_local());
    }

    #[test]
    fn classify_registry_source() {
        let source = ModuleSource::from_source_string("hashicorp/consul/aws");
        assert!(matches!(
            source.source_type,
            ModuleSourceType::Registry { .. }
        ));
        assert!(!source.is_local());
    }

    #[test]
    fn parse_modules_json_extracts_local_sources() {
        let temp_dir = TempDir::new().unwrap();
        fs::create_dir_all(temp_dir.path().join(".terraform/modules")).unwrap();

        let json = r#"{"Modules":[
            {"Key":"","Source":"","Dir":"."},
            {"Key":"budgets","Source":"../../modules/budgets","Dir":"../../modules/budgets"},
            {"Key":"vpc","Source":"git::https://github.com/org/vpc.git","Dir":".terraform/modules/vpc"}
        ]}"#;

        fs::write(
            temp_dir.path().join(".terraform/modules/modules.json"),
            json,
        )
        .unwrap();

        let sources = parse_modules_json(temp_dir.path()).unwrap();

        let local_sources: Vec<_> = sources.iter().filter(|s| s.is_local()).collect();
        assert_eq!(local_sources.len(), 1);
        assert_eq!(local_sources[0].source, "../../modules/budgets");
    }

    #[test]
    fn regex_extracts_module_sources() {
        let temp_dir = TempDir::new().unwrap();

        let tf_content = r#"
            module "budgets" {
              source = "../../modules/budgets"
              name   = "test"
            }

            module "vpc" {
              source = "git::https://github.com/org/vpc.git"
            }
        "#;

        fs::write(temp_dir.path().join("main.tf"), tf_content).unwrap();

        let sources = parse_tf_files_for_modules(temp_dir.path()).unwrap();
        assert_eq!(sources.len(), 2);

        let source_strings: Vec<_> = sources.iter().map(|s| s.source.as_str()).collect();
        assert!(source_strings.contains(&"../../modules/budgets"));
        assert!(source_strings.contains(&"git::https://github.com/org/vpc.git"));
    }

    #[test]
    fn regex_skips_terraform_directory() {
        let temp_dir = TempDir::new().unwrap();

        // File in .terraform that should be skipped
        fs::create_dir_all(temp_dir.path().join(".terraform/modules")).unwrap();
        fs::write(
            temp_dir.path().join(".terraform/modules/test.tf"),
            r#"module "skip" { source = "./should-be-skipped" }"#,
        )
        .unwrap();

        // File outside .terraform that should be parsed
        fs::write(
            temp_dir.path().join("main.tf"),
            r#"module "include" { source = "./should-be-included" }"#,
        )
        .unwrap();

        let sources = parse_tf_files_for_modules(temp_dir.path()).unwrap();
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].source, "./should-be-included");
    }

    #[test]
    fn regex_deduplicates_sources() {
        let temp_dir = TempDir::new().unwrap();

        // Same module referenced in two files
        fs::write(
            temp_dir.path().join("main.tf"),
            r#"module "one" { source = "./modules/shared" }"#,
        )
        .unwrap();
        fs::write(
            temp_dir.path().join("other.tf"),
            r#"module "two" { source = "./modules/shared" }"#,
        )
        .unwrap();

        let sources = parse_tf_files_for_modules(temp_dir.path()).unwrap();
        assert_eq!(sources.len(), 1);
    }

    #[test]
    fn find_common_ancestor_two_paths() {
        let paths = vec![
            PathBuf::from("/project/envs/dev"),
            PathBuf::from("/project/modules/budgets"),
        ];

        let ancestor = find_common_ancestor(&paths);
        assert_eq!(ancestor, PathBuf::from("/project"));
    }

    #[test]
    fn find_common_ancestor_nested() {
        let paths = vec![
            PathBuf::from("/a/b/c/d"),
            PathBuf::from("/a/b/x/y"),
            PathBuf::from("/a/b/c/z"),
        ];

        let ancestor = find_common_ancestor(&paths);
        assert_eq!(ancestor, PathBuf::from("/a/b"));
    }

    #[test]
    fn find_common_ancestor_single_path() {
        let paths = vec![PathBuf::from("/a/b/c")];
        let ancestor = find_common_ancestor(&paths);
        assert_eq!(ancestor, PathBuf::from("/a/b/c"));
    }

    #[test]
    fn find_common_ancestor_empty() {
        let paths: Vec<PathBuf> = vec![];
        let ancestor = find_common_ancestor(&paths);
        assert_eq!(ancestor, PathBuf::new());
    }

    #[test]
    fn find_common_ancestor_different_roots() {
        // On Unix, different drives don't exist, but paths could diverge at root
        let paths = vec![PathBuf::from("/a/b/c"), PathBuf::from("/x/y/z")];

        let ancestor = find_common_ancestor(&paths);
        assert_eq!(ancestor, PathBuf::from("/"));
    }

    #[test]
    fn module_is_external_when_outside_working_dir() {
        let temp_dir = TempDir::new().unwrap();

        // Create structure: temp/envs/dev (working dir) and temp/modules/budgets (module)
        let working_dir = temp_dir.path().join("envs/dev");
        let module_dir = temp_dir.path().join("modules/budgets");
        fs::create_dir_all(&working_dir).unwrap();
        fs::create_dir_all(&module_dir).unwrap();

        let source = ModuleSource::from_source_string("../../modules/budgets");
        assert!(source.is_external_to(&working_dir));
    }

    #[test]
    fn module_is_internal_when_inside_working_dir() {
        let temp_dir = TempDir::new().unwrap();

        // Create structure with internal module
        let working_dir = temp_dir.path();
        let module_dir = temp_dir.path().join("local_modules/foo");
        fs::create_dir_all(&module_dir).unwrap();

        let source = ModuleSource::from_source_string("./local_modules/foo");
        assert!(!source.is_external_to(working_dir));
    }

    #[test]
    fn resolve_external_modules_filters_correctly() {
        let temp_dir = TempDir::new().unwrap();

        // Create structure
        let working_dir = temp_dir.path().join("envs/dev");
        let internal_module = temp_dir.path().join("envs/dev/modules/internal");
        let external_module = temp_dir.path().join("shared/external");

        fs::create_dir_all(&working_dir).unwrap();
        fs::create_dir_all(&internal_module).unwrap();
        fs::create_dir_all(&external_module).unwrap();

        let sources = vec![
            ModuleSource::from_source_string("./modules/internal"),
            ModuleSource::from_source_string("../../shared/external"),
            ModuleSource::from_source_string("git::https://github.com/org/repo.git"),
        ];

        let external = resolve_external_modules(&working_dir, &sources).unwrap();

        assert_eq!(external.len(), 1);
        assert!(external[0].ends_with("shared/external"));
    }

    #[test]
    fn regex_handles_multiline_module_blocks() {
        let temp_dir = TempDir::new().unwrap();

        let tf_content = r#"
            module "multiline" {
              source = "../modules/test"

              variable1 = "value1"
              variable2 = "value2"
            }
        "#;

        fs::write(temp_dir.path().join("main.tf"), tf_content).unwrap();

        let sources = parse_tf_files_for_modules(temp_dir.path()).unwrap();
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].source, "../modules/test");
    }

    #[test]
    fn modules_manifest_load_parses_correctly() {
        let temp_dir = TempDir::new().unwrap();
        fs::create_dir_all(temp_dir.path().join(".terraform/modules")).unwrap();

        let json = r#"{"Modules":[
            {"Key":"","Source":"","Dir":"."},
            {"Key":"budgets","Source":"./modules/budgets","Dir":"modules/budgets"},
            {"Key":"budgets.nested","Source":"./nested","Dir":"modules/budgets/nested"}
        ]}"#;

        fs::write(
            temp_dir.path().join(".terraform/modules/modules.json"),
            json,
        )
        .unwrap();

        let manifest = ModulesManifest::load(temp_dir.path()).unwrap();
        assert_eq!(manifest.entries().len(), 3);
    }

    #[test]
    fn modules_manifest_find_module_dir_returns_path() {
        let temp_dir = TempDir::new().unwrap();
        fs::create_dir_all(temp_dir.path().join(".terraform/modules")).unwrap();
        fs::create_dir_all(temp_dir.path().join("modules/budgets")).unwrap();

        let json = r#"{"Modules":[
            {"Key":"","Source":"","Dir":"."},
            {"Key":"budgets","Source":"./modules/budgets","Dir":"modules/budgets"}
        ]}"#;

        fs::write(
            temp_dir.path().join(".terraform/modules/modules.json"),
            json,
        )
        .unwrap();

        let manifest = ModulesManifest::load(temp_dir.path()).unwrap();

        // Find root module - compare canonicalized paths since find_module_dir returns canonicalized
        let root_dir = manifest.find_module_dir("").unwrap();
        let expected_root = temp_dir.path().canonicalize().unwrap();
        assert_eq!(root_dir, expected_root);

        // Find child module - compare canonicalized paths
        let budgets_dir = manifest.find_module_dir("budgets").unwrap();
        let expected_budgets = temp_dir
            .path()
            .join("modules/budgets")
            .canonicalize()
            .unwrap();
        assert_eq!(budgets_dir, expected_budgets);

        // Module not found
        assert!(manifest.find_module_dir("nonexistent").is_none());
    }

    #[test]
    fn modules_manifest_build_child_key() {
        // From root
        assert_eq!(ModulesManifest::build_child_key("", "budgets"), "budgets");

        // From nested
        assert_eq!(
            ModulesManifest::build_child_key("budgets", "nested"),
            "budgets.nested"
        );

        // Deeply nested
        assert_eq!(
            ModulesManifest::build_child_key("level1.level2", "level3"),
            "level1.level2.level3"
        );
    }

    #[test]
    fn modules_manifest_returns_none_when_file_missing() {
        let temp_dir = TempDir::new().unwrap();
        let manifest = ModulesManifest::load(temp_dir.path());
        assert!(manifest.is_none());
    }

    #[test]
    fn modules_manifest_find_module_dir_returns_none_for_nonexistent_path() {
        let temp_dir = TempDir::new().unwrap();
        fs::create_dir_all(temp_dir.path().join(".terraform/modules")).unwrap();

        // Create a modules.json with a non-existent path
        let json = r#"{"Modules":[
            {"Key":"","Source":"","Dir":"."},
            {"Key":"missing","Source":"./missing","Dir":"./does_not_exist"}
        ]}"#;

        fs::write(
            temp_dir.path().join(".terraform/modules/modules.json"),
            json,
        )
        .unwrap();

        let manifest = ModulesManifest::load(temp_dir.path()).unwrap();

        // The missing module should return None because the path doesn't exist
        // (canonicalize() fails for non-existent paths)
        let missing_dir = manifest.find_module_dir("missing");
        assert!(
            missing_dir.is_none(),
            "Non-existent path should return None, got: {:?}",
            missing_dir
        );

        // Root module should still work (current directory always exists)
        assert!(manifest.find_module_dir("").is_some());
    }

    // ==================== ModulesManifest remote module tests ====================

    #[test]
    fn modules_manifest_remote_modules_returns_only_remote() {
        let temp_dir = TempDir::new().unwrap();
        fs::create_dir_all(temp_dir.path().join(".terraform/modules")).unwrap();

        let json = r#"{"Modules":[
            {"Key":"","Source":"","Dir":"."},
            {"Key":"vpc","Source":"terraform-aws-modules/vpc/aws","Dir":".terraform/modules/vpc"},
            {"Key":"s3","Source":"git::https://github.com/org/s3.git?ref=v1.0","Dir":".terraform/modules/s3"},
            {"Key":"local","Source":"./modules/local","Dir":"modules/local"}
        ]}"#;

        fs::write(
            temp_dir.path().join(".terraform/modules/modules.json"),
            json,
        )
        .unwrap();

        let manifest = ModulesManifest::load(temp_dir.path()).unwrap();
        let remote = manifest.remote_modules();

        assert_eq!(remote.len(), 2);
        assert_eq!(manifest.remote_module_count(), 2);

        // Verify both are remote modules
        assert!(remote[0].source_type.is_remote());
        assert!(remote[1].source_type.is_remote());

        // Verify keys
        let keys: Vec<_> = remote.iter().map(|m| m.key.as_str()).collect();
        assert!(keys.contains(&"vpc"));
        assert!(keys.contains(&"s3"));
    }

    #[test]
    fn modules_manifest_find_entry_returns_module() {
        let temp_dir = TempDir::new().unwrap();
        fs::create_dir_all(temp_dir.path().join(".terraform/modules")).unwrap();

        let json = r#"{"Modules":[
            {"Key":"","Source":"","Dir":"."},
            {"Key":"vpc","Source":"terraform-aws-modules/vpc/aws","Dir":".terraform/modules/vpc"}
        ]}"#;

        fs::write(
            temp_dir.path().join(".terraform/modules/modules.json"),
            json,
        )
        .unwrap();

        let manifest = ModulesManifest::load(temp_dir.path()).unwrap();

        let vpc_entry = manifest.find_entry("vpc").unwrap();
        assert_eq!(vpc_entry.key, "vpc");
        assert_eq!(vpc_entry.source, "terraform-aws-modules/vpc/aws");
        assert!(matches!(
            vpc_entry.source_type,
            ModuleSourceType::Registry { .. }
        ));

        assert!(manifest.find_entry("nonexistent").is_none());
    }

    #[test]
    fn modules_manifest_parses_registry_source_details() {
        let temp_dir = TempDir::new().unwrap();
        fs::create_dir_all(temp_dir.path().join(".terraform/modules")).unwrap();

        let json = r#"{"Modules":[
            {"Key":"vpc","Source":"registry.terraform.io/terraform-aws-modules/vpc/aws","Dir":".terraform/modules/vpc"}
        ]}"#;

        fs::write(
            temp_dir.path().join(".terraform/modules/modules.json"),
            json,
        )
        .unwrap();

        let manifest = ModulesManifest::load(temp_dir.path()).unwrap();
        let entry = manifest.find_entry("vpc").unwrap();

        match &entry.source_type {
            ModuleSourceType::Registry {
                namespace,
                name,
                provider,
                registry,
                ..
            } => {
                assert_eq!(namespace, "terraform-aws-modules");
                assert_eq!(name, "vpc");
                assert_eq!(provider, "aws");
                assert_eq!(registry.as_deref(), Some("registry.terraform.io"));
            }
            _ => panic!("Expected Registry source type"),
        }
    }

    #[test]
    fn modules_manifest_parses_git_source_with_ref() {
        let temp_dir = TempDir::new().unwrap();
        fs::create_dir_all(temp_dir.path().join(".terraform/modules")).unwrap();

        let json = r#"{"Modules":[
            {"Key":"s3","Source":"git::https://github.com/org/repo.git?ref=v2.1.0","Dir":".terraform/modules/s3"}
        ]}"#;

        fs::write(
            temp_dir.path().join(".terraform/modules/modules.json"),
            json,
        )
        .unwrap();

        let manifest = ModulesManifest::load(temp_dir.path()).unwrap();
        let entry = manifest.find_entry("s3").unwrap();

        match &entry.source_type {
            ModuleSourceType::Git { url, ref_spec, .. } => {
                assert_eq!(url, "https://github.com/org/repo.git");
                assert_eq!(ref_spec.as_deref(), Some("v2.1.0"));
            }
            _ => panic!("Expected Git source type"),
        }
    }

    #[test]
    fn modules_manifest_parses_nested_submodule() {
        let temp_dir = TempDir::new().unwrap();
        fs::create_dir_all(temp_dir.path().join(".terraform/modules")).unwrap();

        // Nested submodule within a registry module
        let json = r#"{"Modules":[
            {"Key":"vpc","Source":"terraform-aws-modules/vpc/aws","Dir":".terraform/modules/vpc"},
            {"Key":"vpc.vpc_endpoints","Source":"./modules/vpc-endpoints","Dir":".terraform/modules/vpc/modules/vpc-endpoints"}
        ]}"#;

        fs::write(
            temp_dir.path().join(".terraform/modules/modules.json"),
            json,
        )
        .unwrap();

        let manifest = ModulesManifest::load(temp_dir.path()).unwrap();

        // The nested module should be parsed as local (relative to parent)
        let nested = manifest.find_entry("vpc.vpc_endpoints").unwrap();
        assert!(matches!(nested.source_type, ModuleSourceType::Local { .. }));

        // Parent is registry
        let parent = manifest.find_entry("vpc").unwrap();
        assert!(matches!(
            parent.source_type,
            ModuleSourceType::Registry { .. }
        ));
    }

    #[test]
    fn parse_registry_submodule_source() {
        // Registry module with submodule path like be-bold/account-lookup/aws//modules/filter
        let source = "registry.terraform.io/be-bold/account-lookup/aws//modules/filter";
        let parsed = ModuleSourceType::parse(source);

        match parsed {
            ModuleSourceType::Registry {
                namespace,
                name,
                provider,
                registry,
                subdir,
            } => {
                assert_eq!(namespace, "be-bold");
                assert_eq!(name, "account-lookup");
                assert_eq!(provider, "aws");
                assert_eq!(registry.as_deref(), Some("registry.terraform.io"));
                assert_eq!(subdir.as_deref(), Some("modules/filter"));
            }
            _ => panic!("Expected Registry source type"),
        }
    }
}
