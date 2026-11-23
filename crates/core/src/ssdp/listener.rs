//! Passive listening for `NOTIFY` announcements.
//!
//! Devices announce themselves (and say goodbye) by multicast on port 1900.
//! This module opens the shared socket and joins the group on each interface;
//! reading from it is done by [`receive_loop`](super::receive::receive_loop).

use std::io;
use std::net::Ipv4Addr;
use std::sync::Arc;

use tokio::net::UdpSocket;
use tracing::{debug, warn};

use super::socket::{bind_listener_socket, join_group};

/// An open listener socket and the interfaces it joined the group on.
#[derive(Debug)]
pub struct Listener {
    /// The shared socket on port 1900.
    pub socket: Arc<UdpSocket>,
    /// The interface addresses where joining the multicast group worked.
    pub joined: Vec<Ipv4Addr>,
}

/// Opens the listener socket and joins the multicast group on every interface in `interfaces`.
///
/// Failing to join on one interface is logged and skipped. If the group could
/// not be joined anywhere a warning is logged, but the socket is still
/// returned: the caller decides whether that is fatal. Binding port 1900
/// itself can fail (for example when another program holds it without
/// sharing); the caller should then continue with active searching only.
///
/// Must be called from within a tokio runtime.
pub fn open_listener(interfaces: &[Ipv4Addr]) -> io::Result<Listener> {
    let socket = bind_listener_socket()?;
    let mut joined = Vec::new();
    for &interface in interfaces {
        match join_group(&socket, interface) {
            Ok(()) => joined.push(interface),
            Err(error) => debug!(%interface, %error, "joining the multicast group failed"),
        }
    }
    if joined.is_empty() && !interfaces.is_empty() {
        warn!("could not join the SSDP multicast group on any interface");
    }
    Ok(Listener {
        socket: Arc::new(socket),
        joined,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ssdp::socket::list_interfaces;

    #[tokio::test]
    async fn opening_without_interfaces_binds_the_port_and_joins_nothing() {
        match open_listener(&[]) {
            Ok(listener) => {
                assert!(listener.joined.is_empty());
                assert_eq!(listener.socket.local_addr().unwrap().port(), 1900);
            }
            Err(error) if error.kind() == io::ErrorKind::AddrInUse => {
                eprintln!("skipped: port 1900 is used by a program that does not share it");
            }
            Err(error) => panic!("opening the listener failed: {error}"),
        }
    }

    #[tokio::test]
    async fn joined_interfaces_are_reported() {
        let addresses: Vec<Ipv4Addr> = list_interfaces(None)
            .unwrap()
            .into_iter()
            .map(|interface| interface.addr)
            .collect();
        if addresses.is_empty() {
            eprintln!("skipped: this machine has no usable network interface");
            return;
        }
        let listener = match open_listener(&addresses) {
            Ok(listener) => listener,
            Err(error) if error.kind() == io::ErrorKind::AddrInUse => {
                eprintln!("skipped: port 1900 is used by a program that does not share it");
                return;
            }
            Err(error) => panic!("opening the listener failed: {error}"),
        };
        // every joined address is one we asked for (joining may be refused in sandboxes)
        assert!(listener.joined.iter().all(|a| addresses.contains(a)));
    }

    #[tokio::test]
    async fn an_unusable_interface_is_skipped_not_fatal() {
        // 192.0.2.0/24 is reserved for documentation and is not on this machine
        let bogus = [Ipv4Addr::new(192, 0, 2, 254)];
        match open_listener(&bogus) {
            Ok(listener) => assert!(listener.joined.is_empty()),
            Err(error) if error.kind() == io::ErrorKind::AddrInUse => {}
            Err(error) => panic!("opening the listener failed: {error}"),
        }
    }
}
