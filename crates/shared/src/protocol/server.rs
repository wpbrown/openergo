use crate::codec::PostcardCodec;
use crate::model::UsageDelta;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Wire-format protocol version. Bump on any incompatible change to
/// `Command`, `ServerMessage`, or framing.
pub const PROTOCOL_VERSION: u32 = 3;

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
    Activity,
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

mod diag {
    use crate::{model::ModifierUsageDelta, protocol::server::UsageIncrement};
    use std::{fmt, time::Duration};

    impl fmt::Display for UsageIncrement {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            write!(
                f,
                "{}-{}",
                self.start.strftime("%H:%M:%S"),
                self.end.strftime("%H:%M:%S")
            )?;

            write_count(f, "clicks", self.delta.click_count)?;
            write_duration(f, "drag", self.delta.drag_duration)?;
            write_count(f, "keys.l", self.delta.key_count.left)?;
            write_count(f, "keys.r", self.delta.key_count.right)?;
            write_count(f, "keys.o", self.delta.key_count.other)?;
            write_count(f, "scroll", self.delta.scroll_count)?;
            write_modifier_duration(
                f,
                ["lmod.shift", "lmod.ctrl", "lmod.alt", "lmod.meta"],
                self.delta.left_modifier_duration,
            )?;
            write_modifier_duration(
                f,
                ["rmod.shift", "rmod.ctrl", "rmod.alt", "rmod.meta"],
                self.delta.right_modifier_duration,
            )?;
            write_duration(f, "active", self.delta.active_duration)?;

            Ok(())
        }
    }

    fn write_count(f: &mut fmt::Formatter<'_>, label: &str, value: u64) -> fmt::Result {
        if value == 0 {
            return Ok(());
        }
        write!(f, " {label}={value}")
    }

    fn write_duration(f: &mut fmt::Formatter<'_>, label: &str, duration: Duration) -> fmt::Result {
        if duration.is_zero() {
            return Ok(());
        }
        write!(f, " {label}=")?;
        write_compact_duration(f, duration)
    }

    fn write_modifier_duration(
        f: &mut fmt::Formatter<'_>,
        labels: [&str; 4],
        duration: ModifierUsageDelta,
    ) -> fmt::Result {
        write_duration(f, labels[0], duration.shift)?;
        write_duration(f, labels[1], duration.ctrl)?;
        write_duration(f, labels[2], duration.alt)?;
        write_duration(f, labels[3], duration.meta)
    }

    fn write_compact_duration(f: &mut fmt::Formatter<'_>, duration: Duration) -> fmt::Result {
        if duration.as_secs() > 0 && duration.subsec_nanos() == 0 {
            write!(f, "{}s", duration.as_secs())
        } else if duration.as_millis() > 0 {
            write!(f, "{}ms", duration.as_millis())
        } else if duration.as_micros() > 0 {
            write!(f, "{}us", duration.as_micros())
        } else {
            write!(f, "{}ns", duration.as_nanos())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{KeyCount, ModifierUsageDelta};

    fn ts(second: i64) -> Timestamp {
        Timestamp::from_second(second).expect("valid test timestamp")
    }

    #[test]
    fn display_shows_only_non_zero_updates() {
        let increment = UsageIncrement::new(
            UsageDelta {
                click_count: 2,
                drag_duration: Duration::from_millis(3),
                key_count: KeyCount {
                    left: 1,
                    right: 0,
                    other: 4,
                },
                scroll_count: 0,
                left_modifier_duration: ModifierUsageDelta {
                    shift: Duration::from_millis(5),
                    ..ModifierUsageDelta::default()
                },
                right_modifier_duration: ModifierUsageDelta {
                    ctrl: Duration::from_micros(250),
                    ..ModifierUsageDelta::default()
                },
                active_duration: Duration::from_secs(2),
            },
            ts(3661),
            ts(3662),
        );

        assert_eq!(
            increment.to_string(),
            "01:01:01-01:01:02 clicks=2 drag=3ms keys.l=1 keys.o=4 lmod.shift=5ms rmod.ctrl=250us active=2s"
        );
    }

    #[test]
    fn display_omits_date_and_zero_updates() {
        let increment = UsageIncrement::new(UsageDelta::default(), ts(3661), ts(3662));

        assert_eq!(increment.to_string(), "01:01:01-01:01:02");
    }
}
