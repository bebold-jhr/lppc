use anyhow::{Context, Result};
use log::{debug, warn};
use std::fs;
use std::path::Path;

use crate::block_type::BlockType;

fn is_valid_type_name(name: &str) -> bool {
    !name.is_empty()
        && !name.contains('/')
        && !name.contains('\\')
        && !name.contains('\0')
        && !name.starts_with('.')
        && !name.contains("..")
}

pub fn load_terraform_types(working_dir: &Path, block_type: BlockType) -> Result<Vec<String>> {
    let schema_path = working_dir.join(block_type.schema_file());

    debug!("Loading schema from: {}", schema_path.display());

    let content = fs::read_to_string(&schema_path)
        .with_context(|| format!("Failed to read schema file: {}", schema_path.display()))?;

    let types: Vec<String> = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse schema file: {}", schema_path.display()))?;

    debug!("Loaded {} types from schema", types.len());

    Ok(types)
}

pub fn filter_unmapped_types(
    working_dir: &Path,
    block_type: BlockType,
    types: Vec<String>,
) -> Vec<String> {
    let mapping_dir = working_dir.join(block_type.mapping_dir());

    types
        .into_iter()
        .filter(|terraform_type| {
            if !is_valid_type_name(terraform_type) {
                warn!("Skipping invalid type name: {}", terraform_type);
                return false;
            }

            let mapping_file = mapping_dir.join(format!("{}.yml", terraform_type));
            let is_mapped = mapping_file.exists();

            if is_mapped {
                debug!("Filtering out mapped type: {}", terraform_type);
            }

            !is_mapped
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn setup_test_dir() -> TempDir {
        let temp_dir = TempDir::new().unwrap();

        // Create schema directory
        let schema_dir = temp_dir.path().join("sources/terraform");
        fs::create_dir_all(&schema_dir).unwrap();

        // Create mapping directories
        for block_type in BlockType::ALL {
            let mapping_dir = temp_dir.path().join(block_type.mapping_dir());
            fs::create_dir_all(&mapping_dir).unwrap();
        }

        temp_dir
    }

    #[test]
    fn load_terraform_types_parses_valid_json() {
        let temp_dir = setup_test_dir();
        let schema_path = temp_dir
            .path()
            .join("sources/terraform/resource_schemas.json");

        let types = vec!["aws_subnet", "aws_vpc", "aws_iam_role"];
        fs::write(&schema_path, serde_json::to_string(&types).unwrap()).unwrap();

        let result = load_terraform_types(temp_dir.path(), BlockType::Resource);

        assert!(result.is_ok());
        let loaded_types = result.unwrap();
        assert_eq!(loaded_types.len(), 3);
        assert!(loaded_types.contains(&"aws_subnet".to_string()));
        assert!(loaded_types.contains(&"aws_vpc".to_string()));
        assert!(loaded_types.contains(&"aws_iam_role".to_string()));
    }

    #[test]
    fn load_terraform_types_fails_for_missing_file() {
        let temp_dir = setup_test_dir();

        let result = load_terraform_types(temp_dir.path(), BlockType::Resource);

        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Failed to read schema file"));
    }

    #[test]
    fn load_terraform_types_fails_for_invalid_json() {
        let temp_dir = setup_test_dir();
        let schema_path = temp_dir
            .path()
            .join("sources/terraform/resource_schemas.json");

        fs::write(&schema_path, "not valid json").unwrap();

        let result = load_terraform_types(temp_dir.path(), BlockType::Resource);

        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Failed to parse schema file"));
    }

    #[test]
    fn filter_unmapped_types_excludes_types_with_mapping_files() {
        let temp_dir = setup_test_dir();

        // Create a mapping file for aws_subnet
        let mapping_file = temp_dir.path().join("mappings/resource/aws_subnet.yml");
        fs::write(&mapping_file, "# mapping").unwrap();

        let types = vec![
            "aws_subnet".to_string(),
            "aws_vpc".to_string(),
            "aws_iam_role".to_string(),
        ];

        let filtered = filter_unmapped_types(temp_dir.path(), BlockType::Resource, types);

        assert_eq!(filtered.len(), 2);
        assert!(!filtered.contains(&"aws_subnet".to_string()));
        assert!(filtered.contains(&"aws_vpc".to_string()));
        assert!(filtered.contains(&"aws_iam_role".to_string()));
    }

    #[test]
    fn filter_unmapped_types_returns_all_when_no_mappings_exist() {
        let temp_dir = setup_test_dir();

        let types = vec![
            "aws_subnet".to_string(),
            "aws_vpc".to_string(),
            "aws_iam_role".to_string(),
        ];

        let filtered = filter_unmapped_types(temp_dir.path(), BlockType::Resource, types);

        assert_eq!(filtered.len(), 3);
    }

    #[test]
    fn filter_unmapped_types_returns_empty_when_all_mapped() {
        let temp_dir = setup_test_dir();

        // Create mapping files for all types
        let mapping_dir = temp_dir.path().join("mappings/resource");
        fs::write(mapping_dir.join("aws_subnet.yml"), "# mapping").unwrap();
        fs::write(mapping_dir.join("aws_vpc.yml"), "# mapping").unwrap();

        let types = vec!["aws_subnet".to_string(), "aws_vpc".to_string()];

        let filtered = filter_unmapped_types(temp_dir.path(), BlockType::Resource, types);

        assert!(filtered.is_empty());
    }

    #[test]
    fn is_valid_type_name_accepts_valid_names() {
        assert!(is_valid_type_name("aws_subnet"));
        assert!(is_valid_type_name("aws_vpc"));
        assert!(is_valid_type_name("aws_iam_role"));
        assert!(is_valid_type_name("google_compute_instance"));
    }

    #[test]
    fn is_valid_type_name_rejects_path_traversal() {
        assert!(!is_valid_type_name("../../../etc/passwd"));
        assert!(!is_valid_type_name(".."));
        assert!(!is_valid_type_name("foo/bar"));
        assert!(!is_valid_type_name("foo\\bar"));
        assert!(!is_valid_type_name(".hidden"));
        assert!(!is_valid_type_name(""));
    }

    #[test]
    fn filter_unmapped_types_rejects_path_traversal_attempts() {
        let temp_dir = setup_test_dir();

        let types = vec![
            "aws_vpc".to_string(),
            "../../../etc/passwd".to_string(),
            "aws_subnet".to_string(),
            "foo/bar".to_string(),
        ];

        let filtered = filter_unmapped_types(temp_dir.path(), BlockType::Resource, types);

        assert_eq!(filtered.len(), 2);
        assert!(filtered.contains(&"aws_vpc".to_string()));
        assert!(filtered.contains(&"aws_subnet".to_string()));
        assert!(!filtered.contains(&"../../../etc/passwd".to_string()));
        assert!(!filtered.contains(&"foo/bar".to_string()));
    }
}
