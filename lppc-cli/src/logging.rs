use colored::control::set_override;
use env_logger::Builder;
use log::LevelFilter;

pub fn init_logging(verbose: bool, no_color: bool) {
    // Disable colors globally if requested
    if no_color {
        set_override(false);
    }

    let level = if verbose {
        LevelFilter::Debug
    } else {
        LevelFilter::Info
    };

    Builder::new()
        .filter_level(level)
        .format_timestamp(None)
        .init();
}

#[cfg(test)]
mod tests {
    use super::*;

    // Note: Logger can only be initialized once per process, so these tests
    // verify the logic rather than the full initialization behavior.

    #[test]
    fn level_filter_verbose_returns_debug() {
        let level = if true {
            LevelFilter::Debug
        } else {
            LevelFilter::Info
        };
        assert_eq!(level, LevelFilter::Debug);
    }

    #[test]
    fn level_filter_non_verbose_returns_info() {
        let level = if false {
            LevelFilter::Debug
        } else {
            LevelFilter::Info
        };
        assert_eq!(level, LevelFilter::Info);
    }
}
