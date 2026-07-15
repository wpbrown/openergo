use clap::Parser;
use rootcause::prelude::*;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
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
    let Args {
        server_socket_path,
        client_socket_path,
        config,
    } = args;
    let config_args = openergo_client::ConfigArgs {
        server_socket_path,
        client_socket_path,
    };
    rt.block_on(openergo_client::run(config_args, config.as_deref()))
}
