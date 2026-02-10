use clap::Parser;
use lppc::{
    cli::Cli,
    config::Config,
    logging::init_logging,
    mapping::{MappingLoader, MappingRepository, PermissionMatcher},
    output::OutputWriter,
    terraform::PlanExecutor,
};

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    init_logging(cli.verbose, cli.no_color);

    let config = Config::from_cli(cli)?;

    log::debug!("Configuration: {:?}", config);

    // Ensure mapping repository is available
    let mapping_repo =
        MappingRepository::ensure_available(&config.mappings_url, config.refresh_mappings)?;

    log::debug!("Mapping repository path: {:?}", mapping_repo.local_path);
    if mapping_repo.was_refreshed {
        log::debug!("Mapping repository was refreshed in this run");
    }

    // Execute terraform init and parse HCL files directly
    // No AWS credentials or backend configuration required!
    let executor = PlanExecutor::new()?;
    let terraform_config = match executor.execute(&config.working_dir)? {
        Some(config) => config,
        None => {
            log::info!("No Terraform files found, nothing to analyze");
            return Ok(());
        }
    };

    log::debug!(
        "Parsed {} provider groups from HCL files",
        terraform_config.provider_groups.len()
    );

    for (name, group) in &terraform_config.provider_groups {
        log::debug!("  {}: {} blocks", name, group.blocks.len());
    }

    if !terraform_config.unmapped_blocks.is_empty() {
        log::warn!(
            "{} blocks could not be mapped to a provider",
            terraform_config.unmapped_blocks.len()
        );
        for block in &terraform_config.unmapped_blocks {
            log::warn!("  Unmapped: {}", block.address);
        }
    }

    if terraform_config.provider_groups.is_empty() {
        log::info!("No AWS resources found to analyze");
        return Ok(());
    }

    // Resolve permissions
    let loader = MappingLoader::new(mapping_repo.local_path);
    let matcher = PermissionMatcher::new(&loader);
    let result = matcher.resolve(&terraform_config)?;

    // Generate output
    let writer = OutputWriter::new(config.output_format, config.output_dir, config.no_color);

    // Write missing mappings warning to stderr
    writer.write_missing_mappings(&result);

    // Write formatted permissions
    writer.write(&result)?;

    Ok(())
}
