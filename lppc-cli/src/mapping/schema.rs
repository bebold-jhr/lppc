//! Mapping schema types for YAML mapping files.
//!
//! This module defines the data structures that represent parsed YAML mapping files
//! which map Terraform resource types to AWS IAM actions.

use std::collections::{HashMap, HashSet};

/// Represents a YAML mapping file for a Terraform block type.
///
/// Each mapping file contains allow actions (always needed), deny actions
/// (explicitly denied), and conditional actions that depend on the presence
/// of specific attributes in the Terraform block.
#[derive(Debug, Clone)]
pub struct ActionMapping {
    /// Allow actions (always needed for this resource type)
    pub allow: Vec<String>,

    /// Deny actions (explicitly denied for this resource type)
    pub deny: Vec<String>,

    /// Conditional actions based on attribute presence.
    /// Can be nested to any depth. Always produces allow-effect permissions.
    pub conditional: ConditionalActions,
}

/// Represents conditional actions that depend on attribute presence.
///
/// This is a recursive structure supporting arbitrary nesting depth.
/// For example, a mapping might specify that the `route53:AssociateVPCWithHostedZone`
/// action is only needed when `vpc.vpc_id` is present in the Terraform block.
#[derive(Debug, Clone, Default)]
pub enum ConditionalActions {
    /// A list of actions (leaf node)
    Actions(Vec<String>),

    /// Nested attributes mapping
    Nested(HashMap<String, ConditionalActions>),

    /// No conditional actions
    #[default]
    None,
}

impl ConditionalActions {
    /// Checks if this is an empty/none value
    pub fn is_none(&self) -> bool {
        matches!(self, ConditionalActions::None)
    }

    /// Resolves actions based on present attribute paths.
    ///
    /// # Arguments
    ///
    /// * `present_paths` - Set of attribute paths present in the terraform block.
    ///                     Each path is a `Vec<String>` like `["vpc", "vpc_id"]`.
    ///
    /// # Returns
    ///
    /// Vector of actions that should be included based on which attributes are present.
    ///
    /// # Example
    ///
    /// Given a mapping with:
    /// ```yaml
    /// conditional:
    ///   vpc:
    ///     vpc_id:
    ///       - "route53:AssociateVPCWithHostedZone"
    ///   tags:
    ///     - "route53:ChangeTagsForResource"
    /// ```
    ///
    /// If `present_paths` contains `["tags"]`, the result will include
    /// `"route53:ChangeTagsForResource"`.
    ///
    /// If `present_paths` contains `["vpc"]` and `["vpc", "vpc_id"]`, the result
    /// will include `"route53:AssociateVPCWithHostedZone"`.
    pub fn resolve(&self, present_paths: &HashSet<Vec<String>>) -> Vec<String> {
        self.resolve_recursive(&[], present_paths)
    }

    fn resolve_recursive(
        &self,
        current_path: &[String],
        present_paths: &HashSet<Vec<String>>,
    ) -> Vec<String> {
        match self {
            ConditionalActions::None => Vec::new(),

            ConditionalActions::Actions(actions) => {
                // This is a leaf node - check if the current path is present
                let path_vec = current_path.to_vec();
                if present_paths.contains(&path_vec) {
                    actions.clone()
                } else {
                    Vec::new()
                }
            }

            ConditionalActions::Nested(map) => {
                let mut result = Vec::new();

                for (key, value) in map {
                    let mut new_path = current_path.to_vec();
                    new_path.push(key.clone());

                    // Check if this path exists in present_paths
                    if present_paths.contains(&new_path) {
                        match value {
                            ConditionalActions::Actions(actions) => {
                                // Direct actions at this level
                                result.extend(actions.clone());
                            }
                            ConditionalActions::Nested(_) => {
                                // Recurse deeper
                                result.extend(value.resolve_recursive(&new_path, present_paths));
                            }
                            ConditionalActions::None => {}
                        }
                    }
                }

                result
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conditional_actions_none_is_none() {
        assert!(ConditionalActions::None.is_none());
    }

    #[test]
    fn conditional_actions_with_actions_is_not_none() {
        let actions = ConditionalActions::Actions(vec!["s3:CreateBucket".to_string()]);
        assert!(!actions.is_none());
    }

    #[test]
    fn conditional_actions_nested_is_not_none() {
        let nested = ConditionalActions::Nested(HashMap::new());
        assert!(!nested.is_none());
    }

    #[test]
    fn resolve_returns_empty_for_none() {
        let conditional = ConditionalActions::None;
        let present = HashSet::new();
        assert!(conditional.resolve(&present).is_empty());
    }

    #[test]
    fn resolve_returns_actions_when_path_present() {
        let conditional =
            ConditionalActions::Actions(vec!["s3:PutBucketTagging".to_string()]);

        let mut present = HashSet::new();
        present.insert(Vec::new()); // Empty path for root-level Actions

        let resolved = conditional.resolve(&present);
        assert_eq!(resolved, vec!["s3:PutBucketTagging".to_string()]);
    }

    #[test]
    fn resolve_returns_empty_when_path_absent() {
        let conditional =
            ConditionalActions::Actions(vec!["s3:PutBucketTagging".to_string()]);
        let present = HashSet::new(); // No paths present

        let resolved = conditional.resolve(&present);
        assert!(resolved.is_empty());
    }

    #[test]
    fn resolve_nested_single_level() {
        let mut nested_map = HashMap::new();
        nested_map.insert(
            "tags".to_string(),
            ConditionalActions::Actions(vec!["s3:PutBucketTagging".to_string()]),
        );

        let conditional = ConditionalActions::Nested(nested_map);

        let mut present = HashSet::new();
        present.insert(vec!["tags".to_string()]);

        let resolved = conditional.resolve(&present);
        assert_eq!(resolved, vec!["s3:PutBucketTagging".to_string()]);
    }

    #[test]
    fn resolve_nested_multi_level() {
        // Create: vpc -> vpc_id -> [actions]
        let mut vpc_id_map = HashMap::new();
        vpc_id_map.insert(
            "vpc_id".to_string(),
            ConditionalActions::Actions(vec![
                "route53:AssociateVPCWithHostedZone".to_string(),
            ]),
        );

        let mut root_map = HashMap::new();
        root_map.insert(
            "vpc".to_string(),
            ConditionalActions::Nested(vpc_id_map),
        );

        let conditional = ConditionalActions::Nested(root_map);

        // Both vpc and vpc.vpc_id must be present
        let mut present = HashSet::new();
        present.insert(vec!["vpc".to_string()]);
        present.insert(vec!["vpc".to_string(), "vpc_id".to_string()]);

        let resolved = conditional.resolve(&present);
        assert_eq!(
            resolved,
            vec!["route53:AssociateVPCWithHostedZone".to_string()]
        );
    }

    #[test]
    fn resolve_nested_missing_intermediate_path() {
        // Create: vpc -> vpc_id -> [actions]
        let mut vpc_id_map = HashMap::new();
        vpc_id_map.insert(
            "vpc_id".to_string(),
            ConditionalActions::Actions(vec![
                "route53:AssociateVPCWithHostedZone".to_string(),
            ]),
        );

        let mut root_map = HashMap::new();
        root_map.insert(
            "vpc".to_string(),
            ConditionalActions::Nested(vpc_id_map),
        );

        let conditional = ConditionalActions::Nested(root_map);

        // Only vpc.vpc_id present, but not vpc itself
        let mut present = HashSet::new();
        present.insert(vec!["vpc".to_string(), "vpc_id".to_string()]);

        let resolved = conditional.resolve(&present);
        // Should be empty because "vpc" path is not in present_paths
        assert!(resolved.is_empty());
    }

    #[test]
    fn resolve_multiple_conditional_paths() {
        let mut root_map = HashMap::new();
        root_map.insert(
            "tags".to_string(),
            ConditionalActions::Actions(vec!["s3:PutBucketTagging".to_string()]),
        );
        root_map.insert(
            "versioning".to_string(),
            ConditionalActions::Actions(vec!["s3:PutBucketVersioning".to_string()]),
        );

        let conditional = ConditionalActions::Nested(root_map);

        let mut present = HashSet::new();
        present.insert(vec!["tags".to_string()]);
        present.insert(vec!["versioning".to_string()]);

        let mut resolved = conditional.resolve(&present);
        resolved.sort();

        assert_eq!(
            resolved,
            vec![
                "s3:PutBucketTagging".to_string(),
                "s3:PutBucketVersioning".to_string()
            ]
        );
    }

    #[test]
    fn resolve_deeply_nested() {
        // Create: level1 -> level2 -> level3 -> [actions]
        let mut level3_map = HashMap::new();
        level3_map.insert(
            "level3".to_string(),
            ConditionalActions::Actions(vec!["action:DeepAction".to_string()]),
        );

        let mut level2_map = HashMap::new();
        level2_map.insert(
            "level2".to_string(),
            ConditionalActions::Nested(level3_map),
        );

        let mut level1_map = HashMap::new();
        level1_map.insert(
            "level1".to_string(),
            ConditionalActions::Nested(level2_map),
        );

        let conditional = ConditionalActions::Nested(level1_map);

        let mut present = HashSet::new();
        present.insert(vec!["level1".to_string()]);
        present.insert(vec!["level1".to_string(), "level2".to_string()]);
        present.insert(vec![
            "level1".to_string(),
            "level2".to_string(),
            "level3".to_string(),
        ]);

        let resolved = conditional.resolve(&present);
        assert_eq!(resolved, vec!["action:DeepAction".to_string()]);
    }

    #[test]
    fn action_mapping_with_deny_field() {
        let mapping = ActionMapping {
            allow: vec!["s3:Get*".to_string(), "s3:List*".to_string()],
            deny: vec!["s3:GetObject".to_string()],
            conditional: ConditionalActions::None,
        };

        assert_eq!(mapping.allow.len(), 2);
        assert_eq!(mapping.deny.len(), 1);
        assert!(mapping.deny.contains(&"s3:GetObject".to_string()));
        assert!(!mapping.allow.contains(&"s3:GetObject".to_string()));
    }
}
