//! SSDP (Simple Service Discovery Protocol) discovery of DLNA/UPnP devices.
//!
//! SSDP only tells us that a device exists and where its description document
//! lives. Fetching that description and talking to the device is a separate step.

pub mod cache;
pub mod error;
pub mod message;
pub mod types;

pub use cache::DeviceCache;
pub use error::SsdpError;
pub use types::{Device, DeviceKind, DiscoveryConfig, DiscoveryEvent, SearchTarget};
