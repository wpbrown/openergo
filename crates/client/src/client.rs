use crate::activity::ActivityProducer;
use crate::assets;
use crate::sound::SoundPlayer;
use futures::future::{Either, select};
use futures::{Sink, SinkExt, StreamExt};
use rootcause::prelude::*;
use shared::protocol::read_protocol_version;
use shared::protocol::server::{ClientCodec, PROTOCOL_VERSION, ServerMessage, UsageIncrement};
use shared::shutdown::ShutdownSignal;
use std::path::PathBuf;
use std::pin::pin;
use std::time::Duration;
use tokio::net::UnixStream;
use tokio::time::timeout;
use tokio_util::codec::Framed;
use tracing::{debug, error, info, trace};

/// Sleeps for `RECONNECT_DELAY` or returns early if shutdown is requested.
/// Returns `true` if shutdown was requested.
async fn sleep_or_shutdown(shutdown: &mut ShutdownSignal) -> bool {
    timeout(RECONNECT_DELAY, shutdown.wait()).await.is_ok()
}

const RECONNECT_DELAY: Duration = Duration::from_secs(1);

pub async fn reconnect_loop<S>(
    socket_path: PathBuf,
    mut usage_producer: S,
    activity_producer: ActivityProducer,
    mut shutdown: ShutdownSignal,
) -> Result<(), Report>
where
    S: Sink<UsageIncrement> + Unpin,
{
    let sound_player = SoundPlayer::new()?;
    info!("using server socket path: {}", socket_path.display());

    loop {
        let stream = {
            let connect = UnixStream::connect(&socket_path);
            match select(pin!(connect), shutdown.wait()).await {
                Either::Left((Ok(stream), _)) => Some(stream),
                Either::Left((Err(e), _)) => {
                    debug!("Failed to connect: {e}");
                    None
                }
                Either::Right(_) => {
                    info!("shutting down");
                    return Ok(());
                }
            }
        };

        if let Some(mut stream) = stream {
            info!("Connected to server");
            match read_protocol_version(&mut stream, PROTOCOL_VERSION).await {
                Ok(None) => {}
                Ok(Some(peer)) => {
                    return Err(report!(
                        "protocol version mismatch: server={peer}, client={PROTOCOL_VERSION}"
                    ));
                }
                Err(e) => {
                    error!("Failed to read protocol version: {e}");
                    if sleep_or_shutdown(&mut shutdown).await {
                        info!("shutting down");
                        return Ok(());
                    }
                    continue;
                }
            }
            let mut framed = Framed::new(stream, ClientCodec::default()).fuse();
            let was_shutdown = handle_connection(
                &mut framed,
                &mut usage_producer,
                &activity_producer,
                &sound_player,
                &mut shutdown,
            )
            .await;
            info!("Disconnected from server");
            if was_shutdown {
                return Ok(());
            }
        }

        // Whether we failed to connect or got disconnected, back off before
        // retrying so a misbehaving server can't pin us in a tight loop.
        if sleep_or_shutdown(&mut shutdown).await {
            info!("shutting down");
            return Ok(());
        }
    }
}

type FramedStream = futures::stream::Fuse<Framed<UnixStream, ClientCodec>>;

/// Returns `true` if shutdown was requested.
async fn handle_connection<S>(
    framed: &mut FramedStream,
    usage_producer: &mut S,
    activity_producer: &ActivityProducer,
    sound_player: &SoundPlayer,
    shutdown: &mut ShutdownSignal,
) -> bool
where
    S: Sink<UsageIncrement> + Unpin,
{
    loop {
        let next = framed.next();
        let msg = match select(next, shutdown.wait()).await {
            Either::Left((Some(msg), _)) => msg,
            Either::Left((None, _)) => return false,
            Either::Right(_) => {
                info!("shutting down: disconnecting");
                return true;
            }
        };

        match msg {
            Ok(ServerMessage::Click) => {
                info!("Click");
                sound_player.play(assets::CLICK);
            }
            Ok(ServerMessage::NewUsage(increment)) => {
                trace!(increment = ?increment, "new usage");
                let _ = usage_producer.send(*increment).await;
            }
            Ok(ServerMessage::Activity) => {
                activity_producer.notify();
            }
            Err(e) => {
                error!("Error receiving message: {e}");
                return false;
            }
        }
    }
}
