use bachelor::broadcast::spmc::SpmcBroadcastProducer;
use bachelor::signal::mpmc_latched::MpmcLatchedSignalConsumer;
use futures::StreamExt;
use futures::future::{Either, select};
use rootcause::prelude::*;
use shared::protocol::{
    ClientCodec, PROTOCOL_VERSION, ServerMessage, UsageIncrement, read_protocol_version,
};
use std::path::PathBuf;
use std::pin::pin;
use std::time::Duration;
use tokio::net::UnixStream;
use tokio::time::timeout;
use tokio_util::codec::Framed;

use crate::click::ClickHandler;

/// Sleeps for `RECONNECT_DELAY` or returns early if shutdown is requested.
/// Returns `true` if shutdown was requested.
async fn sleep_or_shutdown(shutdown: &mut MpmcLatchedSignalConsumer) -> bool {
    timeout(RECONNECT_DELAY, shutdown.observe()).await.is_ok()
}

const RECONNECT_DELAY: Duration = Duration::from_secs(1);

pub async fn reconnect_loop(
    socket_path: PathBuf,
    mut usage_producer: SpmcBroadcastProducer<UsageIncrement>,
    mut shutdown: MpmcLatchedSignalConsumer,
) -> Result<(), Report> {
    let click_handler = ClickHandler::new()?;

    loop {
        let stream = {
            let connect = UnixStream::connect(&socket_path);
            match select(pin!(connect), shutdown.observe()).await {
                Either::Left((Ok(stream), _)) => Some(stream),
                Either::Left((Err(e), _)) => {
                    log::debug!("Failed to connect: {e}");
                    None
                }
                Either::Right(_) => {
                    log::info!("shutting down");
                    return Ok(());
                }
            }
        };

        if let Some(mut stream) = stream {
            log::info!("Connected to server");
            match read_protocol_version(&mut stream).await {
                Ok(None) => {}
                Ok(Some(peer)) => {
                    return Err(report!(
                        "protocol version mismatch: server={peer}, client={PROTOCOL_VERSION}"
                    ));
                }
                Err(e) => {
                    log::error!("Failed to read protocol version: {e}");
                    if sleep_or_shutdown(&mut shutdown).await {
                        log::info!("shutting down");
                        return Ok(());
                    }
                    continue;
                }
            }
            let mut framed = Framed::new(stream, ClientCodec::default()).fuse();
            let was_shutdown = handle_connection(
                &mut framed,
                &mut usage_producer,
                &click_handler,
                &mut shutdown,
            )
            .await;
            log::info!("Disconnected from server");
            if was_shutdown {
                return Ok(());
            }
        }

        // Whether we failed to connect or got disconnected, back off before
        // retrying so a misbehaving server can't pin us in a tight loop.
        if sleep_or_shutdown(&mut shutdown).await {
            log::info!("shutting down");
            return Ok(());
        }
    }
}

type FramedStream = futures::stream::Fuse<Framed<UnixStream, ClientCodec>>;

/// Returns `true` if shutdown was requested.
async fn handle_connection(
    framed: &mut FramedStream,
    usage_producer: &mut SpmcBroadcastProducer<UsageIncrement>,
    click_handler: &ClickHandler,
    shutdown: &mut MpmcLatchedSignalConsumer,
) -> bool {
    loop {
        let next = framed.next();
        let msg = match select(pin!(next), shutdown.observe()).await {
            Either::Left((Some(msg), _)) => msg,
            Either::Left((None, _)) => return false,
            Either::Right(_) => {
                log::info!("shutting down: disconnecting");
                return true;
            }
        };

        match msg {
            Ok(ServerMessage::Click) => {
                log::info!("Click");
                click_handler.click();
            }
            Ok(ServerMessage::NewUsage(increment)) => {
                log::trace!("Usage: {increment:?}");
                let _ = usage_producer.send(*increment).await;
            }
            Err(e) => {
                log::error!("Error receiving message: {e}");
                return false;
            }
        }
    }
}
