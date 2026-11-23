//! Receiving SSDP messages from a socket.
//!
//! The same loop serves the search sockets (unicast answers) and the shared
//! listener socket (multicast `NOTIFY` announcements).

use std::io;
use std::net::IpAddr;
use std::sync::Arc;

use tokio::net::UdpSocket;
use tokio::sync::mpsc::{Sender, error::TrySendError};
use tracing::debug;

use super::message::{Message, parse};
use super::socket::recv_datagram;

/// A parsed message together with the address it came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Received {
    /// The message.
    pub message: Message,
    /// The sender's IP address.
    pub from: IpAddr,
}

/// Reads datagrams from `socket`, parses them and sends the interesting ones to `output`.
///
/// - Malformed packets and other control points' searches are dropped.
/// - If `output` is full, the packet is dropped instead of waiting, so a slow
///   consumer can never stall the network side.
/// - Errors that only say "an earlier packet of ours was unreachable" (which
///   Windows reports on the receiving side of a UDP socket) are ignored, and
///   so is an oversized datagram that Windows reports as an error.
///
/// Returns `Ok(())` once `output` is closed, and an error for a failure of the
/// socket itself. Stop the loop from outside by aborting the task or dropping it.
pub async fn receive_loop(socket: Arc<UdpSocket>, output: Sender<Received>) -> io::Result<()> {
    loop {
        let (data, from) = match recv_datagram(&socket).await {
            Ok(received) => received,
            Err(error) if is_transient(&error) => {
                debug!(%error, "ignoring a transient receive error");
                tokio::task::yield_now().await;
                continue;
            }
            Err(error) => return Err(error),
        };

        let message = match parse(&data) {
            Ok(Some(message)) => message,
            Ok(None) => continue,
            Err(error) => {
                debug!(%from, %error, "dropping an unusable packet");
                continue;
            }
        };

        match output.try_send(Received {
            message,
            from: from.ip(),
        }) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => debug!(%from, "output is full, dropping a message"),
            Err(TrySendError::Closed(_)) => return Ok(()),
        }
    }
}

fn is_transient(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionRefused
            | io::ErrorKind::InvalidData
            | io::ErrorKind::Interrupted
            | io::ErrorKind::WouldBlock
    )
}
