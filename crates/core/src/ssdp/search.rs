//! Active search: sending `M-SEARCH` requests.
//!
//! UDP loses packets, so every request is sent several times with random gaps,
//! and the search is repeated periodically. The answers are read by
//! [`receive_loop`](super::receive::receive_loop) on the same socket.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::UdpSocket;
use tracing::debug;

use super::error::SsdpError;
use super::message::{build_search, default_user_agent};
use super::types::SearchTarget;

/// Most times one request may be repeated per round.
const MAX_REPEAT: u8 = 10;

/// The random gaps used while sending.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Timings {
    /// Gap (minimum, maximum) between repeats of the same request.
    pub repeat_gap: (Duration, Duration),
    /// Gap (minimum, maximum) between different targets.
    pub target_gap: (Duration, Duration),
}

impl Default for Timings {
    fn default() -> Self {
        Timings {
            repeat_gap: (Duration::from_millis(50), Duration::from_millis(250)),
            target_gap: (Duration::from_millis(20), Duration::from_millis(100)),
        }
    }
}

/// Builds one `M-SEARCH` request per target.
///
/// Fails if there are no targets, or a target cannot be sent safely.
pub fn build_requests(targets: &[SearchTarget], mx_secs: u8) -> Result<Vec<String>, SsdpError> {
    if targets.is_empty() {
        return Err(SsdpError::InvalidConfig(
            "at least one search target is needed".to_string(),
        ));
    }
    let user_agent = default_user_agent();
    targets
        .iter()
        .map(|target| build_search(target, mx_secs, &user_agent))
        .collect()
}

/// Sends every request `repeat` times to `dest` and returns how many datagrams went out.
///
/// `repeat` is clamped to 1..=10. A failed send is logged and skipped; it does
/// not stop the round, so a vanished interface only costs its own packets.
pub async fn send_round(
    socket: &UdpSocket,
    dest: SocketAddr,
    requests: &[String],
    repeat: u8,
    timings: &Timings,
) -> usize {
    let repeat = repeat.clamp(1, MAX_REPEAT);
    let mut sent = 0;
    for (index, request) in requests.iter().enumerate() {
        for attempt in 0..repeat {
            match socket.send_to(request.as_bytes(), dest).await {
                Ok(_) => sent += 1,
                Err(error) => debug!(%dest, %error, "sending a search failed"),
            }
            if attempt + 1 < repeat {
                tokio::time::sleep(jitter(timings.repeat_gap)).await;
            }
        }
        if index + 1 < requests.len() {
            tokio::time::sleep(jitter(timings.target_gap)).await;
        }
    }
    sent
}

/// Searches now and then again every `interval` (plus up to 10 % random delay).
///
/// With `interval` of `None` it searches once and returns. Otherwise it runs
/// until the task is aborted.
pub async fn search_loop(
    socket: Arc<UdpSocket>,
    dest: SocketAddr,
    requests: Vec<String>,
    repeat: u8,
    interval: Option<Duration>,
    timings: Timings,
) {
    loop {
        let sent = send_round(&socket, dest, &requests, repeat, &timings).await;
        debug!(%dest, sent, "search round finished");
        let Some(interval) = interval else {
            return;
        };
        tokio::time::sleep(interval + jitter((Duration::ZERO, interval / 10))).await;
    }
}

/// A random duration between the two bounds (inclusive).
fn jitter((min, max): (Duration, Duration)) -> Duration {
    if max <= min {
        return min;
    }
    let span = u64::try_from((max - min).as_micros()).unwrap_or(u64::MAX);
    let random = RandomState::new().build_hasher().finish();
    min + Duration::from_micros(random % span.saturating_add(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jitter_stays_within_its_bounds() {
        let (min, max) = (Duration::from_millis(50), Duration::from_millis(250));
        let mut seen = std::collections::HashSet::new();
        for _ in 0..500 {
            let value = jitter((min, max));
            assert!(value >= min && value <= max, "{value:?}");
            seen.insert(value);
        }
        // it is actually random
        assert!(seen.len() > 50);
    }

    #[test]
    fn jitter_handles_degenerate_bounds() {
        let ten = Duration::from_millis(10);
        assert_eq!(jitter((ten, ten)), ten);
        assert_eq!(jitter((ten, Duration::ZERO)), ten);
        assert_eq!(jitter((Duration::ZERO, Duration::ZERO)), Duration::ZERO);
        assert!(jitter((Duration::ZERO, Duration::MAX)) <= Duration::MAX);
    }

    #[test]
    fn requests_are_built_per_target() {
        let requests = build_requests(&[SearchTarget::MediaServer, SearchTarget::All], 3).unwrap();
        assert_eq!(requests.len(), 2);
        assert!(requests[0].contains("ST: urn:schemas-upnp-org:device:MediaServer:1\r\n"));
        assert!(requests[1].contains("ST: ssdp:all\r\n"));
        assert!(requests[0].contains("MX: 3\r\n"));
        assert!(requests[0].contains("USER-AGENT: "));
    }

    #[test]
    fn invalid_targets_are_rejected() {
        assert!(build_requests(&[], 3).is_err());
        assert!(build_requests(&[SearchTarget::Custom("a\r\nb".to_string())], 3).is_err());
    }
}
