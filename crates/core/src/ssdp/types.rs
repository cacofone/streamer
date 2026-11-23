//! Public data types of SSDP discovery.
//!
//! These types are plain, owned and serializable on purpose: they are used
//! across FFI boundaries and become API payloads later.

use serde::{Deserialize, Serialize};

/// URN prefix of the standard UPnP device types.
const DEVICE_URN: &str = "urn:schemas-upnp-org:device:";

/// What kind of DLNA/UPnP device something is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DeviceKind {
    /// A media server (offers music, video, pictures).
    MediaServer,
    /// A media renderer (plays media, for example a speaker or receiver).
    MediaRenderer,
    /// Anything else.
    Other,
}

impl DeviceKind {
    /// Derives the kind from an SSDP `ST` or `NT` value.
    ///
    /// Accepts every version of the standard device types, for example
    /// `urn:schemas-upnp-org:device:MediaRenderer:1` and `...:MediaRenderer:3`.
    pub fn from_target(target: &str) -> DeviceKind {
        let lower = target.trim().to_ascii_lowercase();
        let device_urn = DEVICE_URN.to_ascii_lowercase();
        match lower.strip_prefix(&device_urn) {
            Some(rest) if rest.starts_with("mediaserver:") => DeviceKind::MediaServer,
            Some(rest) if rest.starts_with("mediarenderer:") => DeviceKind::MediaRenderer,
            _ => DeviceKind::Other,
        }
    }
}

/// What to search for with `M-SEARCH`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SearchTarget {
    /// `urn:schemas-upnp-org:device:MediaServer:1`
    MediaServer,
    /// `urn:schemas-upnp-org:device:MediaRenderer:1`
    MediaRenderer,
    /// `ssdp:all`, every device and service (useful for diagnostics).
    All,
    /// Any other search target, sent as is.
    Custom(String),
}

impl SearchTarget {
    /// The value of the `ST` header for this target.
    pub fn st(&self) -> String {
        match self {
            SearchTarget::MediaServer => format!("{DEVICE_URN}MediaServer:1"),
            SearchTarget::MediaRenderer => format!("{DEVICE_URN}MediaRenderer:1"),
            SearchTarget::All => "ssdp:all".to_string(),
            SearchTarget::Custom(value) => value.clone(),
        }
    }
}

/// A device found on the network.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Device {
    /// Unique device identifier (the `uuid:` part of the USN, without the prefix).
    pub uuid: String,
    /// URL of the device description document (always `http` or `https`).
    pub location: String,
    /// What kind of device this is.
    pub kind: DeviceKind,
    /// Value of the `SERVER` header, if the device sent one.
    pub server: Option<String>,
    /// IP address the packet came from, as text.
    pub address: String,
    /// Seconds left until the announcement expires, when the event was created.
    pub expires_in_secs: u64,
}

/// A change in the set of known devices.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiscoveryEvent {
    /// A device was seen for the first time.
    Found(Device),
    /// A known device changed its location or address.
    Updated(Device),
    /// A device said goodbye or its announcement expired.
    Lost {
        /// The uuid of the device that is gone.
        uuid: String,
    },
}

/// Settings for SSDP discovery.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiscoveryConfig {
    /// What to search for. Default: media servers and media renderers.
    pub targets: Vec<SearchTarget>,
    /// `MX` value in seconds (devices answer within this window). Clamped to 1..=5.
    pub mx_secs: u8,
    /// How many times each search is sent per target and interface.
    pub search_repeat: u8,
    /// Seconds between periodic searches; 0 disables periodic searching.
    pub search_interval_secs: u64,
    /// Also listen passively for `NOTIFY` announcements.
    pub listen: bool,
    /// Seconds between checks for changed network interfaces.
    pub interface_rescan_secs: u64,
    /// Limit discovery to these interface names or addresses; `None` means all suitable ones.
    pub interfaces: Option<Vec<String>>,
    /// Maximum number of devices kept at once.
    pub max_devices: usize,
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        DiscoveryConfig {
            targets: vec![SearchTarget::MediaServer, SearchTarget::MediaRenderer],
            mx_secs: 3,
            search_repeat: 3,
            search_interval_secs: 60,
            listen: true,
            interface_rescan_secs: 30,
            interfaces: None,
            max_devices: 512,
        }
    }
}
