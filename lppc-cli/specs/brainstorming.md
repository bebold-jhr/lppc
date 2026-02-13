# Motivation

In terraform with AWS providers we usually have a setup in which we have an initial authentication mechanism (OIDC, AWS Roles Anywhere, ...). This is solely the entry point to get into the the account. The only permission of the first role which the user takes is to assume other roles. We call them "deployer roles". Each terraform root module gets its own deployer role(s).

Example:

Given the following folder structure:

```
10-network/
20-storage/
30-compute/
```

we would end up with 3 deployer roles:

```
NetworkDeployer
StorageDeployer
ComputeDeployer
```

```hcl
provider "aws" {
  assume_role {
    role_arn = "arn:aws:iam::123456789012:role/NetworkDeployer"
  }
}
```

The reason is that each root module only has a single AWS provider.
What if a root module contains multiple AWS providers?
Let's assume that network contains two different providers.

```hcl
provider "aws" {
  assume_role {
    role_arn = "arn:aws:iam::123456789012:role/NetworkDeployer"
  }
}

provider "aws" {
  alias = "dns_account"

  assume_role {
    role_arn = "arn:aws:iam::987654321012:role/DnsLookupRole"
  }
}
```

Given this setup, we would end up with 2 deployer permission sets:

```
DefaultDeployer      (from provider without alias)
DnsAccountDeployer   (from provider with alias "dns_account")
```

A deployer role only uses CRUD permissions to create, read, update and delete resources.
It is crucial to use least-privilege permissions for these roles. Doing this manually is an error-prone process, tedious and requires maintenance on each newly introduced resource or data source. This always leads to fatigue with the developers leading to permissions which are not least-privilege. There are possibilities to reduce permissions, but they are situated late in the process. The idea of this tool is to shift-left and automate the creation for the policies of deployer roles, based on static code analysis.

Because there can be various AWS providers in each root module, the tool must collect separate permission lists for each provider. Providers are grouped by their `role_arn` expression string. Providers which only differ in region but use the same `role_arn` are grouped together.
If there is a case with two or more providers using the same `role_arn`, the first alias in alphabetical order wins for naming internally and for the output.

## How it works

The working directory is scanned for terraform code using direct HCL parsing (no terraform plan required). The terraform code is analyzed block type by block type (data, resource, ephemeral, ...). For each block type a respective mapping exists as a yaml file.

Example:

```hcl
data "aws_availability_zones" "this" {
    # ...
}
```

```yaml
allow:
  - "ec2:DescribeAvailabilityZones"
```

The mapping allows optional mapping based on the presence of (nested) properties as well.
Example:

```hcl
resource "aws_route53_zone" "private" {
  #...

  vpc {
    vpc_id = aws_vpc.this.id
  }

  comment = "managed by terraform"
}
```

```yaml
allow:
  - "route53:List*"
  - "route53:Get*"
  - "route53:CreateHostedZone"
  - "route53:DeleteHostedZone"
conditional:
  vpc:
    vpc_id:
      - route53:AssociateVPCWithHostedZone
      - route53:DisassociateVPCFromHostedZone
  comment:
    - route53:UpdateHostedZoneComment
```

We have to assume that nesting can have any depth. To keep it simple we only check for existence of the nested property in the code, not for a specific value.
Valid block types are `resource`, `data`, `ephemeral`, `action`. `action` is a fairly new block type available in terraform core: https://developer.hashicorp.com/terraform/language/block/action
Yaml files are available as an external source (git repository). The repository is cloned and each block types is checked for the necessary permissions. If a yaml file does not exist, it is tracked separately to inform the user that these resources are missing in the output and manual adjustments are still required.

```hcl
resource "aws_s3_bucket" "example" {
  bucket = "my-tf-test-bucket"
}
```
It is possible to define an explicit deny on permissions:

```yaml
deny:
  - "s3:GetObject"
allow:
  - "s3:List*"
  - "s3:Get*"
  - "s3:Describe*"
```

# Overall application

The application is built in Rust and is a CLI application.

## Name

The project name is `lppc` which stands for `least privilege policy creator`.

## Process

1. Check if the mapping exists and is up-to-date. Clone the mapping repo if it doesn't exist or update it if the last refresh was more than 24 hours ago
2. Create an isolated temporary directory for terraform execution (user's working directory is never modified)
3. Copy terraform files and resolve module dependencies to the temp directory
4. Run `terraform init -backend=false` in the isolated environment (no AWS credentials or backend configuration required)
5. Parse terraform files directly using HCL parsing
6. Lookup each resource/data source for each provider in the mapping repository
    + Only lookup yaml files for which code exists in terraform files
    + Keep mappings data in-memory for recurring lookups to reduce I/O
7. Create output as configured

## Features

+ Use colors and formatting to make an appealing and easy to read CLI application
+ Parameter `--working-dir` (short `-d`) Can take an absolute path or a relative path to change the directory from the default (see section "Working directory").
+ Parameter `--no-color` (short `-n`) Suppress the usage of colors for better readability in environments like CICD pipeline runners.
+ Parameter `--output-dir` (short `-o`) Define an absolute path or a relative path to an output directory. If the parameter has not been set, the output will be shown directly on stdout. When set, one file per role is created in the specified directory with the naming pattern `{OUTPUT_NAME}.{EXTENSION}` (e.g., `NetworkDeployer.json`). The extension is determined by the output format: `.json` for json/json-grouped, `.hcl` for hcl/hcl-grouped. If the directory does not exist, it will be created automatically. The info regarding missing mappings is solely shown on stderr.
+ Parameter `--output-format` (short `-f`) Defines the format of the output. Default is `hcl-grouped`. The following are supported:
    `json`: JSON format of an AWS policy document. Permissions are listed in alphabetical order.
    `json-grouped`: Like `json`, but each service prefix (`ec2`, `s3`,...) gets its own `statement` block.
    `hcl`: AWS policy document represented in HCL using `jsonencode()`. Permissions are in alphabetical order.
    `hcl-grouped`: Like `hcl`, but each service prefix (`ec2`, `s3`,...) gets its own `statement` block.
+ Parameter `--mappings-url` (short `-m`) URL which overwrites the default git repo URL containing the `*.yml` files with the mappings. The default repo is `https://github.com/bebold-jhr/lppc-aws-mappings`
+ Parameter `--version` (short `-v`) showing the current release version as offered by many rust libraries for parameter parsing.
+ Parameter `--help` (short `-h`) as offered by many rust libraries for parameter parsing. This section also shows a disclaimer that manual review and adding further constraints using conditions and setting specific resources is encouraged.
+ Parameter `--verbose` showing verbose log statements making debugging possible.
+ Parameter `--refresh-mappings` (short `-r`) force refresh of the mapping repo on startup.

## Error handling

Success exits with exit code `0` and any error exits with error code `1`.

## Working directory

The working directory is the directory which the app checks for terraform code. If nothing else is configured it's the same directory from which the binary has been executed. If the working directory doesn't contain any terraform files, the output is empty.

The user's working directory is **never modified**. All terraform operations (init, etc.) happen in an isolated temporary directory.

## Module Support

### Local Modules

Local modules are supported both inside and outside the working directory:

```hcl
# Internal module (inside working directory)
module "vpc" {
  source = "./modules/vpc"
}

# External module (outside working directory)
module "shared" {
  source = "../../shared-modules/networking"
}
```

External local modules are automatically detected and copied to the isolated execution environment with the correct relative path structure preserved.

Module detection works via:
1. **Primary**: Parsing `.terraform/modules/modules.json` (if available from a previous `terraform init`)
2. **Fallback**: Regex parsing of `.tf` files (for CI/CD environments without `.terraform/`)

### Remote Modules

Remote modules from Git repositories and the Terraform Registry are analyzed after `terraform init` downloads them to `.terraform/modules/`.

```hcl
# Terraform Registry module
module "vpc" {
  source  = "terraform-aws-modules/vpc/aws"
  version = "5.0.0"
}

# Git module
module "s3" {
  source = "git::https://github.com/org/terraform-aws-s3.git?ref=v1.0.0"
}

# Registry submodule
module "filter" {
  source = "be-bold/account-lookup/aws//modules/filter"
}
```

Supported source formats:

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

### Module Provider Mappings

Modules can map their internal provider references to root providers:

```hcl
module "vpc" {
  source = "./modules/vpc"

  providers = {
    aws           = aws.network
    aws.secondary = aws.network_dr
  }
}
```

The tool tracks provider mappings through nested module hierarchies, resolving module-local provider keys to root provider keys.

## Mapping permissions

### Mapping repository

The structure of the mapping repository is as follows: `{PROVIDER}/{BLOCK_TYPE}/{TYPE}.yaml`
Example: `aws/data/aws_availability_zones.yaml`
The repository is cached locally in the user's home directory. That works on every OS without having to implement different idiomatic ways.
A refresh is triggered if the last check for updates was 24 hours ago. A shallow clone is used to minimize bandwidth.
The cache directory is a hidden directory named `.lppc`. It contains all mapping repos preserving the structure of `username/repo-name` from the git repo. Example: `~/.lppc/bebold-jhr/lppc-aws-mappings`
If the upstream of the mapping repository cannot be reached, but a local, cached version exists, then the cached version is used. A log statement shown in verbose mode makes this clear.

### Mapping files

Structure of the mapping files is as follows:

```yaml
# Example: aws/resource/aws_route53_zone.yaml
allow:
  - "route53:List*"
  - "route53:Get*"
  - "route53:CreateHostedZone"
  - "route53:DeleteHostedZone"
conditional:
  vpc:
    vpc_id:
      - route53:AssociateVPCWithHostedZone
      - route53:DisassociateVPCFromHostedZone
  comment:
    - route53:UpdateHostedZoneComment
```

The permissions under `conditional` are only relevant if the path in terraform exists. Each index under `conditional` can be either a block or a property in terraform.

For the example above `vpc` is a block within `aws_route53_zone` with having a property `vpc_id`. Whereas `comment` is a property of `aws_route53_zone`.

### Resource-to-Provider Mapping

Each resource, data source, ephemeral, or action block can specify which provider to use via the `provider` attribute (e.g., `provider = aws.dns_account`). If no `provider` attribute is present, the block uses the default provider (the one without an alias).

Permissions are grouped by the provider's `role_arn` expression string (exact match, not resolved). All resources using providers with the same `role_arn` string contribute to the same permission set.

## Output

The list of permissions is a distinct list. For example `ec2:CreateSubnet` can only occur once per provider even if multiple mappings add this to the list of permissions.

### Output Naming Convention

The name used in output headers and file names is derived from the provider's `alias` attribute, converted to PascalCase with a `Deployer` suffix:

- Provider alias is converted to PascalCase
- `Deployer` suffix is appended
- If no alias exists, use `DefaultDeployer`

Examples:
- `alias = "network"` → `NetworkDeployer`
- `alias = "dns_account"` → `DnsAccountDeployer`
- `alias = "workload_test"` → `WorkloadTestDeployer`
- No alias → `DefaultDeployer`

This approach was chosen because `role_arn` values often contain Terraform variables (e.g., `var.account_id`) that cannot be resolved during static analysis. Using the alias provides a stable, meaningful name without requiring variable resolution.

### JSON/HCL formats

+ We don't set the `Sid`.
+ `Effect` for `allow` and `conditional` in the mapping file is always `Allow`
+ `Effect` for `deny` in the mapping file is always `Deny`
+ `Resource` is always `*`
+ Due to how AWS IAM policies work, there must be separate statements for `"Effect": "Allow"` and `"Effect": "Deny"`
+ If the output is stdout then simply print each result one after another separated by a newline. Separated with `----------- {OUTPUT_NAME} -----------`
+ If `--output-dir` is set, create a file for each result in the specified directory. The file contains only the JSON or HCL code (no headers). File name structure: `{OUTPUT_NAME}.{EXTENSION}`. Example: `NetworkDeployer.json`

# Decisions

This section documents key decisions made during design and implementation, along with their rationale.

## Language: Rust

**Decision:** Build the tool in Rust.

**Rationale:**
  * Performance
  * Safety
  * Good choice for creating CLI tools (clap)
  * Binary compilation (no dependency on additional runtimes or interpreter)

## Direct HCL parsing instead of terraform plan

**Decision:** Parse Terraform files directly using an HCL parser instead of relying on `terraform plan` and `terraform show -json`.

**Rationale:**
- Terraform plan requires valid AWS credentials to run and a configured backend (or backend override)

This created friction for users who wanted to analyze Terraform code without deploying or having credentials available. Direct HCL parsing eliminates these requirements entirely. The tool now only needs `terraform init -backend=false` to resolve module dependencies.

**Trade-off:** We lose access to fully resolved values (variables, locals, expressions). This is acceptable because we only need to know *which* resources exist and *which* attributes are present, not their actual values.

## Alias-based naming instead of ARN parsing

**Decision:** Derive output names from the provider's `alias` attribute (e.g., `dns_account` → `DnsAccountDeployer`) instead of parsing the `role_arn` to extract account ID and role name.

**Rationale:** The original plan was to parse `role_arn` values like `arn:aws:iam::123456789012:role/NetworkDeployer` to create names like `123456789012NetworkDeployer`. This would make it obvious for which role the permission set is created. However, in practice, `role_arn` values often contain Terraform variables:

```hcl
assume_role {
  role_arn = "arn:aws:iam::${var.account_id}:role/${var.role_name}"
}
```

These variables cannot be resolved during static analysis. Using the provider alias provides a stable, meaningful name without requiring variable resolution. Another point is that this approach doesn't group different providers of the same role together. Example: A set of providers making multi-region deployments using the same role (see also section below).

**Trade-off:** Output names no longer contain account IDs, which could be useful for multi-account setups. Users can infer the account from the alias naming convention they use.

## Provider grouping by role_arn string (not resolved)

**Decision:** Group providers by their exact `role_arn` expression string, not by resolved values.

**Rationale:** Since we can't resolve variables, two providers with `role_arn = "arn:aws:iam::${var.account_id}:role/Deployer"` are considered the same (same string), even if `var.account_id` might differ at runtime. This is the only consistent approach without variable resolution. This should be fine, because the string is still the same even though the dynamic values are not resolved.

## First alias alphabetically wins

**Decision:** When multiple providers share the same `role_arn` expression, the first alias in alphabetical order is used for the output name.

**Rationale:** Deterministic behavior is important for reproducible outputs. Alphabetical ordering provides a simple, predictable rule. 

## Isolated execution environment

**Decision:** Never modify the user's working directory. All Terraform operations happen in a temporary directory.

**Rationale:** Running `terraform init -backend=false` creates a `.terraform/` directory and potentially modifies `.terraform.lock.hcl`. Users should not have their working directory modified by a read-only analysis tool. This also prevents conflicts with existing Terraform state or provider locks.

## Two-pronged module detection

**Decision:** Detect modules via `.terraform/modules/modules.json` (primary) with regex fallback parsing of `.tf` files.

**Rationale:** The `modules.json` file provides accurate module information after `terraform init`. However if the terraform code is only run in CI/CD environments, `.terraform/` might not exist locally where you run this tool. The regex fallback ensures the tool works even without prior initialization, though with potentially less accurate results for complex module configurations.

## 24-hour cache expiry for mapping repository

**Decision:** Automatically refresh the mapping repository if the last update was more than 24 hours ago.

**Rationale:** 
* This feels like a good balance between freshness and avoiding too many network calls
* Updates on two consecutive days are realistic, multiple times a day is more unlikely
* Additionally the users can choose to force an update

## Shallow git clone for mapping repository

**Decision:** Use shallow clones when fetching the mapping repository.

**Rationale:** The mapping repository's history is not needed for the tool's operation. Shallow clones reduce bandwidth and disk usage, which matters for CI/CD environments with fresh clones.

## Cache directory in user's home directory

**Decision:** Store the mapping repository cache in `~/.lppc/` rather than a system-wide location or project-local directory.

**Rationale:** The home directory works consistently across operating systems (Linux, macOS, Windows) without requiring elevated permissions. A hidden directory (`.lppc`) keeps it unobtrusive.

## External mapping repository (not bundled)

**Decision:** Maintain mappings in a separate Git repository rather than bundling them with the tool.

**Rationale:** This loosely couples the content from the core functionality. Users can easily exchange the default repository with a custom mapping.

## DefaultDeployer for providers without alias

**Decision:** Use `DefaultDeployer` as the output name for providers without an explicit alias.

**Rationale:** Providers without an alias are the "default" provider for that type. The name clearly indicates this is the fallback/default case.

## PascalCase conversion for output names

**Decision:** Convert provider aliases to PascalCase (e.g., `dns_account` → `DnsAccount`).

**Rationale:** This is consistent with naming of the AWS managed roles.

## Resource is always `*` (wildcard)

**Decision:** Generated policies always use `"Resource": "*"` instead of specific resource ARNs.

**Rationale:** Static analysis cannot determine the actual ARNs of resources that will be created. The tool generates a starting point that users should refine by:
1. Adding specific resource ARNs where possible
2. Adding IAM conditions for further restriction
3. Deployer roles are more likely to be flexible

This is explicitly called out in the `--help` output as a disclaimer.

## No Sid in policy statements

**Decision:** Generated policy statements do not include a `Sid` (Statement ID).

**Rationale:** It was omitted for simplicity. The context is selective and should provide enough information.

## Output formats: json, json-grouped, hcl, hcl-grouped

**Decision:** Support four output formats with two grouping strategies (flat vs. grouped by service prefix). Default is `hcl-grouped`.

**Rationale:**
- **json/json-grouped**: Direct use in AWS IAM policies or Terraform `aws_iam_policy_document` data sources
- **hcl/hcl-grouped**: Inline in Terraform configurations using `jsonencode()`
- **Grouped variants**: Improve readability for large permission sets by organizing statements by service (ec2, s3, etc.)
- **hcl-grouped as default**: The primary use case is within Terraform configurations, and grouped output is more readable.

## 10MB file size limit

**Decision:** Skip files larger than 10MB during parsing.

**Rationale:** Prevents memory exhaustion from accidentally processing large binary files or generated code. Terraform files are typically small; a 10MB limit is generous while still providing protection.

## The primary focus lies on `Allow` for both `allow` and `conditional`

**Decision:** Generated policy are primarily using `"Effect": "Allow"`. `"Effect": "Deny"` is only used for actions that allow a role to read data.

**Rationale:** The tool identifies permissions *needed* to deploy resources. By default, roles start with no permission. It's easy and straight forward to just add what the role is allowed to do. Complex statements are more difficult to read and to maintain in the mapping repo. Allowing to read everything using a wildcard with a `Deny` override for a single action is a rare case keeps the policy size small. A use case for this is preventing deployer roles to access business data.

## Using hcl-rs crate for HCL parsing

**Decision:** Use the `hcl-rs` Rust crate for parsing Terraform/HCL files.

**Rationale:** Other libraries have been considered, but this seemed like the most feasable one in terms of features being needed and active maintenance.

## Using git2 (libgit2) for Git operations

**Decision:** Use the `git2` crate (Rust bindings for libgit2) instead of shelling out to the `git` CLI.

**Rationale:**
* No dependency on system git installation - the tool works even if git isn't installed
* Consistent behavior across systems - no differences between git versions, configurations, or platform-specific quirks
* Better error handling - programmatic access to errors rather than parsing CLI output
* No PATH issues - don't need to worry about git being in PATH or non-standard locations

## Graceful degradation when mapping repository is unreachable

**Decision:** If the remote mapping repository cannot be reached but a cached version exists, use the cached version with a warning.

**Rationale:** The tool should work offline or in network-restricted environments. A stale mapping is better than a complete failure, as long as the user is informed via verbose logging.

# Milestones

## 1 Project scaffolding

+ Create the essential project structure
+ Lay foundation to parse application arguments
+ Implement `--help`, `--version`, `--no-color`, `--verbose`

## 2 Mapping repo lifecycle

+ Implement the mapping repo lifecycle including the parameters `--refresh-mappings` and `--mappings-url`

## 3 Working directory validation

+ Implement `--working-dir` parameter
+ Never modify the user's working directory
+ Create isolated temporary directory for terraform operations
+ Copy terraform files to temp directory for safe execution
+ Allow terraform validation on the working directory
+ Parsing will be part of the next milestone

## 4 Terraform parsing

+ Implement terraform parsing excluding terraform modules and prepare the internal data model

## 5 External local modules

+ Support local modules outside the working directory
+ Detect modules via `.terraform/modules/modules.json` (primary)
+ Fallback to regex parsing of `.tf` files for CI/CD environments
+ Copy external modules to isolated environment preserving relative paths
+ Track provider mappings through module hierarchies
+ Resolve module-local provider keys to root provider keys
+ Support nested module provider chains

## 6 Internal mapping

+ Implement the lookup for permissions based on the mapping repo and enrich the internal data model

## 7 Output

+ Implement the output creation including the parameters `--output-dir` and `--output-format`

## 8 Remote module analysis

+ Parse `.terraform/modules/modules.json` for remote module metadata
+ Classify module sources (registry, git, local, root)
+ Parse downloaded module code from `.terraform/modules/`
+ Merge permissions from remote modules into final output