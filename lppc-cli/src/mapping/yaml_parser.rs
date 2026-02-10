//! YAML parser for mapping files.
//!
//! This module parses YAML mapping files into `ActionMapping` structures using
//! the saphyr YAML library. It handles the recursive `optional` structure that
//! can nest to arbitrary depth.

use saphyr::{LoadableYamlNode, Yaml};
use std::collections::HashMap;
use thiserror::Error;

use super::schema::{ActionMapping, OptionalActions};

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
/// An `ActionMapping` containing the required actions and optional actions structure.
///
/// # Errors
///
/// Returns an error if the YAML is invalid or has an unexpected structure.
///
/// # Example
///
/// ```ignore
/// let yaml = r#"
/// actions:
///   - "s3:CreateBucket"
///   - "s3:DeleteBucket"
/// optional:
///   tags:
///     - "s3:PutBucketTagging"
/// "#;
///
/// let mapping = parse_mapping(yaml).unwrap();
/// assert_eq!(mapping.actions.len(), 2);
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

    // Parse 'actions' field (required, but defaults to empty)
    let actions = parse_actions_from_mapping(mapping);

    // Parse 'optional' field (recursive structure)
    let optional = parse_optional_from_mapping(mapping)?;

    Ok(ActionMapping { actions, optional })
}

/// Parses the 'actions' array from a YAML mapping.
fn parse_actions_from_mapping(mapping: &saphyr::Mapping) -> Vec<String> {
    for (key, value) in mapping {
        if key.as_str() == Some("actions") {
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

/// Parses the 'optional' structure from a YAML mapping.
fn parse_optional_from_mapping(mapping: &saphyr::Mapping) -> Result<OptionalActions, ParseError> {
    for (key, value) in mapping {
        if key.as_str() == Some("optional") {
            return parse_optional_actions(value);
        }
    }
    Ok(OptionalActions::None)
}

/// Recursively parses the optional actions structure.
///
/// The optional structure can be:
/// - An array of strings (leaf node with actions)
/// - A hash/map of nested structures
/// - Null/missing (no optional actions)
fn parse_optional_actions(yaml: &Yaml) -> Result<OptionalActions, ParseError> {
    if let Some(arr) = yaml.as_sequence() {
        // Leaf node: array of action strings
        let actions: Vec<String> = arr
            .iter()
            .filter_map(|v: &Yaml| v.as_str().map(|s| s.to_string()))
            .collect();
        return Ok(OptionalActions::Actions(actions));
    }

    if let Some(hash) = yaml.as_mapping() {
        // Nested structure: map of key -> OptionalActions
        let mut map = HashMap::new();

        for (key, value) in hash {
            let key_str = match key.as_str() {
                Some(s) => s.to_string(),
                None => continue, // Skip non-string keys
            };

            let nested = parse_optional_actions(value)?;
            map.insert(key_str, nested);
        }

        return Ok(OptionalActions::Nested(map));
    }

    if yaml.is_null() || yaml.is_badvalue() {
        return Ok(OptionalActions::None);
    }

    Err(ParseError::InvalidStructure(
        "Expected array or mapping in optional".to_string(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn parse_simple_mapping() {
        let yaml = r#"
actions:
  - "s3:CreateBucket"
  - "s3:DeleteBucket"
"#;
        let mapping = parse_mapping(yaml).unwrap();
        assert_eq!(mapping.actions.len(), 2);
        assert!(mapping.actions.contains(&"s3:CreateBucket".to_string()));
        assert!(mapping.actions.contains(&"s3:DeleteBucket".to_string()));
        assert!(mapping.optional.is_none());
    }

    #[test]
    fn parse_mapping_with_optional() {
        let yaml = r#"
actions:
  - "route53:CreateHostedZone"
optional:
  tags:
    - "route53:ChangeTagsForResource"
"#;
        let mapping = parse_mapping(yaml).unwrap();
        assert_eq!(mapping.actions.len(), 1);
        assert!(!mapping.optional.is_none());
    }

    #[test]
    fn parse_nested_optional() {
        let yaml = r#"
actions:
  - "route53:CreateHostedZone"
optional:
  vpc:
    vpc_id:
      - "route53:AssociateVPCWithHostedZone"
"#;
        let mapping = parse_mapping(yaml).unwrap();

        let mut present = HashSet::new();
        present.insert(vec!["vpc".to_string()]);
        present.insert(vec!["vpc".to_string(), "vpc_id".to_string()]);

        let resolved = mapping.optional.resolve(&present);
        assert!(resolved.contains(&"route53:AssociateVPCWithHostedZone".to_string()));
    }

    #[test]
    fn parse_optional_not_resolved_when_absent() {
        let yaml = r#"
actions:
  - "route53:CreateHostedZone"
optional:
  tags:
    - "route53:ChangeTagsForResource"
"#;
        let mapping = parse_mapping(yaml).unwrap();

        let present = HashSet::new(); // No attributes present

        let resolved = mapping.optional.resolve(&present);
        assert!(resolved.is_empty());
    }

    #[test]
    fn parse_deeply_nested_optional() {
        let yaml = r#"
optional:
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

        let resolved = mapping.optional.resolve(&present);
        assert!(resolved.contains(&"action:DeepAction".to_string()));
    }

    #[test]
    fn parse_empty_yaml_returns_error() {
        let yaml = "";
        let result = parse_mapping(yaml);
        assert!(matches!(result, Err(ParseError::Empty)));
    }

    #[test]
    fn parse_mapping_with_no_actions() {
        let yaml = r#"
optional:
  tags:
    - "s3:PutBucketTagging"
"#;
        let mapping = parse_mapping(yaml).unwrap();
        assert!(mapping.actions.is_empty());
        assert!(!mapping.optional.is_none());
    }

    #[test]
    fn parse_mapping_with_only_actions() {
        let yaml = r#"
actions:
  - "ec2:DescribeAvailabilityZones"
"#;
        let mapping = parse_mapping(yaml).unwrap();
        assert_eq!(mapping.actions.len(), 1);
        assert!(mapping.optional.is_none());
    }

    #[test]
    fn parse_mixed_optional_structure() {
        let yaml = r#"
actions:
  - "s3:CreateBucket"
optional:
  tags:
    - "s3:PutBucketTagging"
  logging:
    target_bucket:
      - "s3:PutBucketLogging"
"#;
        let mapping = parse_mapping(yaml).unwrap();

        // Test simple optional
        let mut present_tags = HashSet::new();
        present_tags.insert(vec!["tags".to_string()]);
        let resolved_tags = mapping.optional.resolve(&present_tags);
        assert!(resolved_tags.contains(&"s3:PutBucketTagging".to_string()));

        // Test nested optional
        let mut present_logging = HashSet::new();
        present_logging.insert(vec!["logging".to_string()]);
        present_logging.insert(vec!["logging".to_string(), "target_bucket".to_string()]);
        let resolved_logging = mapping.optional.resolve(&present_logging);
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
optional:
  vpc:
    vpc_id:
      - "route53:AssociateVPCWithHostedZone"
      - "route53:DisassociateVPCFromHostedZone"
"#;
        let mapping = parse_mapping(yaml).unwrap();

        let mut present = HashSet::new();
        present.insert(vec!["vpc".to_string()]);
        present.insert(vec!["vpc".to_string(), "vpc_id".to_string()]);

        let resolved = mapping.optional.resolve(&present);
        assert_eq!(resolved.len(), 2);
        assert!(resolved.contains(&"route53:AssociateVPCWithHostedZone".to_string()));
        assert!(resolved.contains(&"route53:DisassociateVPCFromHostedZone".to_string()));
    }

    #[test]
    fn parse_siblings_at_nested_level() {
        let yaml = r#"
optional:
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

        let resolved_id = mapping.optional.resolve(&present_id);
        assert_eq!(resolved_id.len(), 1);

        // Both vpc_id and vpc_region present
        let mut present_both = HashSet::new();
        present_both.insert(vec!["vpc".to_string()]);
        present_both.insert(vec!["vpc".to_string(), "vpc_id".to_string()]);
        present_both.insert(vec!["vpc".to_string(), "vpc_region".to_string()]);

        let resolved_both = mapping.optional.resolve(&present_both);
        assert_eq!(resolved_both.len(), 2);
    }
}
