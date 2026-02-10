use thiserror::Error;

use crate::mapping::MappingError;
use crate::terraform::TerraformError;

#[derive(Error, Debug)]
pub enum LppcError {
    #[error("Configuration error: {0}")]
    Config(String),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Mapping repository error: {0}")]
    Mapping(#[from] MappingError),

    #[error("{0}")]
    Terraform(#[from] TerraformError),
}

pub type Result<T> = std::result::Result<T, LppcError>;
