# lppc-cli Architecture

This document captures the architectural structure, design decisions, and module responsibilities of the `lppc-cli` codebase. It is intended to be read by a software architect before consulting on code changes, replacing the need for a full codebase scan.

**Last updated**: 2026-02-13
**Rust edition**: 2024
**Crate name**: `lppc`

---

## 1. High-Level Overview

`lppc-cli` is a Rust CLI tool that generates minimal AWS IAM policies from Terraform code through static analysis. It does **not** require AWS credentials or a configured backend. The tool:

1. Clones/updates an external YAML mapping repository (cached at `~/.lppc/`)
2. Copies Terraform files into an isolated temp directory (never modifies the user's working directory)
3. Runs `terraform init -backend=false` to resolve module dependencies
4. Parses `.tf` files directly using HCL parsing (no `terraform plan` needed)
5. Looks up IAM permissions from YAML mapping files for each discovered resource/data/ephemeral/action block
6. Outputs IAM policy documents in JSON or HCL format, grouped by AWS provider (deployer role)

The tool supports both **allow** and **deny** effect permissions, plus **conditional** permissions that activate based on the presence of specific HCL attributes.

---

## 2. Project Context

`lppc-cli` lives in a monorepo alongside a sibling tool:

```
/lppc/
  CLAUDE.md          -- Code style, test philosophy, workflow instructions
  README.md          -- Project overview
  .claude/
    agents/
      software-architect.md   -- This agent's system prompt
      security-reviewer.md    -- Security review agent
    rules/
      dependency-check.md     -- Rules for adding Rust dependencies
  lppc-cli/           -- THIS codebase (Rust CLI)
  mapping-creator/    -- Separate tool for creating YAML mappings (not covered here)
```

The external mapping repository (default: `https://github.com/bebold-jhr/lppc-aws-mappings`) is a separate Git repo with structure `mappings/{PROVIDER}/{BLOCK_TYPE}/{TYPE}.yaml` (e.g., `mappings/aws/resource/aws_s3_bucket.yaml`).

---

## 3. Module Structure and Dependency Flow

```
main.rs (entry point, orchestration)
  |
  +-- cli.rs        (CLI argument parsing via clap)
  +-- config.rs     (CLI -> validated Config conversion)
  +-- logging.rs    (env_logger initialization)
  +-- error.rs      (top-level LppcError enum)
  |
  +-- terraform/    (HCL parsing, module detection, terraform execution)
  |     +-- mod.rs           (public re-exports)
  |     +-- model.rs         (core data types: TerraformConfig, TerraformBlock, etc.)
  |     +-- hcl_parser.rs    (direct HCL file parsing, recursive module traversal)
  |     +-- plan.rs          (PlanExecutor: isolated temp dir, copy, init, parse)
  |     +-- runner.rs        (TerraformRunner: shell-outs to terraform binary)
  |     +-- provider.rs      (AwsProvider, ProviderRegistry, PascalCase naming)
  |     +-- module_detector.rs (module source detection, modules.json, regex fallback)
  |     +-- parser.rs        (JSON-based parser -- legacy, for terraform show -json)
  |     +-- json_types.rs    (serde types for terraform plan JSON -- legacy)
  |
  +-- mapping/      (YAML mapping loading, permission resolution)
  |     +-- mod.rs           (MappingRepository lifecycle, MappingError)
  |     +-- cache.rs         (CacheManager: ~/.lppc directory, URL parsing, timestamps)
  |     +-- repository.rs    (GitOperations: clone, update, reachability)
  |     +-- loader.rs        (MappingLoader: file loading with in-memory cache)
  |     +-- schema.rs        (ActionMapping, ConditionalActions data types)
  |     +-- yaml_parser.rs   (YAML -> ActionMapping using saphyr)
  |     +-- matcher.rs       (PermissionMatcher: resolves TerraformConfig -> PermissionResult)
  |
  +-- output/       (policy document formatting and output)
        +-- mod.rs           (OutputWriter: stdout vs directory, missing mappings)
        +-- formatter.rs     (OutputFormatter trait, PermissionSets, factory function)
        +-- json.rs          (JsonFormatter: AWS IAM policy JSON)
        +-- hcl.rs           (HclFormatter: jsonencode() HCL format)
```

### Dependency flow (acyclic)

```
main -> cli, config, logging, mapping, output, terraform
config -> cli, error
error -> mapping::MappingError, terraform::TerraformError
terraform::plan -> terraform::{hcl_parser, module_detector, runner}
terraform::hcl_parser -> terraform::{model, module_detector, provider}
terraform::parser -> terraform::{json_types, model, provider}      [legacy]
mapping::matcher -> mapping::loader, terraform::{BlockType, TerraformConfig}
mapping::loader -> mapping::{schema, yaml_parser}, terraform::BlockType
output -> cli::OutputFormat, mapping::PermissionResult
output::formatter -> cli::OutputFormat
output::{json, hcl} -> output::formatter
```

Key observation: `mapping` depends on `terraform` types (`BlockType`, `TerraformConfig`), and `output` depends on both `cli::OutputFormat` and `mapping::PermissionResult`. Dependencies flow cleanly downward from `main`.

---

## 4. Key Data Types and Their Relationships

### 4.1 Terraform Module

```
TerraformConfig
  +-- provider_groups: HashMap<String, ProviderGroup>    // key = output name (e.g., "NetworkDeployer")
  +-- unmapped_blocks: Vec<TerraformBlock>               // blocks without a provider mapping

ProviderGroup
  +-- output_name: String
  +-- role_arn: Option<String>
  +-- blocks: Vec<TerraformBlock>

TerraformBlock
  +-- block_type: BlockType                              // Resource | Data | Ephemeral | Action
  +-- type_name: String                                  // e.g., "aws_s3_bucket"
  +-- name: String                                       // e.g., "this"
  +-- provider_config_key: String                        // e.g., "aws", "aws.secondary"
  +-- present_attributes: HashSet<Vec<String>>           // nested paths, e.g., {["vpc","vpc_id"], ["tags"]}
  +-- address: String                                    // full address, e.g., "module.vpc.aws_subnet.main"

BlockType: Resource | Data | Ephemeral | Action
  +-- as_str() -> "resource" | "data" | "ephemeral" | "action"

AwsProvider
  +-- config_key: String         // "aws" or "aws.{alias}"
  +-- alias: Option<String>
  +-- role_arn: Option<String>
  +-- region: Option<String>
  +-- output_name() -> String    // alias -> PascalCase + "Deployer" suffix

ProviderRegistry                 // HashMap<config_key, AwsProvider>
  +-- group_by_output_name()     // groups providers by role_arn, names by first alias alphabetically

ModuleContext                    // tracks provider key resolution through nested modules
  +-- address_prefix: String
  +-- resolve_to_root(local_key) -> root_key

ProviderMappings                 // module-level provider key remapping
  +-- resolve(local_key) -> parent_key
```

### 4.2 Mapping Module

```
MappingRepository
  +-- local_path: PathBuf        // e.g., ~/.lppc/bebold-jhr/lppc-aws-mappings
  +-- url: String
  +-- was_refreshed: bool
  +-- ensure_available(url, force_refresh) -> Self       // orchestrates clone/update/cache logic

CacheManager
  +-- base_dir: PathBuf          // ~/.lppc
  +-- get_repo_path(url) -> PathBuf
  +-- is_cached(url) -> bool
  +-- needs_refresh(url) -> bool                         // 24-hour expiry
  +-- update_timestamp(url)

GitOperations                    // stateless, calls system `git`
  +-- shallow_clone(url, path)
  +-- update(repo_path)          // fetch --depth 1 + reset --hard
  +-- is_remote_reachable(url)

ActionMapping                    // parsed from a single YAML file
  +-- allow: Vec<String>         // always-needed IAM actions
  +-- deny: Vec<String>          // explicitly denied IAM actions
  +-- conditional: ConditionalActions

ConditionalActions               // recursive enum
  +-- None                       // no conditional actions
  +-- Actions(Vec<String>)       // leaf: list of IAM actions
  +-- Nested(HashMap<String, ConditionalActions>)
  +-- resolve(present_paths) -> Vec<String>              // resolves based on attribute presence

MappingLoader
  +-- repo_path: PathBuf
  +-- cache: Mutex<HashMap<String, Option<ActionMapping>>>   // in-memory cache
  +-- load(provider, block_type, type_name) -> Option<ActionMapping>
  +-- extract_provider(type_name) -> Option<&str>            // "aws_s3_bucket" -> "aws"

PermissionMatcher
  +-- resolve(TerraformConfig) -> PermissionResult

PermissionResult
  +-- groups: HashMap<String, GroupPermissions>           // output_name -> permissions
  +-- missing_mappings: Vec<MissingMapping>

GroupPermissions
  +-- allow: HashSet<String>
  +-- deny: HashSet<String>
```

### 4.3 Output Module

```
OutputWriter
  +-- format: OutputFormat
  +-- output_dir: Option<PathBuf>
  +-- no_color: bool
  +-- write(PermissionResult)
  +-- write_missing_mappings(PermissionResult)            // to stderr

OutputFormat: Json | JsonGrouped | Hcl | HclGrouped      // clap ValueEnum

PermissionSets<'a>              // passed to formatters
  +-- allow: &HashSet<String>
  +-- deny: &HashSet<String>

trait OutputFormatter
  +-- format(PermissionSets) -> String
  +-- extension() -> &'static str

JsonFormatter { grouped: bool }  // outputs AWS IAM policy document JSON
HclFormatter { grouped: bool }   // outputs jsonencode({...}) HCL

create_formatter(OutputFormat) -> Box<dyn OutputFormatter>   // factory
```

---

## 5. Control Flow / Processing Pipeline

```
main()
  1. Cli::parse()                          // clap derives CLI args
  2. init_logging(verbose, no_color)        // configure env_logger
  3. Config::from_cli(cli)                  // validate & canonicalize working_dir
  4. MappingRepository::ensure_available()  // clone/update/cache the YAML repo
       -> CacheManager checks timestamps
       -> GitOperations::shallow_clone() or ::update()
       -> Graceful fallback to cache if network unreachable
  5. PlanExecutor::new()                    // verify terraform is in PATH
  6. PlanExecutor::execute(working_dir)
       a. Check for .tf files
       b. detect_module_sources()           // modules.json or regex fallback
       c. resolve_external_modules()        // identify modules outside working dir
       d. plan_copy_structure()             // compute common ancestor, relative paths
       e. Create TempDir, copy files        // skip .terraform/, preserve structure
       f. clean_terraform_state()           // remove .tfstate files
       g. runner.init(execution_dir)        // terraform init -backend=false
       h. HclParser::parse_directory()      // parse .tf files recursively
           -> extracts providers, resources, data sources, module calls
           -> recursively parses submodules (local + downloaded)
           -> resolves provider mappings through module hierarchy
           -> groups blocks by role_arn using ProviderRegistry
       i. Return TerraformConfig
  7. MappingLoader::new(repo_path)
  8. PermissionMatcher::resolve(config)
       -> For each block in each provider group:
          - Load YAML mapping (with in-memory cache)
          - Add allow actions to group's allow set
          - Add deny actions to group's deny set
          - Resolve conditional actions (attribute presence) -> allow set
          - Track missing mappings
       -> Return PermissionResult
  9. OutputWriter::write_missing_mappings() // warnings to stderr
 10. OutputWriter::write()                  // formatted output to stdout or files
       -> create_formatter() factory
       -> Deny statements before Allow statements
       -> Grouped mode: one statement per AWS service prefix
```

---

## 6. External Dependencies and Their Roles

| Crate       | Version   | Role                                                     |
|-------------|-----------|----------------------------------------------------------|
| `clap`      | 4.5       | CLI argument parsing with derive macros                  |
| `colored`   | 3.1       | Colored terminal output                                  |
| `env_logger`| 0.11      | Logging initialization                                   |
| `log`       | 0.4       | Logging facade                                           |
| `thiserror` | 2.0       | Derive macro for error enums                             |
| `anyhow`    | 1.0       | Error handling in main() only                            |
| `dirs`      | 6.0       | Home directory detection for `~/.lppc`                   |
| `chrono`    | 0.4       | Timestamp formatting for cache files                     |
| `sha2`      | 0.10      | URL hashing for cache timestamp filenames                |
| `hex`       | 0.4       | Hex encoding of SHA-256 hashes                           |
| `which`     | 8.0       | Finding `terraform` binary in PATH                       |
| `tempfile`  | 3.24      | Isolated temporary directories for terraform execution   |
| `walkdir`   | 2.5       | Recursive directory traversal for file copying           |
| `regex`     | 1.12      | Fallback module source detection from .tf files          |
| `serde`     | 1.0       | Serialization framework (JSON output, terraform plan)    |
| `serde_json`| 1.0       | JSON serialization for IAM policy documents              |
| `saphyr`    | 0.0.6     | YAML parsing for mapping files                           |
| `hcl-rs`    | 0.19      | Direct HCL/Terraform file parsing                        |

Dev dependencies: `assert_cmd`, `predicates` (integration testing).

**Planned migration**: `saphyr` -> `saphyr-serde` when released (see `specs/backlog.md`). This will replace manual YAML deserialization with serde derive macros.

---

## 7. Design Patterns Used

### Strategy Pattern (Output Formatting)
The `OutputFormatter` trait with `JsonFormatter` and `HclFormatter` implementations, created via the `create_formatter()` factory function. Each formatter handles both grouped and non-grouped modes via an internal `grouped: bool` flag.

### Repository Pattern (Mapping Repository)
`MappingRepository` encapsulates the lifecycle of the external data source (clone, cache, update, offline fallback). `CacheManager` handles persistence concerns separately.

### In-Memory Cache (MappingLoader)
`MappingLoader` uses a `Mutex<HashMap<String, Option<ActionMapping>>>` to cache loaded YAML files, avoiding repeated I/O for the same resource type across multiple blocks. Caches both hits and misses.

### Recursive Descent (ConditionalActions)
The `ConditionalActions` enum is a recursive data structure (`Nested` variant contains `HashMap<String, ConditionalActions>`) that supports arbitrary nesting depth. Resolution traverses the tree matching against `present_attributes` paths.

### Builder/Converter Pattern (Config)
`Config::from_cli(cli)` validates and transforms raw CLI arguments into a sanitized, canonical configuration object. Validation includes path resolution, directory existence checks, and canonicalization.

### Isolation via Temp Directory (PlanExecutor)
All terraform operations happen in a temporary directory created by `PlanExecutor`. Files are copied there (excluding `.terraform/`), terraform runs in isolation, and the temp dir is cleaned up on drop. The user's working directory is never modified.

### Context Propagation (ModuleContext)
`ModuleContext` tracks cumulative provider key mappings through nested module hierarchies, allowing resources deep in module trees to be resolved to root-level provider groups.

---

## 8. Important Architectural Decisions and Constraints

These are documented extensively in `specs/brainstorming.md`. Key points for a reviewer:

1. **Direct HCL parsing over terraform plan**: No AWS credentials or backend needed. Trade-off: no access to resolved variable values (acceptable because we only need attribute presence, not values).

2. **Alias-based naming**: Output names derived from provider `alias` (e.g., `dns_account` -> `DnsAccountDeployer`) because `role_arn` often contains unresolvable Terraform variables.

3. **Provider grouping by exact role_arn string**: Providers with identical `role_arn` expression strings share a permission set. First alias alphabetically wins for naming.

4. **Isolated execution**: The user's working directory is NEVER modified. All operations in a temp dir that preserves relative path structure for module resolution.

5. **External mapping repository**: Mappings are not bundled. Decouples content from tool releases. Users can supply custom mappings via `--mappings-url`.

6. **24-hour cache with graceful degradation**: Stale cache is better than failure. Network-unreachable + cached = warning + use cache. Network-unreachable + no cache = error.

7. **Security hardening**: Path traversal prevention in cache paths, mapping file paths, and output filenames. URL validation rejects dangerous protocols. Branch name validation prevents argument injection. File size limits prevent resource exhaustion (1 MB for YAML, 10 MB for .tf).

8. **Deny statements before Allow statements**: In all output formats, Deny blocks appear first. This follows AWS IAM best practice (explicit deny overrides allow).

9. **Resource is always `*`**: Static analysis cannot determine actual resource ARNs. Users refine the output.

10. **Legacy code**: `terraform::parser` and `terraform::json_types` are legacy modules from when the tool used `terraform plan -json`. The `execute_json()` method on `PlanExecutor` is deprecated. These modules remain for backward compatibility but are not used in the current pipeline.

---

## 9. File-by-File Summary

### Root level

| File | Lines | Purpose |
|------|-------|---------|
| `src/main.rs` | ~80 | Entry point. Orchestrates the full pipeline: parse CLI, init logging, ensure mappings, execute terraform, resolve permissions, write output. Uses `anyhow::Result` for top-level error handling. |
| `src/lib.rs` | ~7 | Module declarations. Exposes `cli`, `config`, `error`, `logging`, `mapping`, `output`, `terraform` as public modules. |
| `src/cli.rs` | ~58 | `Cli` struct with clap derive macros. `OutputFormat` enum (Json, JsonGrouped, Hcl, HclGrouped). Default format: HclGrouped. |
| `src/config.rs` | ~75+tests | `Config::from_cli()` validates working_dir (exists, is directory, canonicalized). `resolve_path()` converts relative to absolute paths. |
| `src/error.rs` | ~22 | `LppcError` enum: Config, Io, Mapping, Terraform. Uses `#[from]` for automatic conversion. `Result<T>` type alias. |
| `src/logging.rs` | ~22+tests | `init_logging()` configures `env_logger`. Verbose mode enables Debug level. `colored::control::set_override` for `--no-color`. |

### terraform/ module

| File | Lines | Purpose |
|------|-------|---------|
| `mod.rs` | ~14 | Module declarations (all submodules private except through re-exports). Public API: `HclParser`, `HclParseError`, `BlockType`, `ProviderGroup`, `TerraformBlock`, `TerraformConfig`, `TerraformParser`, `ParseError`, `PlanExecutor`, `TerraformError`, `TerraformRunner`. |
| `model.rs` | ~288 | Core domain types: `TerraformConfig`, `ProviderGroup`, `TerraformBlock`, `BlockType`, `ProviderMappings`, `ModuleContext`. `ModuleContext` enables recursive provider key resolution through nested modules. |
| `hcl_parser.rs` | ~1000+ | **The most complex file.** `HclParser::parse_directory()` recursively parses `.tf` files. Extracts providers (with alias, role_arn, region), resource/data/ephemeral/action blocks with attribute paths, and module calls. Handles `ModulesManifest` for remote modules. Groups blocks by role using `ProviderRegistry`. File size limit: 10 MB. |
| `plan.rs` | ~1280 | `PlanExecutor`: orchestrates isolated terraform execution. Creates temp directory, plans copy structure (handling external modules via common ancestor), copies files, cleans state, runs `terraform init`, then delegates to `HclParser`. Contains deprecated `execute_json()` for legacy plan-based flow. Heavy test coverage including module provider mapping scenarios. |
| `runner.rs` | ~244 | `TerraformRunner`: wraps terraform binary calls (`init`, `plan`, `show`). `has_terraform_files()` checks for `.tf` extension. `TerraformError` enum with descriptive messages. |
| `provider.rs` | ~550 | `AwsProvider`: provider config with `output_name()` (alias -> PascalCase + "Deployer"). `to_pascal_case()` handles snake_case, kebab-case, SCREAMING_CASE, and preserves existing PascalCase. `ProviderRegistry`: indexes providers by config_key, groups by role_arn with deterministic naming (first alias alphabetically). |
| `module_detector.rs` | ~1200+ | Module source detection. `ModuleSourceType` enum: Root, Local, Registry, Git. Parses `.terraform/modules/modules.json` (primary) or falls back to regex parsing of `.tf` files. `ModulesManifest` loads and classifies module entries. `detect_module_sources()` and `resolve_external_modules()` identify modules outside the working directory. `find_common_ancestor()` computes shared path prefix for copy planning. |
| `parser.rs` | ~120+ | **Legacy.** `TerraformParser::parse()` parses `terraform show -json` output. Extracts providers and resources recursively through module hierarchy. Used by the deprecated `execute_json()` path. |
| `json_types.rs` | ~120+ | **Legacy.** Serde deserialize types for terraform plan JSON output: `TerraformPlan`, `Configuration`, `ProviderConfig`, `Module`, `ResourceConfig`, `ModuleCall`. |

### mapping/ module

| File | Lines | Purpose |
|------|-------|---------|
| `mod.rs` | ~217 | `MappingRepository::ensure_available()`: main lifecycle method. Decides whether to clone, update, or use cache based on `force_refresh`, cache age (24h), and network availability. `MappingError` enum. Helper methods: `aws_mappings_path()`, `mapping_file_path()`. |
| `cache.rs` | ~502 | `CacheManager`: manages `~/.lppc` directory. URL parsing for HTTPS and SSH git URLs. Timestamp-based cache expiry using SHA-256 hashed URL filenames. Path traversal validation (`validate_path_component`). Extensive security tests. |
| `repository.rs` | ~402 | `GitOperations`: stateless struct with static methods. `shallow_clone()` and `update()` shell out to system `git`. URL validation (rejects `ext::`, `file://`, dash-prefix). Branch name validation. `classify_error()` maps git error messages to `GitError` variants (notably `NetworkUnreachable` for graceful degradation). |
| `loader.rs` | ~427 | `MappingLoader`: loads YAML mapping files from disk with in-memory Mutex-based cache. Path traversal prevention via `is_valid_path_component()`. File size limit: 1 MB. `extract_provider()` splits type_name on `_` to get provider prefix. |
| `schema.rs` | ~335 | `ActionMapping`: `allow: Vec<String>`, `deny: Vec<String>`, `conditional: ConditionalActions`. `ConditionalActions` is a recursive enum (None, Actions, Nested) with `resolve()` that walks attribute paths. |
| `yaml_parser.rs` | ~433 | `parse_mapping()`: parses YAML string into `ActionMapping` using `saphyr`. Handles `allow`, `deny`, and recursive `conditional` sections. `parse_conditional_actions()` recursively converts YAML nodes into `ConditionalActions`. |
| `matcher.rs` | ~817 | `PermissionMatcher::resolve()`: iterates provider groups and blocks, loads mappings, collects allow/deny/conditional permissions into `GroupPermissions`. Deduplicates via `HashSet`. Tracks missing mappings once per `(BlockType, type_name)` pair. |

### output/ module

| File | Lines | Purpose |
|------|-------|---------|
| `mod.rs` | ~582 | `OutputWriter`: routes to stdout (with colored headers) or directory (one file per group). `sanitize_filename()` prevents path traversal in output names. Canonical path validation ensures output stays within target directory. `write_missing_mappings()` outputs warnings to stderr. |
| `formatter.rs` | ~63 | `OutputFormatter` trait: `format(PermissionSets) -> String` and `extension() -> &str`. `PermissionSets` bundles allow/deny references. `create_formatter()` factory maps `OutputFormat` to concrete formatter. |
| `json.rs` | ~405 | `JsonFormatter`: produces valid AWS IAM policy document JSON (`Version: "2012-10-17"`). `PolicyDocument` and `Statement` are serde-serializable structs. Grouped mode creates one statement per service prefix. Deny before Allow. Actions sorted alphabetically within statements. |
| `hcl.rs` | ~464 | `HclFormatter`: produces `jsonencode({...})` HCL output. Single action uses quoted string, multiple uses array syntax. Grouped mode creates service-prefix statements. Deny before Allow. Manual string formatting (no HCL serialization library). |

### Tests

| Location | Purpose |
|----------|---------|
| `tests/integration/cli_tests.rs` | End-to-end CLI tests using `assert_cmd`. Tests help/version output, flag combinations, working directory validation, terraform execution, and error scenarios. Requires network for mapping repo tests. Should run with `--test-threads=1` to avoid git lock conflicts. |
| Inline `#[cfg(test)] mod tests` | Every source file contains unit tests. Coverage includes happy paths, error cases, security scenarios (path traversal, URL injection), edge cases, and deny-related scenarios. |

---

## 10. Conventions and Patterns to Follow

1. **Error handling**: Use `thiserror` derive for module-specific error enums. Use `#[from]` for automatic conversion. Only `main.rs` uses `anyhow`. Library code uses typed errors.

2. **Module visibility**: Submodules within `terraform/` and `mapping/` are private. Public API is exposed through `mod.rs` re-exports. Internal types stay internal.

3. **Security**: Every user-controlled input that becomes a filesystem path or command argument must be validated. Path traversal checks, URL protocol validation, branch name validation, file size limits, and symlink rejection are consistent patterns.

4. **Testing**: Tests are co-located with source code. Focus on behavior, not getters/setters. Security edge cases are always tested. Integration tests are separated in `tests/` directory.

5. **Logging**: Use `log::debug!` for verbose information, `log::info!` for progress, `log::warn!` for non-fatal issues, `log::error!` for failures that will propagate.

6. **Naming**: PascalCase for types, snake_case for functions/variables. Meaningful names over abbreviations. Types reflect domain concepts (e.g., `PermissionMatcher`, `GroupPermissions`, `MissingMapping`).

7. **Output ordering**: Deny before Allow. Actions sorted alphabetically. Service groups sorted alphabetically. Provider groups sorted alphabetically for stdout output.

---

## 11. Areas of Complexity

1. **`hcl_parser.rs`** (~1000+ lines): The most complex file. Recursive module parsing with provider context propagation. Handles root modules, local modules, and downloaded remote modules. Multiple traversal strategies for HCL bodies.

2. **`module_detector.rs`** (~1200+ lines): Complex module source classification (Registry, Git, Local, Root). Dual detection strategy (modules.json + regex fallback). Path resolution for external modules.

3. **`plan.rs`** (~1280 lines including tests): Copy plan computation with common ancestor detection. Significant test coverage for module scenarios.

4. **`ConditionalActions` resolution**: Recursive tree walk matching `HashSet<Vec<String>>` paths. Each intermediate path must be present for leaf actions to resolve.

---

## 12. Potential Areas for Future Change

Based on `specs/backlog.md` and `specs/brainstorming.md`:

- **saphyr -> saphyr-serde migration**: Will simplify `yaml_parser.rs` and `schema.rs` significantly. The `ActionMapping` and `ConditionalActions` types will gain `#[derive(Deserialize)]`.
- **Legacy code removal**: `parser.rs`, `json_types.rs`, and `PlanResult`/`execute_json()` in `plan.rs` can be removed once the legacy path is fully abandoned.
- **Additional block types**: The `action` block type is relatively new in Terraform. More block types could emerge.
- **Non-AWS providers**: The architecture is provider-prefixed (`MappingLoader::extract_provider()` splits on `_`), but the current implementation is AWS-focused. Extending to other providers would require changes in `hcl_parser.rs` (provider detection) and the mapping repository structure.
