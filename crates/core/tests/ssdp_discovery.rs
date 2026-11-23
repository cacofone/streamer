//! End-to-end tests of `Discovery` over loopback unicast.
//!
//! A fake device listens on 127.0.0.1 and the searches are sent straight to
//! it instead of to the multicast group, so no multicast is needed.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use streamer_core::ssdp::discovery::{Tuning, discover_once_tuned};
use streamer_core::ssdp::search::Timings;
use streamer_core::ssdp::{
    DeviceKind, Discovery, DiscoveryConfig, DiscoveryEvent, SearchTarget, SsdpError,
};
use tokio::net::UdpSocket;
use tokio::time::timeout;

const WAIT: Duration = Duration::from_secs(5);
const RENDERER: &str = "urn:schemas-upnp-org:device:MediaRenderer:1";

fn config() -> DiscoveryConfig {
    DiscoveryConfig {
        targets: vec![SearchTarget::MediaRenderer],
        mx_secs: 1,
        search_repeat: 1,
        search_interval_secs: 0,
        listen: false,
        interface_rescan_secs: 0,
        interfaces: Some(vec!["127.0.0.1".to_string()]),
        max_devices: 16,
    }
}

fn tuning(device: SocketAddr) -> Tuning {
    let gap = (Duration::from_millis(1), Duration::from_millis(3));
    Tuning {
        target: device,
        timings: Timings {
            repeat_gap: gap,
            target_gap: gap,
        },
    }
}

async fn fake_device() -> Arc<UdpSocket> {
    Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap())
}

fn response(uuid: &str, st: &str, location: &str, max_age: u64) -> Vec<u8> {
    format!(
        "HTTP/1.1 200 OK\r\nCACHE-CONTROL: max-age={max_age}\r\nLOCATION: {location}\r\n\
         SERVER: Test/1.0\r\nST: {st}\r\nUSN: uuid:{uuid}::{st}\r\n\r\n"
    )
    .into_bytes()
}

/// Waits for the next datagram (a search) and returns who sent it.
async fn next_search(device: &UdpSocket) -> SocketAddr {
    let mut buffer = [0u8; 2048];
    let (length, from) = timeout(WAIT, device.recv_from(&mut buffer))
        .await
        .expect("a search arrives in time")
        .unwrap();
    assert!(buffer[..length].starts_with(b"M-SEARCH * HTTP/1.1\r\n"));
    from
}

/// Answers every search with one response per uuid, until aborted.
fn spawn_responder(
    device: Arc<UdpSocket>,
    uuids: Vec<&'static str>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut buffer = [0u8; 2048];
        while let Ok((_, from)) = device.recv_from(&mut buffer).await {
            for uuid in &uuids {
                let packet = response(
                    uuid,
                    RENDERER,
                    &format!("http://192.0.2.7/{uuid}.xml"),
                    1800,
                );
                let _ = device.send_to(&packet, from).await;
            }
        }
    })
}

#[tokio::test]
async fn a_device_is_found_updated_and_lost() {
    let device = fake_device().await;
    let addr = device.local_addr().unwrap();
    let mut discovery = Discovery::start_tuned(config(), tuning(addr))
        .await
        .unwrap();
    let asker = next_search(&device).await;

    device
        .send_to(
            &response("dev1", RENDERER, "http://192.0.2.7/one.xml", 1800),
            asker,
        )
        .await
        .unwrap();
    let event = timeout(WAIT, discovery.next()).await.unwrap().unwrap();
    let DiscoveryEvent::Found(found) = event else {
        panic!("expected Found, got {event:?}");
    };
    assert_eq!(found.uuid, "dev1");
    assert_eq!(found.kind, DeviceKind::MediaRenderer);
    assert_eq!(found.location, "http://192.0.2.7/one.xml");
    assert_eq!(found.address, "127.0.0.1");
    assert_eq!(found.server.as_deref(), Some("Test/1.0"));
    assert!(found.expires_in_secs > 1700);

    // the same answer again is only a refresh: no event
    device
        .send_to(
            &response("dev1", RENDERER, "http://192.0.2.7/one.xml", 1800),
            asker,
        )
        .await
        .unwrap();
    assert!(
        timeout(Duration::from_millis(300), discovery.next())
            .await
            .is_err()
    );

    // a new location is an update
    device
        .send_to(
            &response("dev1", RENDERER, "http://192.0.2.7/two.xml", 1800),
            asker,
        )
        .await
        .unwrap();
    let event = timeout(WAIT, discovery.next()).await.unwrap().unwrap();
    assert!(
        matches!(&event, DiscoveryEvent::Updated(d) if d.location == "http://192.0.2.7/two.xml")
    );

    // goodbye
    let byebye = "NOTIFY * HTTP/1.1\r\nHOST: 239.255.255.250:1900\r\nNT: upnp:rootdevice\r\n\
                  NTS: ssdp:byebye\r\nUSN: uuid:dev1::upnp:rootdevice\r\n\r\n";
    device.send_to(byebye.as_bytes(), asker).await.unwrap();
    let event = timeout(WAIT, discovery.next()).await.unwrap().unwrap();
    assert_eq!(
        event,
        DiscoveryEvent::Lost {
            uuid: "dev1".into()
        }
    );

    discovery.shutdown().await;
}

#[tokio::test]
async fn a_silent_device_is_lost_when_its_announcement_expires() {
    let device = fake_device().await;
    let mut discovery = Discovery::start_tuned(config(), tuning(device.local_addr().unwrap()))
        .await
        .unwrap();
    let asker = next_search(&device).await;
    // the shortest lifetime the parser accepts is 5 seconds
    device
        .send_to(
            &response("dev1", RENDERER, "http://192.0.2.7/d.xml", 5),
            asker,
        )
        .await
        .unwrap();
    let started = Instant::now();
    assert!(matches!(
        timeout(WAIT, discovery.next()).await.unwrap(),
        Some(DiscoveryEvent::Found(_))
    ));

    let event = timeout(Duration::from_secs(10), discovery.next())
        .await
        .expect("the device expires")
        .unwrap();
    assert_eq!(
        event,
        DiscoveryEvent::Lost {
            uuid: "dev1".into()
        }
    );
    assert!(
        started.elapsed() >= Duration::from_millis(4500),
        "expired too early"
    );
    discovery.shutdown().await;
}

#[tokio::test]
async fn discover_once_returns_each_device_once_ordered_by_uuid() {
    let device = fake_device().await;
    let addr = device.local_addr().unwrap();
    let responder = spawn_responder(device, vec!["zulu", "alpha"]);

    let mut cfg = config();
    cfg.search_repeat = 3; // every device answers three times
    let devices = discover_once_tuned(cfg, Duration::from_millis(700), tuning(addr))
        .await
        .unwrap();
    responder.abort();

    let uuids: Vec<&str> = devices.iter().map(|d| d.uuid.as_str()).collect();
    assert_eq!(uuids, ["alpha", "zulu"]);
    assert!(devices.iter().all(|d| d.kind == DeviceKind::MediaRenderer));
}

#[tokio::test]
async fn discover_once_with_nobody_answering_returns_nothing() {
    let device = fake_device().await; // never answers
    let started = Instant::now();
    let devices = discover_once_tuned(
        config(),
        Duration::from_millis(300),
        tuning(device.local_addr().unwrap()),
    )
    .await
    .unwrap();
    assert!(devices.is_empty());
    assert!(started.elapsed() >= Duration::from_millis(300));
}

#[tokio::test]
async fn searches_repeat_at_the_interval_and_stop_when_dropped() {
    let device = fake_device().await;
    let mut cfg = config();
    cfg.search_interval_secs = 1;
    let discovery = Discovery::start_tuned(cfg, tuning(device.local_addr().unwrap()))
        .await
        .unwrap();

    let started = Instant::now();
    next_search(&device).await; // immediately
    next_search(&device).await; // after about one second
    let gap = started.elapsed();
    assert!(
        gap >= Duration::from_millis(900),
        "second search after {gap:?}"
    );

    drop(discovery);
    // drain anything already in flight
    tokio::time::sleep(Duration::from_millis(100)).await;
    let mut buffer = [0u8; 2048];
    while timeout(Duration::from_millis(20), device.recv_from(&mut buffer))
        .await
        .is_ok()
    {}
    assert!(
        timeout(Duration::from_millis(1600), device.recv_from(&mut buffer))
            .await
            .is_err(),
        "no more searches after dropping Discovery"
    );
}

#[tokio::test]
async fn devices_of_other_kinds_are_only_reported_when_asked_for() {
    let device = fake_device().await;
    let addr = device.local_addr().unwrap();

    // default targets (renderer): a plain root device is not interesting
    let mut quiet = Discovery::start_tuned(config(), tuning(addr))
        .await
        .unwrap();
    let asker = next_search(&device).await;
    device
        .send_to(
            &response("root1", "upnp:rootdevice", "http://192.0.2.7/r.xml", 1800),
            asker,
        )
        .await
        .unwrap();
    assert!(
        timeout(Duration::from_millis(300), quiet.next())
            .await
            .is_err()
    );
    quiet.shutdown().await;

    // asking for root devices reports it, as kind Other
    let mut cfg = config();
    cfg.targets = vec![SearchTarget::Custom("upnp:rootdevice".to_string())];
    let mut loud = Discovery::start_tuned(cfg, tuning(addr)).await.unwrap();
    let asker = next_search(&device).await;
    device
        .send_to(
            &response("root1", "upnp:rootdevice", "http://192.0.2.7/r.xml", 1800),
            asker,
        )
        .await
        .unwrap();
    let event = timeout(WAIT, loud.next()).await.unwrap().unwrap();
    assert!(matches!(&event, DiscoveryEvent::Found(d) if d.kind == DeviceKind::Other));
    loud.shutdown().await;
}

#[tokio::test]
async fn start_fails_clearly_without_a_usable_interface() {
    let mut cfg = config();
    cfg.interfaces = Some(vec!["no-such-interface".to_string()]);
    let result = Discovery::start(cfg).await;
    assert!(matches!(result, Err(SsdpError::NoUsableInterface)));
}

#[tokio::test]
async fn start_rejects_a_bad_configuration() {
    let mut cfg = config();
    cfg.targets.clear();
    assert!(matches!(
        Discovery::start(cfg).await,
        Err(SsdpError::InvalidConfig(_))
    ));
    let mut cfg = config();
    cfg.targets = vec![SearchTarget::Custom("a\r\nb".to_string())];
    assert!(matches!(
        Discovery::start(cfg).await,
        Err(SsdpError::InvalidConfig(_))
    ));
}
