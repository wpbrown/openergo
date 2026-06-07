use crate::assets;
use crate::credit::utilization::{CreditEvent, CreditEventConsumer, CreditKind};
use crate::sound::SoundPlayer;
use std::future::Future;
use tracing::debug;

mod notify;

#[derive(Debug, Clone, Copy)]
pub struct NotificationSettings {
    pub notifications: bool,
    pub sounds: bool,
}

impl NotificationSettings {
    pub fn new(notifications: bool, sounds: bool) -> Self {
        Self {
            notifications,
            sounds,
        }
    }

    pub fn any(&self) -> bool {
        self.notifications || self.sounds
    }
}

pub fn create(
    config: NotificationSettings,
    events: CreditEventConsumer,
) -> impl Future<Output = ()> + use<> {
    run(config, events)
}

async fn run(config: NotificationSettings, mut events: CreditEventConsumer) {
    let player = if config.sounds {
        match SoundPlayer::new() {
            Ok(p) => Some(p),
            Err(e) => {
                tracing::warn!("Failed to open audio output for notifications: {e}");
                None
            }
        }
    } else {
        None
    };

    let mut hits = LimitHits::default();

    debug!(sound = player.is_some(), "notifications driver started");

    loop {
        let ev = match events.recv().await {
            Err(_) => return,
            Ok(ev) => ev,
        };

        handle_event(ev, &config, &mut hits, player.as_ref());
    }
}

fn handle_event(
    ev: CreditEvent,
    config: &NotificationSettings,
    hits: &mut LimitHits,
    player: Option<&SoundPlayer>,
) {
    match ev {
        CreditEvent::Reached { kind } => {
            hits.set(kind, true);
            if config.notifications {
                notify::show(format!("{} credit limit reached", kind_label(kind)));
            }
            if let Some(p) = player {
                p.play(limit_sound(kind));
            }
        }
        CreditEvent::Escalation { kind, level } => {
            if config.notifications {
                notify::show(format!(
                    "{} credit at {}%",
                    kind_label(kind),
                    u16::from(level) * 10 + 100
                ));
            }
            if let Some(p) = player {
                p.play_repeat(assets::ESCALATION, u32::from(level) * 3);
            }
        }
        CreditEvent::Reset { kind } => {
            if !hits.get(kind) {
                return;
            }
            hits.set(kind, false);
            if config.notifications {
                notify::show(format!("{} credit reset", kind_label(kind)));
            }
            if let (Some(p), Some(sound)) = (player, complete_sound(kind)) {
                p.play(sound);
            }
        }
    }
}

#[derive(Debug, Default)]
struct LimitHits {
    rest: bool,
    breaks: bool,
    day: bool,
}

impl LimitHits {
    fn get(&self, kind: CreditKind) -> bool {
        match kind {
            CreditKind::Rest => self.rest,
            CreditKind::Breaks => self.breaks,
            CreditKind::Day => self.day,
        }
    }

    fn set(&mut self, kind: CreditKind, v: bool) {
        match kind {
            CreditKind::Rest => self.rest = v,
            CreditKind::Breaks => self.breaks = v,
            CreditKind::Day => self.day = v,
        }
    }
}

fn limit_sound(kind: CreditKind) -> &'static [u8] {
    match kind {
        CreditKind::Rest => assets::REST_LIMIT,
        CreditKind::Breaks => assets::BREAK_LIMIT,
        CreditKind::Day => assets::DAY_LIMIT,
    }
}

/// Day has no completion sound; only rest and break do.
fn complete_sound(kind: CreditKind) -> Option<&'static [u8]> {
    match kind {
        CreditKind::Rest => Some(assets::REST_COMPLETE),
        CreditKind::Breaks => Some(assets::BREAK_COMPLETE),
        CreditKind::Day => None,
    }
}

fn kind_label(kind: CreditKind) -> &'static str {
    match kind {
        CreditKind::Rest => "Rest",
        CreditKind::Breaks => "Break",
        CreditKind::Day => "Day",
    }
}
