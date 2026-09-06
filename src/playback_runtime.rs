use std::{future::Future, io, sync::mpsc, thread, thread::JoinHandle, time::Duration};

use crate::playback::{
    MprisPlaybackSource, PlaybackCommand, PlaybackError, PlaybackSnapshot, PlaybackSource,
};

#[derive(Debug, Clone, PartialEq)]
pub enum PlaybackEvent {
    Connecting,
    Updated(PlaybackSnapshot),
    Disconnected,
    Failed(String),
}

pub struct PlaybackRuntime {
    command_tx: tokio::sync::mpsc::UnboundedSender<WorkerCommand>,
    event_rx: mpsc::Receiver<PlaybackEvent>,
    worker: Option<JoinHandle<()>>,
}

const MIN_RECONNECT_DELAY: Duration = Duration::from_millis(250);
const MAX_RECONNECT_DELAY: Duration = Duration::from_secs(5);

enum WorkerCommand {
    Playback(PlaybackCommand),
    Reconnect,
    Shutdown,
}

impl PlaybackRuntime {
    pub fn start(startup_uri: Option<String>) -> io::Result<Self> {
        Self::start_with_factory(MprisPlaybackSource::connect, startup_uri)
    }

    pub fn start_with_source<S>(source: S) -> io::Result<Self>
    where
        S: PlaybackSource + Send + 'static,
    {
        Self::start_with_factory(|| async move { Ok(source) }, None)
    }

    fn start_with_factory<S, F, Fut>(
        source_factory: F,
        startup_uri: Option<String>,
    ) -> io::Result<Self>
    where
        S: PlaybackSource + Send + 'static,
        F: FnOnce() -> Fut + Send + 'static,
        Fut: Future<Output = Result<S, PlaybackError>> + 'static,
    {
        let (event_tx, event_rx) = mpsc::channel();
        let (command_tx, mut command_rx) = tokio::sync::mpsc::unbounded_channel();
        let worker = thread::Builder::new()
            .name("spotify-tui-playback".to_owned())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build();
                let Ok(runtime) = runtime else {
                    let _ = event_tx.send(PlaybackEvent::Failed(
                        "could not start the playback runtime".to_owned(),
                    ));
                    return;
                };

                let _ = event_tx.send(PlaybackEvent::Connecting);
                runtime.block_on(async {
                    let source = match source_factory().await {
                        Ok(source) => source,
                        Err(error) => {
                            publish_result(&event_tx, Err(error));
                            return;
                        }
                    };
                    let (mut connected, mut activation_pending) =
                        match recover_source(&source).await {
                            Ok(Some(snapshot)) => (publish_result(&event_tx, Ok(snapshot)), false),
                            Ok(None) => (false, true),
                            Err(error) => (publish_result(&event_tx, Err(error)), false),
                        };
                    let mut reconnect_delay = MIN_RECONNECT_DELAY;
                    loop {
                        if connected {
                            tokio::select! {
                                change = source.wait_for_change() => {
                                    connected = publish_result(&event_tx, change);
                                    activation_pending = false;
                                }
                                command = command_rx.recv() => {
                                    if !handle_command(
                                        &source,
                                        &event_tx,
                                        &mut connected,
                                        &mut activation_pending,
                                        startup_uri.as_deref(),
                                        command,
                                    ).await {
                                        break;
                                    }
                                }
                            }
                        } else {
                            tokio::select! {
                                () = tokio::time::sleep(reconnect_delay) => {
                                    match recover_source(&source).await {
                                        Ok(Some(snapshot)) => {
                                            let snapshot = finish_activation(
                                                &source,
                                                snapshot,
                                                activation_pending,
                                                startup_uri.as_deref(),
                                            ).await;
                                            connected = publish_result(&event_tx, snapshot);
                                            activation_pending = false;
                                            reconnect_delay = MIN_RECONNECT_DELAY;
                                        }
                                        Ok(None) => {
                                            if !activation_pending {
                                                let _ = event_tx.send(PlaybackEvent::Connecting);
                                            }
                                            activation_pending = true;
                                            reconnect_delay = MIN_RECONNECT_DELAY;
                                        }
                                        Err(_) => {
                                            activation_pending = false;
                                            reconnect_delay = (reconnect_delay * 2)
                                                .min(MAX_RECONNECT_DELAY);
                                        }
                                    }
                                }
                                command = command_rx.recv() => {
                                    if !handle_command(
                                        &source,
                                        &event_tx,
                                        &mut connected,
                                        &mut activation_pending,
                                        startup_uri.as_deref(),
                                        command,
                                    ).await {
                                        break;
                                    }
                                }
                            }
                        }
                    }
                });
            })?;
        Ok(Self {
            command_tx,
            event_rx,
            worker: Some(worker),
        })
    }

    pub fn try_event(&self) -> Option<PlaybackEvent> {
        self.event_rx.try_recv().ok()
    }

    pub fn dispatch(&self, command: PlaybackCommand) -> Result<(), PlaybackRuntimeError> {
        self.command_tx
            .send(WorkerCommand::Playback(command))
            .map_err(|_| PlaybackRuntimeError)
    }

    pub fn reconnect(&self) -> Result<(), PlaybackRuntimeError> {
        self.command_tx
            .send(WorkerCommand::Reconnect)
            .map_err(|_| PlaybackRuntimeError)
    }
}

#[derive(Debug, thiserror::Error)]
#[error("the playback runtime has stopped")]
pub struct PlaybackRuntimeError;

async fn recover_source<S: PlaybackSource>(
    source: &S,
) -> Result<Option<PlaybackSnapshot>, PlaybackError> {
    match source.snapshot().await {
        Ok(snapshot) => Ok(Some(snapshot)),
        Err(PlaybackError::Disconnected) => {
            source.activate().await?;
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

async fn finish_activation<S: PlaybackSource>(
    source: &S,
    snapshot: PlaybackSnapshot,
    activation_pending: bool,
    startup_uri: Option<&str>,
) -> Result<PlaybackSnapshot, PlaybackError> {
    if activation_pending && let Some(uri) = startup_uri {
        source
            .execute(PlaybackCommand::OpenUri(uri.to_owned()))
            .await?;
        source.snapshot().await
    } else {
        Ok(snapshot)
    }
}

async fn handle_command<S: PlaybackSource>(
    source: &S,
    event_tx: &mpsc::Sender<PlaybackEvent>,
    connected: &mut bool,
    activation_pending: &mut bool,
    startup_uri: Option<&str>,
    command: Option<WorkerCommand>,
) -> bool {
    match command {
        Some(WorkerCommand::Shutdown) | None => false,
        Some(WorkerCommand::Playback(command)) => {
            let result = match source.execute(command).await {
                Ok(()) => source.snapshot().await,
                Err(error) => Err(error),
            };
            *connected = publish_result(event_tx, result);
            *activation_pending = false;
            true
        }
        Some(WorkerCommand::Reconnect) => {
            let _ = event_tx.send(PlaybackEvent::Connecting);
            match recover_source(source).await {
                Ok(Some(snapshot)) => {
                    let snapshot =
                        finish_activation(source, snapshot, *activation_pending, startup_uri).await;
                    *connected = publish_result(event_tx, snapshot);
                    *activation_pending = false;
                }
                Ok(None) => {
                    *connected = false;
                    *activation_pending = true;
                }
                Err(error) => {
                    *connected = publish_result(event_tx, Err(error));
                    *activation_pending = false;
                }
            }
            true
        }
    }
}

fn publish_result(
    event_tx: &mpsc::Sender<PlaybackEvent>,
    result: Result<PlaybackSnapshot, PlaybackError>,
) -> bool {
    let (event, connected) = match result {
        Ok(snapshot) => (PlaybackEvent::Updated(snapshot), true),
        Err(PlaybackError::Disconnected) => (PlaybackEvent::Disconnected, false),
        Err(error) => (PlaybackEvent::Failed(error.to_string()), false),
    };
    let _ = event_tx.send(event);
    connected
}

impl Drop for PlaybackRuntime {
    fn drop(&mut self) {
        let _ = self.command_tx.send(WorkerCommand::Shutdown);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        future,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        time::Duration,
    };

    use super::*;
    use crate::playback::{
        PlaybackCommand, PlaybackError, PlaybackSnapshot, PlaybackStatus, TrackMetadata,
    };

    struct FakePlaybackSource {
        snapshot: PlaybackSnapshot,
        changes: Arc<
            tokio::sync::Mutex<
                tokio::sync::mpsc::UnboundedReceiver<Result<PlaybackSnapshot, PlaybackError>>,
            >,
        >,
        commands: Option<mpsc::Sender<PlaybackCommand>>,
        command_failure: Option<String>,
    }

    impl PlaybackSource for FakePlaybackSource {
        fn activate(&self) -> impl Future<Output = Result<(), PlaybackError>> + Send {
            future::ready(Ok(()))
        }

        fn snapshot(&self) -> impl Future<Output = Result<PlaybackSnapshot, PlaybackError>> + Send {
            future::ready(Ok(self.snapshot.clone()))
        }

        async fn wait_for_change(&self) -> Result<PlaybackSnapshot, PlaybackError> {
            self.changes
                .lock()
                .await
                .recv()
                .await
                .ok_or(PlaybackError::Disconnected)?
        }

        fn execute(
            &self,
            command: PlaybackCommand,
        ) -> impl Future<Output = Result<(), PlaybackError>> + Send {
            if let Some(commands) = &self.commands {
                let _ = commands.send(command);
            }
            future::ready(match &self.command_failure {
                Some(message) => Err(PlaybackError::Mpris(message.clone())),
                None => Ok(()),
            })
        }
    }

    struct TransferRequiredSource {
        snapshot: PlaybackSnapshot,
        activated: Arc<AtomicBool>,
        activations: mpsc::Sender<()>,
        commands: Option<mpsc::Sender<PlaybackCommand>>,
    }

    impl PlaybackSource for TransferRequiredSource {
        fn activate(&self) -> impl Future<Output = Result<(), PlaybackError>> + Send {
            self.activated.store(true, Ordering::Release);
            let _ = self.activations.send(());
            future::ready(Ok(()))
        }

        fn snapshot(&self) -> impl Future<Output = Result<PlaybackSnapshot, PlaybackError>> + Send {
            future::ready(if self.activated.load(Ordering::Acquire) {
                Ok(self.snapshot.clone())
            } else {
                Err(PlaybackError::Disconnected)
            })
        }

        async fn wait_for_change(&self) -> Result<PlaybackSnapshot, PlaybackError> {
            future::pending().await
        }

        fn execute(
            &self,
            command: PlaybackCommand,
        ) -> impl Future<Output = Result<(), PlaybackError>> + Send {
            if let Some(commands) = &self.commands {
                let _ = commands.send(command);
            }
            future::ready(Ok(()))
        }
    }

    fn snapshot() -> PlaybackSnapshot {
        PlaybackSnapshot {
            status: PlaybackStatus::Playing,
            track: TrackMetadata {
                track_id: Some("spotify:track:one".to_owned()),
                title: Some("Live track".to_owned()),
                artists: vec!["Artist".to_owned()],
                album: Some("Album".to_owned()),
                duration: Some(Duration::from_secs(180)),
                art_url: Some("https://example.com/art.jpg".to_owned()),
            },
            position: Duration::from_secs(12),
            volume: 0.75,
        }
    }

    fn receive_event(runtime: &PlaybackRuntime) -> PlaybackEvent {
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        loop {
            if let Some(event) = runtime.try_event() {
                return event;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for playback event"
            );
            std::thread::yield_now();
        }
    }

    #[test]
    fn runtime_publishes_initial_snapshot() {
        let expected = snapshot();
        let (_change_tx, change_rx) = tokio::sync::mpsc::unbounded_channel();
        let runtime = PlaybackRuntime::start_with_source(FakePlaybackSource {
            snapshot: expected.clone(),
            changes: Arc::new(tokio::sync::Mutex::new(change_rx)),
            commands: None,
            command_failure: None,
        })
        .expect("runtime should start");

        assert_eq!(receive_event(&runtime), PlaybackEvent::Connecting);
        assert_eq!(receive_event(&runtime), PlaybackEvent::Updated(expected));
    }

    #[test]
    fn runtime_activates_spotifyd_without_an_external_client() {
        let activated = Arc::new(AtomicBool::new(false));
        let (activation_tx, activation_rx) = mpsc::channel();
        let runtime = PlaybackRuntime::start_with_source(TransferRequiredSource {
            snapshot: snapshot(),
            activated,
            activations: activation_tx,
            commands: None,
        })
        .expect("runtime should start");

        assert_eq!(receive_event(&runtime), PlaybackEvent::Connecting);
        assert_eq!(activation_rx.recv_timeout(Duration::from_secs(1)), Ok(()));
        assert!(matches!(receive_event(&runtime), PlaybackEvent::Updated(_)));
    }

    #[test]
    fn runtime_opens_the_configured_context_after_activation() {
        let activated = Arc::new(AtomicBool::new(false));
        let (activation_tx, activation_rx) = mpsc::channel();
        let (command_tx, command_rx) = mpsc::channel();
        let source = TransferRequiredSource {
            snapshot: snapshot(),
            activated,
            activations: activation_tx,
            commands: Some(command_tx),
        };
        let runtime = PlaybackRuntime::start_with_factory(
            || async move { Ok(source) },
            Some("spotify:playlist:37i9dQZF1DXcBWIGoYBM5M".to_owned()),
        )
        .expect("runtime should start");

        assert_eq!(receive_event(&runtime), PlaybackEvent::Connecting);
        assert_eq!(activation_rx.recv_timeout(Duration::from_secs(1)), Ok(()));
        assert_eq!(
            command_rx.recv_timeout(Duration::from_secs(1)),
            Ok(PlaybackCommand::OpenUri(
                "spotify:playlist:37i9dQZF1DXcBWIGoYBM5M".to_owned()
            ))
        );
        assert!(matches!(receive_event(&runtime), PlaybackEvent::Updated(_)));
    }

    #[test]
    fn runtime_preserves_an_existing_context_when_already_active() {
        let (_change_tx, change_rx) = tokio::sync::mpsc::unbounded_channel();
        let (command_tx, command_rx) = mpsc::channel();
        let source = FakePlaybackSource {
            snapshot: snapshot(),
            changes: Arc::new(tokio::sync::Mutex::new(change_rx)),
            commands: Some(command_tx),
            command_failure: None,
        };
        let runtime = PlaybackRuntime::start_with_factory(
            || async move { Ok(source) },
            Some("spotify:playlist:37i9dQZF1DXcBWIGoYBM5M".to_owned()),
        )
        .expect("runtime should start");

        assert_eq!(receive_event(&runtime), PlaybackEvent::Connecting);
        assert!(matches!(receive_event(&runtime), PlaybackEvent::Updated(_)));
        assert_eq!(
            command_rx.recv_timeout(Duration::from_millis(100)),
            Err(mpsc::RecvTimeoutError::Timeout)
        );
    }

    #[test]
    fn runtime_publishes_source_changes() {
        let initial = snapshot();
        let mut changed = initial.clone();
        changed.status = PlaybackStatus::Paused;
        changed.position = Duration::from_secs(42);
        let (change_tx, change_rx) = tokio::sync::mpsc::unbounded_channel();
        let runtime = PlaybackRuntime::start_with_source(FakePlaybackSource {
            snapshot: initial.clone(),
            changes: Arc::new(tokio::sync::Mutex::new(change_rx)),
            commands: None,
            command_failure: None,
        })
        .expect("runtime should start");

        assert_eq!(receive_event(&runtime), PlaybackEvent::Connecting);
        assert_eq!(receive_event(&runtime), PlaybackEvent::Updated(initial));

        change_tx
            .send(Ok(changed.clone()))
            .expect("fake source should still be connected");
        assert_eq!(receive_event(&runtime), PlaybackEvent::Updated(changed));
    }

    #[test]
    fn runtime_forwards_playback_commands_to_the_source() {
        let (change_tx, change_rx) = tokio::sync::mpsc::unbounded_channel();
        let (command_tx, command_rx) = mpsc::channel();
        let runtime = PlaybackRuntime::start_with_source(FakePlaybackSource {
            snapshot: snapshot(),
            changes: Arc::new(tokio::sync::Mutex::new(change_rx)),
            commands: Some(command_tx),
            command_failure: None,
        })
        .expect("runtime should start");

        assert_eq!(receive_event(&runtime), PlaybackEvent::Connecting);
        assert!(matches!(receive_event(&runtime), PlaybackEvent::Updated(_)));

        runtime
            .dispatch(PlaybackCommand::Toggle)
            .expect("runtime should accept commands");
        assert_eq!(
            command_rx.recv_timeout(Duration::from_secs(1)),
            Ok(PlaybackCommand::Toggle)
        );

        drop(change_tx);
    }

    #[test]
    fn runtime_reconnects_on_request() {
        let (change_tx, change_rx) = tokio::sync::mpsc::unbounded_channel();
        let runtime = PlaybackRuntime::start_with_source(FakePlaybackSource {
            snapshot: snapshot(),
            changes: Arc::new(tokio::sync::Mutex::new(change_rx)),
            commands: None,
            command_failure: None,
        })
        .expect("runtime should start");

        assert_eq!(receive_event(&runtime), PlaybackEvent::Connecting);
        assert!(matches!(receive_event(&runtime), PlaybackEvent::Updated(_)));

        runtime
            .reconnect()
            .expect("runtime should accept reconnect requests");
        assert_eq!(receive_event(&runtime), PlaybackEvent::Connecting);
        assert!(matches!(receive_event(&runtime), PlaybackEvent::Updated(_)));

        drop(change_tx);
    }

    #[test]
    fn runtime_reconnects_after_the_source_disappears() {
        let expected = snapshot();
        let (change_tx, change_rx) = tokio::sync::mpsc::unbounded_channel();
        let runtime = PlaybackRuntime::start_with_source(FakePlaybackSource {
            snapshot: expected.clone(),
            changes: Arc::new(tokio::sync::Mutex::new(change_rx)),
            commands: None,
            command_failure: None,
        })
        .expect("runtime should start");

        assert_eq!(receive_event(&runtime), PlaybackEvent::Connecting);
        assert_eq!(
            receive_event(&runtime),
            PlaybackEvent::Updated(expected.clone())
        );

        change_tx
            .send(Err(PlaybackError::Disconnected))
            .expect("fake source should still be connected");
        assert_eq!(receive_event(&runtime), PlaybackEvent::Disconnected);
        assert_eq!(receive_event(&runtime), PlaybackEvent::Updated(expected));
    }

    #[test]
    fn runtime_reports_playback_command_failures() {
        let (change_tx, change_rx) = tokio::sync::mpsc::unbounded_channel();
        let runtime = PlaybackRuntime::start_with_source(FakePlaybackSource {
            snapshot: snapshot(),
            changes: Arc::new(tokio::sync::Mutex::new(change_rx)),
            commands: None,
            command_failure: Some("permission denied".to_owned()),
        })
        .expect("runtime should start");

        assert_eq!(receive_event(&runtime), PlaybackEvent::Connecting);
        assert!(matches!(receive_event(&runtime), PlaybackEvent::Updated(_)));
        runtime
            .dispatch(PlaybackCommand::Toggle)
            .expect("runtime should accept commands");
        assert_eq!(
            receive_event(&runtime),
            PlaybackEvent::Failed("MPRIS request failed: permission denied".to_owned())
        );

        drop(change_tx);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn runtime_rediscovers_mpris_after_a_process_unique_name_changes() {
        use std::{
            collections::HashMap,
            fs,
            io::Read,
            path::PathBuf,
            process::{Child, Command, Stdio},
            sync::atomic::{AtomicU64, Ordering},
        };

        use zbus::zvariant::OwnedValue;

        struct TestBus {
            address: String,
            child: Child,
            socket_directory: PathBuf,
        }

        impl TestBus {
            fn start() -> Self {
                static NEXT_BUS_ID: AtomicU64 = AtomicU64::new(0);

                let socket_directory = std::env::temp_dir().join(format!(
                    "spotify-tui-dbus-{}-{}",
                    std::process::id(),
                    NEXT_BUS_ID.fetch_add(1, Ordering::Relaxed)
                ));
                fs::create_dir(&socket_directory)
                    .expect("private D-Bus socket directory should be created");
                let socket_path = socket_directory.join("bus");
                let address = format!("unix:path={}", socket_path.display());
                let config_path = socket_directory.join("session.conf");
                let config = format!(
                    r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <keep_umask/>
  <listen>{address}</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
"#
                );
                fs::write(&config_path, config)
                    .expect("private D-Bus configuration should be written");
                let mut child = Command::new("dbus-daemon")
                    .arg("--nofork")
                    .arg(format!("--config-file={}", config_path.display()))
                    .stdout(Stdio::null())
                    .stderr(Stdio::piped())
                    .spawn()
                    .expect("private D-Bus daemon should start");

                let deadline = std::time::Instant::now() + Duration::from_secs(5);
                while !socket_path.exists() {
                    if let Some(status) = child
                        .try_wait()
                        .expect("private D-Bus daemon status should be readable")
                    {
                        let mut stderr = String::new();
                        child
                            .stderr
                            .take()
                            .expect("daemon should have stderr")
                            .read_to_string(&mut stderr)
                            .expect("daemon stderr should be readable");
                        let _ = fs::remove_dir_all(&socket_directory);
                        panic!(
                            "private D-Bus daemon exited with {status} before creating its socket: {}",
                            stderr.trim()
                        );
                    }
                    if std::time::Instant::now() >= deadline {
                        let _ = child.kill();
                        let _ = child.wait();
                        let mut stderr = String::new();
                        if let Some(mut output) = child.stderr.take() {
                            let _ = output.read_to_string(&mut stderr);
                        }
                        let _ = fs::remove_dir_all(&socket_directory);
                        panic!(
                            "timed out waiting for the private D-Bus socket: {}",
                            stderr.trim()
                        );
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }

                Self {
                    address,
                    child,
                    socket_directory,
                }
            }
        }

        impl Drop for TestBus {
            fn drop(&mut self) {
                let _ = self.child.kill();
                let _ = self.child.wait();
                let _ = fs::remove_dir_all(&self.socket_directory);
            }
        }

        struct MockMprisPlayer;

        #[zbus::interface(name = "org.mpris.MediaPlayer2.Player")]
        impl MockMprisPlayer {
            #[zbus(property)]
            fn playback_status(&self) -> &str {
                "Paused"
            }

            #[zbus(property)]
            fn metadata(&self) -> HashMap<String, OwnedValue> {
                HashMap::new()
            }

            #[zbus(property)]
            fn position(&self) -> i64 {
                0
            }

            #[zbus(property)]
            fn volume(&self) -> f64 {
                0.5
            }
        }

        struct MockSpotifydControls {
            pid: u32,
            activations: tokio::sync::mpsc::UnboundedSender<u32>,
        }

        #[zbus::interface(name = "rs.spotifyd.Controls")]
        impl MockSpotifydControls {
            fn transfer_playback(&self) {
                let _ = self.activations.send(self.pid);
            }
        }

        enum ServiceCommand {
            Stop(mpsc::Sender<()>),
            Start { pid: u32, ready: mpsc::Sender<()> },
        }

        async fn start_mpris(address: &str, pid: u32) -> zbus::Result<zbus::Connection> {
            zbus::connection::Builder::address(address)?
                .name(format!("org.mpris.MediaPlayer2.spotifyd.instance{pid}"))?
                .serve_at("/org/mpris/MediaPlayer2", MockMprisPlayer)?
                .build()
                .await
        }

        async fn start_controls(
            address: &str,
            pid: u32,
            activations: tokio::sync::mpsc::UnboundedSender<u32>,
        ) -> zbus::Result<zbus::Connection> {
            zbus::connection::Builder::address(address)?
                .name(format!("rs.spotifyd.instance{pid}"))?
                .serve_at(
                    "/rs/spotifyd/Controls",
                    MockSpotifydControls { pid, activations },
                )?
                .build()
                .await
        }

        let bus = TestBus::start();
        let address = bus.address.clone();
        let (service_tx, mut service_rx) = tokio::sync::mpsc::unbounded_channel();
        let (activation_tx, mut activation_rx) = tokio::sync::mpsc::unbounded_channel();
        let runtime = PlaybackRuntime::start_with_factory(move || async move {
            let mut current_pid = Some(100);
            let mut controls = Some(
                start_controls(&address, 100, activation_tx.clone())
                    .await
                    .expect("initial mock controls should start"),
            );
            let mut mpris = None;
            let source = MprisPlaybackSource::connect_to_address(address.clone()).await?;
            tokio::spawn(async move {
                loop {
                    tokio::select! {
                        activation = activation_rx.recv() => {
                            let Some(pid) = activation else {
                                break;
                            };
                            if current_pid == Some(pid) && mpris.is_none() {
                                mpris = Some(
                                    start_mpris(&address, pid)
                                        .await
                                        .expect("activated mock player should start"),
                                );
                            }
                        }
                        command = service_rx.recv() => {
                            match command {
                                Some(ServiceCommand::Stop(ready)) => {
                                    drop(mpris.take());
                                    drop(controls.take());
                                    current_pid = None;
                                    let _ = ready.send(());
                                }
                                Some(ServiceCommand::Start { pid, ready }) => {
                                    assert!(controls.is_none(), "mock daemon should be stopped");
                                    current_pid = Some(pid);
                                    controls = Some(
                                        start_controls(&address, pid, activation_tx.clone())
                                            .await
                                            .expect("replacement mock controls should start"),
                                    );
                                    let _ = ready.send(());
                                }
                                None => break,
                            }
                        }
                    }
                }
                drop(mpris);
                drop(controls);
            });
            Ok(source)
        }, None)
        .expect("runtime should start");

        assert_eq!(receive_event(&runtime), PlaybackEvent::Connecting);
        assert!(matches!(receive_event(&runtime), PlaybackEvent::Updated(_)));

        let (stopped_tx, stopped_rx) = mpsc::channel();
        service_tx
            .send(ServiceCommand::Stop(stopped_tx))
            .expect("service manager should be running");
        stopped_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("mock player should stop");
        assert_eq!(receive_event(&runtime), PlaybackEvent::Disconnected);

        let (started_tx, started_rx) = mpsc::channel();
        service_tx
            .send(ServiceCommand::Start {
                pid: 200,
                ready: started_tx,
            })
            .expect("service manager should be running");
        started_rx
            .recv_timeout(Duration::from_secs(1))
            .expect("replacement mock player should start");
        assert_eq!(receive_event(&runtime), PlaybackEvent::Connecting);
        assert!(matches!(receive_event(&runtime), PlaybackEvent::Updated(_)));
    }
}
