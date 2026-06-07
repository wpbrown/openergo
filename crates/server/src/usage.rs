use crate::device_events::{ButtonState, DeviceLabel, Event, EventKind};
use bachelor::broadcast::spmc::SpmcBroadcastConsumer;
use bachelor::error::Closed;
use bachelor::signal::mpmc_latched::{
    self, MpmcLatchedSignalConsumer, MpmcLatchedSignalProducer, MpmcLatchedSignalSource,
};
use bachelor::watch::{MpmcWatchRefConsumer, MpmcWatchRefProducer, MpmcWatchRefSource, mpmc_watch};
use evdev::KeyCode;
use futures::FutureExt;
use futures::future::{Either, select};
use shared::model::{ModifierUsageSnapshot, UsageSnapshot};
use std::future::Future;
use std::ops::ControlFlow;
use std::time::Duration;
use tokio::time::{Instant, timeout_at};
use tracing::trace;

#[derive(Debug, Clone)]
pub struct DragConfig {
    pub min_distance: u32,
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

/// Runtime configuration for the usage tracker.
#[derive(Debug, Default, Clone)]
pub struct UsageConfig {
    /// Devices whose events should be ignored when computing usage. Expected
    /// to be small (a handful of entries), so a linear scan beats the
    /// overhead of a hash set.
    pub exclude: Vec<DeviceLabel>,
}

#[derive(Clone, Copy)]
pub enum Modifier {
    Shift,
    Ctrl,
    Alt,
    Meta,
}

const NOTIFY_RATE_LIMIT_FAST: Duration = Duration::from_millis(250);

/// Window during which a follow-up usage event is treated as continuing the
/// previous batch, and during which a lone non-usage event waits for a
/// usage event to absorb it.
const BRIDGE_INTERVAL: Duration = Duration::from_millis(150);

/// A snapshot update or activity ping observed by a [`UsageConsumer`].
///
/// The snapshot itself is not carried in the event; read it with
/// [`UsageConsumer::snapshot`] when handling [`UsageEvent::Usage`].
pub enum UsageEvent {
    Usage,
    Activity,
}

/// Producer-side handle that fans out usage snapshots and activity pings.
#[derive(Clone)]
pub struct UsageSource {
    snapshot_src: MpmcWatchRefSource<UsageSnapshot>,
    activity_src: MpmcLatchedSignalSource,
}

impl UsageSource {
    /// Subscribes a new consumer that ignores any already-latched state.
    pub fn subscribe_forward(&self) -> UsageConsumer {
        UsageConsumer {
            snapshot_rx: self.snapshot_src.subscribe_forward(),
            activity_rx: self.activity_src.subscribe_forward(),
        }
    }
}

/// Consumer-side handle that unifies snapshot changes and activity pings.
pub struct UsageConsumer {
    snapshot_rx: MpmcWatchRefConsumer<UsageSnapshot>,
    activity_rx: MpmcLatchedSignalConsumer,
}

impl UsageConsumer {
    /// Returns the current snapshot value without waiting.
    pub fn snapshot(&self) -> UsageSnapshot {
        self.snapshot_rx.get()
    }

    /// Awaits the next snapshot change or activity ping.
    pub fn changed(&mut self) -> impl Future<Output = Result<UsageEvent, Closed>> + Unpin {
        select(self.snapshot_rx.changed(), self.activity_rx.observe()).map(|either| match either {
            Either::Left((Ok(()), _)) => Ok(UsageEvent::Usage),
            Either::Left((Err(closed), _)) => Err(closed),
            Either::Right(((), _)) => Ok(UsageEvent::Activity),
        })
    }
}

/// Creates a new usage tracker driver.
pub fn create(
    drag: DragConfig,
    config: UsageConfig,
    events_rx: SpmcBroadcastConsumer<Event>,
) -> (UsageSource, Driver) {
    let (snapshot_tx, snapshot_src) = mpmc_watch(UsageSnapshot::default());
    let (activity_tx, activity_src) = mpmc_latched::signal();
    let driver = Driver::new(drag, config, events_rx, snapshot_tx, activity_tx);
    let source = UsageSource {
        snapshot_src,
        activity_src,
    };
    (source, driver)
}

pub struct Driver {
    events_rx: SpmcBroadcastConsumer<Event>,
    usage_tx: MpmcWatchRefProducer<UsageSnapshot>,
    activity_tx: MpmcLatchedSignalProducer,
    controller: Controller,
    exclude: Vec<DeviceLabel>,
    /// Time of the most recent usage event seen. Drives bridge eligibility.
    last_usage_event: Option<Instant>,
    /// Time of the most recent batch publish. Used as the bridged batch's
    /// effective start.
    last_publish: Option<Instant>,
    /// Time of the most recent emission (publish or activity notify). Drives
    /// the activity wait timer.
    last_emission: Option<Instant>,
}

impl Driver {
    fn new(
        drag: DragConfig,
        config: UsageConfig,
        events_rx: SpmcBroadcastConsumer<Event>,
        usage_tx: MpmcWatchRefProducer<UsageSnapshot>,
        activity_tx: MpmcLatchedSignalProducer,
    ) -> Self {
        Self {
            events_rx,
            usage_tx,
            activity_tx,
            controller: Controller::new(drag),
            exclude: config.exclude,
            last_usage_event: None,
            last_publish: None,
            last_emission: None,
        }
    }

    pub async fn run(mut self) {
        let mut next = ControlFlow::Continue(());
        while next.is_continue() {
            let Ok(event) = self.events_rx.recv().await else {
                return;
            };
            let now = Instant::now();
            next = match self.classify(&event) {
                Classified::Usage => self.in_batch(now).await,
                Classified::NonUsage => self.pending_activity(now).await,
            };
        }
    }

    /// Drive a usage batch through to its publish. Non-usage events arriving
    /// during the batch are folded into the snapshot via `classify` but do
    /// not extend the publish deadline.
    async fn in_batch(&mut self, first_event: Instant) -> ControlFlow<()> {
        let batch_first = self.compute_batch_start(first_event);
        let publish_at = batch_first + NOTIFY_RATE_LIMIT_FAST;
        self.last_usage_event = Some(first_event);

        loop {
            match timeout_at(publish_at, self.events_rx.recv()).await {
                Ok(Ok(event)) => {
                    if matches!(self.classify(&event), Classified::Usage) {
                        self.last_usage_event = Some(Instant::now());
                    }
                }
                Ok(Err(Closed)) => return ControlFlow::Break(()),
                Err(_elapsed) => {
                    self.publish_batch(batch_first, publish_at);
                    return ControlFlow::Continue(());
                }
            }
        }
    }

    /// Wait up to `activity_at` for a usage event. If one arrives the
    /// non-usage event is absorbed and we promote to a batch; otherwise we
    /// fire an activity notify. Additional non-usage events during the wait
    /// neither extend the timer nor re-arm it.
    async fn pending_activity(&mut self, trigger: Instant) -> ControlFlow<()> {
        let activity_at = self.compute_activity_at(trigger);

        loop {
            match timeout_at(activity_at, self.events_rx.recv()).await {
                Ok(Ok(event)) => {
                    if matches!(self.classify(&event), Classified::Usage) {
                        return self.in_batch(Instant::now()).await;
                    }
                }
                Ok(Err(Closed)) => return ControlFlow::Break(()),
                Err(_elapsed) => {
                    self.notify_activity(activity_at);
                    return ControlFlow::Continue(());
                }
            }
        }
    }

    fn classify(&mut self, event: &Event) -> Classified {
        let is_usage = !self.exclude.contains(&event.label) && self.controller.handle_event(event);
        if is_usage {
            Classified::Usage
        } else {
            Classified::NonUsage
        }
    }

    /// Compute the batch's effective start, used for both active-duration
    /// accounting and (offset by `NOTIFY_RATE_LIMIT_FAST`) the publish
    /// deadline. A bridge is taken when the triggering event follows the
    /// previous usage event by at most `BRIDGE_INTERVAL`, in which case the
    /// new batch picks up at the previous publish and shares its rate-limit
    /// slot.
    fn compute_batch_start(&self, first_event: Instant) -> Instant {
        self.last_usage_event
            .zip(self.last_publish)
            .and_then(|(last_usage, last_publish)| {
                (first_event.saturating_duration_since(last_usage) <= BRIDGE_INTERVAL)
                    .then_some(last_publish)
            })
            .unwrap_or(first_event)
    }

    /// Wait time for a pending activity notification. Inside the rate-limit
    /// window of a previous emission we hold to the same cadence; otherwise
    /// (true silence) we wait only `BRIDGE_INTERVAL` so that a closely
    /// following usage event can absorb the activity.
    fn compute_activity_at(&self, now: Instant) -> Instant {
        match self.last_emission {
            Some(last) if last + NOTIFY_RATE_LIMIT_FAST > now => last + NOTIFY_RATE_LIMIT_FAST,
            _ => now + BRIDGE_INTERVAL,
        }
    }

    fn publish_batch(&mut self, batch_first: Instant, publish_at: Instant) {
        let active = publish_at.saturating_duration_since(batch_first);
        self.controller.add_active_duration(active);
        let _ = self.usage_tx.set(self.controller.snapshot());
        self.last_publish = Some(publish_at);
        self.last_emission = Some(publish_at);
        trace!("usage driver published snapshot");
    }

    fn notify_activity(&mut self, activity_at: Instant) {
        self.activity_tx.notify();
        self.last_emission = Some(activity_at);
        trace!("usage driver notified activity");
    }
}

enum Classified {
    Usage,
    NonUsage,
}

/// Tracks active drag state for a mouse button.
struct DragTracker {
    start_time: std::time::Instant,
    distance: u32,
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

    fn add_active_duration(&mut self, duration: Duration) {
        self.snapshot.active_duration += duration;
    }

    fn handle_event(&mut self, event: &Event) -> bool {
        match event.kind {
            EventKind::MouseMoveX(dx) => self.handle_mouse_move(dx),
            EventKind::MouseMoveY(dy) => self.handle_mouse_move(dy),
            EventKind::MousePress { button, state } => self.handle_mouse_button(button, state),
            EventKind::KeyPress { key, state } => self.handle_key(key, state),
            EventKind::MouseScrollNotch(value) => self.handle_mouse_scroll(value),
            EventKind::MouseScrollHiRes(_) => false,
        }
    }

    fn handle_mouse_move(&mut self, delta: i32) -> bool {
        if let Some(ref mut drag) = self.active_drag {
            drag.distance = drag.distance.saturating_add(delta.unsigned_abs());
        }

        false
    }

    fn handle_mouse_scroll(&mut self, value: i32) -> bool {
        let ticks = u64::from(value.unsigned_abs());
        self.snapshot.scroll_count = self.snapshot.scroll_count.saturating_add(ticks);
        ticks > 0
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
