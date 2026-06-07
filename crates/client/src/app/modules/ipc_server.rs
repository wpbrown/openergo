use crate::credit::limit::CreditLimitSource;
use crate::pain::PainLiveSource;
use crate::server;
use crate::usage::AllUsageSources;
use futures::FutureExt;
use rootcause::prelude::*;
use shared::oe_spawn;
use shared::shutdown::ShutdownSignal;
use shared::spawn::JoinHandle;
use std::path::{Path, PathBuf};
use tokio::net::UnixListener;
use tracing::info;

/// Bind a fresh Unix listener at `path` (removing any stale socket
/// file, permissions default to user-only 0600) and spawn the IPC
/// server task that broadcasts state updates to connected listeners.
pub fn start(
    path: PathBuf,
    sources: AllUsageSources,
    pain: PainLiveSource,
    credit_limits: CreditLimitSource,
    shutdown: ShutdownSignal,
) -> Result<JoinHandle<Result<(), Report>>, Report> {
    info!("hosting client socket at: {}", path.display());
    let listener = bind(&path).context("Failed to bind client socket")?;
    Ok(oe_spawn!(
        "client-ipc-server",
        server::create(listener, sources, pain, credit_limits)
            .run(shutdown)
            .map(|_| Ok(()))
    ))
}

fn bind(path: &Path) -> Result<UnixListener, Report> {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::remove_file(path);
    let listener = std::os::unix::net::UnixListener::bind(path)
        .context("Failed to bind client socket")
        .attach(format!("path: {}", path.display()))?;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .context("Failed to set client socket permissions")?;
    listener.set_nonblocking(true)?;
    Ok(UnixListener::from_std(listener)?)
}
