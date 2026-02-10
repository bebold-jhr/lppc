//! HCL output formatter for Terraform IAM policy documents.
//!
//! This module provides the `HclFormatter` which outputs permissions
//! as valid HCL using `jsonencode()` for inline policy documents.
//! It supports both a flat format (all actions in a single statement)
//! and a grouped format (one statement per AWS service).

use std::collections::{HashMap, HashSet};

use super::formatter::OutputFormatter;

/// Formatter that outputs permissions as HCL with `jsonencode()`.
///
/// The output is suitable for use in Terraform's `aws_iam_policy` or
/// inline policy documents. When `grouped` is false, all permissions
/// are in a single statement. When `grouped` is true, permissions are
/// grouped by service prefix.
pub struct HclFormatter {
    /// Whether to group permissions by service prefix.
    pub grouped: bool,
}

impl OutputFormatter for HclFormatter {
    fn format(&self, permissions: &HashSet<String>) -> String {
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
    /// Formats all permissions in a single statement.
    fn format_single(&self, permissions: &HashSet<String>) -> String {
        let mut sorted: Vec<_> = permissions.iter().collect();
        sorted.sort();

        let actions_hcl = self.format_action_list(&sorted);

        format!(
            r#"jsonencode({{
  Version = "2012-10-17"
  Statement = [
    {{
      Effect   = "Allow"
      Action   = {}
      Resource = "*"
    }}
  ]
}})"#,
            actions_hcl
        )
    }

    /// Formats permissions grouped by service prefix.
    fn format_grouped(&self, permissions: &HashSet<String>) -> String {
        let mut groups: HashMap<String, Vec<&String>> = HashMap::new();

        for perm in permissions {
            let service = perm.split(':').next().unwrap_or("unknown");
            groups.entry(service.to_string()).or_default().push(perm);
        }

        let mut service_names: Vec<_> = groups.keys().cloned().collect();
        service_names.sort();

        let statements: Vec<String> = service_names
            .into_iter()
            .map(|service| {
                let mut actions = groups.remove(&service).unwrap_or_default();
                actions.sort();
                let actions_hcl = self.format_action_list(&actions);

                format!(
                    r#"    {{
      Effect   = "Allow"
      Action   = {}
      Resource = "*"
    }}"#,
                    actions_hcl
                )
            })
            .collect();

        format!(
            r#"jsonencode({{
  Version = "2012-10-17"
  Statement = [
{}
  ]
}})"#,
            statements.join(",\n")
        )
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

    #[test]
    fn format_starts_with_jsonencode() {
        let formatter = HclFormatter { grouped: false };
        let output = formatter.format(&test_permissions());

        assert!(output.starts_with("jsonencode({"));
    }

    #[test]
    fn format_contains_version() {
        let formatter = HclFormatter { grouped: false };
        let output = formatter.format(&test_permissions());

        assert!(output.contains("Version = \"2012-10-17\""));
    }

    #[test]
    fn format_contains_effect_allow() {
        let formatter = HclFormatter { grouped: false };
        let output = formatter.format(&test_permissions());

        assert!(output.contains("Effect   = \"Allow\""));
    }

    #[test]
    fn format_contains_resource_wildcard() {
        let formatter = HclFormatter { grouped: false };
        let output = formatter.format(&test_permissions());

        assert!(output.contains("Resource = \"*\""));
    }

    #[test]
    fn format_non_grouped_contains_all_actions() {
        let formatter = HclFormatter { grouped: false };
        let output = formatter.format(&test_permissions());

        assert!(output.contains("ec2:DescribeInstances"));
        assert!(output.contains("ec2:RunInstances"));
        assert!(output.contains("s3:CreateBucket"));
    }

    #[test]
    fn format_actions_sorted() {
        let formatter = HclFormatter { grouped: false };
        let output = formatter.format(&test_permissions());

        let desc_pos = output.find("ec2:DescribeInstances").unwrap();
        let run_pos = output.find("ec2:RunInstances").unwrap();
        let s3_pos = output.find("s3:CreateBucket").unwrap();

        assert!(desc_pos < run_pos);
        assert!(run_pos < s3_pos);
    }

    #[test]
    fn format_grouped_multiple_statements() {
        let formatter = HclFormatter { grouped: true };
        let output = formatter.format(&test_permissions());

        // Count occurrences of "Effect   = \"Allow\""
        let effect_count = output.matches("Effect   = \"Allow\"").count();
        assert_eq!(effect_count, 2); // One for ec2, one for s3
    }

    #[test]
    fn format_grouped_services_sorted() {
        let formatter = HclFormatter { grouped: true };
        let output = formatter.format(&test_permissions());

        // ec2 should appear before s3 (alphabetically)
        let ec2_pos = output.find("ec2:").unwrap();
        let s3_pos = output.find("s3:").unwrap();
        assert!(ec2_pos < s3_pos);
    }

    #[test]
    fn format_single_action_not_array() {
        let formatter = HclFormatter { grouped: false };
        let mut perms = HashSet::new();
        perms.insert("s3:GetObject".to_string());

        let output = formatter.format(&perms);

        // Single action should be a string, not an array
        assert!(output.contains("Action   = \"s3:GetObject\""));
        assert!(!output.contains("Action   = ["));
    }

    #[test]
    fn format_empty_permissions() {
        let formatter = HclFormatter { grouped: false };
        let output = formatter.format(&HashSet::new());

        assert!(output.contains("Action   = []"));
    }

    #[test]
    fn extension_is_hcl() {
        let formatter = HclFormatter { grouped: false };
        assert_eq!(formatter.extension(), "hcl");
    }
}
