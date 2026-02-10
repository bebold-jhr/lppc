use std::collections::HashMap;

/// Represents an AWS provider configuration
#[derive(Debug, Clone)]
pub struct AwsProvider {
    /// Provider config key (e.g., "aws", "aws.secondary")
    pub config_key: String,

    /// Provider alias (None for default provider)
    #[allow(dead_code)]
    pub alias: Option<String>,

    /// Role ARN from assume_role block (if present)
    pub role_arn: Option<String>,

    /// Region (for informational purposes)
    #[allow(dead_code)]
    pub region: Option<String>,
}

impl AwsProvider {
    /// Derives the output name from the provider alias.
    /// Format: {ALIAS}Deployer (PascalCase, if not already ending in Deployer)
    ///
    /// Examples:
    /// - None -> DefaultDeployer
    /// - Some("") -> DefaultDeployer (empty string treated as no alias)
    /// - Some("network") -> NetworkDeployer
    /// - Some("workload_test") -> WorkloadTestDeployer
    /// - Some("NetworkDeployer") -> NetworkDeployer
    pub fn output_name(&self) -> String {
        match &self.alias {
            Some(alias) if !alias.is_empty() => Self::derive_name_from_alias(alias),
            _ => "DefaultDeployer".to_string(),
        }
    }

    /// Converts an alias to an output name in PascalCase with Deployer suffix.
    fn derive_name_from_alias(alias: &str) -> String {
        let pascal_alias = Self::to_pascal_case(alias);

        // Case-insensitive check for "deployer" suffix since PascalCase lowercases everything after first char
        if pascal_alias.to_lowercase().ends_with("deployer") {
            // Ensure the suffix is properly cased as "Deployer"
            let prefix_len = pascal_alias.len() - 8; // "deployer".len() = 8
            format!("{}Deployer", &pascal_alias[..prefix_len])
        } else {
            format!("{}Deployer", pascal_alias)
        }
    }

    /// Converts a string to PascalCase by splitting on underscores and hyphens
    /// and capitalizing the first letter of each segment.
    ///
    /// Rules:
    /// - If contains `_` or `-`: split and PascalCase each segment
    /// - If all lowercase: capitalize first letter
    /// - If already has mixed case (likely PascalCase): preserve as-is
    ///
    /// Examples:
    /// - "workload_network_test" -> "WorkloadNetworkTest"
    /// - "NETWORK_DEPLOYER" -> "NetworkDeployer"
    /// - "my-role-name" -> "MyRoleName"
    /// - "network" -> "Network" (single lowercase word capitalized)
    /// - "DnsAccount" -> "DnsAccount" (already PascalCase, preserved)
    /// - "NetworkDeployer" -> "NetworkDeployer" (already PascalCase, preserved)
    pub fn to_pascal_case(input: &str) -> String {
        if input.is_empty() {
            return String::new();
        }

        // If contains separators, split and convert each segment
        if input.contains('_') || input.contains('-') {
            return input
                .split(|c| c == '_' || c == '-')
                .filter(|segment| !segment.is_empty())
                .map(|segment| {
                    let mut chars = segment.chars();
                    match chars.next() {
                        Some(first) => {
                            let first_upper = first.to_uppercase().to_string();
                            let rest: String = chars.collect::<String>().to_lowercase();
                            first_upper + &rest
                        }
                        None => String::new(),
                    }
                })
                .collect();
        }

        // No separators - check if it's all lowercase
        let is_all_lowercase = input.chars().all(|c| !c.is_uppercase());

        if is_all_lowercase {
            // Capitalize first letter of single lowercase word
            let mut chars = input.chars();
            match chars.next() {
                Some(first) => {
                    let first_upper = first.to_uppercase().to_string();
                    first_upper + chars.as_str()
                }
                None => String::new(),
            }
        } else {
            // Already has mixed case (likely PascalCase), preserve as-is
            input.to_string()
        }
    }
}

/// Collection of AWS providers indexed by config key
#[derive(Debug, Default)]
pub struct ProviderRegistry {
    /// Providers indexed by config_key (e.g., "aws", "aws.secondary")
    providers: HashMap<String, AwsProvider>,
}

impl ProviderRegistry {
    /// Adds a provider to the registry
    pub fn add(&mut self, provider: AwsProvider) {
        self.providers.insert(provider.config_key.clone(), provider);
    }

    /// Gets the provider for a given config key
    pub fn get(&self, config_key: &str) -> Option<&AwsProvider> {
        self.providers.get(config_key)
    }

    /// Gets the default provider (config key "aws" with no alias)
    pub fn get_default(&self) -> Option<&AwsProvider> {
        self.providers.get("aws")
    }

    /// Returns the number of providers in the registry
    pub fn len(&self) -> usize {
        self.providers.len()
    }

    /// Returns true if the registry contains no providers
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.providers.is_empty()
    }

    /// Groups providers by their role ARN and derives output name from the first alias alphabetically.
    ///
    /// Providers with the same role_arn are grouped together (they represent the same deployer).
    /// The output name for each group is derived from the first alias when sorted alphabetically,
    /// where None (no alias) sorts before any string value.
    ///
    /// Returns map of output_name -> list of config_keys that use that role
    pub fn group_by_output_name(&self) -> HashMap<String, Vec<String>> {
        // First, group providers by role_arn
        let mut role_groups: HashMap<Option<&str>, Vec<&AwsProvider>> = HashMap::new();

        for provider in self.providers.values() {
            let role_key = provider.role_arn.as_deref();
            role_groups.entry(role_key).or_default().push(provider);
        }

        // For each role group, determine output name from first alias alphabetically
        let mut output_groups: HashMap<String, Vec<String>> = HashMap::new();

        for providers in role_groups.into_values() {
            // Sort providers by alias (None comes first, then alphabetically)
            let mut sorted_providers = providers;
            sorted_providers.sort_by(|a, b| match (&a.alias, &b.alias) {
                (None, None) => std::cmp::Ordering::Equal,
                (None, Some(_)) => std::cmp::Ordering::Less,
                (Some(_), None) => std::cmp::Ordering::Greater,
                (Some(a_alias), Some(b_alias)) => a_alias.cmp(b_alias),
            });

            // Use the first provider's output_name for the group
            let output_name = sorted_providers[0].output_name();

            // Collect all config_keys and sort them
            let mut config_keys: Vec<String> = sorted_providers
                .iter()
                .map(|p| p.config_key.clone())
                .collect();
            config_keys.sort();

            output_groups.insert(output_name, config_keys);
        }

        output_groups
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ==================== to_pascal_case tests ====================

    #[test]
    fn to_pascal_case_converts_snake_case() {
        assert_eq!(AwsProvider::to_pascal_case("workload_network_test"), "WorkloadNetworkTest");
    }

    #[test]
    fn to_pascal_case_converts_kebab_case() {
        assert_eq!(AwsProvider::to_pascal_case("my-role-name"), "MyRoleName");
    }

    #[test]
    fn to_pascal_case_converts_screaming_snake_case() {
        assert_eq!(AwsProvider::to_pascal_case("NETWORK_DEPLOYER"), "NetworkDeployer");
    }

    #[test]
    fn to_pascal_case_handles_mixed_separators() {
        assert_eq!(AwsProvider::to_pascal_case("mixed_case-example"), "MixedCaseExample");
    }

    #[test]
    fn to_pascal_case_handles_consecutive_separators() {
        assert_eq!(AwsProvider::to_pascal_case("double__underscore"), "DoubleUnderscore");
        assert_eq!(AwsProvider::to_pascal_case("double--hyphen"), "DoubleHyphen");
    }

    #[test]
    fn to_pascal_case_handles_leading_trailing_separators() {
        assert_eq!(AwsProvider::to_pascal_case("_leading"), "Leading");
        assert_eq!(AwsProvider::to_pascal_case("trailing_"), "Trailing");
        assert_eq!(AwsProvider::to_pascal_case("_both_"), "Both");
    }

    #[test]
    fn to_pascal_case_preserves_existing_pascal_case() {
        // Existing PascalCase (mixed case) is preserved as-is
        assert_eq!(AwsProvider::to_pascal_case("NetworkDeployer"), "NetworkDeployer");
        assert_eq!(AwsProvider::to_pascal_case("DnsAccount"), "DnsAccount");
    }

    #[test]
    fn to_pascal_case_handles_single_lowercase_word() {
        // Single lowercase words are capitalized
        assert_eq!(AwsProvider::to_pascal_case("network"), "Network");
        assert_eq!(AwsProvider::to_pascal_case("dns"), "Dns");
    }

    #[test]
    fn to_pascal_case_handles_single_uppercase_word() {
        // Words with any uppercase are preserved (could be PascalCase or SCREAMING)
        assert_eq!(AwsProvider::to_pascal_case("NETWORK"), "NETWORK");
        assert_eq!(AwsProvider::to_pascal_case("Network"), "Network");
    }

    #[test]
    fn to_pascal_case_handles_empty_string() {
        assert_eq!(AwsProvider::to_pascal_case(""), "");
    }

    // ==================== output_name tests (alias-based) ====================

    #[test]
    fn output_name_no_alias_returns_default() {
        let provider = AwsProvider {
            config_key: "aws".to_string(),
            alias: None,
            role_arn: Some("arn:aws:iam::123456789012:role/NetworkDeployer".to_string()),
            region: None,
        };
        // Output name is derived from alias (None), not role_arn
        assert_eq!(provider.output_name(), "DefaultDeployer");
    }

    #[test]
    fn output_name_simple_alias() {
        let provider = AwsProvider {
            config_key: "aws.network".to_string(),
            alias: Some("network".to_string()),
            role_arn: Some("arn:aws:iam::123456789012:role/SomeRole".to_string()),
            region: None,
        };
        assert_eq!(provider.output_name(), "NetworkDeployer");
    }

    #[test]
    fn output_name_snake_case_alias() {
        let provider = AwsProvider {
            config_key: "aws.workload_network_test".to_string(),
            alias: Some("workload_network_test".to_string()),
            role_arn: Some("arn:aws:iam::123456789012:role/SomeRole".to_string()),
            region: None,
        };
        assert_eq!(provider.output_name(), "WorkloadNetworkTestDeployer");
    }

    #[test]
    fn output_name_kebab_case_alias() {
        let provider = AwsProvider {
            config_key: "aws.my-application".to_string(),
            alias: Some("my-application".to_string()),
            role_arn: None,
            region: None,
        };
        assert_eq!(provider.output_name(), "MyApplicationDeployer");
    }

    #[test]
    fn output_name_alias_with_deployer_suffix() {
        let provider = AwsProvider {
            config_key: "aws.network_deployer".to_string(),
            alias: Some("network_deployer".to_string()),
            role_arn: None,
            region: None,
        };
        // Already ends with Deployer after PascalCase conversion
        assert_eq!(provider.output_name(), "NetworkDeployer");
    }

    #[test]
    fn output_name_alias_already_pascal_case() {
        let provider = AwsProvider {
            config_key: "aws.NetworkDeployer".to_string(),
            alias: Some("NetworkDeployer".to_string()),
            role_arn: None,
            region: None,
        };
        // "NetworkDeployer" is preserved (already PascalCase with mixed case)
        // Already ends with "Deployer", so no suffix added
        assert_eq!(provider.output_name(), "NetworkDeployer");
    }

    #[test]
    fn output_name_empty_alias_returns_default() {
        let provider = AwsProvider {
            config_key: "aws".to_string(),
            alias: Some("".to_string()),
            role_arn: Some("arn:aws:iam::123456789012:role/SomeRole".to_string()),
            region: None,
        };
        // Empty string treated as no alias
        assert_eq!(provider.output_name(), "DefaultDeployer");
    }

    #[test]
    fn output_name_no_role_no_alias() {
        let provider = AwsProvider {
            config_key: "aws".to_string(),
            alias: None,
            role_arn: None,
            region: None,
        };
        assert_eq!(provider.output_name(), "DefaultDeployer");
    }

    // ==================== ProviderRegistry tests ====================

    #[test]
    fn provider_registry_add_and_get() {
        let mut registry = ProviderRegistry::default();

        let provider = AwsProvider {
            config_key: "aws".to_string(),
            alias: None,
            role_arn: Some("arn:aws:iam::123456789012:role/TestRole".to_string()),
            region: Some("us-east-1".to_string()),
        };

        registry.add(provider);

        assert_eq!(registry.len(), 1);
        assert!(registry.get("aws").is_some());
        assert!(registry.get("aws.secondary").is_none());
    }

    #[test]
    fn provider_registry_get_default() {
        let mut registry = ProviderRegistry::default();

        let default_provider = AwsProvider {
            config_key: "aws".to_string(),
            alias: None,
            role_arn: None,
            region: None,
        };

        let aliased_provider = AwsProvider {
            config_key: "aws.secondary".to_string(),
            alias: Some("secondary".to_string()),
            role_arn: Some("arn:aws:iam::123456789012:role/Secondary".to_string()),
            region: None,
        };

        registry.add(default_provider);
        registry.add(aliased_provider);

        let default = registry.get_default().unwrap();
        assert_eq!(default.config_key, "aws");
    }

    #[test]
    fn provider_registry_groups_by_role_arn() {
        let mut registry = ProviderRegistry::default();

        // Two providers with same role ARN should be grouped together
        registry.add(AwsProvider {
            config_key: "aws".to_string(),
            alias: None,
            role_arn: Some("arn:aws:iam::123456789012:role/NetworkDeployer".to_string()),
            region: Some("us-east-1".to_string()),
        });

        registry.add(AwsProvider {
            config_key: "aws.west".to_string(),
            alias: Some("west".to_string()),
            role_arn: Some("arn:aws:iam::123456789012:role/NetworkDeployer".to_string()),
            region: Some("us-west-2".to_string()),
        });

        // One provider with different role is a separate group
        registry.add(AwsProvider {
            config_key: "aws.dns".to_string(),
            alias: Some("dns".to_string()),
            role_arn: Some("arn:aws:iam::987654321012:role/DnsRole".to_string()),
            region: None,
        });

        let groups = registry.group_by_output_name();

        assert_eq!(groups.len(), 2);

        // Same role ARN grouped together, output name from first alias (None -> Default)
        let default_keys = groups.get("DefaultDeployer").unwrap();
        assert_eq!(default_keys.len(), 2);
        assert!(default_keys.contains(&"aws".to_string()));
        assert!(default_keys.contains(&"aws.west".to_string()));

        // Different role ARN is separate group
        let dns_keys = groups.get("DnsDeployer").unwrap();
        assert_eq!(dns_keys.len(), 1);
        assert!(dns_keys.contains(&"aws.dns".to_string()));
    }

    #[test]
    fn provider_registry_uses_first_alias_alphabetically() {
        let mut registry = ProviderRegistry::default();

        // Same role ARN, different aliases - should use first alphabetically
        registry.add(AwsProvider {
            config_key: "aws.zebra".to_string(),
            alias: Some("zebra".to_string()),
            role_arn: Some("arn:aws:iam::123456789012:role/SharedRole".to_string()),
            region: None,
        });

        registry.add(AwsProvider {
            config_key: "aws.alpha".to_string(),
            alias: Some("alpha".to_string()),
            role_arn: Some("arn:aws:iam::123456789012:role/SharedRole".to_string()),
            region: None,
        });

        let groups = registry.group_by_output_name();

        assert_eq!(groups.len(), 1);
        // "alpha" comes before "zebra" alphabetically
        assert!(groups.contains_key("AlphaDeployer"));
        let keys = groups.get("AlphaDeployer").unwrap();
        assert_eq!(keys.len(), 2);
    }

    #[test]
    fn provider_registry_none_alias_comes_first() {
        let mut registry = ProviderRegistry::default();

        // Same role ARN: one with alias, one without
        registry.add(AwsProvider {
            config_key: "aws.west".to_string(),
            alias: Some("west".to_string()),
            role_arn: Some("arn:aws:iam::123456789012:role/SharedRole".to_string()),
            region: None,
        });

        registry.add(AwsProvider {
            config_key: "aws".to_string(),
            alias: None,
            role_arn: Some("arn:aws:iam::123456789012:role/SharedRole".to_string()),
            region: None,
        });

        let groups = registry.group_by_output_name();

        assert_eq!(groups.len(), 1);
        // None alias sorts before "west", so output name is DefaultDeployer
        assert!(groups.contains_key("DefaultDeployer"));
    }

    #[test]
    fn provider_registry_groups_none_role_arn_together() {
        let mut registry = ProviderRegistry::default();

        // Multiple providers without role_arn should be grouped together
        registry.add(AwsProvider {
            config_key: "aws.z_provider".to_string(),
            alias: Some("z_provider".to_string()),
            role_arn: None,
            region: None,
        });

        registry.add(AwsProvider {
            config_key: "aws.a_provider".to_string(),
            alias: Some("a_provider".to_string()),
            role_arn: None,
            region: None,
        });

        registry.add(AwsProvider {
            config_key: "aws".to_string(),
            alias: None,
            role_arn: None,
            region: None,
        });

        let groups = registry.group_by_output_name();

        // All have role_arn=None, so grouped together
        assert_eq!(groups.len(), 1);
        // None alias sorts first, so output name is DefaultDeployer
        let default_keys = groups.get("DefaultDeployer").unwrap();
        assert_eq!(default_keys.len(), 3);
        // Config keys should be sorted alphabetically
        assert_eq!(default_keys[0], "aws");
        assert_eq!(default_keys[1], "aws.a_provider");
        assert_eq!(default_keys[2], "aws.z_provider");
    }

    #[test]
    fn provider_registry_snake_case_alias_in_group() {
        let mut registry = ProviderRegistry::default();

        // Provider with snake_case alias
        registry.add(AwsProvider {
            config_key: "aws.workload_network_test_nonprod".to_string(),
            alias: Some("workload_network_test_nonprod".to_string()),
            role_arn: Some("arn:aws:iam::123456789012:role/SomeRole".to_string()),
            region: None,
        });

        let groups = registry.group_by_output_name();

        assert_eq!(groups.len(), 1);
        assert!(groups.contains_key("WorkloadNetworkTestNonprodDeployer"));
    }
}
