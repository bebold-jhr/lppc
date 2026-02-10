//! Plain text output formatter.
//!
//! This module provides the `PlainFormatter` which outputs permissions
//! as a simple list with one permission per line, sorted alphabetically.

use std::collections::HashSet;

use super::formatter::OutputFormatter;

/// Formatter that outputs permissions as plain text, one per line.
///
/// Permissions are sorted alphabetically for consistent, readable output.
pub struct PlainFormatter;

impl OutputFormatter for PlainFormatter {
    fn format(&self, permissions: &HashSet<String>) -> String {
        let mut sorted: Vec<_> = permissions.iter().collect();
        sorted.sort();
        sorted
            .into_iter()
            .map(|s| s.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn extension(&self) -> &'static str {
        "txt"
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
    fn format_sorts_alphabetically() {
        let formatter = PlainFormatter;
        let output = formatter.format(&test_permissions());

        let lines: Vec<_> = output.lines().collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], "ec2:DescribeInstances");
        assert_eq!(lines[1], "ec2:RunInstances");
        assert_eq!(lines[2], "s3:CreateBucket");
    }

    #[test]
    fn format_empty_permissions() {
        let formatter = PlainFormatter;
        let output = formatter.format(&HashSet::new());

        assert!(output.is_empty());
    }

    #[test]
    fn format_single_permission() {
        let formatter = PlainFormatter;
        let mut perms = HashSet::new();
        perms.insert("s3:GetObject".to_string());

        let output = formatter.format(&perms);

        assert_eq!(output, "s3:GetObject");
        assert!(!output.contains('\n'));
    }

    #[test]
    fn extension_is_txt() {
        let formatter = PlainFormatter;
        assert_eq!(formatter.extension(), "txt");
    }
}
