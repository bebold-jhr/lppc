//! JSON output formatter for AWS IAM policy documents.
//!
//! This module provides the `JsonFormatter` which outputs permissions
//! as valid AWS IAM policy document JSON. It supports both a flat format
//! (all actions in a single statement) and a grouped format (one statement
//! per AWS service).

use std::collections::{HashMap, HashSet};

use serde::Serialize;

use super::formatter::OutputFormatter;

/// AWS IAM policy document structure.
#[derive(Serialize)]
struct PolicyDocument {
    #[serde(rename = "Version")]
    version: &'static str,
    #[serde(rename = "Statement")]
    statement: Vec<Statement>,
}

/// A single statement in an IAM policy.
#[derive(Serialize)]
struct Statement {
    #[serde(rename = "Effect")]
    effect: &'static str,
    #[serde(rename = "Action")]
    action: Vec<String>,
    #[serde(rename = "Resource")]
    resource: &'static str,
}

/// Formatter that outputs permissions as AWS IAM policy document JSON.
///
/// When `grouped` is false, all permissions are in a single statement.
/// When `grouped` is true, permissions are grouped by service prefix
/// (e.g., "ec2", "s3") with one statement per service.
pub struct JsonFormatter {
    /// Whether to group permissions by service prefix.
    pub grouped: bool,
}

impl OutputFormatter for JsonFormatter {
    fn format(&self, permissions: &HashSet<String>) -> String {
        let statements = if self.grouped {
            self.create_grouped_statements(permissions)
        } else {
            self.create_single_statement(permissions)
        };

        let document = PolicyDocument {
            version: "2012-10-17",
            statement: statements,
        };

        serde_json::to_string_pretty(&document).expect("JSON serialization should not fail")
    }

    fn extension(&self) -> &'static str {
        "json"
    }
}

impl JsonFormatter {
    /// Creates a single statement containing all permissions.
    fn create_single_statement(&self, permissions: &HashSet<String>) -> Vec<Statement> {
        let mut sorted: Vec<_> = permissions.iter().cloned().collect();
        sorted.sort();

        vec![Statement {
            effect: "Allow",
            action: sorted,
            resource: "*",
        }]
    }

    /// Creates multiple statements, one per service prefix.
    fn create_grouped_statements(&self, permissions: &HashSet<String>) -> Vec<Statement> {
        // Group by service prefix (e.g., "ec2", "s3")
        let mut groups: HashMap<String, Vec<String>> = HashMap::new();

        for perm in permissions {
            let service = perm.split(':').next().unwrap_or("unknown").to_string();
            groups.entry(service).or_default().push(perm.clone());
        }

        // Sort groups by service name, sort actions within each group
        let mut service_names: Vec<_> = groups.keys().cloned().collect();
        service_names.sort();

        service_names
            .into_iter()
            .map(|service| {
                let mut actions = groups.remove(&service).unwrap_or_default();
                actions.sort();
                Statement {
                    effect: "Allow",
                    action: actions,
                    resource: "*",
                }
            })
            .collect()
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
    fn format_produces_valid_json() {
        let formatter = JsonFormatter { grouped: false };
        let output = formatter.format(&test_permissions());

        // Should be valid JSON
        let parsed: serde_json::Value =
            serde_json::from_str(&output).expect("Output should be valid JSON");
        assert_eq!(parsed["Version"], "2012-10-17");
        assert!(parsed["Statement"].is_array());
    }

    #[test]
    fn format_non_grouped_has_single_statement() {
        let formatter = JsonFormatter { grouped: false };
        let output = formatter.format(&test_permissions());

        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        let statements = parsed["Statement"].as_array().unwrap();

        assert_eq!(statements.len(), 1);
        assert_eq!(statements[0]["Effect"], "Allow");
        assert_eq!(statements[0]["Resource"], "*");
    }

    #[test]
    fn format_non_grouped_actions_sorted() {
        let formatter = JsonFormatter { grouped: false };
        let output = formatter.format(&test_permissions());

        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        let actions = parsed["Statement"][0]["Action"].as_array().unwrap();

        // Should be sorted alphabetically
        assert_eq!(actions[0], "ec2:DescribeInstances");
        assert_eq!(actions[1], "ec2:RunInstances");
        assert_eq!(actions[2], "s3:CreateBucket");
    }

    #[test]
    fn format_grouped_creates_statement_per_service() {
        let formatter = JsonFormatter { grouped: true };
        let output = formatter.format(&test_permissions());

        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        let statements = parsed["Statement"].as_array().unwrap();

        // Should have 2 statements (ec2 and s3)
        assert_eq!(statements.len(), 2);
    }

    #[test]
    fn format_grouped_statements_sorted_by_service() {
        let formatter = JsonFormatter { grouped: true };
        let output = formatter.format(&test_permissions());

        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        let statements = parsed["Statement"].as_array().unwrap();

        // First statement should be ec2 (alphabetically first)
        let first_actions = statements[0]["Action"].as_array().unwrap();
        assert!(first_actions[0].as_str().unwrap().starts_with("ec2:"));

        // Second statement should be s3
        let second_actions = statements[1]["Action"].as_array().unwrap();
        assert!(second_actions[0].as_str().unwrap().starts_with("s3:"));
    }

    #[test]
    fn format_grouped_actions_sorted_within_statement() {
        let formatter = JsonFormatter { grouped: true };
        let output = formatter.format(&test_permissions());

        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        let statements = parsed["Statement"].as_array().unwrap();

        // ec2 actions should be sorted
        let ec2_actions = statements[0]["Action"].as_array().unwrap();
        assert_eq!(ec2_actions[0], "ec2:DescribeInstances");
        assert_eq!(ec2_actions[1], "ec2:RunInstances");
    }

    #[test]
    fn format_empty_permissions() {
        let formatter = JsonFormatter { grouped: false };
        let output = formatter.format(&HashSet::new());

        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        let statements = parsed["Statement"].as_array().unwrap();

        assert_eq!(statements.len(), 1);
        let actions = statements[0]["Action"].as_array().unwrap();
        assert!(actions.is_empty());
    }

    #[test]
    fn extension_is_json() {
        let formatter = JsonFormatter { grouped: false };
        assert_eq!(formatter.extension(), "json");
    }
}
