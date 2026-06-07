use crate::codec::PostcardCodec;
use crate::model::UsageDelta;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// Wire-format protocol version. Bump on any incompatible change to
/// `Command`, `ServerMessage`, or framing.
pub const PROTOCOL_VERSION: u32 = 5;

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

            write_count(f, "click.l", self.delta.left.click_count)?;
            write_count(f, "click.r", self.delta.right.click_count)?;
            write_duration(f, "drag.l", self.delta.left.drag_duration)?;
            write_duration(f, "drag.r", self.delta.right.drag_duration)?;
            write_count(f, "keys.l", self.delta.left.key_count)?;
            write_count(f, "keys.r", self.delta.right.key_count)?;
            write_count(f, "keys.u", self.delta.unclassified_key_count)?;
            write_count(f, "combo.l", self.delta.left.modifier.same_hand_combo)?;
            write_count(f, "combo.r", self.delta.right.modifier.same_hand_combo)?;
            write_count(f, "combo.u", self.delta.unclassified_key_combo)?;
            write_count(f, "scroll.l", self.delta.left.scroll_count)?;
            write_count(f, "scroll.r", self.delta.right.scroll_count)?;
            write_modifier(
                f,
                [
                    "lmod.shift",
                    "lmod.ctrl",
                    "lmod.alt",
                    "lmod.meta",
                    "lmod.multi",
                    "lmod.combo",
                ],
                self.delta.left.modifier,
            )?;
            write_modifier(
                f,
                [
                    "rmod.shift",
                    "rmod.ctrl",
                    "rmod.alt",
                    "rmod.meta",
                    "rmod.multi",
                    "rmod.combo",
                ],
                self.delta.right.modifier,
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

    fn write_modifier(
        f: &mut fmt::Formatter<'_>,
        labels: [&str; 6],
        duration: ModifierUsageDelta,
    ) -> fmt::Result {
        write_duration(f, labels[0], duration.shift)?;
        write_duration(f, labels[1], duration.ctrl)?;
        write_duration(f, labels[2], duration.alt)?;
        write_duration(f, labels[3], duration.meta)?;
        write_duration(f, labels[4], duration.multi)?;
        write_count(f, labels[5], duration.same_hand_combo)
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
    use crate::model::{HandUsageDelta, ModifierUsageDelta};

    fn ts(second: i64) -> Timestamp {
        Timestamp::from_second(second).expect("valid test timestamp")
    }

    #[test]
    fn display_shows_only_non_zero_updates() {
        let increment = UsageIncrement::new(
            UsageDelta {
                left: HandUsageDelta {
                    click_count: 2,
                    drag_duration: Duration::from_millis(3),
                    key_count: 1,
                    modifier: ModifierUsageDelta {
                        shift: Duration::from_millis(5),
                        multi: Duration::from_millis(8),
                        same_hand_combo: 9,
                        ..ModifierUsageDelta::default()
                    },
                    ..HandUsageDelta::default()
                },
                right: HandUsageDelta {
                    click_count: 3,
                    scroll_count: 4,
                    modifier: ModifierUsageDelta {
                        ctrl: Duration::from_micros(250),
                        ..ModifierUsageDelta::default()
                    },
                    ..HandUsageDelta::default()
                },
                unclassified_key_count: 5,
                unclassified_key_combo: 6,
                active_duration: Duration::from_secs(2),
            },
            ts(3661),
            ts(3662),
        );

        assert_eq!(
            increment.to_string(),
            "01:01:01-01:01:02 click.l=2 click.r=3 drag.l=3ms keys.l=1 keys.u=5 combo.l=9 combo.u=6 scroll.r=4 lmod.shift=5ms lmod.multi=8ms lmod.combo=9 rmod.ctrl=250us active=2s"
        );
    }

    #[test]
    fn display_omits_date_and_zero_updates() {
        let increment = UsageIncrement::new(UsageDelta::default(), ts(3661), ts(3662));

        assert_eq!(increment.to_string(), "01:01:01-01:01:02");
    }
}
