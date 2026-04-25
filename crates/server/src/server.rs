//! Unix domain socket server for IPC with user-space clients.
//!
//! The server:
//! - Listens for commands (pause/resume auto-click)
//! - Pushes usage stats to connected clients (rate-limited to every 5 seconds)

use bachelor::channel::mpsc::{self, MpscChannelConsumer, MpscChannelProducer};
use bachelor::signal::mpmc_latched::MpmcLatchedSignalSource;
use bachelor::watch::{MpmcWatchRefConsumer, MpmcWatchRefSource};
use futures::future::{Either, select};
use futures::pin_mut;
use futures::{SinkExt, StreamExt};
use shared::codec::PostcardCodec;
use shared::model::UsageSnapshot;
use shared::protocol::{
    Command, DwellServerConfig, ServerMessage, UsageIncrement, write_protocol_version,
};
use std::io;
use std::num::NonZeroUsize;
use tokio::net::{UnixListener, UnixStream};
use tokio_util::codec::Framed;

/// Commands received from clients.
pub enum ClientCommand {
    ConfigureDwellClick(DwellServerConfig),
    PauseAutoClick,
    ResumeAutoClick,
}

/// Creates the server and returns a receiver for commands.
pub fn create(
    listener: UnixListener,
    usage_source: MpmcWatchRefSource<UsageSnapshot>,
    click_events: Option<MpmcLatchedSignalSource>,
) -> (MpscChannelConsumer<ClientCommand>, Server) {
    let (cmd_tx, cmd_rx) = mpsc::channel(NonZeroUsize::new(32).unwrap());

    let server = Server {
        listener,
        usage_source,
        click_events,
        cmd_tx,
    };

    (cmd_rx, server)
}

/// The server that handles client connections and message routing.
pub struct Server {
    listener: UnixListener,
    usage_source: MpmcWatchRefSource<UsageSnapshot>,
    click_events: Option<MpmcLatchedSignalSource>,
    cmd_tx: MpscChannelProducer<ClientCommand>,
}

impl Server {
    /// Run the server, handling connections and events.
    pub async fn run(self) {
        loop {
            match self.listener.accept().await {
                Ok((stream, _)) => {
                    let usage_rx = self.usage_source.subscribe_forward();
                    let click_rx = self.click_events.as_ref().map(|ce| ce.subscribe_forward());
                    let cmd_tx = self.cmd_tx.clone();
                    tokio::task::spawn_local(async move {
                        handle_client(stream, usage_rx, click_rx, cmd_tx).await;
                    });
                    log::info!("New client connected");
                }
                Err(e) => {
                    log::error!("Failed to accept connection: {}", e);
                }
            }
        }
    }
}

enum ClientLoopEvent {
    UsageChanged,
    UsageChannelClosed,
    Click,
    Command(Option<Result<Command, io::Error>>),
}

async fn wait_client_event(
    usage_rx: &mut MpmcWatchRefConsumer<UsageSnapshot>,
    click_rx: &mut Option<bachelor::signal::mpmc_latched::MpmcLatchedSignalConsumer>,
    framed: &mut Framed<UnixStream, PostcardCodec<Command, ServerMessage>>,
) -> ClientLoopEvent {
    let usage_changed_fut = usage_rx.changed();
    pin_mut!(usage_changed_fut);

    let base_event = async {
        match select(usage_changed_fut, framed.next()).await {
            Either::Left((Ok(()), _)) => ClientLoopEvent::UsageChanged,
            Either::Left((Err(_), _)) => ClientLoopEvent::UsageChannelClosed,
            Either::Right((result, _)) => ClientLoopEvent::Command(result),
        }
    };

    if let Some(click_rx) = click_rx.as_mut() {
        pin_mut!(base_event);
        match select(base_event, click_rx.observe()).await {
            Either::Left((event, _)) => event,
            Either::Right(((), _)) => ClientLoopEvent::Click,
        }
    } else {
        base_event.await
    }
}

async fn handle_client(
    mut stream: UnixStream,
    mut usage_rx: MpmcWatchRefConsumer<UsageSnapshot>,
    mut click_rx: Option<bachelor::signal::mpmc_latched::MpmcLatchedSignalConsumer>,
    cmd_tx: MpscChannelProducer<ClientCommand>,
) {
    use jiff::Timestamp;

    if let Err(e) = write_protocol_version(&mut stream).await {
        log::debug!("Failed to send protocol version: {e}");
        return;
    }

    let codec: PostcardCodec<Command, ServerMessage> = PostcardCodec::default();
    let mut framed = Framed::new(stream, codec);

    let mut previous_usage = usage_rx.get();
    let mut last_end = Timestamp::now();

    loop {
        match wait_client_event(&mut usage_rx, &mut click_rx, &mut framed).await {
            ClientLoopEvent::UsageChanged => {
                log::trace!("usage stats changed");
                let now = Timestamp::now();

                let current_usage = usage_rx.get();
                let increment = UsageIncrement::new(
                    current_usage.saturating_delta(&previous_usage),
                    last_end,
                    now,
                );
                last_end = now;
                previous_usage = current_usage;
                let msg = ServerMessage::NewUsage(Box::new(increment));
                if let Err(e) = framed.send(msg).await {
                    log::debug!("Failed to send to client: {}", e);
                    break;
                }
            }
            ClientLoopEvent::UsageChannelClosed => {
                log::info!("Usage stream closed");
                break;
            }
            ClientLoopEvent::Click => {
                if let Err(e) = framed.send(ServerMessage::Click).await {
                    log::debug!("Failed to send click to client: {}", e);
                    break;
                }
            }
            ClientLoopEvent::Command(result) => match result {
                Some(Ok(cmd)) => {
                    handle_command(cmd, &cmd_tx, &mut framed).await;
                }
                Some(Err(e)) => {
                    log::debug!("Client error: {}", e);
                    break;
                }
                None => {
                    log::info!("Client disconnected");
                    break;
                }
            },
        }
    }
}

async fn handle_command(
    cmd: Command,
    cmd_tx: &MpscChannelProducer<ClientCommand>,
    _framed: &mut Framed<UnixStream, PostcardCodec<Command, ServerMessage>>,
) {
    let client_cmd = match cmd {
        Command::ConfigureDwellClick(config) => ClientCommand::ConfigureDwellClick(config),
        Command::PauseAutoClick => ClientCommand::PauseAutoClick,
        Command::ResumeAutoClick => ClientCommand::ResumeAutoClick,
    };

    // Forward command to main application
    let _ = cmd_tx.send(client_cmd).await;
}
