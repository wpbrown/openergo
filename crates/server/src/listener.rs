use libsystemd::activation::{IsType, receive_descriptors};
use rootcause::prelude::*;
use std::fs;
use std::os::fd::{FromRawFd, IntoRawFd, OwnedFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::{fs as unix_fs, net};
use std::path::Path;
use tokio::net::UnixListener;
use tracing::{info, trace};

const DEFAULT_SOCKET_MODE: u32 = 0o660;

type SocketOwner = (Option<u32>, Option<u32>);

fn try_inherit_systemd_listener() -> Result<Option<UnixListener>, Report> {
    let mut fds = receive_descriptors(true)
        .map_err(|e| report!("Failed to receive systemd file descriptors: {e}"))?;
    let fd = match fds.len() {
        0 => return Ok(None),
        1 => fds.remove(0),
        n => bail!("Expected exactly one inherited systemd socket, got {n}"),
    };
    if !fd.is_unix() {
        bail!("Inherited systemd file descriptor is not a Unix socket");
    }

    // SAFETY: `receive_descriptors` returns owned file descriptors passed
    // by systemd, and `IntoRawFd::into_raw_fd` transfers that ownership to
    // us.
    let owned = unsafe { OwnedFd::from_raw_fd(fd.into_raw_fd()) };
    let listener = net::UnixListener::from(owned);
    listener
        .set_nonblocking(true)
        .context("Failed to set inherited socket nonblocking")?;
    info!("Using inherited systemd socket; local socket bind settings are ignored");
    Ok(Some(UnixListener::from_std(listener)?))
}

fn resolve_group(group_str: &str) -> Result<u32, Report> {
    if let Ok(gid) = group_str.parse::<u32>() {
        return Ok(gid);
    }
    uzers::get_group_by_name(group_str)
        .map(|g| g.gid())
        .ok_or_else(|| report!("Group not found"))
        .attach(format!("group: {group_str}"))
}

fn resolve_socket_owner(user: Option<&str>, group: Option<&str>) -> Result<SocketOwner, Report> {
    match user {
        Some(user_str) => {
            let user = if let Ok(uid) = user_str.parse::<u32>() {
                uzers::get_user_by_uid(uid)
            } else {
                uzers::get_user_by_name(user_str)
            }
            .ok_or_else(|| report!("User not found"))
            .attach(format!("user: {user_str}"))?;

            let uid = user.uid();
            let gid = match group {
                Some(g) => resolve_group(g)?,
                None => user.primary_group_id(),
            };
            Ok((Some(uid), Some(gid)))
        }
        None => {
            let gid = group.map(resolve_group).transpose()?;
            Ok((None, gid))
        }
    }
}

pub fn create_listener(
    socket_path: &Path,
    user: Option<&str>,
    group: Option<&str>,
) -> Result<UnixListener, Report> {
    if let Some(listener) = try_inherit_systemd_listener()? {
        return Ok(listener);
    }

    let (uid, gid) = resolve_socket_owner(user, group)?;
    trace!(
        "socket_path: {:?}, uid: {:?}, gid: {:?}",
        socket_path, uid, gid
    );

    let _ = fs::remove_file(socket_path);
    let listener = net::UnixListener::bind(socket_path)
        .context("Failed to bind socket")
        .attach(format!("socket_path: {}", socket_path.display()))?;

    fs::set_permissions(
        socket_path,
        std::fs::Permissions::from_mode(DEFAULT_SOCKET_MODE),
    )
    .context("Failed to set socket mode")?;

    if uid.is_some() || gid.is_some() {
        unix_fs::chown(socket_path, uid, gid).context("Failed to set socket ownership")?;
    }

    listener.set_nonblocking(true)?;
    Ok(UnixListener::from_std(listener)?)
}
