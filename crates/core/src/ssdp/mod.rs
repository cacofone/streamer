//! SSDP (Simple Service Discovery Protocol) discovery of DLNA/UPnP devices.
//!
//! SSDP tells us that a device exists and where its description document
//! lives (the `LOCATION` URL). It finds media servers (music libraries) and
//! media renderers (speakers and receivers) on the local network. Fetching the
//! description and talking to a device is a separate step.
//!
//! # Finding devices
//!
//! For a one-off search, use [`discover_once`]. To follow devices as they
//! appear and disappear, start a [`Discovery`] and read its events:
//!
//! ```no_run
//! use std::time::Duration;
//! use streamer_core::ssdp::{Discovery, DiscoveryConfig, DiscoveryEvent, discover_once};
//!
//! # #[tokio::main(flavor = "current_thread")]
//! # async fn main() -> Result<(), streamer_core::ssdp::SsdpError> {
//! // search for five seconds and list what answered
//! let devices = discover_once(DiscoveryConfig::default(), Duration::from_secs(5)).await?;
//! for device in &devices {
//!     println!("{:?} at {} ({})", device.kind, device.address, device.location);
//! }
//!
//! // or keep watching
//! let mut discovery = Discovery::start(DiscoveryConfig::default()).await?;
//! while let Some(event) = discovery.next().await {
//!     match event {
//!         DiscoveryEvent::Found(device) => println!("found {}", device.uuid),
//!         DiscoveryEvent::Updated(device) => println!("moved {}", device.uuid),
//!         DiscoveryEvent::Lost { uuid } => println!("lost {uuid}"),
//!     }
//! }
//! # Ok(())
//! # }
//! ```
//!
//! Dropping a [`Discovery`] stops all background work.
//!
//! # How it works
//!
//! - **Searching:** an `M-SEARCH` request is multicast on every suitable
//!   network interface (see [`socket::list_interfaces`]), repeated a few times
//!   with random gaps because UDP loses packets, and again every
//!   [`DiscoveryConfig::search_interval_secs`] seconds.
//! - **Listening:** devices also announce themselves (`NOTIFY` alive and
//!   byebye). A shared socket on port 1900 collects those, so devices that
//!   appear or leave between searches are noticed at once.
//! - **Tracking:** the [`DeviceCache`] turns all messages into
//!   [`DiscoveryEvent`]s. A device is identified by its uuid and expires after
//!   the lifetime it announced, unless it announces again.
//!
//! The pieces are public modules ([`message`], [`socket`], [`search`],
//! [`listener`], [`receive`], [`cache`]) for testing and special setups, but
//! most users only need [`Discovery`] and [`discover_once`].
//!
//! See `docs/discovery.md` in the repository for behaviour details,
//! troubleshooting and the manual test checklist.

pub mod cache;
pub mod discovery;
pub mod error;
pub mod listener;
pub mod message;
pub mod receive;
pub mod search;
pub mod socket;
pub mod types;

pub use cache::DeviceCache;
pub use discovery::{Discovery, discover_once};
pub use error::SsdpError;
pub use types::{Device, DeviceKind, DiscoveryConfig, DiscoveryEvent, SearchTarget};
