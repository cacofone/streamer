//! The table of known devices.
//!
//! Turns the stream of SSDP messages into a stream of [`DiscoveryEvent`]s:
//! a device is *found* once, *updated* when it moves, and *lost* when it says
//! goodbye or its announcement expires.
//!
//! The cache does no I/O and never reads the clock: every call that needs the
//! time takes it as a parameter, which keeps it easy to test.

use std::collections::{HashMap, HashSet};
use std::net::IpAddr;
use std::time::{Duration, Instant};

use tracing::warn;

use super::message::{Announcement, Departure, Message};
use super::types::{Device, DeviceKind, DiscoveryEvent};

/// Most distinct targets remembered per device (bounds memory against hostile packets).
const MAX_TARGETS_PER_DEVICE: usize = 64;

/// The NT value of a root device announcement.
const ROOT_DEVICE: &str = "upnp:rootdevice";

#[derive(Debug)]
struct Entry {
    location: String,
    address: String,
    server: Option<String>,
    /// Every `ST`/`NT` value seen for this device.
    targets: HashSet<String>,
    kind: DeviceKind,
    expires_at: Instant,
    /// Whether a `Found` event has been emitted for this device.
    announced: bool,
}

impl Entry {
    fn device(&self, uuid: &str, now: Instant) -> Device {
        Device {
            uuid: uuid.to_string(),
            location: self.location.clone(),
            kind: self.kind,
            server: self.server.clone(),
            address: self.address.clone(),
            expires_in_secs: self.expires_at.saturating_duration_since(now).as_secs(),
        }
    }
}

/// Known devices, keyed by uuid.
#[derive(Debug)]
pub struct DeviceCache {
    devices: HashMap<String, Entry>,
    max_devices: usize,
    report_other: bool,
    warned_full: bool,
}

impl DeviceCache {
    /// Creates an empty cache.
    ///
    /// `max_devices` bounds the number of devices kept (new devices are
    /// ignored when it is reached). With `report_other` false, only media
    /// servers and renderers are reported; with true, every device is.
    pub fn new(max_devices: usize, report_other: bool) -> DeviceCache {
        DeviceCache {
            devices: HashMap::new(),
            max_devices,
            report_other,
            warned_full: false,
        }
    }

    /// Number of devices currently tracked (including ones not reported yet).
    pub fn len(&self) -> usize {
        self.devices.len()
    }

    /// Whether no devices are tracked.
    pub fn is_empty(&self) -> bool {
        self.devices.is_empty()
    }

    /// Feeds a parsed message into the cache and returns the resulting events
    /// (at most one).
    pub fn handle(&mut self, message: &Message, from: IpAddr, now: Instant) -> Vec<DiscoveryEvent> {
        match message {
            Message::SearchResponse(announcement) | Message::Alive(announcement) => {
                self.announce(announcement, from, now)
            }
            Message::ByeBye(departure) => self.depart(departure, now),
        }
    }

    /// Removes devices whose announcement has expired and returns a `Lost`
    /// event for each reported one, ordered by uuid.
    pub fn expire(&mut self, now: Instant) -> Vec<DiscoveryEvent> {
        let mut gone: Vec<String> = self
            .devices
            .iter()
            .filter(|(_, entry)| entry.expires_at <= now)
            .map(|(uuid, _)| uuid.clone())
            .collect();
        gone.sort();
        let mut events = Vec::new();
        for uuid in gone {
            if let Some(entry) = self.devices.remove(&uuid)
                && entry.announced
            {
                events.push(DiscoveryEvent::Lost { uuid });
            }
        }
        events
    }

    /// When the next device expires, for scheduling the next [`expire`](Self::expire) call.
    pub fn next_expiry(&self) -> Option<Instant> {
        self.devices.values().map(|entry| entry.expires_at).min()
    }

    /// The reported devices, ordered by uuid.
    pub fn devices(&self, now: Instant) -> Vec<Device> {
        let mut devices: Vec<Device> = self
            .devices
            .iter()
            .filter(|(_, entry)| entry.announced)
            .map(|(uuid, entry)| entry.device(uuid, now))
            .collect();
        devices.sort_by(|a, b| a.uuid.cmp(&b.uuid));
        devices
    }

    fn is_reportable(&self, kind: DeviceKind) -> bool {
        self.report_other || kind != DeviceKind::Other
    }

    fn announce(
        &mut self,
        announcement: &Announcement,
        from: IpAddr,
        now: Instant,
    ) -> Vec<DiscoveryEvent> {
        let uuid = &announcement.usn.uuid;
        let target_kind = DeviceKind::from_target(&announcement.target);
        let expires_at = now + Duration::from_secs(announcement.max_age_secs);
        let address = from.to_string();

        if !self.devices.contains_key(uuid) {
            if self.devices.len() >= self.max_devices {
                if !self.warned_full {
                    self.warned_full = true;
                    warn!(
                        max_devices = self.max_devices,
                        "device table is full, ignoring new devices"
                    );
                }
                return Vec::new();
            }
            self.devices.insert(
                uuid.clone(),
                Entry {
                    location: announcement.location.clone(),
                    address,
                    server: announcement.server.clone(),
                    targets: HashSet::from([announcement.target.clone()]),
                    kind: target_kind,
                    expires_at,
                    announced: false,
                },
            );
            let reportable = self.is_reportable(target_kind);
            let Some(entry) = self.devices.get_mut(uuid) else {
                return Vec::new();
            };
            if reportable {
                entry.announced = true;
                return vec![DiscoveryEvent::Found(entry.device(uuid, now))];
            }
            return Vec::new();
        }

        let reportable_before = self
            .devices
            .get(uuid)
            .is_some_and(|entry| self.is_reportable(entry.kind));
        let reportable_target = self.is_reportable(target_kind);
        let Some(entry) = self.devices.get_mut(uuid) else {
            return Vec::new();
        };

        let moved = entry.location != announcement.location || entry.address != address;
        entry.location = announcement.location.clone();
        entry.address = address;
        if announcement.server.is_some() {
            entry.server = announcement.server.clone();
        }
        if entry.targets.len() < MAX_TARGETS_PER_DEVICE {
            entry.targets.insert(announcement.target.clone());
        }
        // keep the latest expiry, a shorter announcement must not shorten it
        entry.expires_at = entry.expires_at.max(expires_at);
        // the first target that tells us what the device is decides its kind
        if entry.kind == DeviceKind::Other && target_kind != DeviceKind::Other {
            entry.kind = target_kind;
        }

        if !entry.announced {
            if reportable_before || reportable_target || self.report_other {
                entry.announced = true;
                return vec![DiscoveryEvent::Found(entry.device(uuid, now))];
            }
            return Vec::new();
        }
        if moved {
            return vec![DiscoveryEvent::Updated(entry.device(uuid, now))];
        }
        Vec::new()
    }

    fn depart(&mut self, departure: &Departure, _now: Instant) -> Vec<DiscoveryEvent> {
        let uuid = &departure.usn.uuid;
        let Some(entry) = self.devices.get_mut(uuid) else {
            return Vec::new();
        };

        // The whole device leaves on a root device byebye or a bare "uuid:..." one.
        let whole_device = departure.target.eq_ignore_ascii_case(ROOT_DEVICE)
            || departure.usn.suffix.is_none()
            || departure
                .target
                .get(..5)
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case("uuid:"));
        if !whole_device {
            entry.targets.remove(&departure.target);
            if !entry.targets.is_empty() {
                return Vec::new();
            }
        }

        let announced = entry.announced;
        self.devices.remove(uuid);
        if announced {
            vec![DiscoveryEvent::Lost { uuid: uuid.clone() }]
        } else {
            Vec::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ssdp::message::Usn;

    const RENDERER: &str = "urn:schemas-upnp-org:device:MediaRenderer:1";
    const SERVER: &str = "urn:schemas-upnp-org:device:MediaServer:1";
    const AVT: &str = "urn:schemas-upnp-org:service:AVTransport:1";

    fn ip(last: u8) -> IpAddr {
        IpAddr::from([192, 0, 2, last])
    }

    fn announcement(uuid: &str, target: &str, location: &str, max_age: u64) -> Announcement {
        Announcement {
            usn: Usn {
                uuid: uuid.to_string(),
                suffix: Some(target.to_string()),
            },
            target: target.to_string(),
            location: location.to_string(),
            max_age_secs: max_age,
            server: Some("Test/1.0".to_string()),
        }
    }

    fn alive(uuid: &str, target: &str, max_age: u64) -> Message {
        Message::Alive(announcement(
            uuid,
            target,
            &format!("http://192.0.2.1/{uuid}.xml"),
            max_age,
        ))
    }

    fn byebye(uuid: &str, target: &str, suffix: Option<&str>) -> Message {
        Message::ByeBye(Departure {
            usn: Usn {
                uuid: uuid.to_string(),
                suffix: suffix.map(str::to_string),
            },
            target: target.to_string(),
        })
    }

    fn secs(n: u64) -> Duration {
        Duration::from_secs(n)
    }

    #[test]
    fn a_new_media_renderer_is_found_once() {
        let t0 = Instant::now();
        let mut cache = DeviceCache::new(10, false);

        let events = cache.handle(&alive("a", RENDERER, 1800), ip(10), t0);
        assert_eq!(events.len(), 1);
        let DiscoveryEvent::Found(device) = &events[0] else {
            panic!("expected Found, got {events:?}");
        };
        assert_eq!(device.uuid, "a");
        assert_eq!(device.kind, DeviceKind::MediaRenderer);
        assert_eq!(device.address, "192.0.2.10");
        assert_eq!(device.location, "http://192.0.2.1/a.xml");
        assert_eq!(device.server.as_deref(), Some("Test/1.0"));
        assert_eq!(device.expires_in_secs, 1800);

        // the same announcement again is only a refresh
        assert!(
            cache
                .handle(&alive("a", RENDERER, 1800), ip(10), t0 + secs(5))
                .is_empty()
        );
        assert_eq!(cache.len(), 1);
    }

    #[test]
    fn search_responses_and_notifies_are_treated_alike() {
        let t0 = Instant::now();
        let mut cache = DeviceCache::new(10, false);
        let response =
            Message::SearchResponse(announcement("a", SERVER, "http://192.0.2.1/a.xml", 60));
        let events = cache.handle(&response, ip(1), t0);
        assert!(
            matches!(&events[..], [DiscoveryEvent::Found(d)] if d.kind == DeviceKind::MediaServer)
        );
    }

    #[test]
    fn refresh_extends_the_expiry() {
        let t0 = Instant::now();
        let mut cache = DeviceCache::new(10, false);
        cache.handle(&alive("a", RENDERER, 100), ip(1), t0);
        cache.handle(&alive("a", RENDERER, 100), ip(1), t0 + secs(60));
        // 160 s after the start, but only 100 s after the last refresh
        assert!(cache.expire(t0 + secs(159)).is_empty());
        assert_eq!(cache.devices(t0 + secs(100))[0].expires_in_secs, 60);
    }

    #[test]
    fn a_shorter_announcement_does_not_shorten_the_expiry() {
        let t0 = Instant::now();
        let mut cache = DeviceCache::new(10, false);
        cache.handle(&alive("a", RENDERER, 1800), ip(1), t0);
        // the same device announces a service with a short lifetime
        cache.handle(&alive("a", AVT, 30), ip(1), t0 + secs(1));
        assert!(cache.expire(t0 + secs(100)).is_empty());
        assert_eq!(cache.next_expiry(), Some(t0 + secs(1800)));
    }

    #[test]
    fn devices_expire_after_their_max_age() {
        let t0 = Instant::now();
        let mut cache = DeviceCache::new(10, false);
        cache.handle(&alive("b", RENDERER, 100), ip(2), t0);
        cache.handle(&alive("a", SERVER, 100), ip(1), t0);
        cache.handle(&alive("c", SERVER, 500), ip(3), t0);

        assert!(cache.expire(t0 + secs(99)).is_empty());
        // exactly at the expiry time counts as expired; events are ordered by uuid
        let events = cache.expire(t0 + secs(100));
        assert_eq!(
            events,
            vec![
                DiscoveryEvent::Lost { uuid: "a".into() },
                DiscoveryEvent::Lost { uuid: "b".into() },
            ]
        );
        assert_eq!(cache.len(), 1);
        // a second sweep reports nothing new
        assert!(cache.expire(t0 + secs(100)).is_empty());
    }

    #[test]
    fn a_device_that_moves_is_updated() {
        let t0 = Instant::now();
        let mut cache = DeviceCache::new(10, false);
        cache.handle(&alive("a", RENDERER, 1800), ip(1), t0);

        let moved = Message::Alive(announcement(
            "a",
            RENDERER,
            "http://192.0.2.99/new.xml",
            1800,
        ));
        let events = cache.handle(&moved, ip(1), t0 + secs(1));
        assert!(
            matches!(&events[..], [DiscoveryEvent::Updated(d)] if d.location == "http://192.0.2.99/new.xml")
        );

        // a new source address is also an update
        let events = cache.handle(&moved, ip(77), t0 + secs(2));
        assert!(matches!(&events[..], [DiscoveryEvent::Updated(d)] if d.address == "192.0.2.77"));

        // and then it is quiet again
        assert!(cache.handle(&moved, ip(77), t0 + secs(3)).is_empty());
    }

    #[test]
    fn other_devices_are_hidden_unless_requested() {
        let t0 = Instant::now();
        let mut hidden = DeviceCache::new(10, false);
        let root = alive("a", "upnp:rootdevice", 1800);
        assert!(hidden.handle(&root, ip(1), t0).is_empty());
        assert!(hidden.devices(t0).is_empty());
        // it is tracked, though
        assert_eq!(hidden.len(), 1);

        let mut shown = DeviceCache::new(10, true);
        let events = shown.handle(&root, ip(1), t0);
        assert!(matches!(&events[..], [DiscoveryEvent::Found(d)] if d.kind == DeviceKind::Other));
    }

    #[test]
    fn a_hidden_device_is_found_when_a_relevant_target_shows_up() {
        let t0 = Instant::now();
        let mut cache = DeviceCache::new(10, false);
        assert!(
            cache
                .handle(&alive("a", "upnp:rootdevice", 1800), ip(1), t0)
                .is_empty()
        );

        let events = cache.handle(&alive("a", RENDERER, 1800), ip(1), t0 + secs(1));
        assert!(
            matches!(&events[..], [DiscoveryEvent::Found(d)] if d.kind == DeviceKind::MediaRenderer)
        );
        assert_eq!(cache.devices(t0 + secs(1)).len(), 1);
    }

    #[test]
    fn hidden_devices_never_produce_lost_events() {
        let t0 = Instant::now();
        let mut cache = DeviceCache::new(10, false);
        cache.handle(&alive("a", "upnp:rootdevice", 100), ip(1), t0);
        assert!(cache.expire(t0 + secs(200)).is_empty());
        assert!(cache.is_empty());

        cache.handle(&alive("b", "upnp:rootdevice", 100), ip(1), t0);
        assert!(
            cache
                .handle(
                    &byebye("b", "upnp:rootdevice", Some("upnp:rootdevice")),
                    ip(1),
                    t0
                )
                .is_empty()
        );
        assert!(cache.is_empty());
    }

    #[test]
    fn byebye_for_the_root_device_removes_the_device() {
        let t0 = Instant::now();
        let mut cache = DeviceCache::new(10, false);
        cache.handle(&alive("a", RENDERER, 1800), ip(1), t0);
        cache.handle(&alive("a", AVT, 1800), ip(1), t0);

        let events = cache.handle(
            &byebye("a", "upnp:rootdevice", Some("upnp:rootdevice")),
            ip(1),
            t0,
        );
        assert_eq!(events, vec![DiscoveryEvent::Lost { uuid: "a".into() }]);
        assert!(cache.is_empty());
    }

    #[test]
    fn byebye_for_a_bare_uuid_removes_the_device() {
        let t0 = Instant::now();
        let mut cache = DeviceCache::new(10, false);
        cache.handle(&alive("a", RENDERER, 1800), ip(1), t0);
        let events = cache.handle(&byebye("a", "uuid:a", None), ip(1), t0);
        assert_eq!(events, vec![DiscoveryEvent::Lost { uuid: "a".into() }]);
    }

    #[test]
    fn byebye_for_one_service_only_removes_that_target() {
        let t0 = Instant::now();
        let mut cache = DeviceCache::new(10, false);
        cache.handle(&alive("a", RENDERER, 1800), ip(1), t0);
        cache.handle(&alive("a", AVT, 1800), ip(1), t0);

        // one of two targets leaves: the device stays
        assert!(
            cache
                .handle(&byebye("a", AVT, Some(AVT)), ip(1), t0)
                .is_empty()
        );
        assert_eq!(cache.devices(t0).len(), 1);

        // the last known target leaves: the device is gone
        let events = cache.handle(&byebye("a", RENDERER, Some(RENDERER)), ip(1), t0);
        assert_eq!(events, vec![DiscoveryEvent::Lost { uuid: "a".into() }]);
        assert!(cache.is_empty());
    }

    #[test]
    fn byebye_for_an_unknown_device_is_ignored() {
        let mut cache = DeviceCache::new(10, false);
        let events = cache.handle(
            &byebye("nobody", "upnp:rootdevice", Some("upnp:rootdevice")),
            ip(1),
            Instant::now(),
        );
        assert!(events.is_empty());
    }

    #[test]
    fn a_device_can_come_back_after_it_left() {
        let t0 = Instant::now();
        let mut cache = DeviceCache::new(10, false);
        cache.handle(&alive("a", RENDERER, 1800), ip(1), t0);
        cache.handle(
            &byebye("a", "upnp:rootdevice", Some("upnp:rootdevice")),
            ip(1),
            t0,
        );
        let events = cache.handle(&alive("a", RENDERER, 1800), ip(1), t0 + secs(10));
        assert!(matches!(&events[..], [DiscoveryEvent::Found(_)]));
    }

    #[test]
    fn the_table_is_bounded() {
        let t0 = Instant::now();
        let mut cache = DeviceCache::new(2, false);
        assert_eq!(cache.handle(&alive("a", RENDERER, 100), ip(1), t0).len(), 1);
        assert_eq!(
            cache.handle(&alive("b", RENDERER, 1000), ip(2), t0).len(),
            1
        );
        // full: a third device is ignored
        assert!(
            cache
                .handle(&alive("c", RENDERER, 1000), ip(3), t0)
                .is_empty()
        );
        assert_eq!(cache.len(), 2);
        // known devices are still handled
        let moved = Message::Alive(announcement("b", RENDERER, "http://192.0.2.50/x.xml", 1000));
        assert!(matches!(
            &cache.handle(&moved, ip(2), t0)[..],
            [DiscoveryEvent::Updated(_)]
        ));
        // once a slot frees up, new devices are accepted again
        cache.expire(t0 + secs(100));
        assert_eq!(
            cache
                .handle(&alive("c", RENDERER, 1000), ip(3), t0 + secs(101))
                .len(),
            1
        );
    }

    #[test]
    fn the_number_of_targets_per_device_is_bounded() {
        let t0 = Instant::now();
        let mut cache = DeviceCache::new(10, false);
        cache.handle(&alive("a", RENDERER, 1800), ip(1), t0);
        for n in 0..1000 {
            cache.handle(
                &alive("a", &format!("urn:x:service:S{n}:1"), 1800),
                ip(1),
                t0,
            );
        }
        assert!(
            cache
                .devices
                .get("a")
                .is_some_and(|e| e.targets.len() <= MAX_TARGETS_PER_DEVICE)
        );
    }

    #[test]
    fn next_expiry_and_snapshot() {
        let t0 = Instant::now();
        let mut cache = DeviceCache::new(10, false);
        assert_eq!(cache.next_expiry(), None);
        cache.handle(&alive("b", SERVER, 300), ip(2), t0);
        cache.handle(&alive("a", RENDERER, 200), ip(1), t0);
        assert_eq!(cache.next_expiry(), Some(t0 + secs(200)));

        let snapshot = cache.devices(t0 + secs(50));
        let uuids: Vec<&str> = snapshot.iter().map(|d| d.uuid.as_str()).collect();
        assert_eq!(uuids, ["a", "b"]);
        assert_eq!(snapshot[0].expires_in_secs, 150);
        assert_eq!(snapshot[1].expires_in_secs, 250);
    }

    #[test]
    fn remaining_lifetime_never_goes_negative() {
        let t0 = Instant::now();
        let mut cache = DeviceCache::new(10, false);
        cache.handle(&alive("a", RENDERER, 100), ip(1), t0);
        assert_eq!(cache.devices(t0 + secs(5000))[0].expires_in_secs, 0);
    }
}
