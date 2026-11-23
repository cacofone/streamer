//! Tests of the SSDP message builder and parser.

use proptest::prelude::*;
use streamer_core::ssdp::message::{
    DEFAULT_MAX_AGE_SECS, MAX_DATAGRAM, MAX_MAX_AGE_SECS, MIN_MAX_AGE_SECS, Message, ParseError,
    Usn, build_search, parse,
};
use streamer_core::ssdp::{DeviceKind, SearchTarget};

/// Reads a fixture and converts it to CRLF line endings, as sent on the wire.
/// Git may check the files out with LF, so normalize first.
fn fixture(name: &str) -> Vec<u8> {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path).expect("fixture exists");
    text.replace("\r\n", "\n")
        .replace('\n', "\r\n")
        .into_bytes()
}

fn response(headers: &str) -> Vec<u8> {
    format!("HTTP/1.1 200 OK\r\n{headers}\r\n\r\n").into_bytes()
}

const USN: &str = "USN: uuid:00000000-0000-4000-8000-000000000001::upnp:rootdevice";
const ST: &str = "ST: upnp:rootdevice";
const LOC: &str = "LOCATION: http://192.0.2.20:49152/description.xml";

fn max_age_of(cache_control: Option<&str>) -> u64 {
    let mut headers = format!("{USN}\r\n{ST}\r\n{LOC}");
    if let Some(value) = cache_control {
        headers.push_str(&format!("\r\nCACHE-CONTROL: {value}"));
    }
    match parse(&response(&headers)).unwrap() {
        Some(Message::SearchResponse(a)) => a.max_age_secs,
        other => panic!("unexpected result: {other:?}"),
    }
}

#[test]
fn parses_search_response() {
    let Some(Message::SearchResponse(a)) = parse(&fixture("search_response_renderer.txt")).unwrap()
    else {
        panic!("expected a search response");
    };
    assert_eq!(a.usn.uuid, "00000000-0000-4000-8000-000000000001");
    assert_eq!(
        a.usn.suffix.as_deref(),
        Some("urn:schemas-upnp-org:device:MediaRenderer:1")
    );
    assert_eq!(a.target, "urn:schemas-upnp-org:device:MediaRenderer:1");
    assert_eq!(a.location, "http://192.0.2.20:49152/description.xml");
    assert_eq!(a.max_age_secs, 1800);
    assert_eq!(a.server.as_deref(), Some("Linux/4.9 UPnP/1.0 Example/1.0"));
}

#[test]
fn parses_notify_alive() {
    let Some(Message::Alive(a)) = parse(&fixture("notify_alive_server.txt")).unwrap() else {
        panic!("expected alive");
    };
    assert_eq!(a.usn.uuid, "00000000-0000-4000-8000-000000000002");
    assert_eq!(a.max_age_secs, 900);
    assert_eq!(DeviceKind::from_target(&a.target), DeviceKind::MediaServer);
}

#[test]
fn notify_update_is_treated_as_alive() {
    let packet = "NOTIFY * HTTP/1.1\r\nNT: upnp:rootdevice\r\nNTS: ssdp:update\r\n\
                  USN: uuid:abc::upnp:rootdevice\r\nLOCATION: http://192.0.2.5/d.xml\r\n\r\n";
    assert!(matches!(
        parse(packet.as_bytes()).unwrap(),
        Some(Message::Alive(_))
    ));
}

#[test]
fn parses_byebye_without_location() {
    let Some(Message::ByeBye(d)) = parse(&fixture("notify_byebye.txt")).unwrap() else {
        panic!("expected byebye");
    };
    assert_eq!(d.usn.uuid, "00000000-0000-4000-8000-000000000002");
    assert_eq!(d.target, "upnp:rootdevice");
}

#[test]
fn ignores_searches_from_other_control_points() {
    assert_eq!(parse(&fixture("msearch_other_client.txt")).unwrap(), None);
}

#[test]
fn ignores_http_responses_that_are_not_200() {
    assert_eq!(parse(b"HTTP/1.1 404 Not Found\r\n\r\n").unwrap(), None);
}

#[test]
fn tolerates_odd_header_formatting() {
    let Some(Message::SearchResponse(a)) = parse(&fixture("response_odd_headers.txt")).unwrap()
    else {
        panic!("expected a search response");
    };
    // mixed case names, extra whitespace, extra directives, "UUID:" in capitals
    assert_eq!(a.max_age_secs, 120);
    assert_eq!(a.usn.uuid, "ABCDEF-1234");
    assert_eq!(a.usn.suffix.as_deref(), Some("upnp:rootdevice"));
    // duplicate LOCATION: the first one wins
    assert_eq!(a.location, "http://192.0.2.40:5000/desc.xml");
}

#[test]
fn accepts_bare_line_feeds() {
    let packet =
        "HTTP/1.1 200 OK\nST: upnp:rootdevice\nUSN: uuid:abc\nLOCATION: http://192.0.2.5/d.xml\n\n";
    assert!(matches!(
        parse(packet.as_bytes()).unwrap(),
        Some(Message::SearchResponse(_))
    ));
}

#[test]
fn missing_required_headers_are_reported() {
    assert_eq!(
        parse(&response(&format!("{USN}\r\n{ST}"))),
        Err(ParseError::MissingHeader("location"))
    );
    assert_eq!(
        parse(&response(&format!("{ST}\r\n{LOC}"))),
        Err(ParseError::MissingHeader("usn"))
    );
    assert_eq!(
        parse(&response(&format!("{USN}\r\n{LOC}"))),
        Err(ParseError::MissingHeader("st"))
    );
    // an empty value counts as missing
    assert_eq!(
        parse(&response(&format!("{USN}\r\n{ST}\r\nLOCATION:"))),
        Err(ParseError::MissingHeader("location"))
    );
}

#[test]
fn notify_needs_nts_and_known_value() {
    assert_eq!(
        parse(b"NOTIFY * HTTP/1.1\r\nNT: upnp:rootdevice\r\n\r\n"),
        Err(ParseError::MissingHeader("nts"))
    );
    assert_eq!(
        parse(b"NOTIFY * HTTP/1.1\r\nNTS: ssdp:bogus\r\n\r\n"),
        Err(ParseError::InvalidNts)
    );
}

#[test]
fn max_age_is_parsed_defaulted_and_clamped() {
    assert_eq!(max_age_of(Some("max-age=300")), 300);
    assert_eq!(max_age_of(Some("max-age = 300")), 300);
    assert_eq!(max_age_of(Some("no-cache, MAX-AGE=60")), 60);
    assert_eq!(max_age_of(None), DEFAULT_MAX_AGE_SECS);
    assert_eq!(max_age_of(Some("no-cache")), DEFAULT_MAX_AGE_SECS);
    assert_eq!(max_age_of(Some("max-age=abc")), DEFAULT_MAX_AGE_SECS);
    assert_eq!(max_age_of(Some("max-age=-5")), DEFAULT_MAX_AGE_SECS);
    assert_eq!(max_age_of(Some("max-age=0")), MIN_MAX_AGE_SECS);
    assert_eq!(max_age_of(Some("max-age=999999999")), MAX_MAX_AGE_SECS);
}

#[test]
fn location_must_be_an_http_url() {
    for bad in [
        "ftp://192.0.2.5/d.xml",
        "file:///etc/passwd",
        "javascript:alert(1)",
        "not a url",
        "http://",
    ] {
        assert_eq!(
            parse(&response(&format!("{USN}\r\n{ST}\r\nLOCATION: {bad}"))),
            Err(ParseError::InvalidLocation),
            "location {bad:?}"
        );
    }
    assert!(
        parse(&response(&format!(
            "{USN}\r\n{ST}\r\nLOCATION: https://192.0.2.5:8443/d.xml"
        )))
        .unwrap()
        .is_some()
    );
}

#[test]
fn usn_parsing() {
    let usn = Usn::parse("uuid:abc").unwrap();
    assert_eq!((usn.uuid.as_str(), usn.suffix), ("abc", None));
    let usn = Usn::parse("  UUID:abc::urn:x:device:Y:1 ").unwrap();
    assert_eq!(usn.uuid, "abc");
    assert_eq!(usn.suffix.as_deref(), Some("urn:x:device:Y:1"));
    // an empty suffix after "::" is dropped
    assert_eq!(Usn::parse("uuid:abc::").unwrap().suffix, None);

    for bad in ["", "abc", "uuid:", "uuid:::upnp:rootdevice", "urn:abc"] {
        assert_eq!(Usn::parse(bad), Err(ParseError::InvalidUsn), "usn {bad:?}");
    }
    let long = format!("uuid:{}", "a".repeat(101));
    assert_eq!(Usn::parse(&long), Err(ParseError::InvalidUsn));
}

#[test]
fn rejects_oversized_empty_and_garbage() {
    assert_eq!(
        parse(&vec![b'a'; MAX_DATAGRAM + 1]),
        Err(ParseError::TooLarge(MAX_DATAGRAM + 1))
    );
    assert_eq!(parse(b""), Err(ParseError::Empty));
    assert_eq!(parse(b"\r\n\r\n"), Err(ParseError::Empty));
    assert_eq!(parse(b"hello world"), Err(ParseError::UnknownStartLine));
    assert_eq!(
        parse(&[0xff, 0xfe, 0x00, 0x01]),
        Err(ParseError::UnknownStartLine)
    );
}

#[test]
fn device_kind_accepts_every_version_of_the_standard_types() {
    use DeviceKind::*;
    let cases = [
        ("urn:schemas-upnp-org:device:MediaServer:1", MediaServer),
        ("urn:schemas-upnp-org:device:MediaServer:4", MediaServer),
        ("URN:SCHEMAS-UPNP-ORG:DEVICE:MEDIARENDERER:3", MediaRenderer),
        ("urn:schemas-upnp-org:device:MediaRenderer:1", MediaRenderer),
        ("upnp:rootdevice", Other),
        ("urn:schemas-upnp-org:service:AVTransport:1", Other),
        ("urn:schemas-upnp-org:device:MediaServerX:1", Other),
        ("", Other),
    ];
    for (target, expected) in cases {
        assert_eq!(DeviceKind::from_target(target), expected, "{target:?}");
    }
}

#[test]
fn builds_a_search_request() {
    let request = build_search(&SearchTarget::MediaRenderer, 3, "test/1 UPnP/1.1 x/1").unwrap();
    assert_eq!(
        request,
        "M-SEARCH * HTTP/1.1\r\n\
         HOST: 239.255.255.250:1900\r\n\
         MAN: \"ssdp:discover\"\r\n\
         MX: 3\r\n\
         ST: urn:schemas-upnp-org:device:MediaRenderer:1\r\n\
         USER-AGENT: test/1 UPnP/1.1 x/1\r\n\
         \r\n"
    );
}

#[test]
fn search_mx_is_clamped_and_targets_are_rendered() {
    let mx = |value: u8| {
        build_search(&SearchTarget::All, value, "ua")
            .unwrap()
            .lines()
            .find(|line| line.starts_with("MX:"))
            .unwrap()
            .to_string()
    };
    assert_eq!(mx(0), "MX: 1");
    assert_eq!(mx(3), "MX: 3");
    assert_eq!(mx(200), "MX: 5");
    assert!(
        build_search(&SearchTarget::All, 3, "ua")
            .unwrap()
            .contains("ST: ssdp:all\r\n")
    );
    assert!(
        build_search(&SearchTarget::MediaServer, 3, "ua")
            .unwrap()
            .contains("ST: urn:schemas-upnp-org:device:MediaServer:1\r\n")
    );
}

#[test]
fn search_rejects_header_injection() {
    assert!(build_search(&SearchTarget::Custom("a\r\nX-Evil: 1".into()), 3, "ua").is_err());
    assert!(build_search(&SearchTarget::Custom(String::new()), 3, "ua").is_err());
    assert!(build_search(&SearchTarget::All, 3, "ua\r\nX-Evil: 1").is_err());
}

#[test]
fn a_built_search_is_ignored_by_the_parser() {
    // Our own M-SEARCH (seen if multicast loops back) must not be mistaken for a device.
    let request = build_search(&SearchTarget::All, 3, "ua").unwrap();
    assert_eq!(parse(request.as_bytes()).unwrap(), None);
}

#[test]
fn default_user_agent_has_no_personal_data() {
    let agent = streamer_core::ssdp::message::default_user_agent();
    assert!(agent.contains("UPnP/1.1 cacofone-streamer/"));
    assert!(!agent.chars().any(char::is_control));
}

proptest! {
    #[test]
    fn parser_never_panics_on_random_bytes(data in proptest::collection::vec(any::<u8>(), 0..3000)) {
        let _ = parse(&data);
    }

    #[test]
    fn parser_never_panics_on_header_like_text(
        start in prop_oneof![
            Just("HTTP/1.1 200 OK"), Just("NOTIFY * HTTP/1.1"),
            Just("M-SEARCH * HTTP/1.1"), Just(""), Just("garbage")
        ],
        headers in proptest::collection::vec(
            (prop_oneof![
                Just("LOCATION"), Just("USN"), Just("ST"), Just("NT"), Just("NTS"),
                Just("CACHE-CONTROL"), Just("SERVER"), Just("X-Other")
            ], ".{0,80}"),
            0..12
        )
    ) {
        let mut packet = format!("{start}\r\n");
        for (name, value) in headers {
            packet.push_str(&format!("{name}: {value}\r\n"));
        }
        packet.push_str("\r\n");
        let _ = parse(packet.as_bytes());
    }

    #[test]
    fn usn_parse_never_panics(value in ".{0,200}") {
        let _ = Usn::parse(&value);
    }
}

#[test]
fn multibyte_characters_at_prefix_boundaries_do_not_panic() {
    // Found by the property test: slicing a token at byte 5 panicked inside a character.
    assert_eq!(parse("ééé".as_bytes()), Err(ParseError::UnknownStartLine));
    assert_eq!(
        parse("HTT€ 200".as_bytes()),
        Err(ParseError::UnknownStartLine)
    );
    assert_eq!(Usn::parse("uui€:abc"), Err(ParseError::InvalidUsn));
    assert_eq!(Usn::parse("é"), Err(ParseError::InvalidUsn));
}
