//! Search and receive tests over loopback unicast.
//!
//! No multicast is used, so these run anywhere, including CI runners.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use streamer_core::ssdp::SearchTarget;
use streamer_core::ssdp::message::Message;
use streamer_core::ssdp::receive::{Received, receive_loop};
use streamer_core::ssdp::search::{Timings, build_requests, search_loop, send_round};
use tokio::net::UdpSocket;
use tokio::sync::mpsc;
use tokio::time::timeout;

const WAIT: Duration = Duration::from_secs(5);

fn fast() -> Timings {
    let gap = (Duration::from_millis(1), Duration::from_millis(3));
    Timings {
        repeat_gap: gap,
        target_gap: gap,
    }
}

async fn socket() -> Arc<UdpSocket> {
    Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap())
}

fn response(uuid: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 200 OK\r\nCACHE-CONTROL: max-age=1800\r\n\
         LOCATION: http://192.0.2.20:49152/{uuid}.xml\r\n\
         ST: urn:schemas-upnp-org:device:MediaRenderer:1\r\n\
         USN: uuid:{uuid}::urn:schemas-upnp-org:device:MediaRenderer:1\r\n\r\n"
    )
    .into_bytes()
}

/// Reads datagrams from `socket` until `count` arrived, as text.
async fn read_datagrams(socket: &UdpSocket, count: usize) -> Vec<(String, SocketAddr)> {
    let mut found = Vec::new();
    let mut buffer = [0u8; 2048];
    while found.len() < count {
        let (length, from) = timeout(WAIT, socket.recv_from(&mut buffer))
            .await
            .expect("a datagram arrives in time")
            .unwrap();
        found.push((
            String::from_utf8_lossy(&buffer[..length]).into_owned(),
            from,
        ));
    }
    found
}

#[tokio::test]
async fn a_round_sends_every_target_the_requested_number_of_times() {
    let device = socket().await;
    let client = socket().await;
    let requests =
        build_requests(&[SearchTarget::MediaServer, SearchTarget::MediaRenderer], 3).unwrap();

    let sent = send_round(&client, device.local_addr().unwrap(), &requests, 3, &fast()).await;
    assert_eq!(sent, 6);

    let datagrams = read_datagrams(&device, 6).await;
    let count = |needle: &str| {
        datagrams
            .iter()
            .filter(|(text, _)| text.contains(needle))
            .count()
    };
    assert_eq!(
        count("ST: urn:schemas-upnp-org:device:MediaServer:1\r\n"),
        3
    );
    assert_eq!(
        count("ST: urn:schemas-upnp-org:device:MediaRenderer:1\r\n"),
        3
    );
    for (text, from) in &datagrams {
        assert!(text.starts_with("M-SEARCH * HTTP/1.1\r\n"));
        assert!(text.contains("MAN: \"ssdp:discover\"\r\n"));
        assert_eq!(*from, client.local_addr().unwrap());
    }
}

#[tokio::test]
async fn repeat_is_clamped_to_at_least_once() {
    let device = socket().await;
    let client = socket().await;
    let requests = build_requests(&[SearchTarget::All], 3).unwrap();
    let sent = send_round(&client, device.local_addr().unwrap(), &requests, 0, &fast()).await;
    assert_eq!(sent, 1);
}

#[tokio::test]
async fn a_device_answer_comes_back_as_a_message() {
    let device = socket().await;
    let client = socket().await;
    let (tx, mut rx) = mpsc::channel(16);
    let reader = tokio::spawn(receive_loop(client.clone(), tx));

    let requests = build_requests(&[SearchTarget::MediaRenderer], 3).unwrap();
    send_round(&client, device.local_addr().unwrap(), &requests, 1, &fast()).await;

    // the fake device answers whoever asked
    let (_, asker) = read_datagrams(&device, 1).await.remove(0);
    device.send_to(&response("device-1"), asker).await.unwrap();

    let received = timeout(WAIT, rx.recv()).await.unwrap().unwrap();
    assert_eq!(received.from.to_string(), "127.0.0.1");
    let Message::SearchResponse(announcement) = received.message else {
        panic!("expected a search response");
    };
    assert_eq!(announcement.usn.uuid, "device-1");
    assert_eq!(
        announcement.location,
        "http://192.0.2.20:49152/device-1.xml"
    );
    reader.abort();
}

#[tokio::test]
async fn unusable_packets_are_skipped_and_the_loop_keeps_going() {
    let client = socket().await;
    let sender = socket().await;
    let (tx, mut rx) = mpsc::channel(16);
    let reader = tokio::spawn(receive_loop(client.clone(), tx));
    let to = client.local_addr().unwrap();

    sender.send_to(b"hello", to).await.unwrap();
    sender.send_to(&[0xff, 0x00, 0xfe], to).await.unwrap();
    sender.send_to(&vec![b'a'; 3000], to).await.unwrap();
    sender
        .send_to(b"HTTP/1.1 200 OK\r\nST: upnp:rootdevice\r\n\r\n", to)
        .await
        .unwrap();
    sender
        .send_to(
            b"M-SEARCH * HTTP/1.1\r\nMAN: \"ssdp:discover\"\r\nST: ssdp:all\r\n\r\n",
            to,
        )
        .await
        .unwrap();
    sender.send_to(&response("valid"), to).await.unwrap();

    let received = timeout(WAIT, rx.recv()).await.unwrap().unwrap();
    let Message::SearchResponse(announcement) = received.message else {
        panic!("expected a search response");
    };
    assert_eq!(announcement.usn.uuid, "valid");
    // nothing else was let through
    assert!(rx.try_recv().is_err());
    assert!(!reader.is_finished());
    reader.abort();
}

#[tokio::test]
async fn a_full_output_drops_messages_instead_of_blocking() {
    let client = socket().await;
    let sender = socket().await;
    let (tx, mut rx) = mpsc::channel::<Received>(1);
    let reader = tokio::spawn(receive_loop(client.clone(), tx));
    let to = client.local_addr().unwrap();

    for n in 0..20 {
        sender
            .send_to(&response(&format!("d{n}")), to)
            .await
            .unwrap();
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(!reader.is_finished(), "the loop must not stall or stop");

    // draining makes room again, and later messages get through
    let first = timeout(WAIT, rx.recv()).await.unwrap().unwrap();
    assert!(matches!(first.message, Message::SearchResponse(_)));
    while rx.try_recv().is_ok() {}
    sender.send_to(&response("later"), to).await.unwrap();
    let later = timeout(WAIT, rx.recv()).await.unwrap().unwrap();
    let Message::SearchResponse(announcement) = later.message else {
        panic!("expected a search response");
    };
    assert_eq!(announcement.usn.uuid, "later");
    reader.abort();
}

#[tokio::test]
async fn the_loop_ends_when_the_output_is_closed() {
    let client = socket().await;
    let sender = socket().await;
    let (tx, rx) = mpsc::channel(4);
    let reader = tokio::spawn(receive_loop(client.clone(), tx));
    drop(rx);
    sender
        .send_to(&response("anyone"), client.local_addr().unwrap())
        .await
        .unwrap();
    let result = timeout(WAIT, reader).await.expect("the loop ends").unwrap();
    assert!(result.is_ok());
}

#[tokio::test]
async fn searching_repeats_at_the_interval_and_stops_when_aborted() {
    let device = socket().await;
    let client = socket().await;
    let requests = build_requests(&[SearchTarget::All], 3).unwrap();
    let task = tokio::spawn(search_loop(
        client.clone(),
        device.local_addr().unwrap(),
        requests,
        1,
        Some(Duration::from_millis(40)),
        fast(),
    ));

    // three rounds arrive without being asked again
    let received = read_datagrams(&device, 3).await;
    assert_eq!(received.len(), 3);
    assert!(!task.is_finished());

    task.abort();
    let _ = task.await;
    // drain what was already in flight, then check that it is quiet
    tokio::time::sleep(Duration::from_millis(100)).await;
    let mut buffer = [0u8; 2048];
    while timeout(Duration::from_millis(20), device.recv_from(&mut buffer))
        .await
        .is_ok()
    {}
    assert!(
        timeout(Duration::from_millis(200), device.recv_from(&mut buffer))
            .await
            .is_err(),
        "no more searches after abort"
    );
}

#[tokio::test]
async fn without_an_interval_the_search_runs_once() {
    let device = socket().await;
    let client = socket().await;
    let requests = build_requests(&[SearchTarget::MediaServer], 3).unwrap();
    timeout(
        WAIT,
        search_loop(
            client,
            device.local_addr().unwrap(),
            requests,
            2,
            None,
            fast(),
        ),
    )
    .await
    .expect("returns after one round");
    assert_eq!(read_datagrams(&device, 2).await.len(), 2);
}
