use bachelor::broadcast::spmc::SpmcBroadcastProducer;
use futures::StreamExt;
use futures::future::{Either, select};
use rootcause::prelude::*;
use shared::protocol::{
    ClientCodec, PROTOCOL_VERSION, ServerMessage, UsageIncrement, read_protocol_version,
};
use std::path::PathBuf;
use std::pin::{Pin, pin};
use std::time::Duration;
use tokio::net::UnixStream;
use tokio::time::timeout;
use tokio_util::codec::Framed;

use crate::click::ClickHandler;

pub async fn reconnect_loop(
    socket_path: &PathBuf,
    mut usage_producer: SpmcBroadcastProducer<UsageIncrement>,
) -> Result<(), Report> {
    let click_handler = ClickHandler::new()?;
    let ctrl_c = tokio::signal::ctrl_c();
    tokio::pin!(ctrl_c);

    loop {
        let stream = {
            let connect = UnixStream::connect(socket_path);
            match select(pin!(connect), ctrl_c.as_mut()).await {
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
                    if timeout(Duration::from_secs(1), ctrl_c.as_mut())
                        .await
                        .is_ok()
                    {
                        log::info!("shutting down");
                        return Ok(());
                    }
                    continue;
                }
            }
            let mut framed = Framed::new(stream, ClientCodec::default()).fuse();
            let shutdown = handle_connection(
                &mut framed,
                &mut usage_producer,
                &click_handler,
                ctrl_c.as_mut(),
            )
            .await;
            log::info!("Disconnected from server");
            if shutdown {
                return Ok(());
            }
        } else {
            if timeout(Duration::from_secs(1), ctrl_c.as_mut())
                .await
                .is_ok()
            {
                log::info!("shutting down");
                return Ok(());
            }
        }
    }
}

type FramedStream = futures::stream::Fuse<Framed<UnixStream, ClientCodec>>;

/// Returns `true` if ctrl_c was received.
async fn handle_connection(
    framed: &mut FramedStream,
    usage_producer: &mut SpmcBroadcastProducer<UsageIncrement>,
    click_handler: &ClickHandler,
    mut ctrl_c: Pin<&mut impl Future<Output = Result<(), std::io::Error>>>,
) -> bool {
    loop {
        let next = framed.next();
        let msg = match select(pin!(next), ctrl_c.as_mut()).await {
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
