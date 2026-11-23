//! Shared core of Cacofone Streamer.
//!
//! Every front end (CLI, TUI and the native apps) talks to this crate.

/// Version of the core, as set in the workspace manifest.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_not_empty() {
        assert!(!version().is_empty());
    }
}
