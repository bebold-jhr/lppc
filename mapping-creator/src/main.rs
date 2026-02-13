mod action;
mod block_type;
mod cli;
mod generator;
mod schema;
mod service;
mod ui;

use anyhow::{bail, Context, Result};
use clap::Parser;
use log::{debug, info};
use std::path::PathBuf;

use action::{compute_selected_actions, get_preselected_indices, load_service_actions};
use cli::Args;
use generator::{generate_files, print_success_message, GeneratorConfig};
use schema::{filter_unmapped_types, load_terraform_types};
use service::{extract_service_hint, find_best_match, load_service_references};
use ui::{select_actions, select_block_type, select_service_prefix, select_terraform_type};

fn main() {
    if let Err(err) = run() {
        eprintln!("Error: {err:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args = Args::parse();

    init_logging(args.verbose);

    let working_dir = validate_working_directory(&args.working_dir)?;

    debug!("Working directory validated: {}", working_dir.display());

    let block_type = select_block_type()?;
    info!("Selected block type: {}", block_type);

    let all_types = load_terraform_types(&working_dir, block_type)?;
    let unmapped_types = filter_unmapped_types(&working_dir, block_type, all_types);

    if unmapped_types.is_empty() {
        println!("All types have mappings");
        return Ok(());
    }

    debug!("Found {} unmapped types", unmapped_types.len());

    let terraform_type = select_terraform_type(unmapped_types)?;
    info!("Selected Terraform type: {}", terraform_type);

    let services = load_service_references(&working_dir)?;

    let service_hint = extract_service_hint(&terraform_type);
    debug!("Service hint from Terraform type: {:?}", service_hint);

    let preselected_index = service_hint
        .as_ref()
        .and_then(|hint| find_best_match(hint, &services))
        .and_then(|matched| services.iter().position(|s| s.service == matched.service));

    if preselected_index.is_some() {
        debug!("Pre-selecting service at index: {:?}", preselected_index);
    } else {
        debug!("No matching service found for hint");
    }

    let selected_service = select_service_prefix(services, preselected_index)?;
    info!("Selected service prefix: {}", selected_service.service);

    let service_actions = load_service_actions(&working_dir, &selected_service.service)?;
    debug!(
        "Loaded {} actions for service",
        service_actions.actions.len()
    );

    let preselected_action_indices = get_preselected_indices(&service_actions.actions);
    debug!(
        "Pre-selecting {} actions",
        preselected_action_indices.len()
    );

    let selected = select_actions(
        &service_actions.actions,
        &selected_service.service,
        &preselected_action_indices,
    )?;
    info!(
        "Selected {} allow actions and {} deny actions",
        selected.allow_indices.len(),
        selected.deny_indices.len()
    );

    let computed = compute_selected_actions(
        &selected_service.service,
        &service_actions.actions,
        &selected.allow_indices,
        &selected.deny_indices,
    );

    debug!("Allow actions: {:?}", computed.allow);
    debug!("Deny actions: {:?}", computed.deny);

    let config = GeneratorConfig {
        working_dir: &working_dir,
        block_type,
        terraform_type: &terraform_type,
        service_reference_url: &selected_service.url,
        allow_actions: computed.allow,
        deny_actions: computed.deny,
    };

    let generated_files = generate_files(&config)?;
    print_success_message(&generated_files);

    Ok(())
}

fn init_logging(verbose: bool) {
    let log_level = if verbose { "debug" } else { "warn" };

    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(log_level))
        .format_timestamp(None)
        .init();

    debug!("Verbose logging enabled");
}

fn validate_working_directory(path: &PathBuf) -> Result<PathBuf> {
    let resolved_path = if path.is_absolute() {
        path.clone()
    } else {
        std::env::current_dir()
            .context("Failed to get current working directory")?
            .join(path)
    };

    let canonical_path = resolved_path
        .canonicalize()
        .with_context(|| format!("Path does not exist: {}", resolved_path.display()))?;

    if !canonical_path.is_dir() {
        bail!("Path is not a directory: {}", canonical_path.display());
    }

    Ok(canonical_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    #[test]
    fn validate_working_directory_accepts_existing_directory() {
        let temp_dir = TempDir::new().unwrap();
        let result = validate_working_directory(&temp_dir.path().to_path_buf());
        assert!(result.is_ok());
    }

    #[test]
    fn validate_working_directory_rejects_nonexistent_path() {
        let nonexistent_path = PathBuf::from("/this/path/definitely/does/not/exist");
        let result = validate_working_directory(&nonexistent_path);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("does not exist"));
    }

    #[test]
    fn validate_working_directory_rejects_file_path() {
        let temp_dir = TempDir::new().unwrap();
        let file_path = temp_dir.path().join("test_file.txt");
        fs::write(&file_path, "test content").unwrap();

        let result = validate_working_directory(&file_path);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("not a directory"));
    }

    #[test]
    fn validate_working_directory_resolves_relative_paths() {
        // Using "." as a relative path should resolve to the current directory
        let relative_path = PathBuf::from(".");
        let result = validate_working_directory(&relative_path);
        assert!(result.is_ok());

        let resolved = result.unwrap();
        assert!(resolved.is_absolute());
    }

    #[test]
    fn validate_working_directory_handles_absolute_paths() {
        let temp_dir = TempDir::new().unwrap();
        let absolute_path = temp_dir.path().canonicalize().unwrap();

        let result = validate_working_directory(&absolute_path);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), absolute_path);
    }
}
