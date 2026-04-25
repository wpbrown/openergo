use crate::codec::PostcardCodec;
use crate::model::UsageDelta;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use std::io;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Wire-format protocol version. Bump on any incompatible change to
/// `Command`, `ServerMessage`, or framing.
pub const PROTOCOL_VERSION: u32 = 1;

/// Send the protocol version to the peer.
pub async fn write_protocol_version<W: AsyncWriteExt + Unpin>(w: &mut W) -> io::Result<()> {
    w.write_all(&PROTOCOL_VERSION.to_ne_bytes()).await
}

/// Read the peer's protocol version. Returns `None` if it matches ours,
/// or `Some(peer_version)` if it does not.
pub async fn read_protocol_version<R: AsyncReadExt + Unpin>(r: &mut R) -> io::Result<Option<u32>> {
    let mut buf = [0u8; 4];
    r.read_exact(&mut buf).await?;
    let peer = u32::from_ne_bytes(buf);
    Ok((peer != PROTOCOL_VERSION).then_some(peer))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageIncrement {
    pub delta: UsageDelta,
    pub start: Timestamp,
    pub end: Timestamp,
}

impl UsageIncrement {
    pub fn new(delta: UsageDelta, start: Timestamp, end: Timestamp) -> Self {
        Self { delta, start, end }
    }

    /// Calculate the duration of this increment's time window.
    pub fn duration(&self) -> Duration {
        self.end
            .duration_since(self.start)
            .try_into()
            .unwrap_or_default()
    }
}

pub type ClientCodec = PostcardCodec<ServerMessage, Command>;
pub type ServerCodec = PostcardCodec<Command, ServerMessage>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Command {
    ConfigureDwellClick(DwellServerConfig),
    PauseAutoClick,
    ResumeAutoClick,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServerMessage {
    NewUsage(Box<UsageIncrement>),
    Click,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DwellServerConfig {
    pub dwell_duration_threshold: Duration,
    pub movement_threshold: i32,
}

impl Default for DwellServerConfig {
    fn default() -> Self {
        Self {
            dwell_duration_threshold: Duration::from_millis(350),
            movement_threshold: 10,
        }
    }
}
