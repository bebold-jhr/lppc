//! Output formatter trait and factory.
//!
//! This module defines the `OutputFormatter` trait that all formatters implement,
//! and provides a factory function to create the appropriate formatter based on
//! the output format configuration.

use std::collections::HashSet;

use crate::cli::OutputFormat;

/// A pair of allow and deny permission sets passed to formatters.
///
/// Using a struct instead of two separate `HashSet` parameters prevents
/// accidental parameter swap at call sites and is extensible.
pub struct PermissionSets<'a> {
    /// IAM actions to allow
    pub allow: &'a HashSet<String>,

    /// IAM actions to deny
    pub deny: &'a HashSet<String>,
}

/// Trait for formatting permission sets into output strings.
///
/// Implementors of this trait convert allow and deny permission sets
/// into a formatted string suitable for the target output format.
pub trait OutputFormatter {
    /// Formats permission sets into a string.
    ///
    /// The permissions are provided as `PermissionSets` containing both
    /// allow and deny sets. Implementations should sort the permissions
    /// appropriately for consistent output, and generate Deny statements
    /// before Allow statements.
    fn format(&self, permissions: &PermissionSets) -> String;

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
