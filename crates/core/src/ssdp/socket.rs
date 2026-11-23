//! Network interfaces and the UDP sockets SSDP needs.
//!
//! Searching uses one socket per interface, bound to that interface's address,
//! so devices answer on the right network. Listening for `NOTIFY` messages uses
//! a single shared socket on port 1900 that joins the multicast group on every
//! interface. Only IPv4 is implemented.

use std::io;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

use if_addrs::{IfAddr, IfOperStatus};
use socket2::{Domain, Protocol, Socket, Type};
use tokio::net::UdpSocket;

use super::error::SsdpError;
use super::message::{MAX_DATAGRAM, MULTICAST_ADDR_V4, SSDP_PORT};

/// Multicast hop limit for searches (UPnP 1.1 says 2).
const MULTICAST_TTL: u32 = 2;

/// Receive buffer size: one byte more than the largest accepted datagram, so
/// that an oversized datagram is visible to the parser as too large.
pub const RECV_BUFFER_SIZE: usize = MAX_DATAGRAM + 1;

/// A network interface address SSDP can use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkInterface {
    /// The interface name, for example `eth0` or `Wi-Fi`.
    pub name: String,
    /// Its IPv4 address.
    pub addr: Ipv4Addr,
}

/// What we know about one interface address, before choosing.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Candidate {
    name: String,
    addr: Ipv4Addr,
    loopback: bool,
    point_to_point: bool,
    /// False for interfaces that are down or not present.
    up: bool,
}

/// The multicast address and port searches are sent to.
pub fn multicast_target() -> SocketAddr {
    SocketAddr::V4(SocketAddrV4::new(MULTICAST_ADDR_V4, SSDP_PORT))
}

/// Lists the interfaces to use for discovery.
///
/// Without a `filter`, this is every interface that is up and has an IPv4
/// address, except loopback, link-local (`169.254.0.0/16`) and point-to-point
/// ones (VPN tunnels rarely carry multicast). Each interface appears once.
///
/// With a `filter` (interface names or IPv4 addresses, case-insensitive) only
/// matching interfaces are returned, and the loopback, link-local and
/// point-to-point exclusions do not apply to them.
///
/// The result may be empty, for example while a laptop is offline.
pub fn list_interfaces(filter: Option<&[String]>) -> Result<Vec<NetworkInterface>, SsdpError> {
    let candidates: Vec<Candidate> = if_addrs::get_if_addrs()?
        .into_iter()
        .filter_map(|interface| {
            let IfAddr::V4(v4) = &interface.addr else {
                return None;
            };
            Some(Candidate {
                name: interface.name.clone(),
                addr: v4.ip,
                loopback: interface.is_loopback(),
                point_to_point: interface.is_p2p(),
                up: matches!(
                    interface.oper_status,
                    IfOperStatus::Up | IfOperStatus::Unknown
                ),
            })
        })
        .collect();
    Ok(select_interfaces(&candidates, filter))
}

fn select_interfaces(candidates: &[Candidate], filter: Option<&[String]>) -> Vec<NetworkInterface> {
    let mut chosen: Vec<NetworkInterface> = Vec::new();
    for candidate in candidates {
        if !candidate.up {
            continue;
        }
        let wanted = match filter {
            Some(list) => list.iter().any(|entry| {
                let entry = entry.trim();
                entry.eq_ignore_ascii_case(&candidate.name) || entry == candidate.addr.to_string()
            }),
            None => {
                !candidate.loopback
                    && !candidate.point_to_point
                    && !candidate.addr.is_link_local()
                    && !candidate.addr.is_unspecified()
            }
        };
        if !wanted {
            continue;
        }
        let duplicate = chosen.iter().any(|known| match filter {
            // asked for by address: every matching address counts
            Some(_) => known.addr == candidate.addr,
            // otherwise one address per interface is enough
            None => known.name == candidate.name,
        });
        if !duplicate {
            chosen.push(NetworkInterface {
                name: candidate.name.clone(),
                addr: candidate.addr,
            });
        }
    }
    chosen
}

/// Creates the socket used to search on one interface.
///
/// It is bound to the interface's address on an ephemeral port, sends
/// multicast through that interface only, and does not receive its own packets.
/// Must be called from within a tokio runtime.
pub fn bind_search_socket(interface: Ipv4Addr) -> io::Result<UdpSocket> {
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    socket.bind(&SocketAddr::V4(SocketAddrV4::new(interface, 0)).into())?;
    socket.set_multicast_if_v4(&interface)?;
    socket.set_multicast_ttl_v4(MULTICAST_TTL)?;
    socket.set_multicast_loop_v4(false)?;
    into_tokio(socket)
}

/// Creates the socket used to listen for `NOTIFY` messages.
///
/// It is bound to port 1900 on all addresses. Other programs (media servers,
/// other control points) listen there too, so the address and port are
/// shared. Join the multicast group on each interface with [`join_group`].
/// Must be called from within a tokio runtime.
pub fn bind_listener_socket() -> io::Result<UdpSocket> {
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_reuse_address(true)?;
    #[cfg(all(
        unix,
        not(any(target_os = "solaris", target_os = "illumos", target_os = "cygwin"))
    ))]
    socket.set_reuse_port(true)?;
    socket.bind(&SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, SSDP_PORT)).into())?;
    into_tokio(socket)
}

/// Joins the SSDP multicast group on one interface.
pub fn join_group(socket: &UdpSocket, interface: Ipv4Addr) -> io::Result<()> {
    socket.join_multicast_v4(MULTICAST_ADDR_V4, interface)
}

/// Leaves the SSDP multicast group on one interface.
pub fn leave_group(socket: &UdpSocket, interface: Ipv4Addr) -> io::Result<()> {
    socket.leave_multicast_v4(MULTICAST_ADDR_V4, interface)
}

/// Receives one datagram and returns its bytes and the sender.
///
/// A datagram larger than the accepted maximum is cut off. On Linux and macOS
/// it comes back one byte over the limit, which [`super::message::parse`]
/// reports as too large. Windows reports an error instead and discards the
/// datagram; that case is turned into an [`io::ErrorKind::InvalidData`] error,
/// which callers should treat as "skip this packet".
pub async fn recv_datagram(socket: &UdpSocket) -> io::Result<(Vec<u8>, SocketAddr)> {
    let mut buffer = [0u8; RECV_BUFFER_SIZE];
    match socket.recv_from(&mut buffer).await {
        Ok((length, from)) => Ok((buffer[..length].to_vec(), from)),
        Err(error) if is_message_too_long(&error) => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "datagram too large",
        )),
        Err(error) => Err(error),
    }
}

/// Windows' `WSAEMSGSIZE`: the datagram did not fit the receive buffer.
fn is_message_too_long(error: &io::Error) -> bool {
    const WSAEMSGSIZE: i32 = 10040;
    cfg!(windows) && error.raw_os_error() == Some(WSAEMSGSIZE)
}

fn into_tokio(socket: Socket) -> io::Result<UdpSocket> {
    socket.set_nonblocking(true)?;
    UdpSocket::from_std(socket.into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ssdp::message::{Message, parse};

    fn candidate(name: &str, addr: [u8; 4]) -> Candidate {
        Candidate {
            name: name.to_string(),
            addr: Ipv4Addr::from(addr),
            loopback: false,
            point_to_point: false,
            up: true,
        }
    }

    fn names(interfaces: &[NetworkInterface]) -> Vec<&str> {
        interfaces.iter().map(|i| i.name.as_str()).collect()
    }

    #[test]
    fn default_selection_skips_unsuitable_interfaces() {
        let mut lo = candidate("lo", [127, 0, 0, 1]);
        lo.loopback = true;
        let mut tun = candidate("tun0", [10, 8, 0, 2]);
        tun.point_to_point = true;
        let mut down = candidate("eth1", [192, 0, 2, 9]);
        down.up = false;
        let candidates = [
            lo,
            candidate("eth0", [192, 0, 2, 10]),
            candidate("wlan0", [198, 51, 100, 4]),
            candidate("eth2", [169, 254, 3, 4]),
            tun,
            down,
        ];
        let chosen = select_interfaces(&candidates, None);
        assert_eq!(names(&chosen), ["eth0", "wlan0"]);
        assert_eq!(chosen[0].addr, Ipv4Addr::new(192, 0, 2, 10));
    }

    #[test]
    fn default_selection_uses_one_address_per_interface() {
        let candidates = [
            candidate("eth0", [192, 0, 2, 10]),
            candidate("eth0", [192, 0, 2, 11]),
        ];
        let chosen = select_interfaces(&candidates, None);
        assert_eq!(chosen.len(), 1);
        assert_eq!(chosen[0].addr, Ipv4Addr::new(192, 0, 2, 10));
    }

    #[test]
    fn a_filter_matches_names_and_addresses() {
        let candidates = [
            candidate("eth0", [192, 0, 2, 10]),
            candidate("WiFi", [198, 51, 100, 4]),
            candidate("docker0", [172, 17, 0, 1]),
        ];
        let by_name = select_interfaces(&candidates, Some(&["wifi".to_string()]));
        assert_eq!(names(&by_name), ["WiFi"]);

        let by_address = select_interfaces(&candidates, Some(&["192.0.2.10".to_string()]));
        assert_eq!(names(&by_address), ["eth0"]);

        let both = select_interfaces(
            &candidates,
            Some(&["eth0".to_string(), " 172.17.0.1 ".to_string()]),
        );
        assert_eq!(names(&both), ["eth0", "docker0"]);

        assert!(select_interfaces(&candidates, Some(&["nothing".to_string()])).is_empty());
    }

    #[test]
    fn a_filter_allows_loopback_and_link_local_but_never_down_interfaces() {
        let mut lo = candidate("lo", [127, 0, 0, 1]);
        lo.loopback = true;
        let mut down = candidate("eth1", [192, 0, 2, 9]);
        down.up = false;
        let candidates = [lo, candidate("eth2", [169, 254, 3, 4]), down];
        let filter = ["lo".to_string(), "eth2".to_string(), "eth1".to_string()];
        let chosen = select_interfaces(&candidates, Some(&filter));
        assert_eq!(names(&chosen), ["lo", "eth2"]);
    }

    #[test]
    fn an_address_filter_keeps_every_matching_address_of_an_interface() {
        let candidates = [
            candidate("eth0", [192, 0, 2, 10]),
            candidate("eth0", [192, 0, 2, 11]),
        ];
        let filter = ["192.0.2.10".to_string(), "192.0.2.11".to_string()];
        assert_eq!(select_interfaces(&candidates, Some(&filter)).len(), 2);
    }

    #[test]
    fn no_candidates_gives_no_interfaces() {
        assert!(select_interfaces(&[], None).is_empty());
        assert!(select_interfaces(&[], Some(&["eth0".to_string()])).is_empty());
    }

    #[test]
    fn listing_the_real_interfaces_works() {
        // The result depends on the machine; it must not fail or contain
        // loopback addresses.
        let interfaces = list_interfaces(None).expect("listing interfaces works");
        assert!(interfaces.iter().all(|i| !i.addr.is_loopback()));
        assert!(
            interfaces
                .iter()
                .all(|i| !i.addr.is_link_local() && !i.addr.is_unspecified())
        );
    }

    #[test]
    fn the_multicast_target_is_the_ssdp_group() {
        assert_eq!(multicast_target().to_string(), "239.255.255.250:1900");
    }

    #[tokio::test]
    async fn a_search_socket_binds_to_its_interface_address() {
        let Some(interface) = list_interfaces(None).unwrap().into_iter().next() else {
            eprintln!("skipped: this machine has no usable network interface");
            return;
        };
        let socket = bind_search_socket(interface.addr).expect("search socket");
        let local = socket.local_addr().unwrap();
        assert_eq!(local.ip(), std::net::IpAddr::V4(interface.addr));
        assert_ne!(local.port(), 0);
        assert_ne!(local.port(), SSDP_PORT);
    }

    #[tokio::test]
    async fn the_listener_port_can_be_shared() {
        let first = match bind_listener_socket() {
            Ok(socket) => socket,
            Err(error) if error.kind() == io::ErrorKind::AddrInUse => {
                eprintln!("skipped: port 1900 is used by a program that does not share it");
                return;
            }
            Err(error) => panic!("binding the listener failed: {error}"),
        };
        assert_eq!(first.local_addr().unwrap().port(), SSDP_PORT);
        // a second instance (another process in real life) can bind as well
        let second = bind_listener_socket().expect("the port is shared");
        assert_eq!(second.local_addr().unwrap().port(), SSDP_PORT);
    }

    #[tokio::test]
    async fn the_multicast_group_can_be_joined_and_left() {
        let Some(interface) = list_interfaces(None).unwrap().into_iter().next() else {
            eprintln!("skipped: this machine has no usable network interface");
            return;
        };
        let socket = match bind_listener_socket() {
            Ok(socket) => socket,
            Err(error) if error.kind() == io::ErrorKind::AddrInUse => {
                eprintln!("skipped: port 1900 is used by a program that does not share it");
                return;
            }
            Err(error) => panic!("binding the listener failed: {error}"),
        };
        // Some sandboxes and containers do not allow multicast; that is not a bug here.
        if let Err(error) = join_group(&socket, interface.addr) {
            eprintln!("skipped: joining the multicast group is not possible here: {error}");
            return;
        }
        leave_group(&socket, interface.addr).expect("leaving a joined group works");
    }

    #[tokio::test]
    async fn datagrams_are_received_with_their_sender() {
        let receiver = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let sender = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let packet = b"HTTP/1.1 200 OK\r\nST: upnp:rootdevice\r\nUSN: uuid:abc\r\n\
                       LOCATION: http://192.0.2.5/d.xml\r\n\r\n";
        sender
            .send_to(packet, receiver.local_addr().unwrap())
            .await
            .unwrap();

        let (data, from) = recv_datagram(&receiver).await.unwrap();
        assert_eq!(from, sender.local_addr().unwrap());
        assert!(matches!(
            parse(&data).unwrap(),
            Some(Message::SearchResponse(_))
        ));
    }

    #[tokio::test]
    async fn an_oversized_datagram_is_seen_as_too_large() {
        let receiver = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let sender = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        sender
            .send_to(
                &vec![b'a'; MAX_DATAGRAM + 500],
                receiver.local_addr().unwrap(),
            )
            .await
            .unwrap();
        let result = recv_datagram(&receiver).await;
        if cfg!(windows) {
            // Windows reports an error and drops the datagram
            let error = result.expect_err("Windows reports oversized datagrams as an error");
            assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        } else {
            let (data, _) = result.unwrap();
            assert_eq!(data.len(), RECV_BUFFER_SIZE);
            assert!(parse(&data).is_err());
        }
    }
}
