# lppc - Least Privilege Policy Creator

A CLI tool that generates minimal AWS IAM policies based on static analysis of Terraform code. It scans your Terraform configurations and produces least-privilege permission sets for deployer roles, reducing the manual effort and error-prone process of crafting IAM policies.

## Requirements

- **Terraform** must be installed and available in your `PATH`

## Installation

Download a pre-built binary from the [releases page](https://github.com/bebold-jhr/lppc/releases) and add it to your `PATH`.

### Building from Source

Requires **Rust** (1.70 or later):

```bash
git clone https://github.com/bebold-jhr/lppc.git
cd lppc
cargo build --release
```

The binary will be available at `target/release/lppc`.

## Getting Started

The simplest way to use lppc is to run it in a directory containing Terraform files:

```bash
cd /path/to/your/terraform/project
lppc
```

This will:
1. Clone or update the default mapping repository (cached in `~/.lppc/`)
2. Copy files to an isolated temp directory and run `terraform init -backend=false` to resolve modules
3. Parse Terraform files directly using HCL parsing (no AWS credentials required)
4. Output the required IAM permissions in HCL grouped format to stdout

## Parameters

### General Options

| Parameter    | Short | Description                                          |
|--------------|-------|------------------------------------------------------|
| `--help`     | `-h`  | Display help information                             |
| `--version`  | `-v`  | Display the current version                          |
| `--no-color` | `-n`  | Suppress colored output (useful for CI/CD pipelines) |
| `--verbose`  |       | Enable debug-level logging for troubleshooting       |

### Working Directory

| Parameter       | Short | Default           | Description                                                             |
|-----------------|-------|-------------------|-------------------------------------------------------------------------|
| `--working-dir` | `-d`  | Current directory | Path to the directory containing Terraform files (absolute or relative) |

### Mapping Repository

The mapping repository contains YAML files that define which AWS IAM permissions are required for each Terraform resource type.

| Parameter            | Short | Default                                           | Description                                               |
|----------------------|-------|---------------------------------------------------|-----------------------------------------------------------|
| `--mappings-url`     | `-m`  | `https://github.com/bebold-jhr/lppc-aws-mappings` | Git repository URL containing the permission mappings     |
| `--refresh-mappings` | `-r`  |                                                   | Force an immediate update of the mapping repository cache |

The mapping repository is cached locally in `~/.lppc/` and automatically refreshed every 24 hours. If the remote repository is unreachable, the cached version is used with a warning.

## Examples

### Basic Usage

Run in the current directory with default settings:

```bash
lppc
```

### Specify a Working Directory

Analyze Terraform files in a specific directory:

```bash
lppc --working-dir /path/to/terraform/project
# or using short form
lppc -d ./infrastructure/aws
```

### Debug Mode

Enable verbose logging to troubleshoot issues:

```bash
lppc --verbose
```

### CI/CD Pipeline Usage

Disable colors for cleaner log output:

```bash
lppc --no-color --working-dir ./terraform
```

### Custom Mapping Repository

Use a custom mapping repository (supports HTTPS and SSH URLs):

```bash
# HTTPS
lppc --mappings-url https://github.com/your-org/custom-mappings.git

# SSH
lppc -m git@github.com:your-org/custom-mappings.git
```

### Force Mapping Refresh

Force an update of the mapping repository regardless of cache age:

```bash
lppc --refresh-mappings
# or short form
lppc -r
```

### Combined Options

```bash
lppc --verbose --working-dir ./terraform --refresh-mappings --no-color
```

### Output Options

| Parameter         | Short | Default  | Description                                                          |
|-------------------|-------|----------|----------------------------------------------------------------------|
| `--output-format` | `-f`  | `hcl-grouped` | Output format: `json`, `json-grouped`, `hcl`, `hcl-grouped` |
| `--output-dir`    | `-o`  | (stdout) | Directory to write output files (one file per deployer role)         |

#### Output Formats

- **json**: AWS IAM policy document in JSON format
- **json-grouped**: AWS IAM policy with statements grouped by service prefix
- **hcl**: Terraform HCL with `jsonencode()` for inline policies
- **hcl-grouped**: HCL format with statements grouped by service prefix (default)

#### Examples

Generate JSON policy documents:

```bash
lppc -f json
```

Write output files to a directory:

```bash
lppc --output-dir ./policies --output-format json
# Creates files like: ./policies/NetworkDeployer.json
```

## Local Module Support

lppc supports local modules both inside and outside the working directory:

```hcl
# Internal module (inside working directory) - supported
module "vpc" {
  source = "./modules/vpc"
}

# External module (outside working directory) - supported
module "shared" {
  source = "../../shared-modules/networking"
}
```

External local modules are automatically detected and copied to the isolated execution environment with the correct relative path structure preserved. Module detection works via:

1. **Primary**: Parsing `.terraform/modules/modules.json` (if available from a previous `terraform init`)
2. **Fallback**: Regex parsing of `.tf` files (for CI/CD environments without `.terraform/`)

## Remote Module Support

lppc analyzes modules sourced from Git repositories and the Terraform Registry. After `terraform init` downloads these modules to `.terraform/modules/`, their Terraform code is analyzed to extract IAM permissions required by resources within the module.

```hcl
# Terraform Registry module - analyzed
module "vpc" {
  source  = "terraform-aws-modules/vpc/aws"
  version = "5.0.0"
}

# Git module - analyzed
module "s3" {
  source = "git::https://github.com/org/terraform-aws-s3.git?ref=v1.0.0"
}

# Registry submodule - analyzed
module "filter" {
  source = "be-bold/account-lookup/aws//modules/filter"
}
```

### Supported Source Formats

**Terraform Registry:**
- Standard: `namespace/name/provider` (e.g., `terraform-aws-modules/vpc/aws`)
- With registry host: `registry.terraform.io/namespace/name/provider`
- Private registry: `app.terraform.io/org/name/provider`
- With submodule: `namespace/name/provider//submodule/path`

**Git Repositories:**
- HTTPS: `git::https://github.com/org/repo.git`
- SSH: `git::ssh://git@github.com/org/repo.git`
- GitHub shorthand: `github.com/org/repo`
- With ref: `git::https://github.com/org/repo.git?ref=v1.0.0`
- With subdirectory: `git::https://github.com/org/repo.git//modules/vpc`

### How It Works

1. lppc reads `.terraform/modules/modules.json` to discover all modules
2. For each module, lppc parses the downloaded Terraform code
3. IAM permissions from module resources are merged into the final policy

Use `--verbose` to see detailed module analysis information:

```bash
lppc --verbose
```

Example output:
```
[DEBUG] Discovered 2 remote module(s) from modules.json:
[DEBUG]   - vpc (registry: terraform-aws-modules/vpc/aws)
[DEBUG]   - s3 (git: https://github.com/org/s3.git (ref: v2.1.0))
[DEBUG] Extracted 47 block(s) from remote module 'vpc'
[DEBUG] Extracted 12 block(s) from remote module 's3'
```

## Disclaimer

The generated policies should be reviewed manually. Consider adding further constraints using IAM conditions and specifying concrete resource ARNs instead of wildcards where possible.

## License

MIT
