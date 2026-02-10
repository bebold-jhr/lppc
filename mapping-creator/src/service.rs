use anyhow::{Context, Result};
use log::debug;
use serde::Deserialize;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Deserialize)]
pub struct ServiceReference {
    pub service: String,
    pub url: String,
}

const SERVICE_INDEX_PATH: &str = "sources/aws/aws-servicereference-index.json";

pub fn load_service_references(working_dir: &Path) -> Result<Vec<ServiceReference>> {
    let index_path = working_dir.join(SERVICE_INDEX_PATH);

    debug!("Loading service index from: {}", index_path.display());

    let content = fs::read_to_string(&index_path)
        .with_context(|| format!("Failed to read service index: {}", index_path.display()))?;

    let services: Vec<ServiceReference> = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse service index: {}", index_path.display()))?;

    debug!("Loaded {} service references", services.len());

    Ok(services)
}

pub fn extract_service_hint(terraform_type: &str) -> Option<String> {
    let without_prefix = terraform_type.strip_prefix("aws_")?;
    let hint = without_prefix.split('_').next()?;

    if hint.is_empty() {
        return None;
    }

    Some(hint.to_string())
}

pub fn find_best_match<'a>(
    hint: &str,
    services: &'a [ServiceReference],
) -> Option<&'a ServiceReference> {
    services.iter().find(|s| s.service == hint)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn setup_test_dir() -> TempDir {
        let temp_dir = TempDir::new().unwrap();
        let aws_dir = temp_dir.path().join("sources/aws");
        fs::create_dir_all(&aws_dir).unwrap();
        temp_dir
    }

    #[test]
    fn load_service_references_parses_valid_json() {
        let temp_dir = setup_test_dir();
        let index_path = temp_dir
            .path()
            .join("sources/aws/aws-servicereference-index.json");

        let services = r#"[
            {"service": "ec2", "url": "https://example.com/ec2.json"},
            {"service": "iam", "url": "https://example.com/iam.json"}
        ]"#;
        fs::write(&index_path, services).unwrap();

        let result = load_service_references(temp_dir.path());

        assert!(result.is_ok());
        let loaded = result.unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].service, "ec2");
        assert_eq!(loaded[1].service, "iam");
    }

    #[test]
    fn load_service_references_fails_for_missing_file() {
        let temp_dir = setup_test_dir();

        let result = load_service_references(temp_dir.path());

        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Failed to read service index"));
    }

    #[test]
    fn load_service_references_fails_for_invalid_json() {
        let temp_dir = setup_test_dir();
        let index_path = temp_dir
            .path()
            .join("sources/aws/aws-servicereference-index.json");

        fs::write(&index_path, "not valid json").unwrap();

        let result = load_service_references(temp_dir.path());

        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Failed to parse service index"));
    }

    #[test]
    fn extract_service_hint_extracts_first_segment() {
        assert_eq!(
            extract_service_hint("aws_iam_role"),
            Some("iam".to_string())
        );
        assert_eq!(
            extract_service_hint("aws_iam_policy"),
            Some("iam".to_string())
        );
        assert_eq!(
            extract_service_hint("aws_ec2_fleet"),
            Some("ec2".to_string())
        );
        assert_eq!(
            extract_service_hint("aws_s3_bucket"),
            Some("s3".to_string())
        );
    }

    #[test]
    fn extract_service_hint_handles_single_segment() {
        assert_eq!(
            extract_service_hint("aws_subnet"),
            Some("subnet".to_string())
        );
    }

    #[test]
    fn extract_service_hint_returns_none_for_non_aws() {
        assert_eq!(extract_service_hint("google_compute_instance"), None);
        assert_eq!(extract_service_hint("azurerm_resource_group"), None);
    }

    #[test]
    fn extract_service_hint_returns_none_for_empty_after_prefix() {
        assert_eq!(extract_service_hint("aws_"), None);
    }

    #[test]
    fn find_best_match_finds_exact_match() {
        let services = vec![
            ServiceReference {
                service: "ec2".to_string(),
                url: "https://example.com/ec2.json".to_string(),
            },
            ServiceReference {
                service: "iam".to_string(),
                url: "https://example.com/iam.json".to_string(),
            },
        ];

        let result = find_best_match("iam", &services);

        assert!(result.is_some());
        assert_eq!(result.unwrap().service, "iam");
    }

    #[test]
    fn find_best_match_returns_none_for_no_match() {
        let services = vec![
            ServiceReference {
                service: "ec2".to_string(),
                url: "https://example.com/ec2.json".to_string(),
            },
            ServiceReference {
                service: "iam".to_string(),
                url: "https://example.com/iam.json".to_string(),
            },
        ];

        let result = find_best_match("subnet", &services);

        assert!(result.is_none());
    }

    #[test]
    fn find_best_match_requires_exact_match() {
        let services = vec![ServiceReference {
            service: "iam".to_string(),
            url: "https://example.com/iam.json".to_string(),
        }];

        // "ia" should not match "iam"
        assert!(find_best_match("ia", &services).is_none());
        // "iamx" should not match "iam"
        assert!(find_best_match("iamx", &services).is_none());
    }
}
