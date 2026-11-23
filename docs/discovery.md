# Device discovery

Cacofone Streamer finds DLNA/UPnP media servers (music libraries) and media renderers (speakers and receivers) on your local network with SSDP, the Simple Service Discovery Protocol. This page explains what discovery does, how to use it, how to troubleshoot it and how to test it by hand.

SSDP only tells us that a device exists and where its description document lives. Reading that description, browsing a library and playing music are separate steps.

## Using it

### Command line

```sh
streamer discover                       # 5 second search, table of servers and renderers
streamer discover --json                # one JSON object per line
streamer discover --watch               # keep running, print found / updated / lost
streamer discover --type renderer --timeout 10
streamer discover --interface wlp3s0    # only this network interface (name or IPv4 address)
streamer discover --type all            # every UPnP device and service, for diagnostics
streamer -vv discover                   # debug logging on stderr
```

Output goes to stdout and logs to stderr. The exit code is 0 when the search ran (even if nothing was found), 1 for errors such as "no usable network interface" and 2 for bad arguments.

### Rust library

See the module documentation of `streamer_core::ssdp` (`cargo doc --open`). In short:

```rust
let devices = discover_once(DiscoveryConfig::default(), Duration::from_secs(5)).await?;

let mut discovery = Discovery::start(DiscoveryConfig::default()).await?;
while let Some(event) = discovery.next().await { /* Found, Updated, Lost */ }
```

### Settings (`DiscoveryConfig`)

| Setting | Default | Meaning |
| --- | --- | --- |
| `targets` | media server, media renderer | What to search for. `All` is `ssdp:all`, `Custom` any search target. |
| `mx_secs` | 3 | How long devices may wait before answering (clamped to 1 to 5). |
| `search_repeat` | 3 | How often each search is sent per target and interface. |
| `search_interval_secs` | 60 | Seconds between periodic searches, 0 for a single search. |
| `listen` | true | Also listen for announcements on port 1900. |
| `interface_rescan_secs` | 30 | How often to look for changed network interfaces, 0 to turn it off. |
| `interfaces` | all suitable | Only these interfaces (names or IPv4 addresses). |
| `max_devices` | 512 | Upper bound on the number of devices kept. |

## What it does

- **Searches** are multicast to `239.255.255.250:1900` on every suitable interface, from a socket bound to that interface's address so devices answer on the right network. Each search is sent `search_repeat` times with random gaps of 50 to 250 ms (UDP loses packets), and the whole round again every `search_interval_secs`.
- **Announcements:** a socket shared on port 1900 joins the multicast group on every interface and collects `NOTIFY` messages, so devices that appear or leave between searches show up at once. Other programs can use port 1900 at the same time, and several instances of Streamer can run side by side. If the port cannot be opened, discovery continues with searches only.
- **Devices** are identified by their uuid. Many answers for one physical device (root device, embedded devices, services) become one device.
- **Events:** `Found` when a media server or renderer is seen for the first time, `Updated` when its location or address changes, `Lost` when it says goodbye or its announcement expires. Plain refreshes produce no event. Devices of other kinds are only reported when you search for them (`--type all`, or a custom target).
- **Expiry:** a device lives for the `max-age` it announced (default 30 minutes if missing; accepted range 5 seconds to 24 hours) and is removed when that time passes without a new announcement. A shorter announcement never shortens the lifetime a device already has.
- **Limits** against hostile or broken devices on the network: datagrams over 2048 bytes are dropped, at most 64 targets are remembered per device, at most `max_devices` devices are kept, and a slow consumer cannot stall discovery (the oldest `Updated` events are dropped first).

### Which interfaces are used

Without `--interface`, every interface that is up and has an IPv4 address is used, except:

- loopback (`127.0.0.0/8`) and link-local (`169.254.0.0/16`) addresses,
- point-to-point interfaces, such as VPN tunnels, which rarely carry multicast,
- virtual interfaces of container and virtual machine hosts, recognized by name: `docker*`, `br-<12 hex digits>` (Docker bridges), `veth*`, `virbr*`, `lxcbr*`, `lxdbr*`, `cni*`, `flannel*`, `podman*`, `vboxnet*`, `vmnet*` and `vEthernet*` (Hyper-V and WSL). Ordinary bridges such as `br0` are kept.

Naming interfaces with `--interface` overrides all of these exclusions. This is how you reach a media server that really lives in a virtual network. Interfaces are checked again every 30 seconds, so switching from Wi-Fi to Ethernet, or connecting to a different network, should be picked up without restarting.

## Platform notes

- **Windows:** the first search may trigger a firewall prompt. Allow private networks, otherwise answers from devices are blocked. A datagram larger than the buffer is reported as an error by Windows (and discarded); Streamer skips it.
- **macOS:** the terminal application needs the "Local Network" permission (System Settings, Privacy and Security). A native app needs the `NSLocalNetworkUsageDescription` key.
- **iOS** apps additionally need the multicast networking entitlement (`com.apple.developer.networking.multicast`), which Apple grants on request.
- **Android** apps must hold a `WifiManager.MulticastLock` while discovering, otherwise multicast packets are dropped.
- **Linux:** a firewall that drops inbound UDP can block the answers. Searches use random high ports, so allow UDP from the local network.

## Troubleshooting

**Nothing is found.**

1. Check that the device and your computer are on the same network. Guest networks, separate VLANs and many access points with "client isolation" block multicast.
2. Run `streamer -vv discover --timeout 10` and look at the interfaces it reports ("searching on interface ..."). If your real network is missing, name it with `--interface`.
3. Check firewalls (see the platform notes), and turn off a VPN to rule it out.
4. Look at the traffic itself: in Wireshark filter on `udp.port == 1900`. You should see `M-SEARCH` requests from this machine and `HTTP/1.1 200 OK` answers from the devices. `gssdp-device-sniffer` (GUPnP tools) is another independent view.
5. Search for everything with `streamer discover --type all`. If devices show up there but not as server or renderer, they are not DLNA/UPnP AV devices.

**A device appears twice.** It answered from two different uuids (some multi-function devices are a server and a renderer under separate uuids). That is expected, and the two entries share an address.

**A device does not disappear when switched off.** Many devices do not send a goodbye. They are removed when their announcement expires, which can take up to the `max-age` they announced (often 30 minutes).

## Manual test checklist

Run these on a real network with at least one renderer and one media server. Tick them off when you change discovery code or before a release. Automated tests cover the logic on loopback, but not real multicast or network changes.

- [ ] `streamer discover` lists every device you own that supports DLNA/UPnP AV, each once, with the right kind.
- [ ] The same devices appear with `--json`, and stdout holds nothing but JSON lines.
- [ ] `streamer discover --type all` shows more entries (services and embedded devices).
- [ ] `streamer discover --watch`: switch a device off and on. Expect `found` when it starts, `lost` when it says goodbye, or after its announcement expires if it just vanishes.
- [ ] While `--watch` runs, switch to another Wi-Fi network or from Wi-Fi to Ethernet. After at most about 30 seconds, devices on the new network are found. *(Not covered by any automated test.)*
- [ ] With a VPN connected, devices on the local network are still found, and nothing hangs.
- [ ] With Docker or a virtual machine running, discovery only searches on your real network (`-v` shows the interfaces).
- [ ] Two copies of `streamer discover --watch` at once both work (the port is shared).
- [ ] With another UPnP program running (a media player or server), both work.
- [ ] Windows, with the firewall on: the first run prompts for access, and devices are found after allowing it.
- [ ] macOS: devices are found from a terminal that has the Local Network permission.
- [ ] Compare the result with an independent tool (Wireshark, `gssdp-device-sniffer`) and check that no device is missing.
- [ ] `streamer discover --interface <name>` with a wrong name fails with exit code 1 and a clear message.

`mise run test-cli -- --live` (or `crates/cli/bin/test.sh --live`) runs the first item for you and asks you to confirm the result.
