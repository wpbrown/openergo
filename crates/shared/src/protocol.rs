use crate::codec::PostcardCodec;
use crate::model::UsageDelta;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use std::time::Duration;

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
