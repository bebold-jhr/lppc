//! HCL output formatter for Terraform IAM policy documents.
//!
//! This module provides the `HclFormatter` which outputs permissions
//! as valid HCL using `jsonencode()` for inline policy documents.
//! It supports both a flat format (all actions in a single statement)
//! and a grouped format (one statement per AWS service).
//! Deny statements appear before Allow statements.

use std::collections::{HashMap, HashSet};

use super::formatter::{OutputFormatter, PermissionSets};

/// Formatter that outputs permissions as HCL with `jsonencode()`.
///
/// The output is suitable for use in Terraform's `aws_iam_policy` or
/// inline policy documents. When `grouped` is false, up to two statements
/// are generated (Deny then Allow). When `grouped` is true, permissions
/// are grouped by service prefix with all Deny groups before Allow groups.
pub struct HclFormatter {
    /// Whether to group permissions by service prefix.
    pub grouped: bool,
}

impl OutputFormatter for HclFormatter {
    fn format(&self, permissions: &PermissionSets) -> String {
        if self.grouped {
            self.format_grouped(permissions)
        } else {
            self.format_single(permissions)
        }
    }

    fn extension(&self) -> &'static str {
        "hcl"
    }
}

impl HclFormatter {
    /// Formats permissions with up to two statements (Deny then Allow).
    fn format_single(&self, permissions: &PermissionSets) -> String {
        let mut statement_blocks: Vec<String> = Vec::new();

        if let Some(block) = self.format_statement_block(permissions.deny, "Deny", 4) {
            statement_blocks.push(block);
        }
        if let Some(block) = self.format_statement_block(permissions.allow, "Allow", 4) {
            statement_blocks.push(block);
        }

        let statements_content = if statement_blocks.is_empty() {
            "[]".to_string()
        } else if statement_blocks.len() == 1 {
            format!(
                "[\n{}\n  ]",
                statement_blocks[0]
            )
        } else {
            format!(
                "[\n{}\n  ]",
                statement_blocks.join(",\n")
            )
        };

        format!(
            r#"jsonencode({{
  Version = "2012-10-17"
  Statement = {}
}})"#,
            statements_content
        )
    }

    /// Formats permissions grouped by service prefix with Deny groups before Allow groups.
    fn format_grouped(&self, permissions: &PermissionSets) -> String {
        let mut all_statements: Vec<String> = Vec::new();

        all_statements.extend(self.create_grouped_statement_blocks(permissions.deny, "Deny"));
        all_statements.extend(self.create_grouped_statement_blocks(permissions.allow, "Allow"));

        let statements_content = if all_statements.is_empty() {
            "[]".to_string()
        } else {
            format!(
                "[\n{}\n  ]",
                all_statements.join(",\n")
            )
        };

        format!(
            r#"jsonencode({{
  Version = "2012-10-17"
  Statement = {}
}})"#,
            statements_content
        )
    }

    /// Creates grouped statement blocks for a given effect, sorted by service prefix.
    fn create_grouped_statement_blocks(
        &self,
        permissions: &HashSet<String>,
        effect: &str,
    ) -> Vec<String> {
        if permissions.is_empty() {
            return Vec::new();
        }

        let mut groups: HashMap<String, Vec<&String>> = HashMap::new();

        for perm in permissions {
            let service = perm.split(':').next().unwrap_or("unknown");
            groups.entry(service.to_string()).or_default().push(perm);
        }

        let mut service_names: Vec<_> = groups.keys().cloned().collect();
        service_names.sort();

        service_names
            .into_iter()
            .map(|service| {
                let mut actions = groups.remove(&service).unwrap_or_default();
                actions.sort();
                let actions_hcl = self.format_action_list(&actions);

                format!(
                    r#"    {{
      Effect   = "{}"
      Action   = {}
      Resource = "*"
    }}"#,
                    effect, actions_hcl
                )
            })
            .collect()
    }

    /// Formats a single statement block for the given effect. Returns None if empty.
    fn format_statement_block(
        &self,
        permissions: &HashSet<String>,
        effect: &str,
        indent: usize,
    ) -> Option<String> {
        if permissions.is_empty() {
            return None;
        }

        let mut sorted: Vec<_> = permissions.iter().collect();
        sorted.sort();

        let actions_hcl = self.format_action_list(&sorted);
        let indent_str = " ".repeat(indent);

        Some(format!(
            r#"{indent_str}{{
{indent_str}  Effect   = "{effect}"
{indent_str}  Action   = {actions_hcl}
{indent_str}  Resource = "*"
{indent_str}}}"#
        ))
    }

    /// Formats a list of actions as HCL.
    ///
    /// For a single action, returns a quoted string.
    /// For multiple actions, returns an HCL list with proper indentation.
    fn format_action_list<S: AsRef<str>>(&self, actions: &[S]) -> String {
        if actions.is_empty() {
            return "[]".to_string();
        }

        if actions.len() == 1 {
            format!("\"{}\"", actions[0].as_ref())
        } else {
            let formatted: Vec<String> = actions
                .iter()
                .map(|a| format!("        \"{}\"", a.as_ref()))
                .collect();
            format!("[\n{}\n      ]", formatted.join(",\n"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_permissions() -> HashSet<String> {
        let mut perms = HashSet::new();
        perms.insert("s3:CreateBucket".to_string());
        perms.insert("ec2:DescribeInstances".to_string());
        perms.insert("ec2:RunInstances".to_string());
        perms
    }

    fn empty_permissions() -> HashSet<String> {
        HashSet::new()
    }

    #[test]
    fn format_starts_with_jsonencode() {
        let formatter = HclFormatter { grouped: false };
        let allow = test_permissions();
        let deny = empty_permissions();
        let output = formatter.format(&PermissionSets {
            allow: &allow,
            deny: &deny,
        });

        assert!(output.starts_with("jsonencode({"));
    }

    #[test]
    fn format_contains_version() {
        let formatter = HclFormatter { grouped: false };
        let allow = test_permissions();
        let deny = empty_permissions();
        let output = formatter.format(&PermissionSets {
            allow: &allow,
            deny: &deny,
        });

        assert!(output.contains("Version = \"2012-10-17\""));
    }

    #[test]
    fn format_contains_effect_allow() {
        let formatter = HclFormatter { grouped: false };
        let allow = test_permissions();
        let deny = empty_permissions();
        let output = formatter.format(&PermissionSets {
            allow: &allow,
            deny: &deny,
        });

        assert!(output.contains("Effect   = \"Allow\""));
    }

    #[test]
    fn format_contains_resource_wildcard() {
        let formatter = HclFormatter { grouped: false };
        let allow = test_permissions();
        let deny = empty_permissions();
        let output = formatter.format(&PermissionSets {
            allow: &allow,
            deny: &deny,
        });

        assert!(output.contains("Resource = \"*\""));
    }

    #[test]
    fn format_non_grouped_contains_all_actions() {
        let formatter = HclFormatter { grouped: false };
        let allow = test_permissions();
        let deny = empty_permissions();
        let output = formatter.format(&PermissionSets {
            allow: &allow,
            deny: &deny,
        });

        assert!(output.contains("ec2:DescribeInstances"));
        assert!(output.contains("ec2:RunInstances"));
        assert!(output.contains("s3:CreateBucket"));
    }

    #[test]
    fn format_actions_sorted() {
        let formatter = HclFormatter { grouped: false };
        let allow = test_permissions();
        let deny = empty_permissions();
        let output = formatter.format(&PermissionSets {
            allow: &allow,
            deny: &deny,
        });

        let desc_pos = output.find("ec2:DescribeInstances").unwrap();
        let run_pos = output.find("ec2:RunInstances").unwrap();
        let s3_pos = output.find("s3:CreateBucket").unwrap();

        assert!(desc_pos < run_pos);
        assert!(run_pos < s3_pos);
    }

    #[test]
    fn format_grouped_multiple_statements() {
        let formatter = HclFormatter { grouped: true };
        let allow = test_permissions();
        let deny = empty_permissions();
        let output = formatter.format(&PermissionSets {
            allow: &allow,
            deny: &deny,
        });

        let effect_count = output.matches("Effect   = \"Allow\"").count();
        assert_eq!(effect_count, 2); // One for ec2, one for s3
    }

    #[test]
    fn format_grouped_services_sorted() {
        let formatter = HclFormatter { grouped: true };
        let allow = test_permissions();
        let deny = empty_permissions();
        let output = formatter.format(&PermissionSets {
            allow: &allow,
            deny: &deny,
        });

        let ec2_pos = output.find("ec2:").unwrap();
        let s3_pos = output.find("s3:").unwrap();
        assert!(ec2_pos < s3_pos);
    }

    #[test]
    fn format_single_action_not_array() {
        let formatter = HclFormatter { grouped: false };
        let mut allow = HashSet::new();
        allow.insert("s3:GetObject".to_string());
        let deny = empty_permissions();

        let output = formatter.format(&PermissionSets {
            allow: &allow,
            deny: &deny,
        });

        assert!(output.contains("Action   = \"s3:GetObject\""));
        assert!(!output.contains("Action   = ["));
    }

    #[test]
    fn format_empty_permissions() {
        let formatter = HclFormatter { grouped: false };
        let allow = empty_permissions();
        let deny = empty_permissions();
        let output = formatter.format(&PermissionSets {
            allow: &allow,
            deny: &deny,
        });

        assert!(output.contains("Statement = []"));
    }

    #[test]
    fn extension_is_hcl() {
        let formatter = HclFormatter { grouped: false };
        assert_eq!(formatter.extension(), "hcl");
    }

    // --- New deny tests ---

    #[test]
    fn format_deny_only() {
        let formatter = HclFormatter { grouped: false };
        let allow = empty_permissions();
        let mut deny = HashSet::new();
        deny.insert("s3:GetObject".to_string());

        let output = formatter.format(&PermissionSets {
            allow: &allow,
            deny: &deny,
        });

        assert!(output.contains("Effect   = \"Deny\""));
        assert!(!output.contains("Effect   = \"Allow\""));
        assert!(output.contains("s3:GetObject"));
    }

    #[test]
    fn format_mixed_allow_and_deny() {
        let formatter = HclFormatter { grouped: false };
        let mut allow = HashSet::new();
        allow.insert("s3:Get*".to_string());
        let mut deny = HashSet::new();
        deny.insert("s3:GetObject".to_string());

        let output = formatter.format(&PermissionSets {
            allow: &allow,
            deny: &deny,
        });

        assert!(output.contains("Effect   = \"Deny\""));
        assert!(output.contains("Effect   = \"Allow\""));

        // Deny comes before Allow
        let deny_pos = output.find("Effect   = \"Deny\"").unwrap();
        let allow_pos = output.find("Effect   = \"Allow\"").unwrap();
        assert!(deny_pos < allow_pos);
    }

    #[test]
    fn format_deny_actions_sorted() {
        let formatter = HclFormatter { grouped: false };
        let allow = empty_permissions();
        let mut deny = HashSet::new();
        deny.insert("s3:PutObject".to_string());
        deny.insert("s3:GetObject".to_string());

        let output = formatter.format(&PermissionSets {
            allow: &allow,
            deny: &deny,
        });

        let get_pos = output.find("s3:GetObject").unwrap();
        let put_pos = output.find("s3:PutObject").unwrap();
        assert!(get_pos < put_pos);
    }

    #[test]
    fn format_grouped_deny_before_allow() {
        let formatter = HclFormatter { grouped: true };
        let mut allow = HashSet::new();
        allow.insert("ec2:DescribeInstances".to_string());
        let mut deny = HashSet::new();
        deny.insert("s3:GetObject".to_string());

        let output = formatter.format(&PermissionSets {
            allow: &allow,
            deny: &deny,
        });

        let deny_pos = output.find("Effect   = \"Deny\"").unwrap();
        let allow_pos = output.find("Effect   = \"Allow\"").unwrap();
        assert!(deny_pos < allow_pos);
    }

    #[test]
    fn format_grouped_deny_grouped_by_service() {
        let formatter = HclFormatter { grouped: true };
        let allow = empty_permissions();
        let mut deny = HashSet::new();
        deny.insert("s3:GetObject".to_string());
        deny.insert("ec2:TerminateInstances".to_string());

        let output = formatter.format(&PermissionSets {
            allow: &allow,
            deny: &deny,
        });

        let deny_count = output.matches("Effect   = \"Deny\"").count();
        assert_eq!(deny_count, 2);

        // ec2 before s3 (alphabetical)
        let ec2_pos = output.find("ec2:TerminateInstances").unwrap();
        let s3_pos = output.find("s3:GetObject").unwrap();
        assert!(ec2_pos < s3_pos);
    }

    #[test]
    fn format_no_empty_statements() {
        let formatter = HclFormatter { grouped: false };
        let mut allow = HashSet::new();
        allow.insert("s3:CreateBucket".to_string());
        let deny = empty_permissions();

        let output = formatter.format(&PermissionSets {
            allow: &allow,
            deny: &deny,
        });

        assert!(output.contains("Effect   = \"Allow\""));
        assert!(!output.contains("Effect   = \"Deny\""));
    }
}
