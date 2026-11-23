//! Formatting of discovery results for the terminal and for scripts.

use serde::Serialize;
use streamer_core::ssdp::{Device, DeviceKind, DiscoveryEvent};

/// Short, plain name of a device kind.
pub fn kind_name(kind: DeviceKind) -> &'static str {
    match kind {
        DeviceKind::MediaServer => "server",
        DeviceKind::MediaRenderer => "renderer",
        DeviceKind::Other => "other",
    }
}

/// An aligned table of devices with a header row, without trailing spaces.
pub fn device_table(devices: &[Device]) -> String {
    let header = ["KIND", "ADDRESS", "UUID", "LOCATION", "SERVER"];
    let rows: Vec<[String; 5]> = devices
        .iter()
        .map(|device| {
            [
                kind_name(device.kind).to_string(),
                device.address.clone(),
                device.uuid.clone(),
                device.location.clone(),
                device.server.clone().unwrap_or_else(|| "-".to_string()),
            ]
        })
        .collect();

    // every column but the last is padded to its widest entry
    let mut widths = [0usize; 4];
    for (index, width) in widths.iter_mut().enumerate() {
        *width = rows
            .iter()
            .map(|row| row[index].chars().count())
            .chain(std::iter::once(header[index].chars().count()))
            .max()
            .unwrap_or(0);
    }

    let format_row = |cells: [&str; 5]| -> String {
        let mut line = String::new();
        for (index, cell) in cells.iter().enumerate() {
            line.push_str(cell);
            if let Some(width) = widths.get(index) {
                let padding = width.saturating_sub(cell.chars().count()) + 2;
                line.push_str(&" ".repeat(padding));
            }
        }
        line.trim_end().to_string()
    };

    let mut lines = vec![format_row(header)];
    for row in &rows {
        lines.push(format_row([&row[0], &row[1], &row[2], &row[3], &row[4]]));
    }
    lines.join("\n")
}

/// One device as a single JSON object (no trailing newline).
pub fn device_json(device: &Device) -> String {
    // a Device only holds strings and numbers, serializing cannot fail
    serde_json::to_string(device).unwrap_or_default()
}

#[derive(Serialize)]
#[serde(tag = "event", rename_all = "lowercase")]
enum EventLine<'a> {
    Found { device: &'a Device },
    Updated { device: &'a Device },
    Lost { uuid: &'a str },
}

/// One event as a single JSON object (no trailing newline).
pub fn event_json(event: &DiscoveryEvent) -> String {
    let line = match event {
        DiscoveryEvent::Found(device) => EventLine::Found { device },
        DiscoveryEvent::Updated(device) => EventLine::Updated { device },
        DiscoveryEvent::Lost { uuid } => EventLine::Lost { uuid },
    };
    serde_json::to_string(&line).unwrap_or_default()
}

/// One event as a line of plain text.
pub fn event_text(event: &DiscoveryEvent) -> String {
    match event {
        DiscoveryEvent::Found(device) => device_line("found", device),
        DiscoveryEvent::Updated(device) => device_line("updated", device),
        DiscoveryEvent::Lost { uuid } => format!("{:<8} {uuid}", "lost"),
    }
}

fn device_line(word: &str, device: &Device) -> String {
    let mut line = format!(
        "{word:<8} {:<9} {}  {}  {}",
        kind_name(device.kind),
        device.address,
        device.uuid,
        device.location
    );
    if let Some(server) = &device.server {
        line.push_str("  ");
        line.push_str(server);
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(kind: DeviceKind, address: &str, uuid: &str, server: Option<&str>) -> Device {
        Device {
            uuid: uuid.to_string(),
            location: format!("http://{address}:49152/description.xml"),
            kind,
            server: server.map(str::to_string),
            address: address.to_string(),
            expires_in_secs: 1800,
        }
    }

    #[test]
    fn kinds_have_short_names() {
        assert_eq!(kind_name(DeviceKind::MediaServer), "server");
        assert_eq!(kind_name(DeviceKind::MediaRenderer), "renderer");
        assert_eq!(kind_name(DeviceKind::Other), "other");
    }

    #[test]
    fn the_table_is_aligned_and_has_no_trailing_spaces() {
        let devices = [
            device(
                DeviceKind::MediaServer,
                "192.0.2.9",
                "a",
                Some("Linux UPnP/1.0"),
            ),
            device(
                DeviceKind::MediaRenderer,
                "192.0.2.123",
                "longer-uuid",
                None,
            ),
        ];
        let table = device_table(&devices);
        let lines: Vec<&str> = table.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].starts_with("KIND      ADDRESS      UUID         LOCATION"));
        // columns start at the same position in every row
        let column = |line: &str, text: &str| line.find(text).unwrap();
        assert_eq!(
            column(lines[1], "192.0.2.9"),
            column(lines[2], "192.0.2.123")
        );
        assert_eq!(column(lines[1], "a  "), column(lines[2], "longer-uuid"));
        assert!(lines[1].ends_with("Linux UPnP/1.0"));
        assert!(lines[2].ends_with("  -"), "a missing server shows a dash");
        assert!(lines.iter().all(|line| line == &line.trim_end()));
    }

    #[test]
    fn an_empty_table_is_just_the_header() {
        assert_eq!(device_table(&[]).lines().count(), 1);
    }

    #[test]
    fn a_device_is_one_json_object() {
        let json = device_json(&device(
            DeviceKind::MediaRenderer,
            "192.0.2.5",
            "u1",
            Some("S/1"),
        ));
        assert!(!json.contains('\n'));
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(value["uuid"], "u1");
        assert_eq!(value["kind"], "MediaRenderer");
        assert_eq!(value["address"], "192.0.2.5");
        assert_eq!(value["server"], "S/1");
        assert_eq!(value["expires_in_secs"], 1800);
        assert!(
            value["location"]
                .as_str()
                .unwrap()
                .starts_with("http://192.0.2.5")
        );
    }

    #[test]
    fn a_missing_server_is_json_null() {
        let json = device_json(&device(DeviceKind::Other, "192.0.2.5", "u1", None));
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(value["server"].is_null());
    }

    #[test]
    fn events_are_json_objects_with_an_event_field() {
        let found = DiscoveryEvent::Found(device(DeviceKind::MediaServer, "192.0.2.5", "u1", None));
        let value: serde_json::Value = serde_json::from_str(&event_json(&found)).unwrap();
        assert_eq!(value["event"], "found");
        assert_eq!(value["device"]["uuid"], "u1");

        let updated =
            DiscoveryEvent::Updated(device(DeviceKind::MediaServer, "192.0.2.5", "u1", None));
        let value: serde_json::Value = serde_json::from_str(&event_json(&updated)).unwrap();
        assert_eq!(value["event"], "updated");

        let lost = DiscoveryEvent::Lost { uuid: "u1".into() };
        let value: serde_json::Value = serde_json::from_str(&event_json(&lost)).unwrap();
        assert_eq!(value["event"], "lost");
        assert_eq!(value["uuid"], "u1");
    }

    #[test]
    fn events_are_readable_text_lines() {
        let found = DiscoveryEvent::Found(device(
            DeviceKind::MediaRenderer,
            "192.0.2.5",
            "u1",
            Some("S/1"),
        ));
        let text = event_text(&found);
        assert!(text.starts_with(
            "found    renderer  192.0.2.5  u1  http://192.0.2.5:49152/description.xml  S/1"
        ));
        assert_eq!(
            event_text(&DiscoveryEvent::Lost { uuid: "u1".into() }),
            "lost     u1"
        );
        assert!(!event_text(&found).contains('\n'));
    }
}
