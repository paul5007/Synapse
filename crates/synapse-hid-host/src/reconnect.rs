use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

use tracing::{debug, warn};

use crate::discover::connect_auto;
use crate::error::{HidError, HidResult};
use crate::pipeline::HostCommandRequest;
use crate::transport::HidGateway;

pub const RECONNECT_INTERVAL_MS: u64 = 500;

pub trait ReconnectLink: Send + 'static {
    /// Sends one command through this connected link.
    ///
    /// # Errors
    ///
    /// Returns the underlying HID transport/protocol error when the command is
    /// not accepted by the firmware or the serial link fails.
    fn send_command(&mut self, command: u8, payload: &[u8]) -> HidResult<u32>;

    /// Sends a batch of commands through this connected link.
    ///
    /// # Errors
    ///
    /// Returns the underlying HID transport/protocol error when any command is
    /// not accepted by the firmware or the serial link fails.
    fn send_commands(&mut self, commands: &[HostCommandRequest<'_>]) -> HidResult<Vec<u32>>;

    /// Returns a stable label for readbacks/logging, normally the serial port name.
    fn link_label(&self) -> String;
}

impl ReconnectLink for HidGateway {
    fn send_command(&mut self, command: u8, payload: &[u8]) -> HidResult<u32> {
        Self::send_command(self, command, payload)
    }

    fn send_commands(&mut self, commands: &[HostCommandRequest<'_>]) -> HidResult<Vec<u32>> {
        Self::send_commands(self, commands)
    }

    fn link_label(&self) -> String {
        self.port_name().to_owned()
    }
}

pub trait ReconnectConnector<L>: Send + Sync + 'static
where
    L: ReconnectLink,
{
    /// Opens or reopens the underlying link.
    ///
    /// # Errors
    ///
    /// Returns the open/enumeration/handshake failure when the link is not ready.
    fn connect(&self) -> HidResult<L>;

    /// Returns the configured reconnect target label.
    fn description(&self) -> String;
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HidReconnectTarget {
    Port(String),
    Auto,
}

impl ReconnectConnector<HidGateway> for HidReconnectTarget {
    fn connect(&self) -> HidResult<HidGateway> {
        match self {
            Self::Port(port_name) => HidGateway::connect(port_name.clone()),
            Self::Auto => connect_auto(),
        }
    }

    fn description(&self) -> String {
        match self {
            Self::Port(port_name) => port_name.clone(),
            Self::Auto => "auto".to_owned(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReconnectStateKind {
    Connected,
    Connecting,
    Disconnected,
    Stopping,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReconnectSnapshot {
    pub state: ReconnectStateKind,
    pub target: String,
    pub link_label: Option<String>,
    pub reconnect_attempts: u64,
    pub last_error_code: Option<&'static str>,
    pub last_error_detail: Option<String>,
}

pub type HidReconnectGateway = ReconnectGateway<HidGateway, HidReconnectTarget>;

pub struct ReconnectGateway<L, C>
where
    L: ReconnectLink,
    C: ReconnectConnector<L>,
{
    shared: Arc<Shared<L>>,
    worker: Option<thread::JoinHandle<()>>,
    _connector: Arc<C>,
}

impl ReconnectGateway<HidGateway, HidReconnectTarget> {
    /// Opens a configured CDC ACM serial port and starts the reconnect worker.
    ///
    /// # Errors
    ///
    /// Returns the initial [`HidGateway::connect`] error when the first open or
    /// handshake fails. Later serial I/O loss is converted into a disconnected
    /// state and retried by the worker.
    pub fn connect_port(port_name: impl Into<String>) -> HidResult<Self> {
        Self::connect(HidReconnectTarget::Port(port_name.into()))
    }

    /// Auto-detects the first Synapse Pico HID port and starts the reconnect worker.
    ///
    /// # Errors
    ///
    /// Returns the initial auto-detect/open/handshake error when no matching
    /// device is ready.
    pub fn connect_auto() -> HidResult<Self> {
        Self::connect(HidReconnectTarget::Auto)
    }
}

impl<L, C> ReconnectGateway<L, C>
where
    L: ReconnectLink,
    C: ReconnectConnector<L>,
{
    /// Opens the link once, then starts a 500 ms reconnect loop for later link loss.
    ///
    /// # Errors
    ///
    /// Returns the connector error when the initial open/handshake fails.
    pub fn connect(connector: C) -> HidResult<Self> {
        Self::connect_with_interval(connector, Duration::from_millis(RECONNECT_INTERVAL_MS))
    }

    fn connect_with_interval(connector: C, reconnect_interval: Duration) -> HidResult<Self> {
        let target = connector.description();
        let connector = Arc::new(connector);
        let link = connector.connect()?;
        Ok(Self::from_connected(
            connector,
            reconnect_interval,
            target,
            link,
        ))
    }

    fn from_connected(
        connector: Arc<C>,
        reconnect_interval: Duration,
        target: String,
        link: L,
    ) -> Self {
        let link_label = link.link_label();
        let shared = Arc::new(Shared {
            state: Mutex::new(LinkState::Connected {
                link,
                link_label,
                reconnect_attempts: 0,
                last_error: None,
            }),
            wake: Condvar::new(),
            target,
        });
        let worker_shared = Arc::clone(&shared);
        let worker_connector = Arc::clone(&connector);
        let worker = thread::spawn(move || {
            reconnect_worker(&worker_shared, &worker_connector, reconnect_interval);
        });

        Self {
            shared,
            worker: Some(worker),
            _connector: connector,
        }
    }

    #[must_use]
    pub fn snapshot(&self) -> ReconnectSnapshot {
        snapshot_from_state(&self.shared.target, &lock_state(&self.shared.state))
    }

    /// Sends one command through the current link.
    ///
    /// # Errors
    ///
    /// Returns [`HidError::PortDisconnected`] immediately when the link is in
    /// reconnect state. Serial link-loss errors from a connected link also move
    /// the gateway into reconnect state before returning `PortDisconnected`.
    #[allow(clippy::significant_drop_tightening)]
    pub fn send_command(&self, command: u8, payload: &[u8]) -> HidResult<u32> {
        {
            let mut state = lock_state(&self.shared.state);
            match &mut *state {
                LinkState::Connected {
                    link,
                    reconnect_attempts,
                    last_error,
                    ..
                } => match link.send_command(command, payload) {
                    Ok(seq) => Ok(seq),
                    Err(error) if should_enter_reconnect(&error) => {
                        let disconnected = disconnected_error(&self.shared.target, &error);
                        let stored = StoredError::from_hid_error(&error);
                        let attempts = *reconnect_attempts;
                        *state = LinkState::Disconnected {
                            reconnect_attempts: attempts,
                            last_error: stored,
                        };
                        self.shared.wake.notify_all();
                        Err(disconnected)
                    }
                    Err(error) => {
                        *last_error = Some(StoredError::from_hid_error(&error));
                        Err(error)
                    }
                },
                LinkState::Disconnected { last_error, .. } => Err(fail_fast_disconnected(
                    &self.shared.target,
                    Some(last_error),
                )),
                LinkState::Connecting { last_error, .. } => Err(fail_fast_disconnected(
                    &self.shared.target,
                    last_error.as_ref(),
                )),
                LinkState::Stopping => Err(HidError::PortDisconnected {
                    detail: format!(
                        "HID reconnect worker for {} is stopping",
                        self.shared.target
                    ),
                }),
            }
        }
    }

    /// Sends a command batch through the current link.
    ///
    /// # Errors
    ///
    /// Returns [`HidError::PortDisconnected`] immediately when the link is in
    /// reconnect state. Serial link-loss errors from a connected link also move
    /// the gateway into reconnect state before returning `PortDisconnected`.
    #[allow(clippy::significant_drop_tightening)]
    pub fn send_commands(&self, commands: &[HostCommandRequest<'_>]) -> HidResult<Vec<u32>> {
        {
            let mut state = lock_state(&self.shared.state);
            match &mut *state {
                LinkState::Connected {
                    link,
                    reconnect_attempts,
                    last_error,
                    ..
                } => match link.send_commands(commands) {
                    Ok(seqs) => Ok(seqs),
                    Err(error) if should_enter_reconnect(&error) => {
                        let disconnected = disconnected_error(&self.shared.target, &error);
                        let stored = StoredError::from_hid_error(&error);
                        let attempts = *reconnect_attempts;
                        *state = LinkState::Disconnected {
                            reconnect_attempts: attempts,
                            last_error: stored,
                        };
                        self.shared.wake.notify_all();
                        Err(disconnected)
                    }
                    Err(error) => {
                        *last_error = Some(StoredError::from_hid_error(&error));
                        Err(error)
                    }
                },
                LinkState::Disconnected { last_error, .. } => Err(fail_fast_disconnected(
                    &self.shared.target,
                    Some(last_error),
                )),
                LinkState::Connecting { last_error, .. } => Err(fail_fast_disconnected(
                    &self.shared.target,
                    last_error.as_ref(),
                )),
                LinkState::Stopping => Err(HidError::PortDisconnected {
                    detail: format!(
                        "HID reconnect worker for {} is stopping",
                        self.shared.target
                    ),
                }),
            }
        }
    }
}

impl<L, C> Drop for ReconnectGateway<L, C>
where
    L: ReconnectLink,
    C: ReconnectConnector<L>,
{
    fn drop(&mut self) {
        {
            let mut state = lock_state(&self.shared.state);
            *state = LinkState::Stopping;
        }
        self.shared.wake.notify_all();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

struct Shared<L>
where
    L: ReconnectLink,
{
    state: Mutex<LinkState<L>>,
    wake: Condvar,
    target: String,
}

enum LinkState<L>
where
    L: ReconnectLink,
{
    Connected {
        link: L,
        link_label: String,
        reconnect_attempts: u64,
        last_error: Option<StoredError>,
    },
    Connecting {
        reconnect_attempts: u64,
        last_error: Option<StoredError>,
    },
    Disconnected {
        reconnect_attempts: u64,
        last_error: StoredError,
    },
    Stopping,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct StoredError {
    code: &'static str,
    detail: String,
}

impl StoredError {
    fn from_hid_error(error: &HidError) -> Self {
        Self {
            code: error.code(),
            detail: error.to_string(),
        }
    }
}

fn reconnect_worker<L, C>(shared: &Arc<Shared<L>>, connector: &Arc<C>, reconnect_interval: Duration)
where
    L: ReconnectLink,
    C: ReconnectConnector<L>,
{
    let mut state = lock_state(&shared.state);
    loop {
        let (reconnect_attempts, last_error) = match &*state {
            LinkState::Stopping => break,
            LinkState::Connected { .. } | LinkState::Connecting { .. } => {
                state = wait_state(&shared.wake, state);
                continue;
            }
            LinkState::Disconnected {
                reconnect_attempts,
                last_error,
            } => (
                reconnect_attempts.saturating_add(1),
                Some(last_error.clone()),
            ),
        };

        *state = LinkState::Connecting {
            reconnect_attempts,
            last_error,
        };
        drop(state);

        debug!(
            target = %shared.target,
            reconnect_attempts,
            "attempting HID reconnect"
        );
        let result = connector.connect();
        state = lock_state(&shared.state);

        if matches!(*state, LinkState::Stopping) {
            break;
        }

        match result {
            Ok(link) => {
                let link_label = link.link_label();
                debug!(
                    target = %shared.target,
                    %link_label,
                    reconnect_attempts,
                    "HID reconnect succeeded"
                );
                *state = LinkState::Connected {
                    link,
                    link_label,
                    reconnect_attempts,
                    last_error: None,
                };
                shared.wake.notify_all();
            }
            Err(error) => {
                let stored = StoredError::from_hid_error(&error);
                warn!(
                    target = %shared.target,
                    reconnect_attempts,
                    error_code = stored.code,
                    error = %stored.detail,
                    "HID reconnect attempt failed"
                );
                *state = LinkState::Disconnected {
                    reconnect_attempts,
                    last_error: stored,
                };
                state = wait_timeout_state(&shared.wake, state, reconnect_interval);
            }
        }
    }
}

const fn should_enter_reconnect(error: &HidError) -> bool {
    matches!(
        error,
        HidError::PortNotFound { .. }
            | HidError::PortOpenFailed { .. }
            | HidError::ProtocolHandshakeFailed { .. }
            | HidError::FirmwareVersionMismatch { .. }
            | HidError::LinkTimeout { .. }
            | HidError::PortDisconnected { .. }
    )
}

fn disconnected_error(target: &str, cause: &HidError) -> HidError {
    HidError::PortDisconnected {
        detail: format!(
            "HID link {target} entered reconnect state after {}: {cause}",
            cause.code()
        ),
    }
}

fn fail_fast_disconnected(target: &str, last_error: Option<&StoredError>) -> HidError {
    let detail = last_error.map_or_else(
        || format!("HID link {target} is reconnecting"),
        |error| {
            format!(
                "HID link {target} is reconnecting; last error {}: {}",
                error.code, error.detail
            )
        },
    );
    HidError::PortDisconnected { detail }
}

fn snapshot_from_state<L>(target: &str, state: &LinkState<L>) -> ReconnectSnapshot
where
    L: ReconnectLink,
{
    match state {
        LinkState::Connected {
            link_label,
            reconnect_attempts,
            last_error,
            ..
        } => ReconnectSnapshot {
            state: ReconnectStateKind::Connected,
            target: target.to_owned(),
            link_label: Some(link_label.clone()),
            reconnect_attempts: *reconnect_attempts,
            last_error_code: last_error.as_ref().map(|error| error.code),
            last_error_detail: last_error.as_ref().map(|error| error.detail.clone()),
        },
        LinkState::Connecting {
            reconnect_attempts,
            last_error,
        } => ReconnectSnapshot {
            state: ReconnectStateKind::Connecting,
            target: target.to_owned(),
            link_label: None,
            reconnect_attempts: *reconnect_attempts,
            last_error_code: last_error.as_ref().map(|error| error.code),
            last_error_detail: last_error.as_ref().map(|error| error.detail.clone()),
        },
        LinkState::Disconnected {
            reconnect_attempts,
            last_error,
        } => ReconnectSnapshot {
            state: ReconnectStateKind::Disconnected,
            target: target.to_owned(),
            link_label: None,
            reconnect_attempts: *reconnect_attempts,
            last_error_code: Some(last_error.code),
            last_error_detail: Some(last_error.detail.clone()),
        },
        LinkState::Stopping => ReconnectSnapshot {
            state: ReconnectStateKind::Stopping,
            target: target.to_owned(),
            link_label: None,
            reconnect_attempts: 0,
            last_error_code: None,
            last_error_detail: None,
        },
    }
}

fn lock_state<L>(state: &Mutex<LinkState<L>>) -> MutexGuard<'_, LinkState<L>>
where
    L: ReconnectLink,
{
    match state.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn wait_state<'a, L>(
    wake: &Condvar,
    guard: MutexGuard<'a, LinkState<L>>,
) -> MutexGuard<'a, LinkState<L>>
where
    L: ReconnectLink,
{
    match wake.wait(guard) {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn wait_timeout_state<'a, L>(
    wake: &Condvar,
    guard: MutexGuard<'a, LinkState<L>>,
    timeout: Duration,
) -> MutexGuard<'a, LinkState<L>>
where
    L: ReconnectLink,
{
    match wake.wait_timeout(guard, timeout) {
        Ok((guard, _timeout)) => guard,
        Err(poisoned) => poisoned.into_inner().0,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::{Duration, Instant};

    use synapse_core::error_codes;

    use super::{
        HidError, HidResult, HostCommandRequest, RECONNECT_INTERVAL_MS, ReconnectConnector,
        ReconnectGateway, ReconnectLink, ReconnectStateKind,
    };
    use crate::protocol::HOST_COMMAND_MOUSE_MOVE_REL;

    #[test]
    fn reconnect_interval_contract_is_500_ms() {
        assert_eq!(RECONNECT_INTERVAL_MS, 500);
    }

    #[test]
    fn serial_io_error_enters_reconnect_and_calls_fail_fast() {
        let initial_link = FakeLink::new(
            "COM7",
            [Err(HidError::LinkTimeout {
                operation: "reading ACK",
                timeout_ms: 5,
            })],
        );
        let connector = ArcFakeConnector::new([Err(missing_port_error())]);
        let gateway = ReconnectGateway::from_connected(
            connector.clone_arc(),
            Duration::from_millis(100),
            "COM7".to_owned(),
            initial_link,
        );

        let before = gateway.snapshot();
        assert_eq!(before.state, ReconnectStateKind::Connected);
        assert_eq!(before.link_label.as_deref(), Some("COM7"));

        let first = match gateway.send_command(HOST_COMMAND_MOUSE_MOVE_REL, &[1, 0, 2, 0]) {
            Ok(seq) => panic!("link timeout should disconnect, returned seq {seq}"),
            Err(error) => error,
        };
        assert_eq!(first.code(), error_codes::ACTION_HID_PORT_DISCONNECTED);

        let before_fast = Instant::now();
        let second = match gateway.send_command(HOST_COMMAND_MOUSE_MOVE_REL, &[1, 0, 2, 0]) {
            Ok(seq) => panic!("disconnected call should fail fast, returned seq {seq}"),
            Err(error) => error,
        };
        let elapsed = before_fast.elapsed();

        assert_eq!(second.code(), error_codes::ACTION_HID_PORT_DISCONNECTED);
        assert!(
            elapsed < Duration::from_millis(50),
            "fail-fast call took {elapsed:?}"
        );
    }

    #[test]
    fn reconnect_worker_restores_command_path_after_connector_success() {
        let initial_link = FakeLink::new(
            "COM7",
            [Err(HidError::LinkTimeout {
                operation: "reading ACK",
                timeout_ms: 5,
            })],
        );
        let connector = ArcFakeConnector::new([
            Err(missing_port_error()),
            Ok(FakeLink::new("COM7", [Ok(42)])),
        ]);
        let gateway = ReconnectGateway::from_connected(
            connector.clone_arc(),
            Duration::from_millis(10),
            "COM7".to_owned(),
            initial_link,
        );

        let lost = match gateway.send_command(HOST_COMMAND_MOUSE_MOVE_REL, &[1, 0, 2, 0]) {
            Ok(seq) => panic!("first command should disconnect, returned seq {seq}"),
            Err(error) => error,
        };
        assert_eq!(lost.code(), error_codes::ACTION_HID_PORT_DISCONNECTED);

        wait_for_state(&gateway, ReconnectStateKind::Connected);

        let after = gateway.snapshot();
        assert_eq!(after.state, ReconnectStateKind::Connected);
        assert_eq!(after.reconnect_attempts, 2);
        assert_eq!(after.link_label.as_deref(), Some("COM7"));

        let seq = match gateway.send_command(HOST_COMMAND_MOUSE_MOVE_REL, &[1, 0, 2, 0]) {
            Ok(seq) => seq,
            Err(error) => panic!("reconnected command should pass: {error}"),
        };
        assert_eq!(seq, 42);
    }

    #[test]
    fn non_link_command_rejection_does_not_enter_reconnect_state() {
        let initial_link = FakeLink::new(
            "COM7",
            [Err(HidError::CommandRejected {
                seq: 1,
                command: HOST_COMMAND_MOUSE_MOVE_REL,
                reason: 0x04,
            })],
        );
        let connector = ArcFakeConnector::new([Ok(FakeLink::new("COM7", [Ok(2)]))]);
        let gateway = ReconnectGateway::from_connected(
            connector.clone_arc(),
            Duration::from_millis(10),
            "COM7".to_owned(),
            initial_link,
        );

        let rejected = match gateway.send_command(HOST_COMMAND_MOUSE_MOVE_REL, &[1, 0, 2, 0]) {
            Ok(seq) => panic!("command rejection should not pass, returned seq {seq}"),
            Err(error) => error,
        };

        assert_eq!(rejected.code(), error_codes::HID_COMMAND_REJECTED);
        let after = gateway.snapshot();
        assert_eq!(after.state, ReconnectStateKind::Connected);
        assert_eq!(connector.attempts(), 0);
    }

    #[test]
    fn drop_stops_reconnect_worker() {
        let initial_link = FakeLink::new(
            "COM7",
            [Err(HidError::LinkTimeout {
                operation: "reading ACK",
                timeout_ms: 5,
            })],
        );
        let connector =
            ArcFakeConnector::new([Err(missing_port_error()), Err(missing_port_error())]);
        let gateway = ReconnectGateway::from_connected(
            connector.clone_arc(),
            Duration::from_millis(10),
            "COM7".to_owned(),
            initial_link,
        );

        let lost = match gateway.send_command(HOST_COMMAND_MOUSE_MOVE_REL, &[1, 0, 2, 0]) {
            Ok(seq) => panic!("first command should disconnect, returned seq {seq}"),
            Err(error) => error,
        };
        assert_eq!(lost.code(), error_codes::ACTION_HID_PORT_DISCONNECTED);
        wait_for_attempts(&gateway, 1);

        let before_drop_attempts = connector.attempts();
        drop(gateway);
        thread::sleep(Duration::from_millis(30));
        assert_eq!(connector.attempts(), before_drop_attempts);
    }

    fn wait_for_state<C>(
        gateway: &ReconnectGateway<FakeLink, C>,
        expected_state: ReconnectStateKind,
    ) where
        C: ReconnectConnector<FakeLink>,
    {
        let deadline = Instant::now() + Duration::from_millis(250);
        while Instant::now() < deadline {
            if gateway.snapshot().state == expected_state {
                return;
            }
            thread::sleep(Duration::from_millis(2));
        }
        panic!(
            "gateway did not reach {expected_state:?}; last snapshot {:?}",
            gateway.snapshot()
        );
    }

    fn wait_for_attempts<C>(gateway: &ReconnectGateway<FakeLink, C>, expected_attempts: u64)
    where
        C: ReconnectConnector<FakeLink>,
    {
        let deadline = Instant::now() + Duration::from_millis(250);
        while Instant::now() < deadline {
            if gateway.snapshot().reconnect_attempts >= expected_attempts {
                return;
            }
            thread::sleep(Duration::from_millis(2));
        }
        panic!(
            "gateway did not reach {expected_attempts} attempts; last snapshot {:?}",
            gateway.snapshot()
        );
    }

    fn missing_port_error() -> HidError {
        HidError::PortNotFound {
            port_name: "COM7".to_owned(),
        }
    }

    struct FakeLink {
        label: String,
        responses: VecDeque<HidResult<u32>>,
    }

    impl FakeLink {
        fn new<const N: usize>(label: &str, responses: [HidResult<u32>; N]) -> Self {
            Self {
                label: label.to_owned(),
                responses: VecDeque::from(responses),
            }
        }
    }

    impl ReconnectLink for FakeLink {
        fn send_command(&mut self, _command: u8, _payload: &[u8]) -> HidResult<u32> {
            self.responses
                .pop_front()
                .unwrap_or(Err(HidError::LinkTimeout {
                    operation: "reading ACK",
                    timeout_ms: 5,
                }))
        }

        fn send_commands(&mut self, commands: &[HostCommandRequest<'_>]) -> HidResult<Vec<u32>> {
            let seq = self.send_command(0, &[])?;
            Ok(commands.iter().map(|_| seq).collect())
        }

        fn link_label(&self) -> String {
            self.label.clone()
        }
    }

    #[derive(Clone)]
    struct ArcFakeConnector {
        inner: Arc<FakeConnector>,
    }

    impl ArcFakeConnector {
        fn new<const N: usize>(connections: [HidResult<FakeLink>; N]) -> Self {
            Self {
                inner: Arc::new(FakeConnector {
                    connections: Mutex::new(VecDeque::from(connections)),
                    attempts: AtomicUsize::new(0),
                }),
            }
        }

        fn clone_arc(&self) -> Arc<Self> {
            Arc::new(self.clone())
        }

        fn attempts(&self) -> usize {
            self.inner.attempts.load(Ordering::SeqCst)
        }
    }

    impl ReconnectConnector<FakeLink> for ArcFakeConnector {
        fn connect(&self) -> HidResult<FakeLink> {
            self.inner.attempts.fetch_add(1, Ordering::SeqCst);
            let mut connections = match self.inner.connections.lock() {
                Ok(guard) => guard,
                Err(poisoned) => poisoned.into_inner(),
            };
            connections
                .pop_front()
                .unwrap_or_else(|| Err(missing_port_error()))
        }

        fn description(&self) -> String {
            "COM7".to_owned()
        }
    }

    struct FakeConnector {
        connections: Mutex<VecDeque<HidResult<FakeLink>>>,
        attempts: AtomicUsize,
    }
}
