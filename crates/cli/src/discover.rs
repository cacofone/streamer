//! The `discover` command.

use std::future::Future;
use std::io::{self, Write};
use std::time::Duration;

use clap::{Args, ValueEnum};
use streamer_core::ssdp::discovery::{Tuning, discover_once_tuned};
use streamer_core::ssdp::{Discovery, DiscoveryConfig, SearchTarget, SsdpError};

use crate::output::{device_json, device_table, event_json, event_text};

/// What went wrong while running a command.
#[derive(Debug, thiserror::Error)]
pub enum CliError {
    /// Discovery could not run.
    #[error(transparent)]
    Discovery(#[from] SsdpError),
    /// Writing the output failed.
    #[error("cannot write output: {0}")]
    Io(#[from] io::Error),
}

/// Kinds of devices to look for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum DeviceType {
    /// Media servers (music libraries)
    Server,
    /// Media renderers (speakers and receivers)
    Renderer,
    /// Every UPnP device and service (for diagnostics)
    All,
}

#[derive(Debug, Clone, Args)]
pub struct DiscoverArgs {
    /// How long to search, in seconds
    #[arg(long, default_value_t = 5, value_parser = clap::value_parser!(u64).range(1..=600))]
    pub timeout: u64,

    /// What to look for; repeat for several (default: server and renderer)
    #[arg(long = "type", value_enum, value_name = "TYPE")]
    pub types: Vec<DeviceType>,

    /// Only use this network interface (name or IPv4 address); repeat for several
    #[arg(long = "interface", value_name = "NAME_OR_IP")]
    pub interfaces: Vec<String>,

    /// Print one JSON object per line instead of a table
    #[arg(long)]
    pub json: bool,

    /// Keep running and print devices as they appear and disappear (stop with Ctrl-C)
    #[arg(long)]
    pub watch: bool,
}

/// The search targets for the chosen device types.
fn targets(types: &[DeviceType]) -> Vec<SearchTarget> {
    if types.is_empty() {
        return vec![SearchTarget::MediaServer, SearchTarget::MediaRenderer];
    }
    if types.contains(&DeviceType::All) {
        return vec![SearchTarget::All];
    }
    let mut targets = Vec::new();
    for device_type in types {
        let target = match device_type {
            DeviceType::Server => SearchTarget::MediaServer,
            DeviceType::Renderer => SearchTarget::MediaRenderer,
            DeviceType::All => continue,
        };
        if !targets.contains(&target) {
            targets.push(target);
        }
    }
    targets
}

/// The discovery settings for these arguments.
pub fn build_config(args: &DiscoverArgs) -> DiscoveryConfig {
    let defaults = DiscoveryConfig::default();
    DiscoveryConfig {
        targets: targets(&args.types),
        // devices answer within MX seconds, so it must not exceed the search time
        mx_secs: u8::try_from(args.timeout.clamp(1, 3)).unwrap_or(3),
        // a one-shot search is a single round; watching repeats it
        search_interval_secs: if args.watch {
            defaults.search_interval_secs
        } else {
            0
        },
        interfaces: if args.interfaces.is_empty() {
            None
        } else {
            Some(args.interfaces.clone())
        },
        ..defaults
    }
}

/// Runs the command. `stop` ends watch mode when it completes (Ctrl-C in the real program).
pub async fn run<O: Write, E: Write>(
    args: &DiscoverArgs,
    tuning: Tuning,
    out: &mut O,
    err: &mut E,
    stop: impl Future<Output = ()>,
) -> Result<(), CliError> {
    let config = build_config(args);
    if args.watch {
        watch(args, config, tuning, out, err, stop).await
    } else {
        once(args, config, tuning, out, err).await
    }
}

async fn once<O: Write, E: Write>(
    args: &DiscoverArgs,
    config: DiscoveryConfig,
    tuning: Tuning,
    out: &mut O,
    err: &mut E,
) -> Result<(), CliError> {
    if !args.json {
        writeln!(err, "Searching for {} seconds...", args.timeout)?;
    }
    let devices = discover_once_tuned(config, Duration::from_secs(args.timeout), tuning).await?;

    if devices.is_empty() {
        writeln!(err, "No devices found.")?;
        return Ok(());
    }
    if args.json {
        for device in &devices {
            writeln!(out, "{}", device_json(device))?;
        }
    } else {
        writeln!(out, "{}", device_table(&devices))?;
    }
    out.flush()?;
    Ok(())
}

async fn watch<O: Write, E: Write>(
    args: &DiscoverArgs,
    config: DiscoveryConfig,
    tuning: Tuning,
    out: &mut O,
    err: &mut E,
    stop: impl Future<Output = ()>,
) -> Result<(), CliError> {
    let mut discovery = Discovery::start_tuned(config, tuning).await?;
    if !args.json {
        writeln!(err, "Watching for devices, press Ctrl-C to stop...")?;
    }
    tokio::pin!(stop);
    let mut result = Ok(());
    loop {
        tokio::select! {
            () = &mut stop => break,
            event = discovery.next() => {
                let Some(event) = event else { break };
                let line = if args.json { event_json(&event) } else { event_text(&event) };
                // flush every line so that pipes and `tail -f` style readers see it at once
                if let Err(error) = writeln!(out, "{line}").and_then(|()| out.flush()) {
                    result = Err(CliError::Io(error));
                    break;
                }
            }
        }
    }
    discovery.shutdown().await;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::SocketAddr;
    use std::sync::Arc;

    use clap::Parser;
    use streamer_core::ssdp::search::Timings;
    use tokio::net::UdpSocket;

    #[derive(Debug, Parser)]
    struct Wrapper {
        #[command(flatten)]
        args: DiscoverArgs,
    }

    fn parse(arguments: &[&str]) -> Result<DiscoverArgs, clap::Error> {
        let mut full = vec!["test"];
        full.extend_from_slice(arguments);
        Wrapper::try_parse_from(full).map(|wrapper| wrapper.args)
    }

    #[test]
    fn defaults() {
        let args = parse(&[]).unwrap();
        assert_eq!(args.timeout, 5);
        assert!(args.types.is_empty() && args.interfaces.is_empty());
        assert!(!args.json && !args.watch);
    }

    #[test]
    fn all_options_are_parsed() {
        let args = parse(&[
            "--timeout",
            "8",
            "--type",
            "server",
            "--type",
            "renderer",
            "--interface",
            "eth0",
            "--interface",
            "192.0.2.5",
            "--json",
            "--watch",
        ])
        .unwrap();
        assert_eq!(args.timeout, 8);
        assert_eq!(args.types, [DeviceType::Server, DeviceType::Renderer]);
        assert_eq!(args.interfaces, ["eth0", "192.0.2.5"]);
        assert!(args.json && args.watch);
    }

    #[test]
    fn bad_arguments_are_rejected() {
        assert!(parse(&["--timeout", "0"]).is_err());
        assert!(parse(&["--timeout", "601"]).is_err());
        assert!(parse(&["--timeout", "abc"]).is_err());
        assert!(parse(&["--type", "speaker"]).is_err());
        assert!(parse(&["--bogus"]).is_err());
    }

    #[test]
    fn device_types_map_to_search_targets() {
        use DeviceType::*;
        assert_eq!(
            targets(&[]),
            [SearchTarget::MediaServer, SearchTarget::MediaRenderer]
        );
        assert_eq!(targets(&[Server]), [SearchTarget::MediaServer]);
        assert_eq!(targets(&[Renderer]), [SearchTarget::MediaRenderer]);
        assert_eq!(
            targets(&[Renderer, Server, Renderer]),
            [SearchTarget::MediaRenderer, SearchTarget::MediaServer]
        );
        assert_eq!(targets(&[Server, All]), [SearchTarget::All]);
    }

    #[test]
    fn the_config_follows_the_arguments() {
        let once = build_config(&parse(&["--timeout", "2"]).unwrap());
        assert_eq!(once.mx_secs, 2);
        assert_eq!(once.search_interval_secs, 0);
        assert_eq!(once.interfaces, None);

        let long = build_config(&parse(&["--timeout", "60"]).unwrap());
        assert_eq!(long.mx_secs, 3);

        let watching = build_config(&parse(&["--watch", "--interface", "eth0"]).unwrap());
        assert_eq!(
            watching.search_interval_secs,
            DiscoveryConfig::default().search_interval_secs
        );
        assert_eq!(watching.interfaces, Some(vec!["eth0".to_string()]));
    }

    // ---- end to end, with a fake device on loopback ----

    const RENDERER: &str = "urn:schemas-upnp-org:device:MediaRenderer:1";

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

    fn loopback_args(extra: &[&str]) -> DiscoverArgs {
        let mut arguments = vec![
            "--timeout",
            "1",
            "--interface",
            "127.0.0.1",
            "--type",
            "renderer",
        ];
        arguments.extend_from_slice(extra);
        parse(&arguments).unwrap()
    }

    /// A device that answers every search with the given uuid.
    async fn fake_device(uuid: &'static str) -> (SocketAddr, tokio::task::JoinHandle<()>) {
        let socket = Arc::new(UdpSocket::bind("127.0.0.1:0").await.unwrap());
        let addr = socket.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let mut buffer = [0u8; 2048];
            while let Ok((_, from)) = socket.recv_from(&mut buffer).await {
                let packet = format!(
                    "HTTP/1.1 200 OK\r\nCACHE-CONTROL: max-age=1800\r\n\
                     LOCATION: http://192.0.2.7/{uuid}.xml\r\nSERVER: Test/1.0\r\n\
                     ST: {RENDERER}\r\nUSN: uuid:{uuid}::{RENDERER}\r\n\r\n"
                );
                let _ = socket.send_to(packet.as_bytes(), from).await;
            }
        });
        (addr, task)
    }

    async fn run_capturing(
        args: &DiscoverArgs,
        tuning: Tuning,
        stop: impl Future<Output = ()>,
    ) -> (Result<(), CliError>, String, String) {
        let (mut out, mut err) = (Vec::new(), Vec::new());
        let result = run(args, tuning, &mut out, &mut err, stop).await;
        (
            result,
            String::from_utf8(out).unwrap(),
            String::from_utf8(err).unwrap(),
        )
    }

    #[tokio::test]
    async fn a_one_shot_search_prints_a_table() {
        let (addr, device) = fake_device("dev1").await;
        let (result, out, err) =
            run_capturing(&loopback_args(&[]), tuning(addr), std::future::pending()).await;
        device.abort();

        result.unwrap();
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 2, "header and one device: {out:?}");
        assert!(lines[0].starts_with("KIND"));
        assert!(lines[1].starts_with("renderer"));
        assert!(lines[1].contains("127.0.0.1") && lines[1].contains("dev1"));
        assert!(lines[1].ends_with("Test/1.0"));
        assert!(err.contains("Searching for 1 seconds"));
    }

    #[tokio::test]
    async fn json_output_is_only_json_lines() {
        let (addr, device) = fake_device("dev1").await;
        let (result, out, err) = run_capturing(
            &loopback_args(&["--json"]),
            tuning(addr),
            std::future::pending(),
        )
        .await;
        device.abort();

        result.unwrap();
        assert_eq!(out.lines().count(), 1);
        let value: serde_json::Value = serde_json::from_str(out.trim_end()).unwrap();
        assert_eq!(value["uuid"], "dev1");
        assert_eq!(value["kind"], "MediaRenderer");
        assert_eq!(value["address"], "127.0.0.1");
        assert!(err.is_empty(), "nothing but JSON is printed: {err:?}");
    }

    #[tokio::test]
    async fn finding_nothing_is_success_with_a_message_on_stderr() {
        // a socket that never answers
        let silent = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let (result, out, err) = run_capturing(
            &loopback_args(&["--json"]),
            tuning(silent.local_addr().unwrap()),
            std::future::pending(),
        )
        .await;
        result.unwrap();
        assert!(out.is_empty());
        assert!(err.contains("No devices found"));
    }

    #[tokio::test]
    async fn an_unknown_interface_is_an_error() {
        let args = parse(&["--timeout", "1", "--interface", "no-such-interface"]).unwrap();
        let (result, out, _) =
            run_capturing(&args, Tuning::default(), std::future::pending()).await;
        assert!(matches!(
            result,
            Err(CliError::Discovery(SsdpError::NoUsableInterface))
        ));
        assert!(out.is_empty());
    }

    #[tokio::test]
    async fn watching_prints_events_until_stopped() {
        let (addr, device) = fake_device("dev1").await;
        let stop = tokio::time::sleep(Duration::from_millis(800));
        let (result, out, err) =
            run_capturing(&loopback_args(&["--watch", "--json"]), tuning(addr), stop).await;
        device.abort();

        result.unwrap();
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 1, "one found event: {out:?}");
        let value: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(value["event"], "found");
        assert_eq!(value["device"]["uuid"], "dev1");
        assert!(err.is_empty());
    }

    #[tokio::test]
    async fn watching_in_text_mode_prints_readable_lines() {
        let (addr, device) = fake_device("dev1").await;
        let stop = tokio::time::sleep(Duration::from_millis(800));
        let (result, out, err) =
            run_capturing(&loopback_args(&["--watch"]), tuning(addr), stop).await;
        device.abort();

        result.unwrap();
        assert!(out.starts_with("found    renderer"), "{out:?}");
        assert!(err.contains("press Ctrl-C"));
    }

    /// A writer whose reader has gone away.
    struct BrokenPipe;

    impl Write for BrokenPipe {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::ErrorKind::BrokenPipe.into())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn a_closed_pipe_is_reported_as_an_io_error() {
        let (addr, device) = fake_device("dev1").await;
        let mut err = Vec::new();
        let result = run(
            &loopback_args(&["--json"]),
            tuning(addr),
            &mut BrokenPipe,
            &mut err,
            std::future::pending(),
        )
        .await;
        device.abort();
        assert!(matches!(&result, Err(CliError::Io(e)) if e.kind() == io::ErrorKind::BrokenPipe));
    }
}
