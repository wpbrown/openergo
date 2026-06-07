use std::io;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub mod client;
pub mod server;

/// Send the protocol version to the peer.
pub async fn write_protocol_version<W: AsyncWriteExt + Unpin>(
    w: &mut W,
    version: u32,
) -> io::Result<()> {
    w.write_all(&version.to_ne_bytes()).await
}

/// Read the peer's protocol version. Returns `None` if it matches `version`,
/// or `Some(peer_version)` if it does not.
pub async fn read_protocol_version<R: AsyncReadExt + Unpin>(
    r: &mut R,
    version: u32,
) -> io::Result<Option<u32>> {
    let mut buf = [0u8; 4];
    r.read_exact(&mut buf).await?;
    let peer = u32::from_ne_bytes(buf);
    Ok((peer != version).then_some(peer))
}
