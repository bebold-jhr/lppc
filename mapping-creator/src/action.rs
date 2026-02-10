use anyhow::{bail, Context, Result};
use log::debug;
use serde::Deserialize;
use std::collections::HashSet;
use std::fs;
use std::path::Path;

fn is_valid_service_prefix(prefix: &str) -> bool {
    !prefix.is_empty()
        && !prefix.contains('/')
        && !prefix.contains('\\')
        && !prefix.contains('\0')
        && !prefix.starts_with('.')
        && !prefix.contains("..")
}

#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)] // Fields required for JSON deserialization
pub struct ActionProperties {
    #[serde(rename = "IsList")]
    pub is_list: bool,
    #[serde(rename = "IsPermissionManagement")]
    pub is_permission_management: bool,
    #[serde(rename = "IsTaggingOnly")]
    pub is_tagging_only: bool,
    #[serde(rename = "IsWrite")]
    pub is_write: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ActionAnnotations {
    #[serde(rename = "Properties")]
    pub properties: ActionProperties,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Action {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Annotations")]
    pub annotations: Option<ActionAnnotations>,
}

impl Action {
    pub fn is_tagging_only(&self) -> bool {
        self.annotations
            .as_ref()
            .map(|a| a.properties.is_tagging_only)
            .unwrap_or(false)
    }

    pub fn should_preselect(&self) -> bool {
        self.is_tagging_only()
            || self.name.starts_with("List")
            || self.name.starts_with("Describe")
            || self.name.starts_with("Get")
    }
}

#[derive(Debug, Deserialize)]
pub struct ServiceActions {
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Actions")]
    pub actions: Vec<Action>,
}

pub fn load_service_actions(working_dir: &Path, service_prefix: &str) -> Result<ServiceActions> {
    if !is_valid_service_prefix(service_prefix) {
        bail!(
            "Invalid service prefix: contains path traversal characters: {}",
            service_prefix
        );
    }

    let service_file = working_dir.join(format!("sources/aws/{}.json", service_prefix));

    debug!("Loading service actions from: {}", service_file.display());

    let content = fs::read_to_string(&service_file)
        .with_context(|| format!("Failed to read service file: {}", service_file.display()))?;

    let service_actions: ServiceActions = serde_json::from_str(&content)
        .with_context(|| format!("Failed to parse service file: {}", service_file.display()))?;

    debug!(
        "Loaded {} actions for service {}",
        service_actions.actions.len(),
        service_actions.name
    );

    Ok(service_actions)
}

pub fn get_preselected_indices(actions: &[Action]) -> Vec<usize> {
    actions
        .iter()
        .enumerate()
        .filter(|(_, action)| action.should_preselect())
        .map(|(index, _)| index)
        .collect()
}

pub fn compute_selected_actions(
    service_prefix: &str,
    all_actions: &[Action],
    selected_indices: &HashSet<usize>,
) -> Vec<String> {
    let mut result = Vec::new();

    let list_actions: Vec<usize> = all_actions
        .iter()
        .enumerate()
        .filter(|(_, a)| a.name.starts_with("List"))
        .map(|(i, _)| i)
        .collect();

    let describe_actions: Vec<usize> = all_actions
        .iter()
        .enumerate()
        .filter(|(_, a)| a.name.starts_with("Describe"))
        .map(|(i, _)| i)
        .collect();

    let get_actions: Vec<usize> = all_actions
        .iter()
        .enumerate()
        .filter(|(_, a)| a.name.starts_with("Get"))
        .map(|(i, _)| i)
        .collect();

    let all_list_selected = list_actions.len() > 1
        && list_actions.iter().all(|i| selected_indices.contains(i));
    let all_describe_selected = describe_actions.len() > 1
        && describe_actions
            .iter()
            .all(|i| selected_indices.contains(i));
    let all_get_selected =
        get_actions.len() > 1 && get_actions.iter().all(|i| selected_indices.contains(i));

    if all_list_selected {
        result.push(format!("{}:List*", service_prefix));
    }

    if all_describe_selected {
        result.push(format!("{}:Describe*", service_prefix));
    }

    if all_get_selected {
        result.push(format!("{}:Get*", service_prefix));
    }

    for index in selected_indices {
        let action = &all_actions[*index];

        let is_list = action.name.starts_with("List");
        let is_describe = action.name.starts_with("Describe");
        let is_get = action.name.starts_with("Get");

        if (is_list && all_list_selected)
            || (is_describe && all_describe_selected)
            || (is_get && all_get_selected)
        {
            continue;
        }

        result.push(format!("{}:{}", service_prefix, action.name));
    }

    result.sort();
    result
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

    fn create_test_action(name: &str, is_tagging_only: bool) -> Action {
        Action {
            name: name.to_string(),
            annotations: Some(ActionAnnotations {
                properties: ActionProperties {
                    is_list: name.starts_with("List"),
                    is_permission_management: false,
                    is_tagging_only,
                    is_write: false,
                },
            }),
        }
    }

    #[test]
    fn load_service_actions_parses_valid_json() {
        let temp_dir = setup_test_dir();
        let service_file = temp_dir.path().join("sources/aws/ec2.json");

        let content = r#"{
            "Name": "ec2",
            "Actions": [
                {
                    "Name": "CreateSubnet",
                    "Annotations": {
                        "Properties": {
                            "IsList": false,
                            "IsPermissionManagement": false,
                            "IsTaggingOnly": false,
                            "IsWrite": true
                        }
                    }
                },
                {
                    "Name": "DescribeSubnets",
                    "Annotations": {
                        "Properties": {
                            "IsList": true,
                            "IsPermissionManagement": false,
                            "IsTaggingOnly": false,
                            "IsWrite": false
                        }
                    }
                }
            ]
        }"#;
        fs::write(&service_file, content).unwrap();

        let result = load_service_actions(temp_dir.path(), "ec2");

        assert!(result.is_ok());
        let service = result.unwrap();
        assert_eq!(service.name, "ec2");
        assert_eq!(service.actions.len(), 2);
        assert_eq!(service.actions[0].name, "CreateSubnet");
        assert_eq!(service.actions[1].name, "DescribeSubnets");
    }

    #[test]
    fn load_service_actions_fails_for_missing_file() {
        let temp_dir = setup_test_dir();

        let result = load_service_actions(temp_dir.path(), "nonexistent");

        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Failed to read service file"));
    }

    #[test]
    fn load_service_actions_fails_for_invalid_json() {
        let temp_dir = setup_test_dir();
        let service_file = temp_dir.path().join("sources/aws/broken.json");

        fs::write(&service_file, "not valid json").unwrap();

        let result = load_service_actions(temp_dir.path(), "broken");

        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Failed to parse service file"));
    }

    #[test]
    fn action_should_preselect_tagging_actions() {
        let action = create_test_action("CreateTags", true);
        assert!(action.should_preselect());

        let non_tagging = create_test_action("CreateSubnet", false);
        assert!(!non_tagging.should_preselect());
    }

    #[test]
    fn action_should_preselect_list_actions() {
        let action = create_test_action("ListSubnets", false);
        assert!(action.should_preselect());
    }

    #[test]
    fn action_should_preselect_describe_actions() {
        let action = create_test_action("DescribeSubnets", false);
        assert!(action.should_preselect());
    }

    #[test]
    fn action_should_preselect_get_actions() {
        let action = create_test_action("GetSubnetCidrReservations", false);
        assert!(action.should_preselect());
    }

    #[test]
    fn action_should_not_preselect_write_actions() {
        let action = create_test_action("CreateSubnet", false);
        assert!(!action.should_preselect());

        let delete = create_test_action("DeleteSubnet", false);
        assert!(!delete.should_preselect());
    }

    #[test]
    fn get_preselected_indices_returns_correct_indices() {
        let actions = vec![
            create_test_action("CreateSubnet", false),
            create_test_action("ListSubnets", false),
            create_test_action("DescribeSubnets", false),
            create_test_action("CreateTags", true),
            create_test_action("DeleteSubnet", false),
        ];

        let indices = get_preselected_indices(&actions);

        assert_eq!(indices, vec![1, 2, 3]);
    }

    #[test]
    fn compute_selected_actions_uses_wildcard_when_all_selected() {
        let actions = vec![
            create_test_action("ListSubnets", false),
            create_test_action("ListVpcs", false),
            create_test_action("CreateSubnet", false),
        ];

        let selected: HashSet<usize> = [0, 1].into_iter().collect();
        let result = compute_selected_actions("ec2", &actions, &selected);

        assert!(result.contains(&"ec2:List*".to_string()));
        assert!(!result.iter().any(|s| s.contains("ListSubnets")));
        assert!(!result.iter().any(|s| s.contains("ListVpcs")));
    }

    #[test]
    fn compute_selected_actions_lists_individual_when_partial() {
        let actions = vec![
            create_test_action("ListSubnets", false),
            create_test_action("ListVpcs", false),
            create_test_action("CreateSubnet", false),
        ];

        let selected: HashSet<usize> = [0].into_iter().collect();
        let result = compute_selected_actions("ec2", &actions, &selected);

        assert!(!result.contains(&"ec2:List*".to_string()));
        assert!(result.contains(&"ec2:ListSubnets".to_string()));
    }

    #[test]
    fn compute_selected_actions_handles_multiple_wildcards() {
        let actions = vec![
            create_test_action("ListSubnets", false),
            create_test_action("ListVpcs", false),
            create_test_action("DescribeSubnets", false),
            create_test_action("DescribeVpcs", false),
            create_test_action("GetSubnetCidr", false),
        ];

        let selected: HashSet<usize> = [0, 1, 2, 3, 4].into_iter().collect();
        let result = compute_selected_actions("ec2", &actions, &selected);

        assert!(result.contains(&"ec2:List*".to_string()));
        assert!(result.contains(&"ec2:Describe*".to_string()));
        assert!(result.contains(&"ec2:GetSubnetCidr".to_string()));
    }

    #[test]
    fn compute_selected_actions_includes_non_wildcard_actions() {
        let actions = vec![
            create_test_action("CreateSubnet", false),
            create_test_action("DeleteSubnet", false),
            create_test_action("CreateTags", true),
        ];

        let selected: HashSet<usize> = [0, 2].into_iter().collect();
        let result = compute_selected_actions("ec2", &actions, &selected);

        assert!(result.contains(&"ec2:CreateSubnet".to_string()));
        assert!(result.contains(&"ec2:CreateTags".to_string()));
        assert!(!result.contains(&"ec2:DeleteSubnet".to_string()));
    }

    #[test]
    fn compute_selected_actions_returns_empty_for_no_selection() {
        let actions = vec![
            create_test_action("CreateSubnet", false),
            create_test_action("ListSubnets", false),
        ];

        let selected: HashSet<usize> = HashSet::new();
        let result = compute_selected_actions("ec2", &actions, &selected);

        assert!(result.is_empty());
    }

    #[test]
    fn is_valid_service_prefix_accepts_valid_names() {
        assert!(is_valid_service_prefix("ec2"));
        assert!(is_valid_service_prefix("iam"));
        assert!(is_valid_service_prefix("access-analyzer"));
        assert!(is_valid_service_prefix("s3"));
    }

    #[test]
    fn is_valid_service_prefix_rejects_path_traversal() {
        assert!(!is_valid_service_prefix("../../../etc/passwd"));
        assert!(!is_valid_service_prefix(".."));
        assert!(!is_valid_service_prefix("foo/bar"));
        assert!(!is_valid_service_prefix("foo\\bar"));
        assert!(!is_valid_service_prefix(".hidden"));
        assert!(!is_valid_service_prefix(""));
    }

    #[test]
    fn load_service_actions_rejects_path_traversal() {
        let temp_dir = setup_test_dir();

        let result = load_service_actions(temp_dir.path(), "../../../etc/passwd");

        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Invalid service prefix"));
    }
}
