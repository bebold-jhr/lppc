//! JSON output formatter for AWS IAM policy documents.
//!
//! This module provides the `JsonFormatter` which outputs permissions
//! as valid AWS IAM policy document JSON. It supports both a flat format
//! (all actions in a single statement) and a grouped format (one statement
//! per AWS service). Deny statements appear before Allow statements.

use std::collections::{HashMap, HashSet};

use serde::Serialize;

use super::formatter::{OutputFormatter, PermissionSets};

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
/// When `grouped` is false, up to two statements are generated: one Deny
/// (if any) and one Allow (if any). When `grouped` is true, permissions
/// are grouped by service prefix (e.g., "ec2", "s3") with one statement
/// per service per effect. All Deny statements appear before Allow statements.
pub struct JsonFormatter {
    /// Whether to group permissions by service prefix.
    pub grouped: bool,
}

impl OutputFormatter for JsonFormatter {
    fn format(&self, permissions: &PermissionSets) -> String {
        let mut statements = Vec::new();

        if self.grouped {
            statements.extend(self.create_grouped_statements(permissions.deny, "Deny"));
            statements.extend(self.create_grouped_statements(permissions.allow, "Allow"));
        } else {
            if let Some(stmt) = self.create_single_statement(permissions.deny, "Deny") {
                statements.push(stmt);
            }
            if let Some(stmt) = self.create_single_statement(permissions.allow, "Allow") {
                statements.push(stmt);
            }
        }

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
    /// Creates a single statement for the given effect. Returns None if permissions is empty.
    fn create_single_statement(
        &self,
        permissions: &HashSet<String>,
        effect: &'static str,
    ) -> Option<Statement> {
        if permissions.is_empty() {
            return None;
        }

        let mut sorted: Vec<_> = permissions.iter().cloned().collect();
        sorted.sort();

        Some(Statement {
            effect,
            action: sorted,
            resource: "*",
        })
    }

    /// Creates multiple statements grouped by service prefix for the given effect.
    fn create_grouped_statements(
        &self,
        permissions: &HashSet<String>,
        effect: &'static str,
    ) -> Vec<Statement> {
        if permissions.is_empty() {
            return Vec::new();
        }

        let mut groups: HashMap<String, Vec<String>> = HashMap::new();

        for perm in permissions {
            let service = perm.split(':').next().unwrap_or("unknown").to_string();
            groups.entry(service).or_default().push(perm.clone());
        }

        let mut service_names: Vec<_> = groups.keys().cloned().collect();
        service_names.sort();

        service_names
            .into_iter()
            .map(|service| {
                let mut actions = groups.remove(&service).unwrap_or_default();
                actions.sort();
                Statement {
                    effect,
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

    fn empty_permissions() -> HashSet<String> {
        HashSet::new()
    }

    #[test]
    fn format_produces_valid_json() {
        let formatter = JsonFormatter { grouped: false };
        let allow = test_permissions();
        let deny = empty_permissions();
        let output = formatter.format(&PermissionSets {
            allow: &allow,
            deny: &deny,
        });

        let parsed: serde_json::Value =
            serde_json::from_str(&output).expect("Output should be valid JSON");
        assert_eq!(parsed["Version"], "2012-10-17");
        assert!(parsed["Statement"].is_array());
    }

    #[test]
    fn format_non_grouped_has_single_statement() {
        let formatter = JsonFormatter { grouped: false };
        let allow = test_permissions();
        let deny = empty_permissions();
        let output = formatter.format(&PermissionSets {
            allow: &allow,
            deny: &deny,
        });

        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        let statements = parsed["Statement"].as_array().unwrap();

        assert_eq!(statements.len(), 1);
        assert_eq!(statements[0]["Effect"], "Allow");
        assert_eq!(statements[0]["Resource"], "*");
    }

    #[test]
    fn format_non_grouped_actions_sorted() {
        let formatter = JsonFormatter { grouped: false };
        let allow = test_permissions();
        let deny = empty_permissions();
        let output = formatter.format(&PermissionSets {
            allow: &allow,
            deny: &deny,
        });

        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        let actions = parsed["Statement"][0]["Action"].as_array().unwrap();

        assert_eq!(actions[0], "ec2:DescribeInstances");
        assert_eq!(actions[1], "ec2:RunInstances");
        assert_eq!(actions[2], "s3:CreateBucket");
    }

    #[test]
    fn format_grouped_creates_statement_per_service() {
        let formatter = JsonFormatter { grouped: true };
        let allow = test_permissions();
        let deny = empty_permissions();
        let output = formatter.format(&PermissionSets {
            allow: &allow,
            deny: &deny,
        });

        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        let statements = parsed["Statement"].as_array().unwrap();

        assert_eq!(statements.len(), 2);
    }

    #[test]
    fn format_grouped_statements_sorted_by_service() {
        let formatter = JsonFormatter { grouped: true };
        let allow = test_permissions();
        let deny = empty_permissions();
        let output = formatter.format(&PermissionSets {
            allow: &allow,
            deny: &deny,
        });

        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        let statements = parsed["Statement"].as_array().unwrap();

        let first_actions = statements[0]["Action"].as_array().unwrap();
        assert!(first_actions[0].as_str().unwrap().starts_with("ec2:"));

        let second_actions = statements[1]["Action"].as_array().unwrap();
        assert!(second_actions[0].as_str().unwrap().starts_with("s3:"));
    }

    #[test]
    fn format_grouped_actions_sorted_within_statement() {
        let formatter = JsonFormatter { grouped: true };
        let allow = test_permissions();
        let deny = empty_permissions();
        let output = formatter.format(&PermissionSets {
            allow: &allow,
            deny: &deny,
        });

        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        let statements = parsed["Statement"].as_array().unwrap();

        let ec2_actions = statements[0]["Action"].as_array().unwrap();
        assert_eq!(ec2_actions[0], "ec2:DescribeInstances");
        assert_eq!(ec2_actions[1], "ec2:RunInstances");
    }

    #[test]
    fn format_empty_permissions() {
        let formatter = JsonFormatter { grouped: false };
        let allow = empty_permissions();
        let deny = empty_permissions();
        let output = formatter.format(&PermissionSets {
            allow: &allow,
            deny: &deny,
        });

        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        let statements = parsed["Statement"].as_array().unwrap();
        assert!(statements.is_empty());
    }

    #[test]
    fn extension_is_json() {
        let formatter = JsonFormatter { grouped: false };
        assert_eq!(formatter.extension(), "json");
    }

    // --- New deny tests ---

    #[test]
    fn format_deny_only() {
        let formatter = JsonFormatter { grouped: false };
        let allow = empty_permissions();
        let mut deny = HashSet::new();
        deny.insert("s3:GetObject".to_string());

        let output = formatter.format(&PermissionSets {
            allow: &allow,
            deny: &deny,
        });

        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        let statements = parsed["Statement"].as_array().unwrap();

        assert_eq!(statements.len(), 1);
        assert_eq!(statements[0]["Effect"], "Deny");
        assert_eq!(statements[0]["Action"][0], "s3:GetObject");
    }

    #[test]
    fn format_mixed_allow_and_deny() {
        let formatter = JsonFormatter { grouped: false };
        let mut allow = HashSet::new();
        allow.insert("s3:Get*".to_string());
        allow.insert("s3:List*".to_string());
        let mut deny = HashSet::new();
        deny.insert("s3:GetObject".to_string());

        let output = formatter.format(&PermissionSets {
            allow: &allow,
            deny: &deny,
        });

        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        let statements = parsed["Statement"].as_array().unwrap();

        assert_eq!(statements.len(), 2);
        // Deny comes first
        assert_eq!(statements[0]["Effect"], "Deny");
        assert_eq!(statements[1]["Effect"], "Allow");
    }

    #[test]
    fn format_deny_actions_sorted() {
        let formatter = JsonFormatter { grouped: false };
        let allow = empty_permissions();
        let mut deny = HashSet::new();
        deny.insert("s3:PutObject".to_string());
        deny.insert("s3:GetObject".to_string());
        deny.insert("ec2:TerminateInstances".to_string());

        let output = formatter.format(&PermissionSets {
            allow: &allow,
            deny: &deny,
        });

        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        let actions = parsed["Statement"][0]["Action"].as_array().unwrap();

        assert_eq!(actions[0], "ec2:TerminateInstances");
        assert_eq!(actions[1], "s3:GetObject");
        assert_eq!(actions[2], "s3:PutObject");
    }

    #[test]
    fn format_grouped_deny_before_allow() {
        let formatter = JsonFormatter { grouped: true };
        let mut allow = HashSet::new();
        allow.insert("ec2:DescribeInstances".to_string());
        let mut deny = HashSet::new();
        deny.insert("s3:GetObject".to_string());

        let output = formatter.format(&PermissionSets {
            allow: &allow,
            deny: &deny,
        });

        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        let statements = parsed["Statement"].as_array().unwrap();

        assert_eq!(statements.len(), 2);
        assert_eq!(statements[0]["Effect"], "Deny");
        assert_eq!(statements[1]["Effect"], "Allow");
    }

    #[test]
    fn format_grouped_deny_grouped_by_service() {
        let formatter = JsonFormatter { grouped: true };
        let allow = empty_permissions();
        let mut deny = HashSet::new();
        deny.insert("s3:GetObject".to_string());
        deny.insert("ec2:TerminateInstances".to_string());

        let output = formatter.format(&PermissionSets {
            allow: &allow,
            deny: &deny,
        });

        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        let statements = parsed["Statement"].as_array().unwrap();

        assert_eq!(statements.len(), 2);
        // Both are Deny, sorted by service
        assert_eq!(statements[0]["Effect"], "Deny");
        assert!(statements[0]["Action"][0].as_str().unwrap().starts_with("ec2:"));
        assert_eq!(statements[1]["Effect"], "Deny");
        assert!(statements[1]["Action"][0].as_str().unwrap().starts_with("s3:"));
    }

    #[test]
    fn format_no_empty_statements() {
        let formatter = JsonFormatter { grouped: false };
        let mut allow = HashSet::new();
        allow.insert("s3:CreateBucket".to_string());
        let deny = empty_permissions();

        let output = formatter.format(&PermissionSets {
            allow: &allow,
            deny: &deny,
        });

        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        let statements = parsed["Statement"].as_array().unwrap();

        // Only Allow statement, no empty Deny
        assert_eq!(statements.len(), 1);
        assert_eq!(statements[0]["Effect"], "Allow");
    }
}
