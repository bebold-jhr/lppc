use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockType {
    Action,
    Data,
    Ephemeral,
    Resource,
}

impl BlockType {
    pub const ALL: [BlockType; 4] = [
        BlockType::Action,
        BlockType::Data,
        BlockType::Ephemeral,
        BlockType::Resource,
    ];

    pub fn schema_file(&self) -> &'static str {
        match self {
            BlockType::Action => "sources/terraform/action_schemas.json",
            BlockType::Data => "sources/terraform/data_source_schemas.json",
            BlockType::Ephemeral => "sources/terraform/ephemeral_resource_schemas.json",
            BlockType::Resource => "sources/terraform/resource_schemas.json",
        }
    }

    pub fn mapping_dir(&self) -> &'static str {
        match self {
            BlockType::Action => "mappings/action",
            BlockType::Data => "mappings/data",
            BlockType::Ephemeral => "mappings/ephemeral",
            BlockType::Resource => "mappings/resource",
        }
    }

    pub fn integration_test_dir(&self) -> &'static str {
        match self {
            BlockType::Action => "integration-tests/action",
            BlockType::Data => "integration-tests/data",
            BlockType::Ephemeral => "integration-tests/ephemeral",
            BlockType::Resource => "integration-tests/resource",
        }
    }

    pub fn terraform_docs_path(&self) -> &'static str {
        match self {
            BlockType::Action => "actions",
            BlockType::Data => "data-sources",
            BlockType::Ephemeral => "ephemeral-resources",
            BlockType::Resource => "resources",
        }
    }
}

impl fmt::Display for BlockType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BlockType::Action => write!(f, "action"),
            BlockType::Data => write!(f, "data"),
            BlockType::Ephemeral => write!(f, "ephemeral"),
            BlockType::Resource => write!(f, "resource"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_file_returns_correct_paths() {
        assert_eq!(
            BlockType::Action.schema_file(),
            "sources/terraform/action_schemas.json"
        );
        assert_eq!(
            BlockType::Data.schema_file(),
            "sources/terraform/data_source_schemas.json"
        );
        assert_eq!(
            BlockType::Ephemeral.schema_file(),
            "sources/terraform/ephemeral_resource_schemas.json"
        );
        assert_eq!(
            BlockType::Resource.schema_file(),
            "sources/terraform/resource_schemas.json"
        );
    }

    #[test]
    fn mapping_dir_returns_correct_paths() {
        assert_eq!(BlockType::Action.mapping_dir(), "mappings/action");
        assert_eq!(BlockType::Data.mapping_dir(), "mappings/data");
        assert_eq!(BlockType::Ephemeral.mapping_dir(), "mappings/ephemeral");
        assert_eq!(BlockType::Resource.mapping_dir(), "mappings/resource");
    }

    #[test]
    fn display_returns_lowercase_name() {
        assert_eq!(format!("{}", BlockType::Action), "action");
        assert_eq!(format!("{}", BlockType::Data), "data");
        assert_eq!(format!("{}", BlockType::Ephemeral), "ephemeral");
        assert_eq!(format!("{}", BlockType::Resource), "resource");
    }

    #[test]
    fn all_contains_all_variants() {
        assert_eq!(BlockType::ALL.len(), 4);
        assert!(BlockType::ALL.contains(&BlockType::Action));
        assert!(BlockType::ALL.contains(&BlockType::Data));
        assert!(BlockType::ALL.contains(&BlockType::Ephemeral));
        assert!(BlockType::ALL.contains(&BlockType::Resource));
    }

    #[test]
    fn integration_test_dir_returns_correct_paths() {
        assert_eq!(
            BlockType::Action.integration_test_dir(),
            "integration-tests/action"
        );
        assert_eq!(
            BlockType::Data.integration_test_dir(),
            "integration-tests/data"
        );
        assert_eq!(
            BlockType::Ephemeral.integration_test_dir(),
            "integration-tests/ephemeral"
        );
        assert_eq!(
            BlockType::Resource.integration_test_dir(),
            "integration-tests/resource"
        );
    }

    #[test]
    fn terraform_docs_path_returns_correct_paths() {
        assert_eq!(BlockType::Action.terraform_docs_path(), "actions");
        assert_eq!(BlockType::Data.terraform_docs_path(), "data-sources");
        assert_eq!(
            BlockType::Ephemeral.terraform_docs_path(),
            "ephemeral-resources"
        );
        assert_eq!(BlockType::Resource.terraform_docs_path(), "resources");
    }
}
