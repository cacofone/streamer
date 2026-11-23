//! Building and parsing of SSDP messages.
//!
//! Pure functions, no I/O. The parser is total: it returns a value for every
//! possible input and never panics.

use std::collections::HashMap;
use std::net::Ipv4Addr;

use thiserror::Error;
use url::Url;

use super::error::SsdpError;
use super::types::SearchTarget;

/// The IPv4 multicast group used by SSDP.
pub const MULTICAST_ADDR_V4: Ipv4Addr = Ipv4Addr::new(239, 255, 255, 250);
/// The UDP port used by SSDP.
pub const SSDP_PORT: u16 = 1900;
/// Largest datagram we accept; bigger ones are dropped.
pub const MAX_DATAGRAM: usize = 2048;
/// Lifetime used when a device does not send a usable `max-age`.
pub const DEFAULT_MAX_AGE_SECS: u64 = 1800;
/// Shortest lifetime we accept (devices send nonsense).
pub const MIN_MAX_AGE_SECS: u64 = 5;
/// Longest lifetime we accept.
pub const MAX_MAX_AGE_SECS: u64 = 86_400;

/// Longest accepted device uuid.
const MAX_UUID_LEN: usize = 100;
/// Most headers read from one packet; the rest are ignored.
const MAX_HEADERS: usize = 64;
/// Most characters kept from the `SERVER` header.
const MAX_SERVER_LEN: usize = 256;

/// A device identifier from a `USN` header: `uuid:<id>[::<type>]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Usn {
    /// The device uuid, without the `uuid:` prefix.
    pub uuid: String,
    /// What follows `::` (for example `upnp:rootdevice` or a device or service type).
    pub suffix: Option<String>,
}

impl Usn {
    /// Parses a `USN` header value.
    pub fn parse(value: &str) -> Result<Usn, ParseError> {
        let value = value.trim();
        let rest = match value.get(..5) {
            Some(prefix) if prefix.eq_ignore_ascii_case("uuid:") => &value[5..],
            _ => return Err(ParseError::InvalidUsn),
        };
        let (uuid, suffix) = match rest.split_once("::") {
            Some((uuid, suffix)) => (uuid, Some(suffix.trim())),
            None => (rest, None),
        };
        let uuid = uuid.trim();
        if uuid.is_empty() || uuid.len() > MAX_UUID_LEN || uuid.chars().any(char::is_control) {
            return Err(ParseError::InvalidUsn);
        }
        let suffix = suffix
            .filter(|s| !s.is_empty())
            .map(|s| s.chars().filter(|c| !c.is_control()).collect::<String>());
        Ok(Usn {
            uuid: uuid.to_string(),
            suffix,
        })
    }
}

/// A device announcing itself, either as a search response or as `NOTIFY` alive/update.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Announcement {
    /// Who is announcing.
    pub usn: Usn,
    /// The `ST` (response) or `NT` (notify) value.
    pub target: String,
    /// Validated `http` or `https` URL of the description document.
    pub location: String,
    /// How long the announcement is valid, in seconds (clamped).
    pub max_age_secs: u64,
    /// The `SERVER` header, if present.
    pub server: Option<String>,
}

/// A device saying goodbye (`NOTIFY` with `ssdp:byebye`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Departure {
    /// Who is leaving.
    pub usn: Usn,
    /// The `NT` value.
    pub target: String,
}

/// An SSDP message we care about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Message {
    /// Answer to one of our searches.
    SearchResponse(Announcement),
    /// `NOTIFY` with `ssdp:alive` or `ssdp:update`.
    Alive(Announcement),
    /// `NOTIFY` with `ssdp:byebye`.
    ByeBye(Departure),
}

/// Why a packet was dropped. These are expected on a busy network; log them at
/// debug level, do not show them to users.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ParseError {
    /// The datagram is larger than [`MAX_DATAGRAM`].
    #[error("datagram too large ({0} bytes)")]
    TooLarge(usize),
    /// The datagram is empty.
    #[error("empty datagram")]
    Empty,
    /// The first line is neither a response nor a known request.
    #[error("unrecognized start line")]
    UnknownStartLine,
    /// A required header is missing or empty.
    #[error("missing header: {0}")]
    MissingHeader(&'static str),
    /// The `USN` header is not valid.
    #[error("invalid USN")]
    InvalidUsn,
    /// The `LOCATION` header is not an http(s) URL.
    #[error("invalid LOCATION")]
    InvalidLocation,
    /// The `NTS` header has an unknown value.
    #[error("invalid NTS")]
    InvalidNts,
}

/// Builds the user agent string sent with searches.
///
/// Contains only the operating system name and the crate version, nothing
/// about the machine or the user.
pub fn default_user_agent() -> String {
    format!(
        "{}/unknown UPnP/1.1 cacofone-streamer/{}",
        std::env::consts::OS,
        env!("CARGO_PKG_VERSION")
    )
}

/// Builds an `M-SEARCH` request.
///
/// `mx_secs` is clamped to 1..=5. Fails if `target` or `user_agent` contain
/// control characters (which would allow header injection).
pub fn build_search(
    target: &SearchTarget,
    mx_secs: u8,
    user_agent: &str,
) -> Result<String, SsdpError> {
    let st = target.st();
    if st.trim().is_empty() || st.chars().any(char::is_control) {
        return Err(SsdpError::InvalidConfig(
            "search target must not be empty or contain control characters".to_string(),
        ));
    }
    if user_agent.chars().any(char::is_control) {
        return Err(SsdpError::InvalidConfig(
            "user agent must not contain control characters".to_string(),
        ));
    }
    let mx = mx_secs.clamp(1, 5);
    Ok(format!(
        "M-SEARCH * HTTP/1.1\r\n\
         HOST: {MULTICAST_ADDR_V4}:{SSDP_PORT}\r\n\
         MAN: \"ssdp:discover\"\r\n\
         MX: {mx}\r\n\
         ST: {st}\r\n\
         USER-AGENT: {user_agent}\r\n\
         \r\n"
    ))
}

/// Parses a received datagram.
///
/// - `Ok(Some(message))`: a message we care about.
/// - `Ok(None)`: valid but not interesting (other control points searching,
///   HTTP responses that are not `200 OK`).
/// - `Err(_)`: malformed; drop the packet.
pub fn parse(data: &[u8]) -> Result<Option<Message>, ParseError> {
    if data.len() > MAX_DATAGRAM {
        return Err(ParseError::TooLarge(data.len()));
    }
    let text = String::from_utf8_lossy(data);
    let mut lines = text.split('\n').map(|line| line.trim_end_matches('\r'));

    let start = lines.next().map(str::trim).unwrap_or("");
    if start.is_empty() {
        return Err(ParseError::Empty);
    }
    let kind = match classify(start)? {
        Some(kind) => kind,
        None => return Ok(None),
    };

    let headers = read_headers(lines);
    match kind {
        StartLine::Response => {
            let announcement = announcement(&headers, "st")?;
            Ok(Some(Message::SearchResponse(announcement)))
        }
        StartLine::Notify => {
            let nts = require(&headers, "nts")?.to_ascii_lowercase();
            match nts.as_str() {
                "ssdp:alive" | "ssdp:update" => {
                    Ok(Some(Message::Alive(announcement(&headers, "nt")?)))
                }
                "ssdp:byebye" => {
                    let target = require(&headers, "nt")?.to_string();
                    let usn = Usn::parse(require(&headers, "usn")?)?;
                    Ok(Some(Message::ByeBye(Departure { usn, target })))
                }
                _ => Err(ParseError::InvalidNts),
            }
        }
    }
}

enum StartLine {
    Response,
    Notify,
}

/// Decides what kind of message the first line announces.
/// `Ok(None)` means "valid, but not for us".
fn classify(start: &str) -> Result<Option<StartLine>, ParseError> {
    let mut parts = start.split_whitespace();
    let first = parts.next().unwrap_or("");
    if first
        .as_bytes()
        .get(..5)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(b"HTTP/"))
    {
        return Ok(match parts.next() {
            Some("200") => Some(StartLine::Response),
            _ => None,
        });
    }
    if first.eq_ignore_ascii_case("NOTIFY") {
        return Ok(Some(StartLine::Notify));
    }
    if first.eq_ignore_ascii_case("M-SEARCH") {
        return Ok(None);
    }
    Err(ParseError::UnknownStartLine)
}

/// Reads headers up to the first empty line. Names are lowercased, the first
/// occurrence of a header wins, lines without a colon are ignored.
fn read_headers<'a>(lines: impl Iterator<Item = &'a str>) -> HashMap<String, String> {
    let mut headers = HashMap::new();
    for line in lines {
        if line.trim().is_empty() {
            break;
        }
        if headers.len() >= MAX_HEADERS {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers
                .entry(name.trim().to_ascii_lowercase())
                .or_insert_with(|| value.trim().to_string());
        }
    }
    headers
}

fn require<'a>(
    headers: &'a HashMap<String, String>,
    name: &'static str,
) -> Result<&'a str, ParseError> {
    match headers.get(name) {
        Some(value) if !value.is_empty() => Ok(value),
        _ => Err(ParseError::MissingHeader(name)),
    }
}

/// Builds an [`Announcement`]; `target_header` is `st` for responses, `nt` for notifies.
fn announcement(
    headers: &HashMap<String, String>,
    target_header: &'static str,
) -> Result<Announcement, ParseError> {
    let target = require(headers, target_header)?.to_string();
    let usn = Usn::parse(require(headers, "usn")?)?;
    let location = parse_location(require(headers, "location")?)?;
    let max_age_secs = parse_max_age(headers.get("cache-control").map(String::as_str));
    let server = headers
        .get("server")
        .filter(|value| !value.is_empty())
        .map(|value| {
            value
                .chars()
                .filter(|c| !c.is_control())
                .take(MAX_SERVER_LEN)
                .collect()
        });
    Ok(Announcement {
        usn,
        target,
        location,
        max_age_secs,
        server,
    })
}

fn parse_location(value: &str) -> Result<String, ParseError> {
    let url = Url::parse(value).map_err(|_| ParseError::InvalidLocation)?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(ParseError::InvalidLocation);
    }
    Ok(url.into())
}

/// Reads `max-age` from a `CACHE-CONTROL` value, tolerating extra directives
/// and whitespace. Missing or unparsable values give the default; the result
/// is clamped.
fn parse_max_age(value: Option<&str>) -> u64 {
    let Some(value) = value else {
        return DEFAULT_MAX_AGE_SECS;
    };
    for directive in value.split(',') {
        if let Some((name, number)) = directive.split_once('=')
            && name.trim().eq_ignore_ascii_case("max-age")
        {
            return match number.trim().trim_matches('"').parse::<u64>() {
                Ok(seconds) => seconds.clamp(MIN_MAX_AGE_SECS, MAX_MAX_AGE_SECS),
                Err(_) => DEFAULT_MAX_AGE_SECS,
            };
        }
    }
    DEFAULT_MAX_AGE_SECS
}
