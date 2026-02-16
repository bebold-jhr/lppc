use anyhow::{bail, Context, Result};
use log::debug;
use std::fs;
use std::path::Path;

use crate::block_type::BlockType;
use crate::provider_versions::ProviderVersions;

const AWS_DOCUMENTATION_URL: &str = "https://docs.aws.amazon.com/service-authorization/latest/reference/reference_policies_actions-resources-contextkeys.html";
const TERRAFORM_REGISTRY_BASE: &str = "https://registry.terraform.io/providers/hashicorp/aws/latest/docs";

fn is_valid_terraform_type(name: &str) -> bool {
    !name.is_empty()
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains('\0')
        && !name.starts_with('.')
        && !name.contains("..")
}

pub struct GeneratorConfig<'a> {
    pub working_dir: &'a Path,
    pub block_type: BlockType,
    pub terraform_type: &'a str,
    pub service_reference_url: &'a str,
    pub allow_actions: Vec<String>,
    pub deny_actions: Vec<String>,
    pub provider_versions: &'a ProviderVersions,
}

pub fn generate_files(config: &GeneratorConfig) -> Result<GeneratedFiles> {
    if !is_valid_terraform_type(config.terraform_type) {
        bail!(
            "Invalid terraform type: contains disallowed characters: {}",
            config.terraform_type
        );
    }

    let mapping_path = generate_mapping_file(config)?;
    let test_files = generate_integration_tests(config)?;

    Ok(GeneratedFiles {
        mapping_file: mapping_path,
        test_dir: test_files.test_dir,
        test_files: test_files.files,
    })
}

#[derive(Debug)]
pub struct GeneratedFiles {
    pub mapping_file: String,
    pub test_dir: String,
    pub test_files: Vec<String>,
}

fn generate_mapping_file(config: &GeneratorConfig) -> Result<String> {
    let mapping_dir = config.working_dir.join(config.block_type.mapping_dir());
    let mapping_file = mapping_dir.join(format!("{}.yml", config.terraform_type));

    if mapping_file.exists() {
        bail!(
            "Mapping file already exists: {}",
            mapping_file.display()
        );
    }

    fs::create_dir_all(&mapping_dir)
        .with_context(|| format!("Failed to create mapping directory: {}", mapping_dir.display()))?;

    let terraform_doc_url = generate_terraform_doc_url(config.block_type, config.terraform_type);

    let yaml_content = generate_mapping_yaml(
        AWS_DOCUMENTATION_URL,
        config.service_reference_url,
        &terraform_doc_url,
        &config.allow_actions,
        &config.deny_actions,
    );

    fs::write(&mapping_file, &yaml_content)
        .with_context(|| format!("Failed to write mapping file: {}", mapping_file.display()))?;

    debug!("Created mapping file: {}", mapping_file.display());

    let relative_path = format!(
        "{}/{}.yml",
        config.block_type.mapping_dir(),
        config.terraform_type
    );

    Ok(relative_path)
}

fn generate_terraform_doc_url(block_type: BlockType, terraform_type: &str) -> String {
    let type_without_prefix = terraform_type
        .strip_prefix("aws_")
        .unwrap_or(terraform_type);

    format!(
        "{}/{}/{}",
        TERRAFORM_REGISTRY_BASE,
        block_type.terraform_docs_path(),
        type_without_prefix
    )
}

fn generate_mapping_yaml(
    aws_documentation: &str,
    service_reference: &str,
    terraform_documentation: &str,
    allow_actions: &[String],
    deny_actions: &[String],
) -> String {
    let mut yaml = String::new();
    yaml.push_str("---\n");
    yaml.push_str("metadata:\n");
    yaml.push_str("  aws:\n");
    yaml.push_str(&format!("    documentation: {}\n", aws_documentation));
    yaml.push_str(&format!("    service-reference: {}\n", service_reference));
    yaml.push_str("  terraform:\n");
    yaml.push_str(&format!("    documentation: {}\n", terraform_documentation));
    if !deny_actions.is_empty() {
        yaml.push_str("deny:\n");
        for action in deny_actions {
            yaml.push_str(&format!("  - {}\n", action));
        }
    }
    if !allow_actions.is_empty() {
        yaml.push_str("allow:\n");
        for action in allow_actions {
            yaml.push_str(&format!("  - {}\n", action));
        }
    }
    yaml
}

struct TestFiles {
    test_dir: String,
    files: Vec<String>,
}

fn generate_integration_tests(config: &GeneratorConfig) -> Result<TestFiles> {
    let test_base_dir = config
        .working_dir
        .join(config.block_type.integration_test_dir())
        .join(config.terraform_type);

    if test_base_dir.exists() {
        bail!(
            "Integration test directory already exists: {}",
            test_base_dir.display()
        );
    }

    let tests_subdir = test_base_dir.join("tests");

    fs::create_dir_all(&tests_subdir)
        .with_context(|| format!("Failed to create test directory: {}", tests_subdir.display()))?;

    let providers_content = generate_providers_tf(config.provider_versions);
    let providers_path = test_base_dir.join("providers.tf");
    fs::write(&providers_path, providers_content)
        .with_context(|| format!("Failed to write providers.tf: {}", providers_path.display()))?;

    let main_content = generate_main_tf(config.block_type, config.terraform_type);
    let main_path = test_base_dir.join("main.tf");
    fs::write(&main_path, main_content)
        .with_context(|| format!("Failed to write main.tf: {}", main_path.display()))?;

    let data_content = generate_data_tf();
    let data_path = test_base_dir.join("data.tf");
    fs::write(&data_path, data_content)
        .with_context(|| format!("Failed to write data.tf: {}", data_path.display()))?;

    let test_content = generate_test_hcl();
    let test_file_path = tests_subdir.join(format!("{}.tftest.hcl", config.terraform_type));
    fs::write(&test_file_path, test_content)
        .with_context(|| format!("Failed to write test file: {}", test_file_path.display()))?;

    debug!("Created integration test directory: {}", test_base_dir.display());

    let relative_dir = format!(
        "{}/{}",
        config.block_type.integration_test_dir(),
        config.terraform_type
    );

    Ok(TestFiles {
        test_dir: relative_dir.clone(),
        files: vec![
            "providers.tf".to_string(),
            "main.tf".to_string(),
            "data.tf".to_string(),
            format!("tests/{}.tftest.hcl", config.terraform_type),
        ],
    })
}

fn generate_providers_tf(versions: &ProviderVersions) -> String {
    format!(
        r#"terraform {{
  required_providers {{
    aws = {{
      source  = "hashicorp/aws"
      version = "{}"
    }}
    time = {{
      source  = "hashicorp/time"
      version = "{}"
    }}
    random = {{
      source  = "hashicorp/random"
      version = "{}"
    }}
  }}
}}
"#,
        versions.aws, versions.time, versions.random
    )
}

fn generate_main_tf(block_type: BlockType, terraform_type: &str) -> String {
    format!("{} \"{}\" \"this\" {{\n}}\n", block_type, terraform_type)
}

fn generate_data_tf() -> &'static str {
    r#"data "aws_caller_identity" "this" {}
"#
}

fn generate_test_hcl() -> &'static str {
    r#"####
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
# Provider using deployer role
####
provider "aws" {
  region = "us-east-1"
  alias  = "deployer_role"

  assume_role {
    role_arn = run.create_deployer_role.deployer_role.arn
  }
}


####
# Perform tests
####
run "TODO name your test" {
  state_key = "main"
  
  module {
    source = "./"
  }

  providers = {
    aws = aws.deployer_role
  }

  command = apply

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
"#
}

pub fn print_success_message(files: &GeneratedFiles) {
    println!("\n✓ Created mapping file:");
    println!("  {}", files.mapping_file);
    println!();
    println!("✓ Created integration test stub:");
    println!("  {}/", files.test_dir);
    for (i, file) in files.test_files.iter().enumerate() {
        let prefix = if i == files.test_files.len() - 1 {
            "└──"
        } else {
            "├──"
        };
        println!("  {} {}", prefix, file);
    }
    println!();
    println!("Remember to review the generated files and add specific resource constraints.");
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn setup_test_dir() -> TempDir {
        TempDir::new().unwrap()
    }

    fn test_provider_versions() -> ProviderVersions {
        ProviderVersions {
            aws: "6.7.0".to_string(),
            time: "0.13.1".to_string(),
            random: "3.7.2".to_string(),
        }
    }

    #[test]
    fn generate_terraform_doc_url_for_resource() {
        let url = generate_terraform_doc_url(BlockType::Resource, "aws_subnet");
        assert_eq!(
            url,
            "https://registry.terraform.io/providers/hashicorp/aws/latest/docs/resources/subnet"
        );
    }

    #[test]
    fn generate_terraform_doc_url_for_data() {
        let url = generate_terraform_doc_url(BlockType::Data, "aws_ami");
        assert_eq!(
            url,
            "https://registry.terraform.io/providers/hashicorp/aws/latest/docs/data-sources/ami"
        );
    }

    #[test]
    fn generate_terraform_doc_url_for_ephemeral() {
        let url = generate_terraform_doc_url(BlockType::Ephemeral, "aws_secretsmanager_secret_version");
        assert_eq!(
            url,
            "https://registry.terraform.io/providers/hashicorp/aws/latest/docs/ephemeral-resources/secretsmanager_secret_version"
        );
    }

    #[test]
    fn generate_terraform_doc_url_for_action() {
        let url = generate_terraform_doc_url(BlockType::Action, "aws_lambda_invoke");
        assert_eq!(
            url,
            "https://registry.terraform.io/providers/hashicorp/aws/latest/docs/actions/lambda_invoke"
        );
    }

    #[test]
    fn generate_files_creates_mapping_file() {
        let temp_dir = setup_test_dir();

        let config = GeneratorConfig {
            working_dir: temp_dir.path(),
            block_type: BlockType::Resource,
            terraform_type: "aws_subnet",
            service_reference_url: "https://example.com/ec2.json",
            allow_actions: vec!["ec2:List*".to_string(), "ec2:CreateSubnet".to_string()],
            deny_actions: vec![],
            provider_versions: &test_provider_versions(),
        };

        let result = generate_files(&config);
        assert!(result.is_ok());

        let mapping_path = temp_dir.path().join("mappings/resource/aws_subnet.yml");
        assert!(mapping_path.exists());

        let content = fs::read_to_string(&mapping_path).unwrap();
        assert!(content.contains("ec2:List*"));
        assert!(content.contains("ec2:CreateSubnet"));
        assert!(content.contains("service-reference: https://example.com/ec2.json"));
    }

    #[test]
    fn generate_files_creates_integration_test_directory() {
        let temp_dir = setup_test_dir();

        let config = GeneratorConfig {
            working_dir: temp_dir.path(),
            block_type: BlockType::Resource,
            terraform_type: "aws_subnet",
            service_reference_url: "https://example.com/ec2.json",
            allow_actions: vec!["ec2:CreateSubnet".to_string()],
            deny_actions: vec![],
            provider_versions: &test_provider_versions(),
        };

        let result = generate_files(&config);
        assert!(result.is_ok());

        let test_dir = temp_dir.path().join("integration-tests/resource/aws_subnet");
        assert!(test_dir.exists());
        assert!(test_dir.join("providers.tf").exists());
        assert!(test_dir.join("main.tf").exists());
        assert!(test_dir.join("data.tf").exists());
        assert!(test_dir.join("tests/aws_subnet.tftest.hcl").exists());
    }

    #[test]
    fn generate_files_fails_if_mapping_exists() {
        let temp_dir = setup_test_dir();

        let mapping_dir = temp_dir.path().join("mappings/resource");
        fs::create_dir_all(&mapping_dir).unwrap();
        fs::write(mapping_dir.join("aws_subnet.yml"), "# existing").unwrap();

        let config = GeneratorConfig {
            working_dir: temp_dir.path(),
            block_type: BlockType::Resource,
            terraform_type: "aws_subnet",
            service_reference_url: "https://example.com/ec2.json",
            allow_actions: vec!["ec2:CreateSubnet".to_string()],
            deny_actions: vec![],
            provider_versions: &test_provider_versions(),
        };

        let result = generate_files(&config);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("already exists"));
    }

    #[test]
    fn generate_files_creates_correct_providers_tf() {
        let temp_dir = setup_test_dir();

        let config = GeneratorConfig {
            working_dir: temp_dir.path(),
            block_type: BlockType::Resource,
            terraform_type: "aws_vpc",
            service_reference_url: "https://example.com/ec2.json",
            allow_actions: vec!["ec2:CreateVpc".to_string()],
            deny_actions: vec![],
            provider_versions: &test_provider_versions(),
        };

        generate_files(&config).unwrap();

        let providers_content = fs::read_to_string(
            temp_dir.path().join("integration-tests/resource/aws_vpc/providers.tf"),
        )
        .unwrap();

        assert!(providers_content.contains("hashicorp/aws"));
        assert!(providers_content.contains("version = \"6.7.0\""));
        assert!(providers_content.contains("hashicorp/time"));
        assert!(providers_content.contains("hashicorp/random"));
    }

    #[test]
    fn generate_files_creates_correct_data_tf() {
        let temp_dir = setup_test_dir();

        let config = GeneratorConfig {
            working_dir: temp_dir.path(),
            block_type: BlockType::Resource,
            terraform_type: "aws_vpc",
            service_reference_url: "https://example.com/ec2.json",
            allow_actions: vec!["ec2:CreateVpc".to_string()],
            deny_actions: vec![],
            provider_versions: &test_provider_versions(),
        };

        generate_files(&config).unwrap();

        let data_content = fs::read_to_string(
            temp_dir.path().join("integration-tests/resource/aws_vpc/data.tf"),
        )
        .unwrap();

        assert!(data_content.contains("data \"aws_caller_identity\" \"this\""));
    }

    #[test]
    fn generate_files_creates_main_tf_with_resource_stub() {
        let temp_dir = setup_test_dir();

        let config = GeneratorConfig {
            working_dir: temp_dir.path(),
            block_type: BlockType::Resource,
            terraform_type: "aws_vpc",
            service_reference_url: "https://example.com/ec2.json",
            allow_actions: vec!["ec2:CreateVpc".to_string()],
            deny_actions: vec![],
            provider_versions: &test_provider_versions(),
        };

        generate_files(&config).unwrap();

        let main_content = fs::read_to_string(
            temp_dir.path().join("integration-tests/resource/aws_vpc/main.tf"),
        )
        .unwrap();

        assert_eq!(main_content, "resource \"aws_vpc\" \"this\" {\n}\n");
    }

    #[test]
    fn generate_files_creates_main_tf_with_data_stub() {
        let temp_dir = setup_test_dir();

        let config = GeneratorConfig {
            working_dir: temp_dir.path(),
            block_type: BlockType::Data,
            terraform_type: "aws_ami",
            service_reference_url: "https://example.com/ec2.json",
            allow_actions: vec!["ec2:DescribeImages".to_string()],
            deny_actions: vec![],
            provider_versions: &test_provider_versions(),
        };

        generate_files(&config).unwrap();

        let main_content = fs::read_to_string(
            temp_dir.path().join("integration-tests/data/aws_ami/main.tf"),
        )
        .unwrap();

        assert_eq!(main_content, "data \"aws_ami\" \"this\" {\n}\n");
    }

    #[test]
    fn generate_files_creates_main_tf_with_ephemeral_stub() {
        let temp_dir = setup_test_dir();

        let config = GeneratorConfig {
            working_dir: temp_dir.path(),
            block_type: BlockType::Ephemeral,
            terraform_type: "aws_secretsmanager_secret_version",
            service_reference_url: "https://example.com/secretsmanager.json",
            allow_actions: vec!["secretsmanager:GetSecretValue".to_string()],
            deny_actions: vec![],
            provider_versions: &test_provider_versions(),
        };

        generate_files(&config).unwrap();

        let main_content = fs::read_to_string(
            temp_dir
                .path()
                .join("integration-tests/ephemeral/aws_secretsmanager_secret_version/main.tf"),
        )
        .unwrap();

        assert_eq!(
            main_content,
            "ephemeral \"aws_secretsmanager_secret_version\" \"this\" {\n}\n"
        );
    }

    #[test]
    fn generate_files_creates_test_hcl_with_template() {
        let temp_dir = setup_test_dir();

        let config = GeneratorConfig {
            working_dir: temp_dir.path(),
            block_type: BlockType::Resource,
            terraform_type: "aws_vpc",
            service_reference_url: "https://example.com/ec2.json",
            allow_actions: vec!["ec2:CreateVpc".to_string()],
            deny_actions: vec![],
            provider_versions: &test_provider_versions(),
        };

        generate_files(&config).unwrap();

        let test_content = fs::read_to_string(
            temp_dir.path().join("integration-tests/resource/aws_vpc/tests/aws_vpc.tftest.hcl"),
        )
        .unwrap();

        assert!(test_content.contains("create_deployer_role"));
        assert!(test_content.contains("aws.admin"));
        assert!(test_content.contains("aws.deployer_role"));
        assert!(test_content.contains("TODO name your test"));
        assert!(test_content.contains("TODO Define your assertion here"));
    }

    #[test]
    fn generated_files_struct_contains_correct_paths() {
        let temp_dir = setup_test_dir();

        let config = GeneratorConfig {
            working_dir: temp_dir.path(),
            block_type: BlockType::Data,
            terraform_type: "aws_ami",
            service_reference_url: "https://example.com/ec2.json",
            allow_actions: vec!["ec2:DescribeImages".to_string()],
            deny_actions: vec![],
            provider_versions: &test_provider_versions(),
        };

        let result = generate_files(&config).unwrap();

        assert_eq!(result.mapping_file, "mappings/data/aws_ami.yml");
        assert_eq!(result.test_dir, "integration-tests/data/aws_ami");
        assert!(result.test_files.contains(&"providers.tf".to_string()));
        assert!(result.test_files.contains(&"main.tf".to_string()));
        assert!(result.test_files.contains(&"data.tf".to_string()));
        assert!(result.test_files.contains(&"tests/aws_ami.tftest.hcl".to_string()));
    }

    #[test]
    fn is_valid_terraform_type_accepts_valid_names() {
        assert!(is_valid_terraform_type("aws_subnet"));
        assert!(is_valid_terraform_type("aws_vpc"));
        assert!(is_valid_terraform_type("aws_iam_role"));
    }

    #[test]
    fn is_valid_terraform_type_rejects_path_traversal() {
        assert!(!is_valid_terraform_type("../../../etc/passwd"));
        assert!(!is_valid_terraform_type(".."));
        assert!(!is_valid_terraform_type("foo/bar"));
        assert!(!is_valid_terraform_type("foo\\bar"));
        assert!(!is_valid_terraform_type(".hidden"));
        assert!(!is_valid_terraform_type(""));
    }

    #[test]
    fn generate_files_rejects_path_traversal_in_terraform_type() {
        let temp_dir = setup_test_dir();

        let config = GeneratorConfig {
            working_dir: temp_dir.path(),
            block_type: BlockType::Resource,
            terraform_type: "../../../etc/passwd",
            service_reference_url: "https://example.com/ec2.json",
            allow_actions: vec!["ec2:CreateSubnet".to_string()],
            deny_actions: vec![],
            provider_versions: &test_provider_versions(),
        };

        let result = generate_files(&config);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Invalid terraform type"));
    }

    #[test]
    fn generate_files_fails_if_test_directory_exists() {
        let temp_dir = setup_test_dir();

        let test_dir = temp_dir
            .path()
            .join("integration-tests/resource/aws_instance");
        fs::create_dir_all(&test_dir).unwrap();

        let config = GeneratorConfig {
            working_dir: temp_dir.path(),
            block_type: BlockType::Resource,
            terraform_type: "aws_instance",
            service_reference_url: "https://example.com/ec2.json",
            allow_actions: vec!["ec2:RunInstances".to_string()],
            deny_actions: vec![],
            provider_versions: &test_provider_versions(),
        };

        let result = generate_files(&config);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("already exists"));
    }

    #[test]
    fn generate_yaml_with_both_allow_and_deny() {
        let yaml = generate_mapping_yaml(
            "https://docs.aws.amazon.com",
            "https://example.com/ec2.json",
            "https://registry.terraform.io/docs/resources/subnet",
            &["ec2:List*".to_string(), "ec2:CreateSubnet".to_string()],
            &["ec2:DeleteSubnet".to_string()],
        );

        assert!(yaml.contains("deny:\n  - ec2:DeleteSubnet\n"));
        assert!(yaml.contains("allow:\n  - ec2:List*\n  - ec2:CreateSubnet\n"));
    }

    #[test]
    fn generate_yaml_with_only_allow() {
        let yaml = generate_mapping_yaml(
            "https://docs.aws.amazon.com",
            "https://example.com/ec2.json",
            "https://registry.terraform.io/docs/resources/subnet",
            &["ec2:CreateSubnet".to_string()],
            &[],
        );

        assert!(yaml.contains("allow:\n"));
        assert!(!yaml.contains("deny:"));
    }

    #[test]
    fn generate_yaml_with_only_deny() {
        let yaml = generate_mapping_yaml(
            "https://docs.aws.amazon.com",
            "https://example.com/ec2.json",
            "https://registry.terraform.io/docs/resources/subnet",
            &[],
            &["ec2:DeleteSubnet".to_string()],
        );

        assert!(yaml.contains("deny:\n"));
        assert!(!yaml.contains("allow:"));
    }

    #[test]
    fn generate_providers_tf_uses_dynamic_versions() {
        let versions = ProviderVersions {
            aws: "7.0.0".to_string(),
            time: "1.0.0".to_string(),
            random: "4.0.0".to_string(),
        };
        let content = generate_providers_tf(&versions);

        assert!(content.contains("version = \"7.0.0\""));
        assert!(content.contains("version = \"1.0.0\""));
        assert!(content.contains("version = \"4.0.0\""));
        assert!(content.contains("hashicorp/aws"));
        assert!(content.contains("hashicorp/time"));
        assert!(content.contains("hashicorp/random"));
    }

    #[test]
    fn deny_section_appears_before_allow_section() {
        let yaml = generate_mapping_yaml(
            "https://docs.aws.amazon.com",
            "https://example.com/ec2.json",
            "https://registry.terraform.io/docs/resources/subnet",
            &["ec2:CreateSubnet".to_string()],
            &["ec2:DeleteSubnet".to_string()],
        );

        let deny_pos = yaml.find("deny:").expect("deny: section not found");
        let allow_pos = yaml.find("allow:").expect("allow: section not found");
        assert!(deny_pos < allow_pos, "deny: must appear before allow:");
    }
}
