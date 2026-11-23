//! Errors of the SSDP module.

use thiserror::Error;

/// Errors returned by SSDP discovery.
///
/// Malformed packets from the network are not reported through this type; they
/// are dropped (see [`crate::ssdp::message::ParseError`]).
#[derive(Debug, Error)]
pub enum SsdpError {
    /// A socket operation failed.
    #[error("network error: {0}")]
    Io(#[from] std::io::Error),

    /// No network interface suitable for discovery was found.
    #[error("no usable network interface found")]
    NoUsableInterface,

    /// The configuration or a value passed in is not valid.
    #[error("invalid configuration: {0}")]
    InvalidConfig(String),

    /// The discovery task has been shut down.
    #[error("discovery is closed")]
    Closed,
}
