# Motivation

This CLI tool is designed to simplify the generation of mapping files while ensuring the process remains efficient, concise, and consistent. The tool allows the user to see which Terraform block types still need a mapping and assists with finding the matching permissions. Furthermore, it generates a step for integration tests.

## Usage

This tool is always supposed to be run on a local dev machine and is only meant for interactive usage. To clarify: There is no use case for the tool to run in a CICD pipeline.

## How it works

1. When you start the tool it verifies and validates the working directory (required parameter) exists.

2. Select a block type.
This can be one of `action`, `data`, `ephemeral`, `resource`. This acts as a pre-filter.

3. Select a type. Next the tool checks which terraform types still require a mapping. All available terraform block types have a list of available types.

Block type to schema file mapping:

| Block type | Schema file                                       |
|------------|---------------------------------------------------|
| resource   | sources/terraform/resource_schemas.json           |
| data       | sources/terraform/data_source_schemas.json        |
| ephemeral  | sources/terraform/ephemeral_resource_schemas.json |
| action     | sources/terraform/action_schemas.json             |

Each JSON file solely contains an array with type names like `aws_subnet`.
These must be compared to the existence of mapping paths under `mappings/resource/aws_subnet.yml` for example.
The tool must only show types without a mapping file.

4. Find a matching service prefix. AWS offers a list of actions for each service prefix called "service reference". There is an overview of all existing service-prefixes. The overview exists in `sources/aws/aws-servicereference-index.json`.
This file is expected to exist.
The JSON looks like this:

```json
[ {
  "service" : "a2c",
  "url" : "https://servicereference.us-east-1.amazonaws.com/v1/a2c/a2c.json"
}, {
  "service" : "a4b",
  "url" : "https://servicereference.us-east-1.amazonaws.com/v1/a4b/a4b.json"
}, {
  "service" : "access-analyzer",
  "url" : "https://servicereference.us-east-1.amazonaws.com/v1/access-analyzer/access-analyzer.json"
}, {
  "service" : "account",
  "url" : "https://servicereference.us-east-1.amazonaws.com/v1/account/account.json"
}, {
  "service" : "acm",
  "url" : "https://servicereference.us-east-1.amazonaws.com/v1/acm/acm.json"
}
]
```
Tool should pre-select the best matching or none if it's not able to. The user has to make the final decision.
The heuristic for finding the best match is removing the `aws_` prefix and then using the remainung substring up to the next underscore `_`. Example: `aws_iam_role` => `iam`. `aws_subnet` would find and therefore select nothing so the user has to make the decision on its own.

5. Selecting the actions. Based on the previous selection can load the corresponding JSON file for the service prefix.
Example: `sources/aws/account.json`
These files are expected to exist.
Here is what the structure of the file looks like, at least the relevant part:

```json
{
  "Name" : "account",
  "Actions" : [
    {
    "Name" : "AcceptPrimaryEmailUpdate",
    "ActionConditionKeys" : [ "account:EmailTargetDomain" ],
    "Annotations" : {
      "Properties" : {
        "IsList" : false,
        "IsPermissionManagement" : false,
        "IsTaggingOnly" : false,
        "IsWrite" : true
      }
    },
    "Resources" : [ {
      "Name" : "accountInOrganization"
    } ],
    "SupportedBy" : {
      "IAM Access Analyzer Policy Generation" : true,
      "IAM Action Last Accessed" : true
    }
  }, {
    "Name" : "CloseAccount",
    "Annotations" : {
      "Properties" : {
        "IsList" : false,
        "IsPermissionManagement" : false,
        "IsTaggingOnly" : false,
        "IsWrite" : true
      }
    },
    "Resources" : [ {
      "Name" : "account"
    } ],
    "SupportedBy" : {
      "IAM Access Analyzer Policy Generation" : true,
      "IAM Action Last Accessed" : false
    }
  }
]
}
```

This should be a vertical split pane with all the actions on the left and the current selection on the right.
The tool must pre-select all action which have `IsTaggingOnly` set to `true`.
Pre-select all actions that start with `List` and add a single action as `{SERVICE_PREFIX}:List*`.
Pre-select all actions that start with `Describe` and add a single action as `{SERVICE_PREFIX}:Describe*`.
Pre-select all actions that start with `Get` and add a single action as `{SERVICE_PREFIX}:Get*`.
In case the user deselects and of the pre-selected `List`, `Describe` or `Get` actions, then instead of listing a wildcard action with `*` an explicit list is used.
The tool must show a static warning that reminds the user to not select read actions on data like `s3:GetObject`, but only in infrastructure.
Any action is added as `{SERVICE_PREFIX}:{ACTION}`. Example: `account:CloseAccount`.
Select and deselect actions using `SPACEBAR`. Toggle (de)select all visible (filtered) entries using `a`. Confirm via `ENTER`.
The user can type to filter the action list (case-insensitive substring match). The filter text is shown in the left pane title. `BACKSPACE` removes the last character from the filter.
The user cannot proceed if no action has been selected.

6. Create mapping file. Create a yaml file in the respective path.
`mappings/{BLOCK_TYPE}/{TERRAFORM_RESOURCE_NAME}.yml`
Here is an example what a YAML mapping file must look like:
```yml
metadata:
  aws:
    documentation: https://docs.aws.amazon.com/service-authorization/latest/reference/reference_policies_actions-resources-contextkeys.html
    service-reference: https://servicereference.us-east-1.amazonaws.com/v1/ec2/ec2.json
  terraform:
    documentation: https://registry.terraform.io/providers/hashicorp/aws/latest/docs/resources/subnet
actions:
  - ec2:List*
  - ec2:Describe*
  - ec2:DeleteSubnet
  - ec2:CreateSubnet
```
Throw an error if a mapping file already exists. The reason is that this case should not occur, because we previously removed all terraform types for which a mapping file already exists. A user couldn't have selected it.

The service reference link is available as-is.
Terraform links can be dynamically infered. Here are examples:
aws_subnet => https://registry.terraform.io/providers/hashicorp/aws/latest/docs/resources/subnet
aws_subnet => https://registry.terraform.io/providers/hashicorp/aws/latest/docs/data-sources/subnet
aws_kms_secrets => https://registry.terraform.io/providers/hashicorp/aws/latest/docs/ephemeral-resources/kms_secrets
aws_lambda_invoke => https://registry.terraform.io/providers/hashicorp/aws/latest/docs/actions/lambda_invoke
The AWS documentation link `https://docs.aws.amazon.com/service-authorization/latest/reference/reference_policies_actions-resources-contextkeys.html` is static.

7. Create a stub for an integration test. Create a new folder in the respective path: `integration-tests/{BLOCK_TYPE}/{TERRAFORM_TYPE}`. Example: `integration-tests/data/aws_availability_zones/`
This requires the creation of the `{TERRAFORM_TYPE}` subfolder.

**providers.tf**
```hcl
terraform {
  required_providers {
    aws = {
      source  = "hashicorp/aws"
      version = "6.7.0"
    }
    time = {
      source  = "hashicorp/time"
      version = "0.13.1"
    }
    random = {
      source  = "hashicorp/random"
      version = "3.7.2"
    }
  }
}
```
These versions are intentionally pinned, because renovate will create updates automatically.

**main.tf**: Contains an empty stub for the respective block type.
```hcl
<BLOCK_TYPE> "<TYPE>" "this" {
}
```
Example for `resource` block type with `aws_subnet`:
```hcl
resource "aws_subnet" "this" {
}
```

**data.tf**

```hcl
data "aws_caller_identity" "this" {}
```

**tests/{TERRAFORM_TYPE}.tftest.hcl**
This requires the creation of the `tests` subfolder.

```hcl
####
# Set up deployer role
####
provider "aws" {
  region = "us-east-1"
  alias  = "admin"
}

run "create_deployer_role" {
  module {
    source = "../../modules/deployer-role"
  }

  providers = {
    aws = aws.admin
  }
}

####
# Perform tests
####
provider "aws" {
  region = "us-east-1"
  alias  = "deployer_role"

  assume_role {
    role_arn = run.create_deployer_role.deployer_role.arn
  }
}

run "TODO name your test" {
  module {
    source = "./"
  }

  providers = {
    aws = aws.deployer_role
  }

  command = plan

  assert {
    condition     = startswith(data.aws_caller_identity.this.arn, "arn:aws:sts::${run.create_deployer_role.account_id}:assumed-role/${run.create_deployer_role.deployer_role.name}")
    error_message = "Used wrong role."
  }

  assert {
    condition     = data.aws_caller_identity.this.account_id == run.create_deployer_role.account_id
    error_message = "Unexpected account ID."
  }

  # TODO Define your assertion here
}
```


# Overall application

The application is built in Rust and offers an interactive CLI application. It's solely created for AWS policy mappings.

## Name

The project name is `lppc-mapping-creator`.

## Required parameters

Can take an absolute path or a relative path. The relative would be relative to the directory from which the binary is run.
Example: `./lppc ../lppc-aws-mappings`

## Look & Feel

Use `ratatui` and `crossterm` for TUI support

## Features

+ Use colors, formatting and interactive selection options to make an appealing and easy to read CLI application
+ Parameter `--version` (short `-v`) showing the current release version as offered by many rust libraries for parameter parsing.
+ Parameter `--help` (short `-h`) as offered by many rust libraries for parameter parsing.
+ Parameter `--verbose` showing verbose log statements making debugging possible.
+ When selecting a terraform type the user is able to move the list with up and down arrow keys and even type a search string to further reduce the list of elements. Selection via `ENTER`.
+ When selecting a service prefix the user is able to move the list with up and down arrow keys and even type a search string to further reduce the list of elements. In comparisoin to the terraform type selection, the user has to select one (exactly one) entry which is then highlighted (e.g. checkbox symbol with a check mark) using `SPACEBAR`. Only if one entry is selected the user can confirm the choice using `ENTER`.
+ When selecting actions the user is able to move the list with up and down arrow keys and type a search string to filter the list. Toggle selection using `SPACEBAR`. Toggle all visible (filtered) entries using `a`. Confirm via `ENTER`.
+ The user can cancel at any step using `ESC`. A confirmation dialog confirms the choice.
+ The user can cancel at any step using `Ctrl+C`. No confirmation dialog.


## Error handling

The tool only returns exit code `0` for success and `1` for any type of error.
If a file from which the tool tries to read doesn't exist, the tool should throw an error.
If a directory from which the tool tries to read doesn't exist, the tool should throw an error.

# Milestones

## Milestone 1

+ Create the essential project structure
+ Lay foundation to parse application arguments
+ Implement `--help`, `--version`, `--verbose`
+ Verify working directory exists.
+ See also "How it works" `1.`
+ Tool simply exits when done

## Milestone 2

+ Block type selection
+ Terraform type selection
+ See also "How it works" `2.` and `3.`
+ Tool simply exits when done

## Milestone 3

+ Selection of AWS service-reference including simple best match heuristic
+ See also "How it works" `4.`
+ Tool simply exits when done

## Milestone 4

+ Action selection including the pre-selection if actions and special handling for `List`, `Describe` and `Get` actions
+ Support filtering/searching actions by typing (case-insensitive substring match)
+ See also "How it works" `5.`
+ Tool simply exits when done

## Milestone 5

+ Implement the generation of the mapping file and integrations test stub
+ See also "How it works" `6.` and `7.`