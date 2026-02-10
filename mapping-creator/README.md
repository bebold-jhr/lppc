# lppc-mapping-creator - Interactive Mapping File Creator

An interactive CLI tool that guides you through creating AWS IAM permission mapping files for use with [lppc](../lppc-cli/README.md). It identifies Terraform block types that still need a mapping, helps you find the matching AWS service and actions, and generates both the mapping YAML file and an integration test stub.
The tool is used to create [lppc-aws-mappings](https://github.com/bebold-jhr/lppc-aws-mappings).

## Requirements

- **Rust** (1.70 or later) for building from source
- A local clone of the [lppc-aws-mappings](https://github.com/bebold-jhr/lppc-aws-mappings) repository (or compatible structure)

## Installation

### Building from Source

```bash
git clone https://github.com/bebold-jhr/lppc.git
cd lppc/mapping-creator
cargo build --release
```

The binary will be available at `target/release/lppc-mapping-creator`.

## Getting Started

Point the tool at your local mappings repository:

```bash
lppc-mapping-creator /path/to/lppc-aws-mappings
```

The tool will launch an interactive TUI that walks you through the full mapping creation workflow.

## Parameters

| Parameter   | Short | Description                                                                    |
|-------------|-------|--------------------------------------------------------------------------------|
| `--help`    | `-h`  | Display help information                                                       |
| `--version` | `-v`  | Display the current version                                                    |
| `--verbose` |       | Enable debug-level logging for troubleshooting                                 |

The first positional argument is the **working directory** — the path to the mappings repository. It accepts both absolute and relative paths.

## How It Works

The tool guides you through an interactive workflow in six steps:

### 1. Validate Working Directory

The provided path is verified to exist and be a directory.

### 2. Select Block Type

Choose one of four Terraform block types:

| Block type  | Schema file                                         | Mapping directory     |
|-------------|-----------------------------------------------------|-----------------------|
| `resource`  | `sources/terraform/resource_schemas.json`           | `mappings/resource/`  |
| `data`      | `sources/terraform/data_source_schemas.json`        | `mappings/data/`      |
| `ephemeral` | `sources/terraform/ephemeral_resource_schemas.json` | `mappings/ephemeral/` |
| `action`    | `sources/terraform/action_schemas.json`             | `mappings/action/`    |

### 3. Select Terraform Type

The tool loads all known types for the selected block type and filters out those that already have a mapping file. Only unmapped types are shown. You can type to filter the list and navigate with arrow keys.

### 4. Select AWS Service Prefix

AWS organizes IAM actions by service prefix. The tool loads the service index from `sources/aws/aws-servicereference-index.json` and attempts to pre-select the best match based on a heuristic: it strips the `aws_` prefix from the Terraform type and uses the substring up to the next underscore (e.g. `aws_iam_role` matches `iam`). You can confirm or change the selection.

### 5. Select Actions

A split-pane view displays all available actions for the selected service. The tool pre-selects:

- All actions marked as **tagging-only**
- All `List*` actions (consolidated as `{service}:List*`)
- All `Describe*` actions (consolidated as `{service}:Describe*`)
- All `Get*` actions (consolidated as `{service}:Get*`)

If you deselect individual actions within a `List*`/`Describe*`/`Get*` group, the wildcard is replaced with explicit action names.

**Controls:**
- `SPACE` — toggle selection of the current item
- `TAB` — toggle all visible (filtered) items
- Type to filter actions (case-insensitive substring match)
- `BACKSPACE` — remove the last character from the filter
- `ENTER` — confirm selection (requires at least one action)

### 6. Generate Files

The tool creates:

**Mapping file** at `mappings/{block_type}/{terraform_type}.yml`:

```yaml
---
metadata:
  aws:
    documentation: https://docs.aws.amazon.com/service-authorization/latest/reference/reference_policies_actions-resources-contextkeys.html
    service-reference: https://servicereference.us-east-1.amazonaws.com/v1/{service}/{service}.json
  terraform:
    documentation: https://registry.terraform.io/providers/hashicorp/aws/latest/docs/resources/{type}
actions:
  - ec2:Describe*
  - ec2:CreateSubnet
  - ec2:DeleteSubnet
```

**Integration test stub** at `integration-tests/{block_type}/{terraform_type}/`:

- `providers.tf` — pinned provider versions (AWS, Time, Random)
- `main.tf` — empty block stub for the selected type
- `data.tf` — `aws_caller_identity` data source
- `tests/{terraform_type}.tftest.hcl` — test template with deployer role setup and assertion placeholders

## Keyboard Navigation

The TUI supports the following keys throughout all selection screens:

| Key          | Action                                 |
|--------------|----------------------------------------|
| `UP` / `k`   | Move cursor up                         |
| `DOWN` / `j` | Move cursor down                       |
| `ENTER`      | Confirm selection                      |
| `ESC` / `q`  | Cancel and exit                        |
| Type text    | Filter list (on filterable screens)    |
| `BACKSPACE`  | Remove last filter character           |
| `SPACE`      | Toggle selection (service and actions) |
| `TAB`        | Toggle all visible items (actions)     |

## Examples

### Basic Usage

```bash
lppc-mapping-creator /path/to/lppc-aws-mappings
```

### Debug Mode

Enable verbose logging to see detailed progress:

```bash
lppc-mapping-creator --verbose /path/to/lppc-aws-mappings
```

### Relative Path

```bash
lppc-mapping-creator ../lppc-aws-mappings
```

## License

MIT
