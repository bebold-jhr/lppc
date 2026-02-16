# mapping-creator Architecture

This document captures the architectural structure, design decisions, and module responsibilities of the `mapping-creator` codebase. It is intended to be read by a software architect before consulting on code changes, replacing the need for a full codebase scan.

**Last updated**: 2026-02-16
**Rust edition**: 2021
**Crate name**: `lppc-mapping-creator`

---

## 1. High-Level Overview

`mapping-creator` is an interactive TUI (Terminal User Interface) application that guides the user through creating AWS IAM permission mapping files for the [lppc-cli](../../lppc-cli/) tool. It is designed exclusively for local developer usage -- there is no CI/CD use case.

The tool:

1. Validates a working directory that points to a local clone of the [lppc-aws-mappings](https://github.com/bebold-jhr/lppc-aws-mappings) repository
2. Presents a sequence of interactive selection screens (block type, terraform type, AWS service prefix, actions)
3. Applies heuristics to pre-select likely matches (service prefix from terraform type name, read/tag actions)
4. Generates a YAML mapping file and an integration test stub directory

The TUI is built with `ratatui` and `crossterm`. The application follows a linear, step-by-step wizard pattern where each step depends on the output of the previous one.

---

## 2. Project Context

`mapping-creator` lives in a monorepo alongside the `lppc-cli` tool:

```
/lppc/
  CLAUDE.md          -- Code style, test philosophy, workflow instructions
  README.md          -- Project overview
  .claude/
    agents/
      software-architect.md   -- Architect agent system prompt
      security-reviewer.md    -- Security review agent
    rules/
      dependency-check.md     -- Rules for adding Rust dependencies
  lppc-cli/           -- The CLI tool that consumes mapping files (Rust)
  mapping-creator/    -- THIS codebase (Rust, interactive TUI)
```

The tool operates on a local clone of the external mapping repository (default: `https://github.com/bebold-jhr/lppc-aws-mappings`). That repository has the following relevant structure:

```
lppc-aws-mappings/
  sources/
    terraform/
      resource_schemas.json            -- JSON array of resource type names
      data_source_schemas.json         -- JSON array of data source type names
      ephemeral_resource_schemas.json  -- JSON array of ephemeral resource type names
      action_schemas.json              -- JSON array of action type names
    aws/
      aws-servicereference-index.json  -- JSON array of {service, url} objects
      {service}.json                   -- Per-service action definitions from AWS
  mappings/
    resource/
      {terraform_type}.yml             -- Generated mapping files
    data/
    ephemeral/
    action/
  integration-tests/
    resource/
      {terraform_type}/                -- Generated integration test stubs
        providers.tf
        main.tf
        data.tf
        tests/{terraform_type}.tftest.hcl
    data/
    ephemeral/
    action/
```

---

## 3. Module Structure and Dependency Flow

The codebase is a flat module structure (no nested module directories). All modules are declared in `main.rs` and are crate-private -- there is no `lib.rs`.

```
main.rs          (entry point, orchestration pipeline, logging, path validation)
  |
  +-- cli.rs               (CLI argument parsing via clap)
  +-- block_type.rs        (BlockType enum: Action | Data | Ephemeral | Resource)
  +-- schema.rs            (Terraform type loading and filtering)
  +-- service.rs           (AWS service reference loading and matching)
  +-- action.rs            (AWS action loading, preselection, wildcard computation)
  +-- ui.rs                (TUI components: single selectors, action multi-selector)
  +-- generator.rs         (YAML mapping file and integration test stub generation)
  +-- provider_versions.rs (Dynamic provider version resolution with GitHub API + cache)
```

### Dependency flow (acyclic)

```
main -> cli, block_type, schema, service, action, ui, generator, provider_versions
schema -> block_type
service -> (standalone, no internal deps)
action -> (standalone, no internal deps)
ui -> action, block_type, service
generator -> block_type, provider_versions
provider_versions -> (standalone, no internal deps)
```

Key observations:
- `ui.rs` depends on `action.rs` (for `compute_selected_actions` and `Action` type), `block_type.rs` (for `BlockType` and `as_str()`), and `service.rs` (for `ServiceReference`).
- `generator.rs` depends on `block_type.rs` and `provider_versions.rs` (for `ProviderVersions` struct).
- `provider_versions.rs` is standalone with no internal dependencies. It handles GitHub API communication and YAML cache management.
- `schema.rs` depends only on `block_type.rs`.
- Dependencies flow cleanly downward from `main.rs`. There are no circular dependencies.

---

## 4. Key Data Types and Their Relationships

### 4.1 Block Type

```
BlockType (enum): Action | Data | Ephemeral | Resource
  +-- ALL: [BlockType; 4]                  -- Constant array of all variants
  +-- schema_file() -> &'static str        -- Path within working dir to schema JSON
  +-- mapping_dir() -> &'static str        -- Path within working dir to mapping directory
  +-- integration_test_dir() -> &'static str -- Path within working dir to test directory
  +-- terraform_docs_path() -> &'static str -- URL path segment for Terraform docs
  +-- Display: "action" | "data" | "ephemeral" | "resource"
  +-- as_str() -> &'static str             -- Same as Display (defined in ui.rs)
```

### 4.2 Service Reference

```
ServiceReference (serde Deserialize)
  +-- service: String       -- e.g., "ec2", "iam", "s3"
  +-- url: String           -- Full URL to service reference JSON
```

### 4.3 Action Types

```
Action (serde Deserialize)
  +-- name: String                          -- e.g., "CreateSubnet", "DescribeSubnets"
  +-- annotations: Option<ActionAnnotations>
  +-- is_tagging_only() -> bool
  +-- should_preselect() -> bool            -- tagging OR starts with List/Describe/Get

ActionAnnotations
  +-- properties: ActionProperties

ActionProperties
  +-- is_list: bool
  +-- is_permission_management: bool
  +-- is_tagging_only: bool
  +-- is_write: bool

ServiceActions (serde Deserialize)
  +-- name: String              -- Service prefix name
  +-- actions: Vec<Action>      -- All available actions for the service

SelectedActions                     -- Return type of select_actions()
  +-- allow_indices: HashSet<usize> -- Indices selected as "allow"
  +-- deny_indices: HashSet<usize>  -- Indices selected as "deny"
  Invariant: allow_indices and deny_indices are always disjoint

ComputedActions                     -- Return type of compute_selected_actions()
  +-- allow: Vec<String>            -- Allow action strings (with wildcards applied)
  +-- deny: Vec<String>             -- Deny action strings (always individual, no wildcards)
```

### 4.4 Generator Types

```
GeneratorConfig<'a>
  +-- working_dir: &'a Path
  +-- block_type: BlockType
  +-- terraform_type: &'a str
  +-- service_reference_url: &'a str
  +-- allow_actions: Vec<String>    -- Computed allow action strings (with wildcards applied)
  +-- deny_actions: Vec<String>     -- Computed deny action strings (always individual)
  +-- provider_versions: &'a ProviderVersions -- Dynamically resolved provider versions

GeneratedFiles
  +-- mapping_file: String          -- Relative path to generated mapping YAML
  +-- test_dir: String              -- Relative path to generated test directory
  +-- test_files: Vec<String>       -- Relative filenames within test_dir
```

### 4.5 Provider Version Types

```
ProviderVersions (serde Serialize + Deserialize, Clone, PartialEq)
  +-- aws: String                   -- Latest version of hashicorp/aws provider
  +-- time: String                  -- Latest version of hashicorp/time provider
  +-- random: String                -- Latest version of hashicorp/random provider
  +-- get(name) -> &str             -- Lookup by provider name
  +-- set(name, version)            -- Update by provider name

ProviderVersionCache (internal to provider_versions.rs)
  +-- last_updated: DateTime<Utc>   -- RFC 3339 timestamp of last successful full refresh
  +-- providers: ProviderVersions   -- Cached version strings
```

### 4.6 UI Components (internal to ui.rs)

```
TerminalGuard                       -- RAII guard for raw mode + alternate screen
  +-- Drop restores terminal state

SingleSelector                      -- Filterable single-selection list
  +-- all_items: Vec<String>
  +-- filtered_indices: Vec<usize>  -- Maps display position to original index
  +-- cursor_position: usize        -- Position within filtered list
  +-- list_state: ListState         -- Ratatui widget state
  +-- filter_text: String
  +-- filterable: bool              -- Whether typing filters the list

ActionSelector<'a>                  -- Three-state multi-selection with filter (split pane)
  +-- actions: &'a [Action]
  +-- service_prefix: &'a str
  +-- allow_indices: HashSet<usize>     -- Original indices of "allow" actions
  +-- deny_indices: HashSet<usize>      -- Original indices of "deny" actions
  +-- filtered_indices: Vec<usize>
  +-- cursor_position: usize
  +-- list_state: ListState
  +-- filter_text: String
```

---

## 5. Control Flow / Processing Pipeline

```
main()
  |
  +-- run() -> Result<()>
       1. Args::parse()                             -- clap parses CLI arguments
       2. init_logging(verbose)                      -- env_logger: debug or warn level
       3. validate_working_directory(path)            -- resolve, canonicalize, verify is dir
       4. select_block_type()                        -- TUI: SingleSelector, non-filterable
            -> BlockType
       5. load_terraform_types(working_dir, block_type)
            -> Read schema JSON, deserialize to Vec<String>
       6. filter_unmapped_types(working_dir, block_type, types)
            -> Remove types that have existing .yml in mapping dir
            -> Validate type names (path traversal check)
            -> If empty, print message and return Ok(())
       7. select_terraform_type(unmapped_types)      -- TUI: SingleSelector, filterable
            -> String (the selected terraform type name)
       8. load_service_references(working_dir)
            -> Read aws-servicereference-index.json
            -> Vec<ServiceReference>
       9. extract_service_hint(terraform_type)
            -> Strip "aws_" prefix, take segment before next "_"
            -> Option<String>
      10. find_best_match(hint, services)
            -> Exact match in service list
            -> Option<&ServiceReference>
      11. select_service_prefix(services, preselected_index)
            -> TUI: SingleSelector, filterable, with pre-positioned cursor
            -> ServiceReference
      12. load_service_actions(working_dir, service_prefix)
            -> Read sources/aws/{service}.json
            -> ServiceActions
      13. get_preselected_indices(actions)
            -> Indices of tagging/List/Describe/Get actions
      14. select_actions(actions, service_prefix, preselected_indices)
            -> TUI: ActionSelector, split pane, three-state multi-select
            -> SelectedActions { allow_indices, deny_indices }
      15. compute_selected_actions(service_prefix, actions, allow_indices, deny_indices)
            -> Apply wildcard consolidation for allow (List*, Describe*, Get*)
            -> Deny actions always listed individually (no wildcards)
            -> ComputedActions { allow, deny }
      16. resolve_provider_versions()
            -> Check ~/.lppc/provider-versions.yml cache (24h expiry)
            -> If fresh, use cached versions
            -> If stale/missing, fetch from GitHub API (3 GET requests)
            -> Partial failure: merge API successes with cached fallbacks
            -> Write updated cache (only update last_updated if all succeed)
            -> ProviderVersions { aws, time, random }
      17. GeneratorConfig { ..., allow_actions, deny_actions, provider_versions }
      18. generate_files(config)
            a. Validate terraform_type (path traversal check)
            b. generate_mapping_file(config)
                -> Verify no existing file (bail if exists)
                -> generate_terraform_doc_url()
                -> generate_mapping_yaml()         -- Build YAML string manually
                -> Write to mappings/{block_type}/{terraform_type}.yml
            c. generate_integration_tests(config)
                -> Verify no existing directory (bail if exists)
                -> Create directory + tests/ subdirectory
                -> Write providers.tf (dynamic versions from ProviderVersions)
                -> Write data.tf (aws_caller_identity)
                -> Write tests/{terraform_type}.tftest.hcl (test template)
            d. Return GeneratedFiles
      18. print_success_message(generated_files)
            -> Pretty-print created file paths to stdout
```

---

## 6. External Dependencies and Their Roles

| Crate           | Version | Role                                                       |
|-----------------|---------|-------------------------------------------------------------|
| `clap`          | 4       | CLI argument parsing with derive macros                    |
| `anyhow`        | 1       | Error handling throughout (not just main, unlike lppc-cli) |
| `env_logger`    | 0.11    | Logging initialization                                     |
| `log`           | 0.4     | Logging facade                                             |
| `serde`         | 1       | JSON deserialization for schema and service files           |
| `serde_json`    | 1       | JSON parsing (schema files, GitHub API responses)          |
| `ratatui`       | 0.30    | Terminal UI framework (widgets, layout, rendering)          |
| `crossterm`     | 0.29    | Terminal manipulation backend (raw mode, events, alternate screen) |
| `ureq`          | 3       | Synchronous HTTP client for GitHub API calls               |
| `serde-saphyr`  | 0.0     | YAML serialization/deserialization for provider version cache |
| `dirs`          | 6       | Home directory resolution for `~/.lppc` cache path         |
| `chrono`        | 0.4     | Timestamp parsing and comparison for cache freshness       |

Dev dependencies: `tempfile` (temporary directories for tests).

**Note on error handling**: Unlike `lppc-cli` which uses `thiserror` for typed library errors and reserves `anyhow` for `main()`, `mapping-creator` uses `anyhow::Result` and `anyhow::bail!` throughout all modules. This is a pragmatic choice for a single-binary interactive tool where granular error recovery is not needed -- all errors ultimately terminate the wizard with an error message to the user.

---

## 7. Design Patterns Used

### Wizard / Pipeline Pattern (main.rs)
The application follows a strict linear pipeline where each step depends on the output of the previous step. The `run()` function orchestrates this sequence. Each step either succeeds and passes its result forward, or fails and terminates the entire pipeline via `?` propagation.

### RAII Terminal Guard (ui.rs)
`TerminalGuard` uses Rust's drop semantics to ensure the terminal is always restored to its normal state (raw mode disabled, alternate screen exited), even if a panic occurs. This is critical for TUI applications that modify terminal state.

### Reusable Selector Components (ui.rs)
Two internal components encapsulate selection UIs:
- `SingleSelector` -- Used for block type selection (non-filterable), terraform type selection (filterable), and service prefix selection (filterable with preselection). The same struct and rendering logic handles all three cases via the `filterable` flag and `initial_position` parameter.
- `ActionSelector` -- Specialized multi-selection component with split pane layout, filter support, toggle/toggle-all, and real-time computed action display.

### Index-Based Selection (action.rs, ui.rs)
Throughout the codebase, selections are tracked by index into the original data arrays rather than by copying selected items. This allows filter and display operations to work with original indices while the filtered view remaps display positions. `filtered_indices: Vec<usize>` maps display positions to original indices.

### Wildcard Consolidation (action.rs)
`compute_selected_actions()` implements a pattern where if NO action within a prefix group (List*, Describe*, Get*) is deselected (i.e., every action in the group is in either allow or deny), the allow side uses a wildcard entry. Moving an action to deny does not break the wildcard -- only deselection (missing from both sets) breaks it. Deny actions are always listed individually, never as wildcards. This only applies when the group has more than one action (a single List action does not become List*).

### Heuristic Pre-selection (service.rs, action.rs)
The tool pre-selects likely choices based on heuristics:
- Service prefix: extracted from the terraform type name by stripping `aws_` and taking the first `_`-delimited segment (e.g., `aws_iam_role` -> `iam`). Exact match only.
- Actions: all tagging-only, List*, Describe*, and Get* actions are pre-selected as a starting point.

### Path Traversal Validation (schema.rs, action.rs, generator.rs)
Every module that constructs filesystem paths from user-influenced data (terraform type names, service prefixes) validates the input against path traversal attacks. The validation pattern is consistent: reject empty strings, forward/backward slashes, null bytes, leading dots, and `..` sequences.

### Cache with Graceful Degradation (provider_versions.rs)
Provider version resolution follows a cache-first strategy with 24-hour expiry. The cache file at `~/.lppc/provider-versions.yml` is shared with `lppc-cli`. On API failure, stale cached versions are used as fallback with a warning. Partial API failures merge successful results with cached values. The `last_updated` timestamp is only refreshed when all providers are successfully resolved, ensuring the next run retries failed providers.

### Testable Network Isolation (provider_versions.rs)
The resolution logic accepts a fetcher function parameter (`impl Fn(&str) -> Result<String>`) to decouple it from the HTTP client. The public API (`resolve_provider_versions()`) passes the real `fetch_latest_version` function, while tests use mock fetchers (success, failure, partial) to exercise all cache/merge/fallback paths without network access.

---

## 8. Important Architectural Decisions and Constraints

1. **Interactive-only tool**: The tool is designed exclusively for interactive local use. There is no non-interactive/batch mode and no CI/CD use case. This justifies the use of `anyhow` everywhere and the TUI-first approach.

2. **No lib.rs**: All modules are declared in `main.rs` as `mod` items. There is no library crate. This is appropriate for a single-binary application with no external consumers of its types.

3. **Flat module structure**: All 8 source files are at the same level (`src/*.rs`). There are no subdirectory modules. This is appropriate for the codebase size and the limited number of responsibilities.

4. **Working directory as parameter, not current directory**: The tool always operates on an explicitly-provided path to the mappings repository. It resolves relative paths against the current directory and canonicalizes them.

5. **Schema files assumed to exist**: The tool does not download or generate schema files. It reads JSON files that are expected to exist in the mappings repository (`sources/terraform/*.json` and `sources/aws/*.json`). Missing files result in clear error messages.

6. **Fail-fast on existing outputs**: Both `generate_mapping_file()` and `generate_integration_tests()` bail if their target files/directories already exist. This guards against overwriting existing work and is consistent with the fact that only unmapped types are shown in the selection step.

7. **Manual YAML generation**: The YAML mapping file is built via string concatenation rather than a YAML serialization library. This is acceptable given the simple, fixed structure of the output.

8. **Dynamic provider versions**: The generated `providers.tf` uses dynamically resolved versions fetched from the GitHub API (`api.github.com/repos/hashicorp/terraform-provider-{name}/releases/latest`). Versions are cached at `~/.lppc/provider-versions.yml` with a 24-hour expiry. The cache is shared with `lppc-cli` and follows the same directory structure. On API failure, stale cached versions are used as fallback. If no cache exists and the API is unreachable, the tool fails with an error.

9. **Exit codes**: The tool exits with code 0 on success and code 1 for any error, including user cancellation (ESC/Ctrl+C in the TUI).

10. **Security**: Path traversal validation is applied to all user-influenced data used in filesystem path construction: terraform type names (in `schema.rs`, `generator.rs`) and service prefixes (in `action.rs`).

---

## 9. File-by-File Summary

| File | Lines | Purpose |
|------|-------|---------|
| `src/main.rs` | ~215 | Entry point. Module declarations. `run()` orchestrates the full wizard pipeline including provider version resolution. `init_logging()` configures env_logger. `validate_working_directory()` resolves and canonicalizes the path. Unit tests for path validation. |
| `src/cli.rs` | ~21 | `Args` struct with clap derive macros. Positional `working_dir: PathBuf` and optional `--verbose` flag. Includes disclaimer in help text. |
| `src/block_type.rs` | ~146 | `BlockType` enum with four variants. `ALL` constant. Path methods for schema files, mapping directories, integration test directories, and Terraform documentation URLs. `Display` impl. Comprehensive unit tests. |
| `src/schema.rs` | ~219 | `load_terraform_types()` reads and parses schema JSON files. `filter_unmapped_types()` removes types that have existing `.yml` mapping files. `is_valid_type_name()` validates against path traversal. Unit tests including security edge cases. |
| `src/service.rs` | ~199 | `ServiceReference` serde type. `load_service_references()` reads the AWS service index JSON. `extract_service_hint()` derives a service prefix guess from a terraform type name. `find_best_match()` performs exact-match lookup. Unit tests cover parsing, hint extraction, and matching. |
| `src/action.rs` | ~510 | `Action`, `ActionProperties`, `ActionAnnotations`, `ServiceActions` serde types. `SelectedActions` and `ComputedActions` structs for three-state selection. `load_service_actions()` reads per-service JSON with path traversal check. `get_preselected_indices()` identifies tagging/read actions. `compute_selected_actions()` applies deny-aware wildcard consolidation logic with disjointness assertion. Extensive unit tests including deny-specific scenarios. |
| `src/ui.rs` | ~920 | **The largest file.** `TerminalGuard` RAII type. `SingleSelector` struct with filter, navigation, and rendering. `ActionSelector` struct with three-state selection (allow/deny/deselected), `cycle_current()` for SPACEBAR cycling, three-state `toggle_all()`, and split-pane rendering with separate Allow/Deny sections. Public functions: `select_block_type()`, `select_terraform_type()`, `select_service_prefix()`, `select_actions()` (returns `SelectedActions`). Left pane uses `[✓]` green / `[✗]` red / `[ ]` indicators. Unit tests for filter, selection preservation, cycling, toggle logic, and navigation. |
| `src/generator.rs` | ~760 | `GeneratorConfig` struct with `allow_actions`, `deny_actions`, and `provider_versions`. `generate_files()` orchestrates mapping file and test stub creation. `generate_mapping_yaml()` outputs separate `deny:` and `allow:` YAML sections (deny before allow, omitting empty sections). `generate_integration_tests()` creates directory structure with four files. `generate_providers_tf()` produces dynamic HCL from `ProviderVersions`. URL generation helpers. `print_success_message()` outputs tree-formatted success output. `is_valid_terraform_type()` path traversal guard. `TestFiles` internal struct. Extensive unit tests including deny-section and dynamic-version scenarios. |
| `src/provider_versions.rs` | ~330 | `ProviderVersions` struct (public) and `ProviderVersionCache` (internal). `resolve_provider_versions()` entry point orchestrates cache check, GitHub API fetch, and cache write. `load_cache()`/`save_cache()` handle YAML serialization via `serde-saphyr`. `fetch_latest_version()` makes HTTPS GET to GitHub API with `ureq` (10s timeout, custom User-Agent). `is_cache_fresh()` checks 24h expiry. `is_valid_version_string()` validates digits-and-dots. `strip_version_prefix()` removes leading `v`. Testable via `resolve_with_cache_and_fetcher()` which accepts a mock fetcher function. Extensive unit tests covering cache roundtrips, freshness, partial failures, and fallback logic. |

---

## 10. Conventions and Patterns to Follow

1. **Error handling**: Use `anyhow::Result`, `anyhow::bail!`, and `.context()` / `.with_context()` for all error propagation. Errors should include descriptive messages that identify the file or input that caused the failure.

2. **Module visibility**: All modules are private (`mod`, not `pub mod`). Public items within modules are exposed via `pub` and imported in `main.rs` with `use` statements. There are no re-exports.

3. **Security**: Every string that becomes part of a filesystem path must be validated with an `is_valid_*` function that rejects: empty strings, forward slashes, backslashes, null bytes, leading dots, and `..` sequences.

4. **Testing**: Tests are co-located with source code in `#[cfg(test)] mod tests` blocks. Tests use `tempfile::TempDir` for filesystem operations. Test helpers like `setup_test_dir()` and `create_test_action()` follow a consistent naming pattern. Tests cover happy paths, error cases, and security edge cases.

5. **Logging**: `log::debug!` for progress and diagnostic information. `log::warn!` for non-fatal issues (e.g., skipping invalid type names). No `log::info!` is used within modules -- only in `main.rs`.

6. **Naming**: PascalCase for types, snake_case for functions/variables. Meaningful names that reflect domain concepts (e.g., `preselected_action_indices`, `service_reference_url`, `terraform_docs_path`).

7. **TUI conventions**: All TUI interactions go through `ui.rs`. Raw terminal setup/teardown is handled by `TerminalGuard`. ESC cancels with an error message. Cursor position always refers to the filtered (display) list. Original indices are used for selection state.

8. **JSON input conventions**: All JSON inputs use serde `Deserialize`. Struct fields use Rust naming conventions with `#[serde(rename = "PascalCase")]` for AWS JSON field names.

---

## 11. Areas of Complexity

1. **`ui.rs`** (~817 lines): The most complex file. Contains two distinct UI components (`SingleSelector` and `ActionSelector`) with their own state management, filtering, rendering, and event handling. The `ActionSelector` is particularly complex due to its split-pane layout, multi-select with toggle semantics, and real-time computed action display in the right pane.

2. **`compute_selected_actions()` in `action.rs`**: The wildcard consolidation logic has several edge cases: groups with only one action, partial selection, mixed selection of wildcard-eligible and non-eligible actions. The threshold for wildcard usage is `len > 1`, meaning a single List action is listed explicitly.

3. **Index indirection in selectors**: Both `SingleSelector` and `ActionSelector` maintain a `filtered_indices: Vec<usize>` that maps display positions to original data indices. The `cursor_position` refers to the display position, while `allow_indices`/`deny_indices` (in `ActionSelector`) and the return value use original indices. This indirection is necessary for filtering but requires careful tracking.

4. **`generator.rs`** (~633 lines): While conceptually straightforward, this file contains multiple template generation functions and thorough test coverage that account for the bulk of its size. The YAML is built manually with string operations.

---

## 12. Potential Areas for Future Change

Based on `specs/brainstorming.md` and `specs/deny-section.md`:

### Other Potential Changes

- **Additional block types**: If Terraform adds new block types beyond resource/data/ephemeral/action, `BlockType` would need new variants and corresponding path mappings.
- **Additional providers**: The `ProviderVersions` struct currently supports `aws`, `time`, and `random`. Adding more providers requires updating the struct, the `PROVIDERS` constant, and the `get()`/`set()` methods.
- **Confirmation dialog for ESC**: The brainstorming spec mentions a confirmation dialog when pressing ESC (vs immediate cancel for Ctrl+C). The current implementation cancels immediately on ESC without confirmation.
- **`BlockType::as_str()` duplication**: `as_str()` is defined in `ui.rs` on `BlockType`, duplicating the logic in `Display::fmt()` in `block_type.rs`. These could be consolidated.
