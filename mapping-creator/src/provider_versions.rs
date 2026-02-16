use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use log::{debug, warn};
use saphyr::{LoadableYamlNode, Yaml};
use serde::Deserialize;
use std::fmt::Write;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

const GITHUB_API_BASE: &str = "https://api.github.com/repos/hashicorp/terraform-provider-";
const CACHE_FILE_NAME: &str = "provider-versions.yml";
const CACHE_MAX_AGE_HOURS: i64 = 24;
const HTTP_TIMEOUT_SECONDS: u64 = 10;

const PROVIDERS: [&str; 3] = ["aws", "time", "random"];

#[derive(Debug, Clone, PartialEq)]
pub struct ProviderVersions {
    pub aws: String,
    pub time: String,
    pub random: String,
}

impl ProviderVersions {
    fn get(&self, name: &str) -> &str {
        match name {
            "aws" => &self.aws,
            "time" => &self.time,
            "random" => &self.random,
            _ => panic!("Unknown provider: {name}"),
        }
    }

    fn set(&mut self, name: &str, version: String) {
        match name {
            "aws" => self.aws = version,
            "time" => self.time = version,
            "random" => self.random = version,
            _ => panic!("Unknown provider: {name}"),
        }
    }
}

#[derive(Debug)]
struct ProviderVersionCache {
    last_updated: DateTime<Utc>,
    providers: ProviderVersions,
}

/// Resolves the latest provider versions, using a cache with 24-hour expiry.
/// Fetches from the GitHub API when the cache is stale or missing.
pub fn resolve_provider_versions() -> Result<ProviderVersions> {
    let cache_path = get_cache_path()?;
    resolve_with_cache_and_fetcher(&cache_path, fetch_latest_version)
}

fn resolve_with_cache_and_fetcher(
    cache_path: &Path,
    fetcher: impl Fn(&str) -> Result<String>,
) -> Result<ProviderVersions> {
    let existing_cache = load_cache(cache_path);

    if let Some(ref cache) = existing_cache {
        if is_cache_fresh(cache) {
            debug!("Using cached provider versions");
            return Ok(cache.providers.clone());
        }
        debug!("Provider version cache is stale, refreshing");
    } else {
        debug!("No provider version cache found, fetching from GitHub");
    }

    let mut all_succeeded = true;
    let mut versions = ProviderVersions {
        aws: String::new(),
        time: String::new(),
        random: String::new(),
    };

    for &provider in &PROVIDERS {
        match fetcher(provider) {
            Ok(version) => {
                versions.set(provider, version);
            }
            Err(err) => {
                all_succeeded = false;
                if let Some(ref cache) = existing_cache {
                    let cached_version = cache.providers.get(provider).to_string();
                    warn!(
                        "Failed to fetch version for provider '{}': {}. Using cached version: {}",
                        provider, err, cached_version
                    );
                    versions.set(provider, cached_version);
                } else {
                    return Err(err.context(format!(
                        "Failed to fetch version for provider '{}' and no cached version available",
                        provider
                    )));
                }
            }
        }
    }

    let last_updated = if all_succeeded {
        Utc::now()
    } else if let Some(ref cache) = existing_cache {
        cache.last_updated
    } else {
        // If no cache existed and we reached here, all providers that failed would have
        // returned an error above. So all remaining providers succeeded.
        Utc::now()
    };

    let new_cache = ProviderVersionCache {
        last_updated,
        providers: versions.clone(),
    };

    if let Err(err) = save_cache(cache_path, &new_cache) {
        warn!("Failed to save provider version cache: {}", err);
    }

    Ok(versions)
}

fn get_cache_path() -> Result<PathBuf> {
    let home = dirs::home_dir().context("Could not determine home directory")?;
    Ok(home.join(".lppc").join(CACHE_FILE_NAME))
}

fn load_cache(path: &Path) -> Option<ProviderVersionCache> {
    let content = fs::read_to_string(path).ok()?;
    let docs = Yaml::load_from_str(&content)
        .map_err(|err| warn!("Failed to parse provider version cache: {}", err))
        .ok()?;

    let doc = docs.first()?;
    let mapping = doc.as_mapping()?;

    let last_updated_str = find_str_in_mapping(mapping, "last_updated")?;
    let last_updated = last_updated_str
        .parse::<DateTime<Utc>>()
        .map_err(|err| warn!("Failed to parse last_updated timestamp: {}", err))
        .ok()?;

    let providers_node = find_mapping_in_mapping(mapping, "providers")?;

    let providers = ProviderVersions {
        aws: find_str_in_mapping(providers_node, "aws")?.to_string(),
        time: find_str_in_mapping(providers_node, "time")?.to_string(),
        random: find_str_in_mapping(providers_node, "random")?.to_string(),
    };

    if !has_valid_cached_versions(&providers) {
        warn!("Cached provider versions contain invalid values, ignoring cache");
        return None;
    }

    Some(ProviderVersionCache {
        last_updated,
        providers,
    })
}

fn find_str_in_mapping<'a>(mapping: &'a saphyr::Mapping, key: &str) -> Option<&'a str> {
    mapping
        .iter()
        .find(|(k, _)| k.as_str() == Some(key))
        .and_then(|(_, v)| v.as_str())
}

fn find_mapping_in_mapping<'a>(
    mapping: &'a saphyr::Mapping,
    key: &str,
) -> Option<&'a saphyr::Mapping<'a>> {
    mapping
        .iter()
        .find(|(k, _)| k.as_str() == Some(key))
        .and_then(|(_, v)| v.as_mapping())
}

fn has_valid_cached_versions(versions: &ProviderVersions) -> bool {
    PROVIDERS
        .iter()
        .all(|&provider| is_valid_version_string(versions.get(provider)))
}

fn is_cache_fresh(cache: &ProviderVersionCache) -> bool {
    let age = Utc::now() - cache.last_updated;
    age.num_hours() < CACHE_MAX_AGE_HOURS
}

fn save_cache(path: &Path, cache: &ProviderVersionCache) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create cache directory: {}", parent.display()))?;
    }
    let yaml = format_cache_as_yaml(cache);
    fs::write(path, yaml)
        .with_context(|| format!("Failed to write cache file: {}", path.display()))
}

fn format_cache_as_yaml(cache: &ProviderVersionCache) -> String {
    let mut yaml = String::new();
    writeln!(
        yaml,
        "last_updated: \"{}\"",
        cache.last_updated.to_rfc3339()
    )
    .unwrap();
    writeln!(yaml, "providers:").unwrap();
    writeln!(yaml, "  aws: \"{}\"", cache.providers.aws).unwrap();
    writeln!(yaml, "  time: \"{}\"", cache.providers.time).unwrap();
    write!(yaml, "  random: \"{}\"", cache.providers.random).unwrap();
    yaml
}

#[derive(Deserialize)]
struct GitHubRelease {
    tag_name: String,
}

fn fetch_latest_version(provider: &str) -> Result<String> {
    let url = format!("{}{}/releases/latest", GITHUB_API_BASE, provider);
    debug!(
        "Fetching latest version for provider '{}' from {}",
        provider, url
    );

    let agent = ureq::Agent::config_builder()
        .timeout_per_call(Some(Duration::from_secs(HTTP_TIMEOUT_SECONDS)))
        .user_agent(format!(
            "lppc-mapping-creator/{}",
            env!("CARGO_PKG_VERSION")
        ))
        .build()
        .new_agent();

    let mut response = agent
        .get(&url)
        .header("Accept", "application/vnd.github.v3+json")
        .call()
        .with_context(|| {
            format!(
                "Failed to fetch latest release for provider '{}'",
                provider
            )
        })?;

    let body = response.body_mut().read_to_string().with_context(|| {
        format!(
            "Failed to read response body for provider '{}'",
            provider
        )
    })?;

    let release: GitHubRelease = serde_json::from_str(&body).with_context(|| {
        format!(
            "Failed to parse GitHub API response for provider '{}'",
            provider
        )
    })?;

    let version = strip_version_prefix(&release.tag_name);

    if !is_valid_version_string(&version) {
        bail!(
            "Invalid version string for provider '{}': '{}'",
            provider,
            version
        );
    }

    debug!("Resolved provider '{}' to version '{}'", provider, version);
    Ok(version)
}

fn strip_version_prefix(tag: &str) -> String {
    tag.strip_prefix('v').unwrap_or(tag).to_string()
}

fn is_valid_version_string(version: &str) -> bool {
    !version.is_empty()
        && version.chars().all(|c| c.is_ascii_digit() || c == '.')
        && !version.starts_with('.')
        && !version.ends_with('.')
        && !version.contains("..")
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeDelta;
    use tempfile::TempDir;

    fn create_test_versions() -> ProviderVersions {
        ProviderVersions {
            aws: "6.31.1".to_string(),
            time: "0.13.1".to_string(),
            random: "3.7.2".to_string(),
        }
    }

    fn create_fresh_cache() -> ProviderVersionCache {
        ProviderVersionCache {
            last_updated: Utc::now(),
            providers: create_test_versions(),
        }
    }

    fn create_stale_cache() -> ProviderVersionCache {
        ProviderVersionCache {
            last_updated: Utc::now() - TimeDelta::hours(25),
            providers: create_test_versions(),
        }
    }

    fn write_cache_to_file(dir: &Path, cache: &ProviderVersionCache) {
        let cache_path = dir.join(CACHE_FILE_NAME);
        let yaml = format_cache_as_yaml(cache);
        fs::write(&cache_path, yaml).unwrap();
    }

    fn success_fetcher(provider: &str) -> Result<String> {
        Ok(match provider {
            "aws" => "7.0.0".to_string(),
            "time" => "1.0.0".to_string(),
            "random" => "4.0.0".to_string(),
            _ => bail!("Unknown provider: {}", provider),
        })
    }

    fn failing_fetcher(_provider: &str) -> Result<String> {
        bail!("Network error")
    }

    fn partial_fetcher(provider: &str) -> Result<String> {
        match provider {
            "aws" => Ok("7.0.0".to_string()),
            "time" => bail!("Network error for time"),
            "random" => Ok("4.0.0".to_string()),
            _ => bail!("Unknown provider"),
        }
    }

    // --- load_cache tests ---

    #[test]
    fn load_cache_returns_valid_cache() {
        let temp_dir = TempDir::new().unwrap();
        let cache = create_fresh_cache();
        write_cache_to_file(temp_dir.path(), &cache);

        let loaded = load_cache(&temp_dir.path().join(CACHE_FILE_NAME));
        assert!(loaded.is_some());
        let loaded = loaded.unwrap();
        assert_eq!(loaded.providers, cache.providers);
    }

    #[test]
    fn load_cache_returns_none_for_missing_file() {
        let temp_dir = TempDir::new().unwrap();
        let loaded = load_cache(&temp_dir.path().join(CACHE_FILE_NAME));
        assert!(loaded.is_none());
    }

    #[test]
    fn load_cache_returns_none_for_malformed_yaml() {
        let temp_dir = TempDir::new().unwrap();
        let cache_path = temp_dir.path().join(CACHE_FILE_NAME);
        fs::write(&cache_path, "this is not valid yaml: [[[").unwrap();

        let loaded = load_cache(&cache_path);
        assert!(loaded.is_none());
    }

    #[test]
    fn load_cache_returns_none_for_tampered_version_strings() {
        let temp_dir = TempDir::new().unwrap();
        let cache_path = temp_dir.path().join(CACHE_FILE_NAME);
        // Simulate a tampered cache with injected content in version field
        let tampered_yaml = r#"last_updated: "2026-02-16T10:00:00Z"
providers:
  aws: "1.0.0\"\n}\n}\nprovider \"evil\" {"
  time: "0.13.1"
  random: "3.7.2"
"#;
        fs::write(&cache_path, tampered_yaml).unwrap();

        let loaded = load_cache(&cache_path);
        assert!(loaded.is_none());
    }

    #[test]
    fn load_cache_returns_none_for_incomplete_yaml() {
        let temp_dir = TempDir::new().unwrap();
        let cache_path = temp_dir.path().join(CACHE_FILE_NAME);
        fs::write(&cache_path, "last_updated: 2026-01-01T00:00:00Z\n").unwrap();

        let loaded = load_cache(&cache_path);
        assert!(loaded.is_none());
    }

    // --- is_cache_fresh tests ---

    #[test]
    fn is_cache_fresh_returns_true_when_recently_updated() {
        let cache = create_fresh_cache();
        assert!(is_cache_fresh(&cache));
    }

    #[test]
    fn is_cache_fresh_returns_false_when_stale() {
        let cache = create_stale_cache();
        assert!(!is_cache_fresh(&cache));
    }

    #[test]
    fn is_cache_fresh_returns_false_at_exactly_24_hours() {
        let cache = ProviderVersionCache {
            last_updated: Utc::now() - TimeDelta::hours(24),
            providers: create_test_versions(),
        };
        assert!(!is_cache_fresh(&cache));
    }

    #[test]
    fn is_cache_fresh_returns_true_just_under_24_hours() {
        let cache = ProviderVersionCache {
            last_updated: Utc::now() - TimeDelta::hours(23),
            providers: create_test_versions(),
        };
        assert!(is_cache_fresh(&cache));
    }

    // --- save_cache roundtrip ---

    #[test]
    fn save_and_load_cache_roundtrip() {
        let temp_dir = TempDir::new().unwrap();
        let cache_path = temp_dir.path().join(CACHE_FILE_NAME);
        let cache = create_fresh_cache();

        save_cache(&cache_path, &cache).unwrap();

        let loaded = load_cache(&cache_path).expect("Should be able to load saved cache");
        assert_eq!(loaded.providers, cache.providers);
    }

    #[test]
    fn save_cache_creates_parent_directories() {
        let temp_dir = TempDir::new().unwrap();
        let cache_path = temp_dir
            .path()
            .join("nested")
            .join("dir")
            .join(CACHE_FILE_NAME);
        let cache = create_fresh_cache();

        save_cache(&cache_path, &cache).unwrap();
        assert!(cache_path.exists());
    }

    // --- strip_version_prefix tests ---

    #[test]
    fn strip_version_prefix_removes_v_prefix() {
        assert_eq!(strip_version_prefix("v6.31.1"), "6.31.1");
    }

    #[test]
    fn strip_version_prefix_leaves_version_without_prefix() {
        assert_eq!(strip_version_prefix("6.31.1"), "6.31.1");
    }

    #[test]
    fn strip_version_prefix_handles_single_v() {
        assert_eq!(strip_version_prefix("v"), "");
    }

    // --- is_valid_version_string tests ---

    #[test]
    fn is_valid_version_string_accepts_semver() {
        assert!(is_valid_version_string("6.31.1"));
        assert!(is_valid_version_string("0.13.1"));
        assert!(is_valid_version_string("3.7.2"));
        assert!(is_valid_version_string("1.0.0"));
    }

    #[test]
    fn is_valid_version_string_rejects_empty() {
        assert!(!is_valid_version_string(""));
    }

    #[test]
    fn is_valid_version_string_rejects_leading_dot() {
        assert!(!is_valid_version_string(".1.0"));
    }

    #[test]
    fn is_valid_version_string_rejects_trailing_dot() {
        assert!(!is_valid_version_string("1.0."));
    }

    #[test]
    fn is_valid_version_string_rejects_consecutive_dots() {
        assert!(!is_valid_version_string("1..0"));
        assert!(!is_valid_version_string("1..0..0"));
    }

    #[test]
    fn is_valid_version_string_rejects_non_digit_characters() {
        assert!(!is_valid_version_string("1.0.0-beta"));
        assert!(!is_valid_version_string("v1.0.0"));
        assert!(!is_valid_version_string("abc"));
    }

    // --- resolve_with_cache_and_fetcher tests ---

    #[test]
    fn resolve_returns_cached_versions_when_fresh() {
        let temp_dir = TempDir::new().unwrap();
        let cache_path = temp_dir.path().join(CACHE_FILE_NAME);
        let cache = create_fresh_cache();
        write_cache_to_file(temp_dir.path(), &cache);

        let result = resolve_with_cache_and_fetcher(&cache_path, failing_fetcher);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), cache.providers);
    }

    #[test]
    fn resolve_fetches_new_versions_when_cache_stale() {
        let temp_dir = TempDir::new().unwrap();
        let cache_path = temp_dir.path().join(CACHE_FILE_NAME);
        let cache = create_stale_cache();
        write_cache_to_file(temp_dir.path(), &cache);

        let result = resolve_with_cache_and_fetcher(&cache_path, success_fetcher);
        assert!(result.is_ok());

        let versions = result.unwrap();
        assert_eq!(versions.aws, "7.0.0");
        assert_eq!(versions.time, "1.0.0");
        assert_eq!(versions.random, "4.0.0");
    }

    #[test]
    fn resolve_fetches_when_no_cache_exists() {
        let temp_dir = TempDir::new().unwrap();
        let cache_path = temp_dir.path().join(CACHE_FILE_NAME);

        let result = resolve_with_cache_and_fetcher(&cache_path, success_fetcher);
        assert!(result.is_ok());

        let versions = result.unwrap();
        assert_eq!(versions.aws, "7.0.0");
        assert_eq!(versions.time, "1.0.0");
        assert_eq!(versions.random, "4.0.0");
    }

    #[test]
    fn resolve_falls_back_to_stale_cache_on_api_failure() {
        let temp_dir = TempDir::new().unwrap();
        let cache_path = temp_dir.path().join(CACHE_FILE_NAME);
        let cache = create_stale_cache();
        write_cache_to_file(temp_dir.path(), &cache);

        let result = resolve_with_cache_and_fetcher(&cache_path, failing_fetcher);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), cache.providers);
    }

    #[test]
    fn resolve_fails_when_api_unreachable_and_no_cache() {
        let temp_dir = TempDir::new().unwrap();
        let cache_path = temp_dir.path().join(CACHE_FILE_NAME);

        let result = resolve_with_cache_and_fetcher(&cache_path, failing_fetcher);
        assert!(result.is_err());
    }

    #[test]
    fn resolve_merges_partial_api_results_with_cache() {
        let temp_dir = TempDir::new().unwrap();
        let cache_path = temp_dir.path().join(CACHE_FILE_NAME);
        let cache = create_stale_cache();
        write_cache_to_file(temp_dir.path(), &cache);

        let result = resolve_with_cache_and_fetcher(&cache_path, partial_fetcher);
        assert!(result.is_ok());

        let versions = result.unwrap();
        assert_eq!(versions.aws, "7.0.0"); // Fresh from API
        assert_eq!(versions.time, "0.13.1"); // Fallback to cache
        assert_eq!(versions.random, "4.0.0"); // Fresh from API
    }

    #[test]
    fn resolve_updates_last_updated_when_all_succeed() {
        let temp_dir = TempDir::new().unwrap();
        let cache_path = temp_dir.path().join(CACHE_FILE_NAME);
        let old_cache = create_stale_cache();
        write_cache_to_file(temp_dir.path(), &old_cache);

        resolve_with_cache_and_fetcher(&cache_path, success_fetcher).unwrap();

        let new_cache = load_cache(&cache_path).unwrap();
        assert!(new_cache.last_updated > old_cache.last_updated);
    }

    #[test]
    fn resolve_keeps_old_last_updated_on_partial_failure() {
        let temp_dir = TempDir::new().unwrap();
        let cache_path = temp_dir.path().join(CACHE_FILE_NAME);
        let old_cache = create_stale_cache();
        let old_last_updated = old_cache.last_updated;
        write_cache_to_file(temp_dir.path(), &old_cache);

        resolve_with_cache_and_fetcher(&cache_path, partial_fetcher).unwrap();

        let new_cache = load_cache(&cache_path).unwrap();
        assert_eq!(
            new_cache.last_updated.timestamp(),
            old_last_updated.timestamp()
        );
    }

    #[test]
    fn resolve_writes_cache_file_after_successful_fetch() {
        let temp_dir = TempDir::new().unwrap();
        let cache_path = temp_dir.path().join(CACHE_FILE_NAME);

        resolve_with_cache_and_fetcher(&cache_path, success_fetcher).unwrap();

        assert!(cache_path.exists());
        let cache = load_cache(&cache_path).unwrap();
        assert_eq!(cache.providers.aws, "7.0.0");
    }

    // --- ProviderVersions get/set tests ---

    #[test]
    fn provider_versions_get_returns_correct_values() {
        let versions = create_test_versions();
        assert_eq!(versions.get("aws"), "6.31.1");
        assert_eq!(versions.get("time"), "0.13.1");
        assert_eq!(versions.get("random"), "3.7.2");
    }

    #[test]
    fn provider_versions_set_updates_correct_field() {
        let mut versions = create_test_versions();
        versions.set("aws", "7.0.0".to_string());
        assert_eq!(versions.aws, "7.0.0");
        assert_eq!(versions.time, "0.13.1");
        assert_eq!(versions.random, "3.7.2");
    }

    #[test]
    #[should_panic(expected = "Unknown provider")]
    fn provider_versions_get_panics_for_unknown_provider() {
        let versions = create_test_versions();
        versions.get("unknown");
    }
}
