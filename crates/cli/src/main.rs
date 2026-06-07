use clap::Parser;
use futures::future::{Either, select};
use futures::{StreamExt, pin_mut};
use rootcause::prelude::*;
use shared::protocol::client::{CliCodec, ClientMessage, PROTOCOL_VERSION};
use shared::protocol::read_protocol_version;
use shared::tracing_fmt::console_port_delta;
use std::path::PathBuf;
use tokio::net::UnixStream;
use tokio_util::codec::Framed;
use tracing::info;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(long, default_value_os_t = shared::socket::default_client_socket_path())]
    client_socket_path: PathBuf,
}

fn main() {
    shared::tracing_fmt::init_tracing(console_port_delta::CLI);
    let args = Args::parse();

    if let Err(report) = startup(args) {
        eprintln!("{report}");
        std::process::exit(1);
    }
}

fn startup(args: Args) -> Result<(), Report> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("Failed to create tokio runtime")?;
    rt.block_on(run(args))
}

async fn run(args: Args) -> Result<(), Report> {
    let path = args.client_socket_path;
    info!("Connecting to client socket at {}", path.display());

    let mut stream = UnixStream::connect(&path)
        .await
        .context("Failed to connect to client socket")
        .attach(format!("path: {}", path.display()))?;

    if let Some(peer) = read_protocol_version(&mut stream, PROTOCOL_VERSION)
        .await
        .context("Failed to read protocol version")?
    {
        bail!(
            "Protocol version mismatch: ours = {}, peer = {peer}",
            PROTOCOL_VERSION
        );
    }

    let mut framed = Framed::new(stream, CliCodec::default());

    loop {
        let next = framed.next();
        let ctrl_c = tokio::signal::ctrl_c();
        pin_mut!(next, ctrl_c);
        match select(next, ctrl_c).await {
            Either::Left((Some(Ok(msg)), _)) => print_message(&msg),
            Either::Left((Some(Err(e)), _)) => {
                return Err(e).context("Error reading from client socket")?;
            }
            Either::Left((None, _)) => {
                info!("Client closed the connection");
                return Ok(());
            }
            Either::Right((Ok(()), _)) => {
                info!("Ctrl-C received, exiting");
                return Ok(());
            }
            Either::Right((Err(e), _)) => {
                return Err(e).context("Failed waiting for Ctrl-C")?;
            }
        }
    }
}

fn print_message(msg: &ClientMessage) {
    use std::io::Write;
    match msg {
        ClientMessage::Rest(c, l) => println!("REST {:.3} {:.3}", c.as_f64(), l.as_f64()),
        ClientMessage::Break(c, l) => println!("BREAK {:.3} {:.3}", c.as_f64(), l.as_f64()),
        ClientMessage::Day(c, l) => println!("DAY {:.3} {:.3}", c.as_f64(), l.as_f64()),
        ClientMessage::Pain { label, ratio } => println!("PAIN {label} {ratio:.3}"),
    }
    let _ = std::io::stdout().flush();
}
