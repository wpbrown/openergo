use futures::StreamExt;
use rodio::Decoder;
use rootcause::prelude::*;
use shared::protocol::{ClientCodec, ServerMessage};
use std::io::Cursor;
use std::path::PathBuf;
use std::time::Duration;
use tokio::net::UnixStream;
use tokio_util::codec::Framed;

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
    let socket_path = find_socket_path();
    log::info!("Using socket path: {}", socket_path.display());

    loop {
        match UnixStream::connect(&socket_path).await {
            Ok(stream) => {
                log::info!("Connected to server");
                let mut framed = Framed::new(stream, ClientCodec::default()).fuse();
                handle_connection(&mut framed).await;
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

async fn handle_connection(framed: &mut FramedStream) {
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
            }
            Err(e) => {
                log::error!("Error receiving message: {e}");
                return;
            }
        }
    }
}
