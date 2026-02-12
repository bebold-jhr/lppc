//! Output formatter trait and factory.
//!
//! This module defines the `OutputFormatter` trait that all formatters implement,
//! and provides a factory function to create the appropriate formatter based on
//! the output format configuration.

use std::collections::HashSet;

use crate::cli::OutputFormat;

/// Trait for formatting permission sets into output strings.
///
/// Implementors of this trait convert a set of IAM permission strings
/// into a formatted string suitable for the target output format.
pub trait OutputFormatter {
    /// Formats a set of permissions into a string.
    ///
    /// The permissions are provided as a `HashSet` to ensure uniqueness.
    /// Implementations should sort the permissions appropriately for
    /// consistent output.
    fn format(&self, permissions: &HashSet<String>) -> String;

    /// Returns the file extension for this format.
    ///
    /// Used when writing output to files to determine the appropriate
    /// file extension.
    fn extension(&self) -> &'static str;
}

/// Creates the appropriate formatter for the given output format.
///
/// # Arguments
///
/// * `format` - The output format to create a formatter for
///
/// # Returns
///
/// A boxed formatter implementing the `OutputFormatter` trait.
pub fn create_formatter(format: OutputFormat) -> Box<dyn OutputFormatter> {
    use super::hcl::HclFormatter;
    use super::json::JsonFormatter;

    match format {
        OutputFormat::Json => Box::new(JsonFormatter { grouped: false }),
        OutputFormat::JsonGrouped => Box::new(JsonFormatter { grouped: true }),
        OutputFormat::Hcl => Box::new(HclFormatter { grouped: false }),
        OutputFormat::HclGrouped => Box::new(HclFormatter { grouped: true }),
    }
}
