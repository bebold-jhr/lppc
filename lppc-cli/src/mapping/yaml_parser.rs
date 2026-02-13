//! YAML parser for mapping files.
//!
//! This module parses YAML mapping files into `ActionMapping` structures using
//! the saphyr YAML library. It handles the recursive `conditional` structure that
//! can nest to arbitrary depth.

use saphyr::{LoadableYamlNode, Yaml};
use std::collections::HashMap;
use thiserror::Error;

use super::schema::{ActionMapping, ConditionalActions};

/// Errors that can occur during YAML parsing.
#[derive(Debug, Error)]
pub enum ParseError {
    #[error("YAML parse error: {0}")]
    Yaml(String),

    #[error("Empty YAML document")]
    Empty,

    #[error("Invalid structure: {0}")]
    InvalidStructure(String),
}

/// Parses YAML content into an `ActionMapping`.
///
/// # Arguments
///
/// * `content` - The raw YAML content as a string
///
/// # Returns
///
/// An `ActionMapping` containing the allow actions, deny actions, and conditional
/// actions structure.
///
/// # Errors
///
/// Returns an error if the YAML is invalid or has an unexpected structure.
///
/// # Example
///
/// ```ignore
/// let yaml = r#"
/// allow:
///   - "s3:CreateBucket"
///   - "s3:DeleteBucket"
/// deny:
///   - "s3:GetObject"
/// conditional:
///   tags:
///     - "s3:PutBucketTagging"
/// "#;
///
/// let mapping = parse_mapping(yaml).unwrap();
/// assert_eq!(mapping.allow.len(), 2);
/// assert_eq!(mapping.deny.len(), 1);
/// ```
pub fn parse_mapping(content: &str) -> Result<ActionMapping, ParseError> {
    let docs = Yaml::load_from_str(content).map_err(|e| ParseError::Yaml(e.to_string()))?;

    if docs.is_empty() {
        return Err(ParseError::Empty);
    }

    let doc = &docs[0];

    // Get the mapping from the document
    let mapping = doc.as_mapping().ok_or_else(|| {
        ParseError::InvalidStructure("Root document must be a mapping".to_string())
    })?;

    let allow = parse_string_list_from_mapping(mapping, "allow");
    let deny = parse_string_list_from_mapping(mapping, "deny");
    let conditional = parse_conditional_from_mapping(mapping)?;

    Ok(ActionMapping {
        allow,
        deny,
        conditional,
    })
}

/// Parses a string list from a YAML mapping under the given key.
///
/// This shared helper is used for both the `allow` and `deny` keys,
/// which have identical parsing logic.
fn parse_string_list_from_mapping(mapping: &saphyr::Mapping, key: &str) -> Vec<String> {
    for (k, value) in mapping {
        if k.as_str() == Some(key) {
            if let Some(arr) = value.as_sequence() {
                return arr
                    .iter()
                    .filter_map(|v: &Yaml| v.as_str().map(|s| s.to_string()))
                    .collect();
            }
        }
    }
    Vec::new()
}

/// Parses the 'conditional' structure from a YAML mapping.
fn parse_conditional_from_mapping(
    mapping: &saphyr::Mapping,
) -> Result<ConditionalActions, ParseError> {
    for (key, value) in mapping {
        if key.as_str() == Some("conditional") {
            return parse_conditional_actions(value);
        }
    }
    Ok(ConditionalActions::None)
}

/// Recursively parses the conditional actions structure.
///
/// The conditional structure can be:
/// - An array of strings (leaf node with actions)
/// - A hash/map of nested structures
/// - Null/missing (no conditional actions)
fn parse_conditional_actions(yaml: &Yaml) -> Result<ConditionalActions, ParseError> {
    if let Some(arr) = yaml.as_sequence() {
        // Leaf node: array of action strings
        let actions: Vec<String> = arr
            .iter()
            .filter_map(|v: &Yaml| v.as_str().map(|s| s.to_string()))
            .collect();
        return Ok(ConditionalActions::Actions(actions));
    }

    if let Some(hash) = yaml.as_mapping() {
        // Nested structure: map of key -> ConditionalActions
        let mut map = HashMap::new();

        for (key, value) in hash {
            let key_str = match key.as_str() {
                Some(s) => s.to_string(),
                None => continue, // Skip non-string keys
            };

            let nested = parse_conditional_actions(value)?;
            map.insert(key_str, nested);
        }

        return Ok(ConditionalActions::Nested(map));
    }

    if yaml.is_null() || yaml.is_badvalue() {
        return Ok(ConditionalActions::None);
    }

    Err(ParseError::InvalidStructure(
        "Expected array or mapping in conditional".to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn parse_simple_mapping() {
        let yaml = r#"
allow:
  - "s3:CreateBucket"
  - "s3:DeleteBucket"
"#;
        let mapping = parse_mapping(yaml).unwrap();
        assert_eq!(mapping.allow.len(), 2);
        assert!(mapping.allow.contains(&"s3:CreateBucket".to_string()));
        assert!(mapping.allow.contains(&"s3:DeleteBucket".to_string()));
        assert!(mapping.conditional.is_none());
        assert!(mapping.deny.is_empty());
    }

    #[test]
    fn parse_mapping_with_conditional() {
        let yaml = r#"
allow:
  - "route53:CreateHostedZone"
conditional:
  tags:
    - "route53:ChangeTagsForResource"
"#;
        let mapping = parse_mapping(yaml).unwrap();
        assert_eq!(mapping.allow.len(), 1);
        assert!(!mapping.conditional.is_none());
    }

    #[test]
    fn parse_nested_conditional() {
        let yaml = r#"
allow:
  - "route53:CreateHostedZone"
conditional:
  vpc:
    vpc_id:
      - "route53:AssociateVPCWithHostedZone"
"#;
        let mapping = parse_mapping(yaml).unwrap();

        let mut present = HashSet::new();
        present.insert(vec!["vpc".to_string()]);
        present.insert(vec!["vpc".to_string(), "vpc_id".to_string()]);

        let resolved = mapping.conditional.resolve(&present);
        assert!(resolved.contains(&"route53:AssociateVPCWithHostedZone".to_string()));
    }

    #[test]
    fn parse_conditional_not_resolved_when_absent() {
        let yaml = r#"
allow:
  - "route53:CreateHostedZone"
conditional:
  tags:
    - "route53:ChangeTagsForResource"
"#;
        let mapping = parse_mapping(yaml).unwrap();

        let present = HashSet::new(); // No attributes present

        let resolved = mapping.conditional.resolve(&present);
        assert!(resolved.is_empty());
    }

    #[test]
    fn parse_deeply_nested_conditional() {
        let yaml = r#"
conditional:
  level1:
    level2:
      level3:
        - "action:DeepAction"
"#;
        let mapping = parse_mapping(yaml).unwrap();

        let mut present = HashSet::new();
        present.insert(vec!["level1".to_string()]);
        present.insert(vec!["level1".to_string(), "level2".to_string()]);
        present.insert(vec![
            "level1".to_string(),
            "level2".to_string(),
            "level3".to_string(),
        ]);

        let resolved = mapping.conditional.resolve(&present);
        assert!(resolved.contains(&"action:DeepAction".to_string()));
    }

    #[test]
    fn parse_empty_yaml_returns_error() {
        let yaml = "";
        let result = parse_mapping(yaml);
        assert!(matches!(result, Err(ParseError::Empty)));
    }

    #[test]
    fn parse_mapping_with_no_allow() {
        let yaml = r#"
conditional:
  tags:
    - "s3:PutBucketTagging"
"#;
        let mapping = parse_mapping(yaml).unwrap();
        assert!(mapping.allow.is_empty());
        assert!(mapping.deny.is_empty());
        assert!(!mapping.conditional.is_none());
    }

    #[test]
    fn parse_mapping_with_only_allow() {
        let yaml = r#"
allow:
  - "ec2:DescribeAvailabilityZones"
"#;
        let mapping = parse_mapping(yaml).unwrap();
        assert_eq!(mapping.allow.len(), 1);
        assert!(mapping.conditional.is_none());
        assert!(mapping.deny.is_empty());
    }

    #[test]
    fn parse_mixed_conditional_structure() {
        let yaml = r#"
allow:
  - "s3:CreateBucket"
conditional:
  tags:
    - "s3:PutBucketTagging"
  logging:
    target_bucket:
      - "s3:PutBucketLogging"
"#;
        let mapping = parse_mapping(yaml).unwrap();

        // Test simple conditional
        let mut present_tags = HashSet::new();
        present_tags.insert(vec!["tags".to_string()]);
        let resolved_tags = mapping.conditional.resolve(&present_tags);
        assert!(resolved_tags.contains(&"s3:PutBucketTagging".to_string()));

        // Test nested conditional
        let mut present_logging = HashSet::new();
        present_logging.insert(vec!["logging".to_string()]);
        present_logging.insert(vec!["logging".to_string(), "target_bucket".to_string()]);
        let resolved_logging = mapping.conditional.resolve(&present_logging);
        assert!(resolved_logging.contains(&"s3:PutBucketLogging".to_string()));
    }

    #[test]
    fn parse_invalid_yaml_returns_error() {
        let yaml = "{{invalid yaml";
        let result = parse_mapping(yaml);
        assert!(matches!(result, Err(ParseError::Yaml(_))));
    }

    #[test]
    fn parse_multiple_actions_at_same_level() {
        let yaml = r#"
conditional:
  vpc:
    vpc_id:
      - "route53:AssociateVPCWithHostedZone"
      - "route53:DisassociateVPCFromHostedZone"
"#;
        let mapping = parse_mapping(yaml).unwrap();

        let mut present = HashSet::new();
        present.insert(vec!["vpc".to_string()]);
        present.insert(vec!["vpc".to_string(), "vpc_id".to_string()]);

        let resolved = mapping.conditional.resolve(&present);
        assert_eq!(resolved.len(), 2);
        assert!(resolved.contains(&"route53:AssociateVPCWithHostedZone".to_string()));
        assert!(resolved.contains(&"route53:DisassociateVPCFromHostedZone".to_string()));
    }

    #[test]
    fn parse_siblings_at_nested_level() {
        let yaml = r#"
conditional:
  vpc:
    vpc_id:
      - "route53:AssociateVPCWithHostedZone"
    vpc_region:
      - "route53:AssociateVPCWithHostedZone"
"#;
        let mapping = parse_mapping(yaml).unwrap();

        // Only vpc_id present
        let mut present_id = HashSet::new();
        present_id.insert(vec!["vpc".to_string()]);
        present_id.insert(vec!["vpc".to_string(), "vpc_id".to_string()]);

        let resolved_id = mapping.conditional.resolve(&present_id);
        assert_eq!(resolved_id.len(), 1);

        // Both vpc_id and vpc_region present
        let mut present_both = HashSet::new();
        present_both.insert(vec!["vpc".to_string()]);
        present_both.insert(vec!["vpc".to_string(), "vpc_id".to_string()]);
        present_both.insert(vec!["vpc".to_string(), "vpc_region".to_string()]);

        let resolved_both = mapping.conditional.resolve(&present_both);
        assert_eq!(resolved_both.len(), 2);
    }

    // --- New deny tests ---

    #[test]
    fn parse_mapping_with_deny() {
        let yaml = r#"
deny:
  - "s3:GetObject"
  - "s3:PutObject"
"#;
        let mapping = parse_mapping(yaml).unwrap();
        assert_eq!(mapping.deny.len(), 2);
        assert!(mapping.deny.contains(&"s3:GetObject".to_string()));
        assert!(mapping.deny.contains(&"s3:PutObject".to_string()));
        assert!(mapping.allow.is_empty());
    }

    #[test]
    fn parse_mapping_with_allow_and_deny() {
        let yaml = r#"
allow:
  - "s3:Get*"
  - "s3:List*"
deny:
  - "s3:GetObject"
"#;
        let mapping = parse_mapping(yaml).unwrap();
        assert_eq!(mapping.allow.len(), 2);
        assert_eq!(mapping.deny.len(), 1);
        assert!(mapping.allow.contains(&"s3:Get*".to_string()));
        assert!(mapping.deny.contains(&"s3:GetObject".to_string()));
    }

    #[test]
    fn parse_mapping_with_all_three_sections() {
        let yaml = r#"
allow:
  - "s3:Get*"
deny:
  - "s3:GetObject"
conditional:
  tags:
    - "s3:PutBucketTagging"
"#;
        let mapping = parse_mapping(yaml).unwrap();
        assert_eq!(mapping.allow.len(), 1);
        assert_eq!(mapping.deny.len(), 1);
        assert!(!mapping.conditional.is_none());

        let mut present = HashSet::new();
        present.insert(vec!["tags".to_string()]);
        let resolved = mapping.conditional.resolve(&present);
        assert!(resolved.contains(&"s3:PutBucketTagging".to_string()));
    }

    #[test]
    fn parse_mapping_deny_empty_when_absent() {
        let yaml = r#"
allow:
  - "s3:CreateBucket"
"#;
        let mapping = parse_mapping(yaml).unwrap();
        assert!(mapping.deny.is_empty());
    }
}
