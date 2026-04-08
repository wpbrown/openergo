use crate::codec::PostcardCodec;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use std::time::Duration;

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
pub struct UsageIncrement {
    pub click_count: u64,
    pub drag_duration: Duration,
    pub key_count: u64,
    pub left_modifier_duration: ModifierUsageIncrement,
    pub right_modifier_duration: ModifierUsageIncrement,
    pub start: Timestamp,
    pub end: Timestamp,
}

impl UsageIncrement {
    /// Calculate the duration of this increment's time window.
    pub fn duration(&self) -> Duration {
        self.end
            .duration_since(self.start)
            .try_into()
            .unwrap_or_default()
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct ModifierUsageIncrement {
    pub shift: Duration,
    pub ctrl: Duration,
    pub alt: Duration,
    pub meta: Duration,
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
