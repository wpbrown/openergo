use clap::Parser;
use rootcause::prelude::*;
use std::path::PathBuf;

mod activity;
mod app;
mod assets;
mod client;
mod credit;
mod fdr;
mod integration;
mod notifications;
mod pain;
mod persistence;
mod server;
mod sound;
mod telemetry;
mod transports;
mod usage;
mod watch_mux;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
pub struct Args {
    /// Path to the server's Unix domain socket.
    #[arg(long, default_value = shared::socket::DEFAULT_SERVER_SOCKET_PATH)]
    pub server_socket_path: PathBuf,

    /// Path to the Unix domain socket this client hosts for CLI/GUI listeners.
    #[arg(long, default_value_os_t = shared::socket::default_client_socket_path())]
    pub client_socket_path: PathBuf,

    /// Path to a TOML configuration file.
    #[arg(short, long)]
    pub config: Option<PathBuf>,
}

fn main() {
    shared::tracing_fmt::init_tracing(shared::tracing_fmt::console_port_delta::CLIENT);
    let args = Args::parse();

    if let Err(report) = startup(args) {
        eprintln!("{report}");
        std::process::exit(1);
    }
}

fn startup(args: Args) -> Result<(), Report> {
    let rt = tokio::runtime::LocalRuntime::new().context("Failed to create tokio runtime")?;
    rt.block_on(app::run(args))
}
