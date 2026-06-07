pub mod key_hand;

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
use key_hand::{KeyHand, KeyHandUsageConfig};
use shared::model::{HandUsageSnapshot, ModifierUsageSnapshot, UsageSnapshot};
use smallvec::SmallVec;
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
    pub key_hand: KeyHandUsageConfig,
    pub pointer_hand: PointerHandUsageConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PointerHand {
    Left,
    #[default]
    Right,
}

#[derive(Debug, Clone, Default)]
pub struct PointerHandUsageConfig {
    pub default_hand: PointerHand,
    pub device_hands: Vec<(DeviceLabel, PointerHand)>,
}

impl PointerHandUsageConfig {
    pub fn hand_for(&self, label: DeviceLabel) -> PointerHand {
        self.device_hands
            .iter()
            .find_map(|(device_label, hand)| (*device_label == label).then_some(*hand))
            .unwrap_or(self.default_hand)
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
        let UsageConfig {
            exclude,
            key_hand,
            pointer_hand,
        } = config;
        Self {
            events_rx,
            usage_tx,
            activity_tx,
            controller: Controller::new(drag, key_hand, pointer_hand),
            exclude,
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
            next = match self.classify(&event, now) {
                Classified::Usage => self.in_batch(now).await,
                Classified::NonUsage => self.pending_activity(now).await,
            };
        }
    }

    /// Drive a usage batch through to its publish. Non-usage events arriving
    /// during the batch are folded into the snapshot via `classify` but do
    /// not extend the publish deadline.
    async fn in_batch(&mut self, first_event: Instant) -> ControlFlow<()> {
        let mut batch_first = self.compute_batch_start(first_event);
        let mut publish_at = batch_first + NOTIFY_RATE_LIMIT_FAST;
        self.last_usage_event = Some(first_event);

        loop {
            match timeout_at(publish_at, self.events_rx.recv()).await {
                Ok(Ok(event)) => {
                    let now = Instant::now();
                    if matches!(self.classify(&event, now), Classified::Usage) {
                        self.last_usage_event = Some(now);
                    }
                }
                Ok(Err(Closed)) => return ControlFlow::Break(()),
                Err(_elapsed) => {
                    self.publish_batch(batch_first, publish_at);
                    if !self.controller.has_streaming_usage() {
                        return ControlFlow::Continue(());
                    }

                    batch_first = publish_at;
                    publish_at = batch_first + NOTIFY_RATE_LIMIT_FAST;
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
                    let now = Instant::now();
                    if matches!(self.classify(&event, now), Classified::Usage) {
                        return self.in_batch(now).await;
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

    fn classify(&mut self, event: &Event, now: Instant) -> Classified {
        let is_usage =
            !self.exclude.contains(&event.label) && self.controller.handle_event(event, now);
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
        let _ = self.usage_tx.set(self.controller.snapshot_at(publish_at));
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
    /// Start of the currently unaccounted drag segment. This remains at
    /// the button-down time until the drag qualifies so the first flush preserves
    /// button-down accounting. A long pre-qualification drag can cause a burst of
    /// duration to be accounted at once, but this is expected to be rare.
    last_accounted: Instant,
    distance: u32,
    qualified: bool,
}

impl DragTracker {
    fn needs_accounting(&self, config: &DragConfig) -> bool {
        self.qualified || self.distance >= config.min_distance
    }

    fn update_qualification(&mut self, config: &DragConfig, now: Instant) -> bool {
        if !self.qualified {
            self.qualified = self.distance >= config.min_distance
                && now.saturating_duration_since(self.last_accounted) >= config.min_duration;
        }

        self.qualified
    }
}

struct ActiveDrag {
    label: DeviceLabel,
    hand: PointerHand,
    tracker: DragTracker,
}

/// Tracks the start of the currently unaccounted modifier hold segment.
struct ModifierTracker {
    last_accounted: Instant,
}

/// Synchronous usage logic controller.
struct Controller {
    config: DragConfig,
    key_hand: KeyHandUsageConfig,
    pointer_hand: PointerHandUsageConfig,
    snapshot: UsageSnapshot,
    active_drags: SmallVec<[ActiveDrag; 2]>,
    left_shift: Option<ModifierTracker>,
    right_shift: Option<ModifierTracker>,
    left_ctrl: Option<ModifierTracker>,
    right_ctrl: Option<ModifierTracker>,
    left_alt: Option<ModifierTracker>,
    right_alt: Option<ModifierTracker>,
    left_meta: Option<ModifierTracker>,
    right_meta: Option<ModifierTracker>,
    left_multi: Option<ModifierTracker>,
    right_multi: Option<ModifierTracker>,
}

impl Controller {
    fn new(
        config: DragConfig,
        key_hand: KeyHandUsageConfig,
        pointer_hand: PointerHandUsageConfig,
    ) -> Self {
        Self {
            config,
            key_hand,
            pointer_hand,
            snapshot: UsageSnapshot::default(),
            active_drags: SmallVec::new(),
            left_shift: None,
            right_shift: None,
            left_ctrl: None,
            right_ctrl: None,
            left_alt: None,
            right_alt: None,
            left_meta: None,
            right_meta: None,
            left_multi: None,
            right_multi: None,
        }
    }

    fn snapshot_at(&mut self, publish_at: Instant) -> UsageSnapshot {
        self.flush_active_modifiers(publish_at);
        self.flush_active_drag(publish_at);
        self.snapshot.clone()
    }

    fn has_streaming_usage(&self) -> bool {
        self.has_active_modifiers() || self.has_active_drag()
    }

    fn has_active_modifiers(&self) -> bool {
        self.has_active_modifier_on(Side::Left) || self.has_active_modifier_on(Side::Right)
    }

    fn has_active_drag(&self) -> bool {
        self.active_drags
            .iter()
            .any(|drag| drag.tracker.needs_accounting(&self.config))
    }

    fn add_active_duration(&mut self, duration: Duration) {
        self.snapshot.active_duration += duration;
    }

    fn handle_event(&mut self, event: &Event, now: Instant) -> bool {
        match event.kind {
            EventKind::MouseMoveX(dx) => self.handle_mouse_move(event.label, dx, now),
            EventKind::MouseMoveY(dy) => self.handle_mouse_move(event.label, dy, now),
            EventKind::MousePress { button, state } => {
                self.handle_mouse_button(event.label, button, state, now)
            }
            EventKind::KeyPress { key, state } => self.handle_key(event.label, key, state, now),
            EventKind::MouseScrollNotch(value) => self.handle_mouse_scroll(event.label, value),
            EventKind::MouseScrollHiRes(_) => false,
        }
    }

    fn active_modifier_count(&self, side: Side) -> usize {
        match side {
            Side::Left => [
                &self.left_shift,
                &self.left_ctrl,
                &self.left_alt,
                &self.left_meta,
            ]
            .iter()
            .filter(|tracker| tracker.is_some())
            .count(),
            Side::Right => [
                &self.right_shift,
                &self.right_ctrl,
                &self.right_alt,
                &self.right_meta,
            ]
            .iter()
            .filter(|tracker| tracker.is_some())
            .count(),
        }
    }

    fn has_active_modifier_on(&self, side: Side) -> bool {
        self.active_modifier_count(side) > 0
    }

    fn handle_mouse_move(&mut self, label: DeviceLabel, delta: i32, now: Instant) -> bool {
        let config = &self.config;
        if let Some(active_drag) = self
            .active_drags
            .iter_mut()
            .find(|active_drag| active_drag.label == label)
        {
            active_drag.tracker.distance = active_drag
                .tracker
                .distance
                .saturating_add(delta.unsigned_abs());
            let distance_ready = active_drag.tracker.distance >= config.min_distance;
            if distance_ready {
                active_drag.tracker.update_qualification(config, now);
            }
            return distance_ready;
        }

        false
    }

    fn handle_mouse_scroll(&mut self, label: DeviceLabel, value: i32) -> bool {
        let ticks = u64::from(value.unsigned_abs());
        let hand = self.pointer_hand.hand_for(label);
        let usage = self.hand_usage_mut(hand);
        usage.scroll_count = usage.scroll_count.saturating_add(ticks);
        ticks > 0
    }

    fn handle_mouse_button(
        &mut self,
        label: DeviceLabel,
        button: KeyCode,
        button_state: ButtonState,
        now: Instant,
    ) -> bool {
        let is_left_button = button == KeyCode::BTN_LEFT;

        match button_state {
            ButtonState::Down => {
                if is_left_button
                    && !self
                        .active_drags
                        .iter()
                        .any(|active_drag| active_drag.label == label)
                {
                    self.active_drags.push(ActiveDrag {
                        label,
                        hand: self.pointer_hand.hand_for(label),
                        tracker: DragTracker {
                            last_accounted: now,
                            distance: 0,
                            qualified: false,
                        },
                    });
                }

                false
            }
            ButtonState::Up => {
                let click_hand = if is_left_button {
                    let config = &self.config;
                    let active_drag = self
                        .active_drags
                        .iter()
                        .position(|active_drag| active_drag.label == label)
                        .map(|index| self.active_drags.remove(index));

                    if let Some(mut active_drag) = active_drag {
                        if active_drag.tracker.update_qualification(config, now) {
                            self.hand_usage_mut(active_drag.hand).drag_duration +=
                                now.saturating_duration_since(active_drag.tracker.last_accounted);
                            None
                        } else {
                            Some(active_drag.hand)
                        }
                    } else {
                        Some(self.pointer_hand.hand_for(label))
                    }
                } else {
                    Some(self.pointer_hand.hand_for(label))
                };

                if let Some(hand) = click_hand {
                    let usage = self.hand_usage_mut(hand);
                    usage.click_count = usage.click_count.saturating_add(1);
                }

                true
            }
        }
    }

    fn flush_active_drag(&mut self, publish_at: Instant) {
        let config = &self.config;
        for active_drag in &mut self.active_drags {
            if !active_drag.tracker.update_qualification(config, publish_at) {
                continue;
            }

            let duration = publish_at.saturating_duration_since(active_drag.tracker.last_accounted);
            active_drag.tracker.last_accounted = publish_at;
            match active_drag.hand {
                PointerHand::Left => self.snapshot.left.drag_duration += duration,
                PointerHand::Right => self.snapshot.right.drag_duration += duration,
            }
        }
    }

    fn handle_key(
        &mut self,
        label: DeviceLabel,
        key: KeyCode,
        key_state: ButtonState,
        now: Instant,
    ) -> bool {
        if let Some((side, modifier)) = classify_modifier(key) {
            match key_state {
                ButtonState::Down => {
                    if self.modifier_tracker(side, modifier).is_some() {
                        return false;
                    }

                    let previous_count = self.active_modifier_count(side);
                    let tracker = self.modifier_tracker_mut(side, modifier);
                    *tracker = Some(ModifierTracker {
                        last_accounted: now,
                    });
                    if previous_count == 1 {
                        *self.multi_tracker_mut(side) = Some(ModifierTracker {
                            last_accounted: now,
                        });
                    }

                    true
                }
                ButtonState::Up => {
                    let previous_count = self.active_modifier_count(side);
                    let Some(mt) = self.modifier_tracker_mut(side, modifier).take() else {
                        return false;
                    };
                    if previous_count == 2 {
                        self.finish_multi_modifier_duration(side, now);
                    }

                    let duration = now.saturating_duration_since(mt.last_accounted);
                    match side {
                        Side::Left => self.add_left_modifier_duration(modifier, duration),
                        Side::Right => self.add_right_modifier_duration(modifier, duration),
                    }
                    true
                }
            }
        } else if key_state == ButtonState::Down {
            self.add_non_modifier_key(label, key);
            true
        } else {
            false
        }
    }

    fn add_non_modifier_key(&mut self, label: DeviceLabel, key: KeyCode) {
        match self.key_hand.profile_for(label).classify(key) {
            KeyHand::Left if self.has_active_modifier_on(Side::Left) => {
                self.snapshot.left.modifier.same_hand_combo = self
                    .snapshot
                    .left
                    .modifier
                    .same_hand_combo
                    .saturating_add(1);
            }
            KeyHand::Left => {
                self.snapshot.left.key_count = self.snapshot.left.key_count.saturating_add(1);
            }
            KeyHand::Right if self.has_active_modifier_on(Side::Right) => {
                self.snapshot.right.modifier.same_hand_combo = self
                    .snapshot
                    .right
                    .modifier
                    .same_hand_combo
                    .saturating_add(1);
            }
            KeyHand::Right => {
                self.snapshot.right.key_count = self.snapshot.right.key_count.saturating_add(1);
            }
            KeyHand::Unclassified if self.has_active_modifiers() => {
                self.snapshot.unclassified_key_combo =
                    self.snapshot.unclassified_key_combo.saturating_add(1);
            }
            KeyHand::Unclassified => {
                self.snapshot.unclassified_key_count =
                    self.snapshot.unclassified_key_count.saturating_add(1);
            }
        }
    }

    fn hand_usage_mut(&mut self, hand: PointerHand) -> &mut HandUsageSnapshot {
        match hand {
            PointerHand::Left => &mut self.snapshot.left,
            PointerHand::Right => &mut self.snapshot.right,
        }
    }

    fn modifier_usage_mut(&mut self, side: Side) -> &mut ModifierUsageSnapshot {
        match side {
            Side::Left => &mut self.snapshot.left.modifier,
            Side::Right => &mut self.snapshot.right.modifier,
        }
    }

    fn flush_active_modifiers(&mut self, publish_at: Instant) {
        macro_rules! flush_modifier {
            ($tracker:ident, $add_duration:ident, $modifier:expr) => {
                if let Some(tracker) = &mut self.$tracker {
                    let duration = publish_at.saturating_duration_since(tracker.last_accounted);
                    tracker.last_accounted = publish_at;
                    self.$add_duration($modifier, duration);
                }
            };
        }

        flush_modifier!(left_shift, add_left_modifier_duration, Modifier::Shift);
        flush_modifier!(right_shift, add_right_modifier_duration, Modifier::Shift);
        flush_modifier!(left_ctrl, add_left_modifier_duration, Modifier::Ctrl);
        flush_modifier!(right_ctrl, add_right_modifier_duration, Modifier::Ctrl);
        flush_modifier!(left_alt, add_left_modifier_duration, Modifier::Alt);
        flush_modifier!(right_alt, add_right_modifier_duration, Modifier::Alt);
        flush_modifier!(left_meta, add_left_modifier_duration, Modifier::Meta);
        flush_modifier!(right_meta, add_right_modifier_duration, Modifier::Meta);
        self.flush_multi_modifier_duration(Side::Left, publish_at);
        self.flush_multi_modifier_duration(Side::Right, publish_at);
    }

    fn flush_multi_modifier_duration(&mut self, side: Side, publish_at: Instant) {
        if let Some(tracker) = self.multi_tracker_mut(side) {
            let duration = publish_at.saturating_duration_since(tracker.last_accounted);
            tracker.last_accounted = publish_at;
            self.add_multi_modifier_duration(side, duration);
        }
    }

    fn finish_multi_modifier_duration(&mut self, side: Side, now: Instant) {
        if let Some(tracker) = self.multi_tracker_mut(side).take() {
            self.add_multi_modifier_duration(
                side,
                now.saturating_duration_since(tracker.last_accounted),
            );
        }
    }

    fn add_multi_modifier_duration(&mut self, side: Side, duration: Duration) {
        self.modifier_usage_mut(side).multi += duration;
    }

    fn add_left_modifier_duration(&mut self, modifier: Modifier, duration: Duration) {
        Self::add_modifier_duration(self.modifier_usage_mut(Side::Left), modifier, duration);
    }

    fn add_right_modifier_duration(&mut self, modifier: Modifier, duration: Duration) {
        Self::add_modifier_duration(self.modifier_usage_mut(Side::Right), modifier, duration);
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

    fn modifier_tracker(&self, side: Side, modifier: Modifier) -> &Option<ModifierTracker> {
        match (side, modifier) {
            (Side::Left, Modifier::Shift) => &self.left_shift,
            (Side::Right, Modifier::Shift) => &self.right_shift,
            (Side::Left, Modifier::Ctrl) => &self.left_ctrl,
            (Side::Right, Modifier::Ctrl) => &self.right_ctrl,
            (Side::Left, Modifier::Alt) => &self.left_alt,
            (Side::Right, Modifier::Alt) => &self.right_alt,
            (Side::Left, Modifier::Meta) => &self.left_meta,
            (Side::Right, Modifier::Meta) => &self.right_meta,
        }
    }

    fn multi_tracker_mut(&mut self, side: Side) -> &mut Option<ModifierTracker> {
        match side {
            Side::Left => &mut self.left_multi,
            Side::Right => &mut self.right_multi,
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

#[cfg(test)]
mod tests;
