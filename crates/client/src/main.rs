mod telemetry;
mod usage;

use bachelor::broadcast::spmc::{SpmcBroadcastProducer, broadcast};
use futures::StreamExt;
use rodio::Decoder;
use rootcause::prelude::*;
use shared::protocol::{ClientCodec, ServerMessage, UsageIncrement};
use std::io::Cursor;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::time::Duration;
use tokio::net::UnixStream;
use tokio::task::spawn_local;
use tokio_util::codec::Framed;

use crate::usage::rest::RestState;

const CLICK_WAV: &[u8] = include_bytes!("../assets/click.wav");

fn main() {
    env_logger::init();

    if let Err(report) = startup() {
        eprintln!("{report}");
        std::process::exit(1);
    }
}

fn startup() -> Result<(), Report> {
    let rt = tokio::runtime::LocalRuntime::new().context("Failed to create tokio runtime")?;
    rt.block_on(run())
}

fn find_socket_path() -> PathBuf {
    let uid = users::get_current_uid();
    let user_path = PathBuf::from(format!("/run/user/{uid}/openergo.sock"));
    if user_path.exists() {
        user_path
    } else {
        PathBuf::from("/run/openergo.sock")
    }
}

async fn run() -> Result<(), Report> {
    let _meter_provider = telemetry::init();
    let socket_path = find_socket_path();
    log::info!("Using socket path: {}", socket_path.display());

    const USAGE_BROADCAST_CAPACITY: NonZeroUsize =
        NonZeroUsize::new(16).expect("broadcast capacity must be non-zero");

    let (mut usage_producer, usage_source) = broadcast(USAGE_BROADCAST_CAPACITY);

    // Rest driver
    let (_rest_state, rest_driver) = usage::rest::create(
        usage_source.subscribe(),
        RestState::default(),
        usage::StartupGap::default(),
    );
    spawn_local(rest_driver);

    // All-time usage driver
    let (_all_usage_source, all_usage_driver) = usage::all::create(
        usage_source.subscribe(),
        Default::default(),
    );
    spawn_local(all_usage_driver);

    loop {
        match UnixStream::connect(&socket_path).await {
            Ok(stream) => {
                log::info!("Connected to server");
                let mut framed = Framed::new(stream, ClientCodec::default()).fuse();
                handle_connection(&mut framed, &mut usage_producer).await;
                log::info!("Disconnected from server");
            }
            Err(e) => {
                log::debug!("Failed to connect: {e}");
            }
        }

        tokio::time::sleep(Duration::from_secs(1)).await;
    }
}

type FramedStream = futures::stream::Fuse<Framed<UnixStream, ClientCodec>>;

async fn handle_connection(
    framed: &mut FramedStream,
    usage_producer: &mut SpmcBroadcastProducer<UsageIncrement>,
) {
    let handle =
        rodio::DeviceSinkBuilder::open_default_sink().expect("Failed to open audio output");

    while let Some(msg) = framed.next().await {
        match msg {
            Ok(ServerMessage::Click) => {
                log::info!("Click");
                let cursor = Cursor::new(CLICK_WAV);
                if let Ok(source) = Decoder::try_from(cursor) {
                    handle.mixer().add(source);
                }
            }
            Ok(ServerMessage::NewUsage(increment)) => {
                log::trace!("Usage: {increment:?}");
                let _ = usage_producer.send(*increment).await;
            }
            Err(e) => {
                log::error!("Error receiving message: {e}");
                return;
            }
        }
    }
}
