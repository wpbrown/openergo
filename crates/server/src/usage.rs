use crate::device_events::{ButtonState, Event};
use bachelor::{
    broadcast::spmc::SpmcBroadcastConsumer,
    error::Closed,
    watch::{mpmc_watch, MpmcWatchRefProducer, MpmcWatchRefSource},
};
use evdev::KeyCode;
use shared::model::{ModifierUsageSnapshot, UsageSnapshot};
use std::time::Duration;
use tokio::time::{Instant, timeout_at};

#[derive(Debug, Clone)]
pub struct DragConfig {
    pub min_distance: i32,
    pub min_duration: Duration,
}

impl Default for DragConfig {
    fn default() -> Self {
        Self {
            min_distance: 5,
            min_duration: Duration::from_millis(100),
        }
    }
}

#[derive(Clone, Copy)]
pub enum Modifier {
    Shift,
    Ctrl,
    Alt,
    Meta,
}

const NOTIFY_RATE_LIMIT_FAST: Duration = Duration::from_millis(250);

/// Creates a new usage tracker driver.
pub fn create(
    config: DragConfig,
    events_rx: SpmcBroadcastConsumer<Event>,
) -> (MpmcWatchRefSource<UsageSnapshot>, Driver) {
    let (producer, source) = mpmc_watch(UsageSnapshot::default());
    let driver = Driver::new(config, events_rx, producer);
    (source, driver)
}

pub struct Driver {
    events_rx: SpmcBroadcastConsumer<Event>,
    usage_tx: MpmcWatchRefProducer<UsageSnapshot>,
    controller: Controller,
}

impl Driver {
    fn new(
        config: DragConfig,
        events_rx: SpmcBroadcastConsumer<Event>,
        usage_tx: MpmcWatchRefProducer<UsageSnapshot>,
    ) -> Self {
        Self {
            events_rx,
            usage_tx,
            controller: Controller::new(config),
        }
    }

    pub async fn run(mut self) {
        let mut publish_not_before = Instant::now();

        loop {
            let Ok(event) = self.events_rx.recv().await else {
                return;
            };

            if self.controller.handle_event(&event) {
                if Instant::now() < publish_not_before {
                    loop {
                        match timeout_at(publish_not_before, self.events_rx.recv()).await {
                            Ok(Ok(event)) => {
                                self.controller.handle_event(&event);
                            }
                            Ok(Err(Closed)) => return,
                            Err(_elapsed) => break,
                        }
                    }
                }
                self.publish_latest();
                publish_not_before = Instant::now() + NOTIFY_RATE_LIMIT_FAST;
            }
        }
    }

    fn publish_latest(&self) {
        let _ = self.usage_tx.set(self.controller.snapshot());
        log::trace!("usage driver published snapshot");
    }
}

/// Tracks active drag state for a mouse button.
struct DragTracker {
    start_time: std::time::Instant,
    distance: i32,
}

/// Tracks when a modifier key was pressed.
struct ModifierTracker {
    start_time: std::time::Instant,
}

/// Synchronous usage logic controller.
struct Controller {
    config: DragConfig,
    snapshot: UsageSnapshot,
    active_drag: Option<DragTracker>,
    left_shift: Option<ModifierTracker>,
    right_shift: Option<ModifierTracker>,
    left_ctrl: Option<ModifierTracker>,
    right_ctrl: Option<ModifierTracker>,
    left_alt: Option<ModifierTracker>,
    right_alt: Option<ModifierTracker>,
    left_meta: Option<ModifierTracker>,
    right_meta: Option<ModifierTracker>,
}

impl Controller {
    fn new(config: DragConfig) -> Self {
        Self {
            config,
            snapshot: UsageSnapshot::default(),
            active_drag: None,
            left_shift: None,
            right_shift: None,
            left_ctrl: None,
            right_ctrl: None,
            left_alt: None,
            right_alt: None,
            left_meta: None,
            right_meta: None,
        }
    }

    fn snapshot(&self) -> UsageSnapshot {
        self.snapshot.clone()
    }

    fn handle_event(&mut self, event: &Event) -> bool {
        match event {
            Event::MouseMoveX(dx) => self.handle_mouse_move(*dx),
            Event::MouseMoveY(dy) => self.handle_mouse_move(*dy),
            Event::MousePress { button, state } => self.handle_mouse_button(*button, *state),
            Event::KeyPress { key, state } => self.handle_key(*key, *state),
            Event::MouseScroll(_) => false,
        }
    }

    fn handle_mouse_move(&mut self, delta: i32) -> bool {
        if let Some(ref mut drag) = self.active_drag {
            drag.distance += delta.abs();
        }

        false
    }

    fn handle_mouse_button(&mut self, button: KeyCode, button_state: ButtonState) -> bool {
        let now = std::time::Instant::now();
        let is_left_button = button == KeyCode::BTN_LEFT;

        match button_state {
            ButtonState::Down => {
                if is_left_button && self.active_drag.is_none() {
                    self.active_drag = Some(DragTracker {
                        start_time: now,
                        distance: 0,
                    });
                }

                false
            }
            ButtonState::Up => {
                let was_drag = is_left_button
                    && self.active_drag.take().is_some_and(|drag| {
                        let duration = now.duration_since(drag.start_time);
                        let dominated_thresholds = drag.distance >= self.config.min_distance
                            && duration >= self.config.min_duration;
                        if dominated_thresholds {
                            self.snapshot.drag_duration += duration;
                        }
                        dominated_thresholds
                    });

                if !was_drag {
                    self.snapshot.click_count = self.snapshot.click_count.saturating_add(1);
                }

                true
            }
        }
    }

    fn handle_key(&mut self, key: KeyCode, key_state: ButtonState) -> bool {
        let now = std::time::Instant::now();

        if let Some((side, modifier)) = classify_modifier(key) {
            match key_state {
                ButtonState::Down => {
                    let tracker = self.modifier_tracker_mut(side, modifier);
                    if tracker.is_none() {
                        *tracker = Some(ModifierTracker { start_time: now });
                    }
                    false
                }
                ButtonState::Up => {
                    if let Some(mt) = self.modifier_tracker_mut(side, modifier).take() {
                        let duration = now.duration_since(mt.start_time);
                        match side {
                            Side::Left => self.add_left_modifier_duration(modifier, duration),
                            Side::Right => self.add_right_modifier_duration(modifier, duration),
                        }
                        true
                    } else {
                        false
                    }
                }
            }
        } else if key_state == ButtonState::Down {
            self.snapshot.key_count = self.snapshot.key_count.saturating_add(1);
            true
        } else {
            false
        }
    }

    fn add_left_modifier_duration(&mut self, modifier: Modifier, duration: Duration) {
        Self::add_modifier_duration(
            &mut self.snapshot.left_modifier_duration,
            modifier,
            duration,
        );
    }

    fn add_right_modifier_duration(&mut self, modifier: Modifier, duration: Duration) {
        Self::add_modifier_duration(
            &mut self.snapshot.right_modifier_duration,
            modifier,
            duration,
        );
    }

    fn add_modifier_duration(
        snapshot: &mut ModifierUsageSnapshot,
        modifier: Modifier,
        duration: Duration,
    ) {
        match modifier {
            Modifier::Shift => snapshot.shift += duration,
            Modifier::Ctrl => snapshot.ctrl += duration,
            Modifier::Alt => snapshot.alt += duration,
            Modifier::Meta => snapshot.meta += duration,
        }
    }

    fn modifier_tracker_mut(
        &mut self,
        side: Side,
        modifier: Modifier,
    ) -> &mut Option<ModifierTracker> {
        match (side, modifier) {
            (Side::Left, Modifier::Shift) => &mut self.left_shift,
            (Side::Right, Modifier::Shift) => &mut self.right_shift,
            (Side::Left, Modifier::Ctrl) => &mut self.left_ctrl,
            (Side::Right, Modifier::Ctrl) => &mut self.right_ctrl,
            (Side::Left, Modifier::Alt) => &mut self.left_alt,
            (Side::Right, Modifier::Alt) => &mut self.right_alt,
            (Side::Left, Modifier::Meta) => &mut self.left_meta,
            (Side::Right, Modifier::Meta) => &mut self.right_meta,
        }
    }
}

#[derive(Clone, Copy)]
enum Side {
    Left,
    Right,
}

fn classify_modifier(key: KeyCode) -> Option<(Side, Modifier)> {
    match key {
        KeyCode::KEY_LEFTSHIFT => Some((Side::Left, Modifier::Shift)),
        KeyCode::KEY_RIGHTSHIFT => Some((Side::Right, Modifier::Shift)),
        KeyCode::KEY_LEFTCTRL => Some((Side::Left, Modifier::Ctrl)),
        KeyCode::KEY_RIGHTCTRL => Some((Side::Right, Modifier::Ctrl)),
        KeyCode::KEY_LEFTALT => Some((Side::Left, Modifier::Alt)),
        KeyCode::KEY_RIGHTALT => Some((Side::Right, Modifier::Alt)),
        KeyCode::KEY_LEFTMETA => Some((Side::Left, Modifier::Meta)),
        KeyCode::KEY_RIGHTMETA => Some((Side::Right, Modifier::Meta)),
        _ => None,
    }
}
