//! The public discovery API: [`Discovery`] and [`discover_once`].
//!
//! A [`Discovery`] runs one background task that owns the device cache. For
//! every network interface it starts a search socket (sending periodic
//! `M-SEARCH`es and reading the answers), optionally a shared listener for
//! `NOTIFY` announcements, and a timer that expires old devices and rescans
//! the network interfaces. Dropping the `Discovery` stops everything.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use tokio::net::UdpSocket;
use tokio::sync::{Notify, mpsc};
use tokio::task::JoinHandle;
use tokio::time::MissedTickBehavior;
use tracing::{debug, info, warn};

use super::cache::DeviceCache;
use super::error::SsdpError;
use super::listener::open_listener;
use super::receive::{Received, receive_loop};
use super::search::{Timings, build_requests, search_loop};
use super::socket::{
    bind_search_socket, join_group, leave_group, list_interfaces, multicast_target,
};
use super::types::{Device, DiscoveryConfig, DiscoveryEvent, SearchTarget};

/// Events waiting for a slow consumer.
const EVENT_QUEUE_CAPACITY: usize = 256;
/// Messages waiting between the network tasks and the cache task.
const RECEIVE_QUEUE_CAPACITY: usize = 1024;

/// Low-level settings, only meant for tests and special setups.
#[doc(hidden)]
#[derive(Debug, Clone)]
pub struct Tuning {
    /// Where searches are sent (normally the SSDP multicast group).
    pub target: SocketAddr,
    /// The random gaps used while sending.
    pub timings: Timings,
}

impl Default for Tuning {
    fn default() -> Self {
        Tuning {
            target: multicast_target(),
            timings: Timings::default(),
        }
    }
}

/// A running discovery. Dropping it stops all background work.
#[derive(Debug)]
pub struct Discovery {
    queue: Arc<EventQueue>,
    manager: Option<JoinHandle<()>>,
}

impl Discovery {
    /// Starts discovery on the local network.
    ///
    /// Fails with [`SsdpError::InvalidConfig`] for a bad configuration, with
    /// [`SsdpError::NoUsableInterface`] if there is no interface to search on,
    /// and with an I/O error if no search socket could be opened. A failing
    /// listener (for example because port 1900 is taken) is not an error:
    /// discovery then works with searches only.
    ///
    /// Must be called from within a tokio runtime.
    pub async fn start(config: DiscoveryConfig) -> Result<Discovery, SsdpError> {
        Discovery::start_tuned(config, Tuning::default()).await
    }

    /// Like [`start`](Self::start), with low-level settings.
    #[doc(hidden)]
    pub async fn start_tuned(
        config: DiscoveryConfig,
        tuning: Tuning,
    ) -> Result<Discovery, SsdpError> {
        validate(&config)?;
        let requests = build_requests(&config.targets, config.mx_secs)?;
        let interfaces = list_interfaces(config.interfaces.as_deref())?;
        if interfaces.is_empty() {
            return Err(SsdpError::NoUsableInterface);
        }

        let queue = Arc::new(EventQueue::new(EVENT_QUEUE_CAPACITY));
        let (input_tx, input_rx) = mpsc::channel(RECEIVE_QUEUE_CAPACITY);
        let report_other = config
            .targets
            .iter()
            .any(|target| matches!(target, SearchTarget::All | SearchTarget::Custom(_)));
        let interval = (config.search_interval_secs > 0)
            .then(|| Duration::from_secs(config.search_interval_secs));
        let mut manager = Manager {
            cache: DeviceCache::new(config.max_devices, report_other),
            requests,
            interval,
            tuning,
            queue: queue.clone(),
            input: input_tx,
            interfaces: HashMap::new(),
            listener: None,
            config,
        };

        let mut first_error: Option<io::Error> = None;
        for interface in &interfaces {
            if let Err(error) = manager.add_interface(interface.addr) {
                warn!(interface = %interface.addr, %error, "cannot search on this interface");
                first_error.get_or_insert(error);
            }
        }
        if manager.interfaces.is_empty() {
            return Err(first_error.map_or(SsdpError::NoUsableInterface, SsdpError::Io));
        }
        if manager.config.listen {
            manager.open_listener();
        }

        let handle = tokio::spawn(manager.run(input_rx));
        Ok(Discovery {
            queue,
            manager: Some(handle),
        })
    }

    /// Waits for the next event. Returns `None` once discovery has stopped and
    /// all remaining events were delivered.
    ///
    /// If the consumer is slower than the network, the oldest `Updated` events
    /// are dropped first, and `Found` and `Lost` events only as a last resort.
    pub async fn next(&mut self) -> Option<DiscoveryEvent> {
        self.queue.pop().await
    }

    /// Stops discovery and waits until its background task is gone.
    pub async fn shutdown(mut self) {
        self.queue.close();
        if let Some(handle) = self.manager.take() {
            handle.abort();
            let _ = handle.await;
        }
    }
}

impl Drop for Discovery {
    fn drop(&mut self) {
        self.queue.close();
        if let Some(handle) = self.manager.take() {
            handle.abort();
        }
    }
}

/// Searches for `timeout` and returns the devices that answered, ordered by uuid.
///
/// Must be called from within a tokio runtime.
pub async fn discover_once(
    config: DiscoveryConfig,
    timeout: Duration,
) -> Result<Vec<Device>, SsdpError> {
    discover_once_tuned(config, timeout, Tuning::default()).await
}

/// Like [`discover_once`], with low-level settings.
#[doc(hidden)]
pub async fn discover_once_tuned(
    config: DiscoveryConfig,
    timeout: Duration,
    tuning: Tuning,
) -> Result<Vec<Device>, SsdpError> {
    let mut discovery = Discovery::start_tuned(config, tuning).await?;
    let deadline = tokio::time::Instant::now() + timeout;
    let mut found: BTreeMap<String, Device> = BTreeMap::new();
    while let Ok(Some(event)) = tokio::time::timeout_at(deadline, discovery.next()).await {
        match event {
            DiscoveryEvent::Found(device) | DiscoveryEvent::Updated(device) => {
                found.insert(device.uuid.clone(), device);
            }
            DiscoveryEvent::Lost { uuid } => {
                found.remove(&uuid);
            }
        }
    }
    discovery.shutdown().await;
    Ok(found.into_values().collect())
}

fn validate(config: &DiscoveryConfig) -> Result<(), SsdpError> {
    if config.targets.is_empty() {
        return Err(SsdpError::InvalidConfig(
            "at least one search target is needed".to_string(),
        ));
    }
    if config.max_devices == 0 {
        return Err(SsdpError::InvalidConfig(
            "max_devices must be at least 1".to_string(),
        ));
    }
    if config.interfaces.as_ref().is_some_and(Vec::is_empty) {
        return Err(SsdpError::InvalidConfig(
            "the interface list is empty; use None to search on all interfaces".to_string(),
        ));
    }
    Ok(())
}

/// Aborts its task when dropped.
#[derive(Debug)]
struct TaskGuard(JoinHandle<()>);

impl Drop for TaskGuard {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// The tasks and socket of one interface.
struct InterfaceTasks {
    _socket: Arc<UdpSocket>,
    _receive: TaskGuard,
    _search: TaskGuard,
}

/// The shared `NOTIFY` listener.
struct ListenerState {
    socket: Arc<UdpSocket>,
    joined: HashSet<Ipv4Addr>,
    _receive: TaskGuard,
}

/// Owns everything that changes while discovery runs.
struct Manager {
    config: DiscoveryConfig,
    tuning: Tuning,
    requests: Vec<String>,
    interval: Option<Duration>,
    cache: DeviceCache,
    queue: Arc<EventQueue>,
    input: mpsc::Sender<Received>,
    interfaces: HashMap<Ipv4Addr, InterfaceTasks>,
    listener: Option<ListenerState>,
}

impl Manager {
    /// Starts searching on one interface.
    fn add_interface(&mut self, addr: Ipv4Addr) -> io::Result<()> {
        let socket = Arc::new(bind_search_socket(addr)?);
        let receive = spawn_receiver(socket.clone(), self.input.clone());
        let search = tokio::spawn(search_loop(
            socket.clone(),
            self.tuning.target,
            self.requests.clone(),
            self.config.search_repeat,
            self.interval,
            self.tuning.timings.clone(),
        ));
        self.interfaces.insert(
            addr,
            InterfaceTasks {
                _socket: socket,
                _receive: receive,
                _search: TaskGuard(search),
            },
        );
        info!(interface = %addr, "searching on interface");
        Ok(())
    }

    /// Stops using an interface that went away.
    fn remove_interface(&mut self, addr: Ipv4Addr) {
        if self.interfaces.remove(&addr).is_some() {
            info!(interface = %addr, "interface is gone, no longer searching on it");
        }
        if let Some(listener) = &mut self.listener
            && listener.joined.remove(&addr)
        {
            // the address may be gone, so failing here is expected
            let _ = leave_group(&listener.socket, addr);
        }
    }

    /// Opens the shared listener on the interfaces currently in use.
    fn open_listener(&mut self) {
        let addrs: Vec<Ipv4Addr> = self.interfaces.keys().copied().collect();
        match open_listener(&addrs) {
            Ok(listener) => {
                let receive = spawn_receiver(listener.socket.clone(), self.input.clone());
                self.listener = Some(ListenerState {
                    socket: listener.socket,
                    joined: listener.joined.into_iter().collect(),
                    _receive: receive,
                });
            }
            Err(error) => {
                warn!(%error, "cannot listen for announcements, using searches only");
            }
        }
    }

    /// Looks at the network interfaces again and adapts.
    fn rescan(&mut self) {
        let wanted = match list_interfaces(self.config.interfaces.as_deref()) {
            Ok(list) => list,
            Err(error) => {
                debug!(%error, "listing interfaces failed, keeping the current ones");
                return;
            }
        };
        let wanted_addrs: HashSet<Ipv4Addr> = wanted.iter().map(|i| i.addr).collect();

        let gone: Vec<Ipv4Addr> = self
            .interfaces
            .keys()
            .filter(|addr| !wanted_addrs.contains(addr))
            .copied()
            .collect();
        for addr in gone {
            self.remove_interface(addr);
        }

        for interface in wanted {
            if !self.interfaces.contains_key(&interface.addr)
                && let Err(error) = self.add_interface(interface.addr)
            {
                debug!(interface = %interface.addr, %error, "cannot search on this interface");
            }
            if let Some(listener) = &mut self.listener
                && self.interfaces.contains_key(&interface.addr)
                && !listener.joined.contains(&interface.addr)
                && join_group(&listener.socket, interface.addr).is_ok()
            {
                listener.joined.insert(interface.addr);
            }
        }
    }

    fn publish(&self, events: Vec<DiscoveryEvent>) {
        for event in events {
            self.queue.push(event);
        }
    }

    async fn run(mut self, mut input: mpsc::Receiver<Received>) {
        let period = Duration::from_secs(self.config.interface_rescan_secs.max(1));
        let rescan_enabled = self.config.interface_rescan_secs > 0;
        let mut rescan = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
        rescan.set_missed_tick_behavior(MissedTickBehavior::Delay);

        loop {
            let deadline = self.cache.next_expiry().map(tokio::time::Instant::from_std);
            tokio::select! {
                received = input.recv() => {
                    let Some(received) = received else { break };
                    let events = self.cache.handle(&received.message, received.from, Instant::now());
                    self.publish(events);
                }
                () = wait_until(deadline) => {
                    let events = self.cache.expire(Instant::now());
                    self.publish(events);
                }
                _ = rescan.tick(), if rescan_enabled => self.rescan(),
            }
        }
        self.queue.close();
    }
}

fn spawn_receiver(socket: Arc<UdpSocket>, output: mpsc::Sender<Received>) -> TaskGuard {
    TaskGuard(tokio::spawn(async move {
        if let Err(error) = receive_loop(socket, output).await {
            warn!(%error, "receiving stopped");
        }
    }))
}

async fn wait_until(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending::<()>().await,
    }
}

/// A bounded queue of events with a policy for overflow.
#[derive(Debug)]
struct EventQueue {
    state: Mutex<QueueState>,
    notify: Notify,
    capacity: usize,
}

#[derive(Debug, Default)]
struct QueueState {
    events: VecDeque<DiscoveryEvent>,
    closed: bool,
}

impl EventQueue {
    fn new(capacity: usize) -> EventQueue {
        EventQueue {
            state: Mutex::new(QueueState::default()),
            notify: Notify::new(),
            capacity,
        }
    }

    fn lock(&self) -> MutexGuard<'_, QueueState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Adds an event. When the queue is full the oldest `Updated` event makes
    /// room; with none left, a new `Updated` is dropped, and anything else
    /// replaces the oldest event.
    fn push(&self, event: DiscoveryEvent) {
        {
            let mut state = self.lock();
            if state.closed {
                return;
            }
            if state.events.len() >= self.capacity {
                let oldest_update = state
                    .events
                    .iter()
                    .position(|e| matches!(e, DiscoveryEvent::Updated(_)));
                if let Some(index) = oldest_update {
                    state.events.remove(index);
                } else if matches!(event, DiscoveryEvent::Updated(_)) {
                    return;
                } else {
                    warn!("event queue is full, dropping the oldest event");
                    state.events.pop_front();
                }
            }
            state.events.push_back(event);
        }
        self.notify.notify_one();
    }

    async fn pop(&self) -> Option<DiscoveryEvent> {
        loop {
            {
                let mut state = self.lock();
                if let Some(event) = state.events.pop_front() {
                    return Some(event);
                }
                if state.closed {
                    return None;
                }
            }
            self.notify.notified().await;
        }
    }

    fn close(&self) {
        self.lock().closed = true;
        self.notify.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ssdp::types::DeviceKind;

    fn device(uuid: &str) -> Device {
        Device {
            uuid: uuid.to_string(),
            location: "http://192.0.2.1/d.xml".to_string(),
            kind: DeviceKind::MediaRenderer,
            server: None,
            address: "192.0.2.1".to_string(),
            expires_in_secs: 100,
        }
    }

    fn found(uuid: &str) -> DiscoveryEvent {
        DiscoveryEvent::Found(device(uuid))
    }

    fn updated(uuid: &str) -> DiscoveryEvent {
        DiscoveryEvent::Updated(device(uuid))
    }

    fn lost(uuid: &str) -> DiscoveryEvent {
        DiscoveryEvent::Lost {
            uuid: uuid.to_string(),
        }
    }

    async fn drain(queue: &EventQueue) -> Vec<DiscoveryEvent> {
        queue.close();
        let mut events = Vec::new();
        while let Some(event) = queue.pop().await {
            events.push(event);
        }
        events
    }

    #[tokio::test]
    async fn events_come_out_in_order() {
        let queue = EventQueue::new(10);
        queue.push(found("a"));
        queue.push(updated("a"));
        queue.push(lost("a"));
        assert_eq!(
            drain(&queue).await,
            vec![found("a"), updated("a"), lost("a")]
        );
    }

    #[tokio::test]
    async fn a_full_queue_drops_the_oldest_update_first() {
        let queue = EventQueue::new(3);
        queue.push(found("a"));
        queue.push(updated("a"));
        queue.push(updated("b"));
        queue.push(found("c")); // full: the oldest Updated (a) makes room
        assert_eq!(
            drain(&queue).await,
            vec![found("a"), updated("b"), found("c")]
        );
    }

    #[tokio::test]
    async fn a_full_queue_without_updates_drops_a_new_update() {
        let queue = EventQueue::new(2);
        queue.push(found("a"));
        queue.push(found("b"));
        queue.push(updated("a")); // nothing to make room with: dropped
        assert_eq!(drain(&queue).await, vec![found("a"), found("b")]);
    }

    #[tokio::test]
    async fn a_full_queue_without_updates_drops_the_oldest_for_found_and_lost() {
        let queue = EventQueue::new(2);
        queue.push(found("a"));
        queue.push(found("b"));
        queue.push(lost("a")); // last resort: the oldest event goes
        assert_eq!(drain(&queue).await, vec![found("b"), lost("a")]);
    }

    #[tokio::test]
    async fn popping_waits_for_a_push_and_ends_when_closed() {
        let queue = Arc::new(EventQueue::new(4));
        let waiter = {
            let queue = queue.clone();
            tokio::spawn(async move { (queue.pop().await, queue.pop().await) })
        };
        tokio::time::sleep(Duration::from_millis(20)).await;
        queue.push(found("a"));
        tokio::time::sleep(Duration::from_millis(20)).await;
        queue.close();
        let (first, second) = waiter.await.unwrap();
        assert_eq!(first, Some(found("a")));
        assert_eq!(second, None);
    }

    #[tokio::test]
    async fn nothing_is_accepted_after_closing_but_queued_events_are_kept() {
        let queue = EventQueue::new(4);
        queue.push(found("a"));
        queue.close();
        queue.push(found("b"));
        assert_eq!(queue.pop().await, Some(found("a")));
        assert_eq!(queue.pop().await, None);
    }

    #[test]
    fn configuration_is_validated() {
        let ok = DiscoveryConfig::default();
        assert!(validate(&ok).is_ok());

        let no_targets = DiscoveryConfig {
            targets: vec![],
            ..DiscoveryConfig::default()
        };
        assert!(matches!(
            validate(&no_targets),
            Err(SsdpError::InvalidConfig(_))
        ));

        let no_room = DiscoveryConfig {
            max_devices: 0,
            ..DiscoveryConfig::default()
        };
        assert!(matches!(
            validate(&no_room),
            Err(SsdpError::InvalidConfig(_))
        ));

        let empty_filter = DiscoveryConfig {
            interfaces: Some(vec![]),
            ..DiscoveryConfig::default()
        };
        assert!(matches!(
            validate(&empty_filter),
            Err(SsdpError::InvalidConfig(_))
        ));
    }
}
