use super::*;
use crate::device_events::DeviceLabelStore;
use bachelor::broadcast::spmc::{SpmcBroadcastProducer, SpmcBroadcastSource, broadcast};
use evdev::KeyCode;
use futures::FutureExt;
use key_hand::{KeyHand, KeyHandClassifier, KeyHandUsageConfig};
use std::num::NonZeroUsize;
use tokio::task::JoinHandle;

const CAPACITY: NonZeroUsize = NonZeroUsize::new(8).expect("capacity must be non-zero");
const ONE_MS: Duration = Duration::from_millis(1);

struct DriverHarness {
    producer: Option<SpmcBroadcastProducer<Event>>,
    _source: SpmcBroadcastSource<Event>,
    consumer: UsageConsumer,
    task: JoinHandle<()>,
    label: DeviceLabel,
}

impl DriverHarness {
    async fn new(config: UsageConfig) -> Self {
        let mut labels = DeviceLabelStore::new();
        let label = labels.get_or_intern("keyboard");
        Self::new_with_label(config, label).await
    }

    async fn new_with_label(config: UsageConfig, label: DeviceLabel) -> Self {
        let (producer, source) = broadcast(CAPACITY);
        let (usage_source, driver) = create(DragConfig::default(), config, source.subscribe());
        let consumer = usage_source.subscribe_forward();
        let task = tokio::task::spawn_local(driver.run());
        yield_driver().await;

        Self {
            producer: Some(producer),
            _source: source,
            consumer,
            task,
            label,
        }
    }

    async fn send(&mut self, label: DeviceLabel, kind: EventKind) {
        let event = Event { label, kind };
        let result = self
            .producer
            .as_mut()
            .expect("producer should be open")
            .send(event)
            .await;
        assert!(result.is_ok(), "event broadcast closed unexpectedly");
        yield_driver().await;
    }

    async fn send_key_down(&mut self, key: KeyCode) {
        self.send(
            self.label,
            EventKind::KeyPress {
                key,
                state: ButtonState::Down,
            },
        )
        .await;
    }

    async fn send_key_up(&mut self, key: KeyCode) {
        self.send(
            self.label,
            EventKind::KeyPress {
                key,
                state: ButtonState::Up,
            },
        )
        .await;
    }

    async fn send_left_down(&mut self) {
        self.send(
            self.label,
            EventKind::MousePress {
                button: KeyCode::BTN_LEFT,
                state: ButtonState::Down,
            },
        )
        .await;
    }

    async fn send_left_up(&mut self) {
        self.send(
            self.label,
            EventKind::MousePress {
                button: KeyCode::BTN_LEFT,
                state: ButtonState::Up,
            },
        )
        .await;
    }

    async fn send_mouse_move_x(&mut self, delta: i32) {
        self.send(self.label, EventKind::MouseMoveX(delta)).await;
    }

    async fn close(mut self) {
        drop(self.producer.take());
        self.task.await.expect("usage driver panicked");
    }
}

async fn yield_driver() {
    for _ in 0..4 {
        tokio::task::yield_now().await;
    }
}

async fn advance_and_yield(duration: Duration) {
    tokio::time::advance(duration).await;
    yield_driver().await;
}

fn assert_no_event(consumer: &mut UsageConsumer) {
    assert!(
        consumer.changed().now_or_never().is_none(),
        "unexpected usage/activity event"
    );
}

async fn expect_usage(consumer: &mut UsageConsumer) -> UsageSnapshot {
    match consumer
        .changed()
        .await
        .expect("usage consumer should be open")
    {
        UsageEvent::Usage => consumer.snapshot(),
        UsageEvent::Activity => panic!("expected usage event, got activity"),
    }
}

async fn expect_activity(consumer: &mut UsageConsumer) -> UsageSnapshot {
    match consumer
        .changed()
        .await
        .expect("usage consumer should be open")
    {
        UsageEvent::Activity => consumer.snapshot(),
        UsageEvent::Usage => panic!("expected activity event, got usage"),
    }
}

fn key_event(label: DeviceLabel, key: KeyCode, state: ButtonState) -> Event {
    Event {
        label,
        kind: EventKind::KeyPress { key, state },
    }
}

fn mouse_event(label: DeviceLabel, button: KeyCode, state: ButtonState) -> Event {
    Event {
        label,
        kind: EventKind::MousePress { button, state },
    }
}

fn controller_with_defaults() -> Controller {
    Controller::new(DragConfig::default(), KeyHandUsageConfig::default())
}

fn label() -> DeviceLabel {
    DeviceLabelStore::new().auto_detect()
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn usage_event_publishes_after_fast_rate_limit_and_counts_active_window() {
    let mut harness = DriverHarness::new(UsageConfig::default()).await;

    harness.send_key_down(KeyCode::KEY_A).await;
    assert_no_event(&mut harness.consumer);

    advance_and_yield(NOTIFY_RATE_LIMIT_FAST - ONE_MS).await;
    assert_no_event(&mut harness.consumer);

    advance_and_yield(ONE_MS).await;
    let snapshot = expect_usage(&mut harness.consumer).await;
    assert_eq!(snapshot.key_count.total(), 1);
    assert_eq!(snapshot.active_duration, NOTIFY_RATE_LIMIT_FAST);

    harness.close().await;
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn usage_events_inside_one_batch_do_not_extend_publish_deadline() {
    let mut harness = DriverHarness::new(UsageConfig::default()).await;

    harness.send_key_down(KeyCode::KEY_A).await;
    advance_and_yield(Duration::from_millis(100)).await;
    harness.send_key_down(KeyCode::KEY_B).await;

    advance_and_yield(Duration::from_millis(149)).await;
    assert_no_event(&mut harness.consumer);

    advance_and_yield(ONE_MS).await;
    let snapshot = expect_usage(&mut harness.consumer).await;
    assert_eq!(snapshot.key_count.total(), 2);
    assert_eq!(snapshot.active_duration, NOTIFY_RATE_LIMIT_FAST);

    harness.close().await;
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn lone_non_usage_event_emits_activity_after_bridge_without_snapshot_mutation() {
    let mut harness = DriverHarness::new(UsageConfig::default()).await;

    harness.send(harness.label, EventKind::MouseMoveX(12)).await;

    advance_and_yield(BRIDGE_INTERVAL - ONE_MS).await;
    assert_no_event(&mut harness.consumer);

    advance_and_yield(ONE_MS).await;
    let snapshot = expect_activity(&mut harness.consumer).await;
    assert_eq!(snapshot.key_count.total(), 0);
    assert_eq!(snapshot.click_count, 0);
    assert_eq!(snapshot.scroll_count, 0);
    assert_eq!(snapshot.active_duration, Duration::ZERO);

    harness.close().await;
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn non_usage_followed_by_usage_within_bridge_is_absorbed() {
    let mut harness = DriverHarness::new(UsageConfig::default()).await;

    harness.send(harness.label, EventKind::MouseMoveY(7)).await;
    advance_and_yield(Duration::from_millis(100)).await;
    harness.send_key_down(KeyCode::KEY_A).await;

    advance_and_yield(Duration::from_millis(50)).await;
    assert_no_event(&mut harness.consumer);

    advance_and_yield(NOTIFY_RATE_LIMIT_FAST - Duration::from_millis(50)).await;
    let snapshot = expect_usage(&mut harness.consumer).await;
    assert_eq!(snapshot.key_count.total(), 1);
    assert_eq!(snapshot.active_duration, NOTIFY_RATE_LIMIT_FAST);

    harness.close().await;
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn additional_non_usage_events_do_not_extend_pending_activity_timer() {
    let mut harness = DriverHarness::new(UsageConfig::default()).await;

    harness.send(harness.label, EventKind::MouseMoveX(1)).await;
    advance_and_yield(Duration::from_millis(100)).await;
    harness.send(harness.label, EventKind::MouseMoveY(1)).await;

    advance_and_yield(Duration::from_millis(49)).await;
    assert_no_event(&mut harness.consumer);

    advance_and_yield(ONE_MS).await;
    let snapshot = expect_activity(&mut harness.consumer).await;
    assert_eq!(snapshot.active_duration, Duration::ZERO);

    harness.close().await;
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn activity_inside_previous_emission_rate_limit_waits_for_fast_window() {
    let mut harness = DriverHarness::new(UsageConfig::default()).await;

    harness.send(harness.label, EventKind::MouseMoveX(1)).await;
    advance_and_yield(BRIDGE_INTERVAL).await;
    let snapshot = expect_activity(&mut harness.consumer).await;
    assert_eq!(snapshot.active_duration, Duration::ZERO);

    advance_and_yield(Duration::from_millis(50)).await;
    harness.send(harness.label, EventKind::MouseMoveY(1)).await;

    advance_and_yield(NOTIFY_RATE_LIMIT_FAST - Duration::from_millis(51)).await;
    assert_no_event(&mut harness.consumer);

    advance_and_yield(ONE_MS).await;
    let snapshot = expect_activity(&mut harness.consumer).await;
    assert_eq!(snapshot.active_duration, Duration::ZERO);

    harness.close().await;
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn non_usage_events_inside_batch_do_not_extend_or_mutate_publish() {
    let mut harness = DriverHarness::new(UsageConfig::default()).await;

    harness.send_key_down(KeyCode::KEY_A).await;
    advance_and_yield(Duration::from_millis(100)).await;
    harness.send(harness.label, EventKind::MouseMoveX(10)).await;

    advance_and_yield(Duration::from_millis(149)).await;
    assert_no_event(&mut harness.consumer);

    advance_and_yield(ONE_MS).await;
    let snapshot = expect_usage(&mut harness.consumer).await;
    assert_eq!(snapshot.key_count.total(), 1);
    assert_eq!(snapshot.click_count, 0);
    assert_eq!(snapshot.scroll_count, 0);
    assert_eq!(snapshot.active_duration, NOTIFY_RATE_LIMIT_FAST);

    harness.close().await;
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn driver_captures_event_time_after_await_returns() {
    let mut harness = DriverHarness::new(UsageConfig::default()).await;

    advance_and_yield(Duration::from_secs(5)).await;
    harness.send_key_down(KeyCode::KEY_A).await;

    advance_and_yield(NOTIFY_RATE_LIMIT_FAST - ONE_MS).await;
    // This assertion is load-bearing: a stale pre-await timestamp would make
    // the timeout appear elapsed and publish immediately after the send.
    assert_no_event(&mut harness.consumer);

    advance_and_yield(ONE_MS).await;
    let snapshot = expect_usage(&mut harness.consumer).await;
    assert_eq!(snapshot.key_count.total(), 1);
    assert_eq!(snapshot.active_duration, NOTIFY_RATE_LIMIT_FAST);

    harness.close().await;
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn usage_after_bridge_window_starts_fresh_batch() {
    let mut harness = DriverHarness::new(UsageConfig::default()).await;

    harness.send_key_down(KeyCode::KEY_A).await;
    advance_and_yield(NOTIFY_RATE_LIMIT_FAST).await;
    let snapshot = expect_usage(&mut harness.consumer).await;
    assert_eq!(snapshot.key_count.total(), 1);
    assert_eq!(snapshot.active_duration, NOTIFY_RATE_LIMIT_FAST);

    harness.send_key_down(KeyCode::KEY_B).await;
    advance_and_yield(NOTIFY_RATE_LIMIT_FAST - ONE_MS).await;
    assert_no_event(&mut harness.consumer);

    advance_and_yield(ONE_MS).await;
    let snapshot = expect_usage(&mut harness.consumer).await;
    assert_eq!(snapshot.key_count.total(), 2);
    assert_eq!(snapshot.active_duration, NOTIFY_RATE_LIMIT_FAST * 2);

    harness.close().await;
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn follow_up_usage_within_bridge_resumes_from_last_publish() {
    let mut harness = DriverHarness::new(UsageConfig::default()).await;

    harness.send_key_down(KeyCode::KEY_A).await;
    advance_and_yield(Duration::from_millis(200)).await;
    harness.send_key_down(KeyCode::KEY_B).await;
    advance_and_yield(Duration::from_millis(50)).await;
    let snapshot = expect_usage(&mut harness.consumer).await;
    assert_eq!(snapshot.key_count.total(), 2);
    assert_eq!(snapshot.active_duration, NOTIFY_RATE_LIMIT_FAST);

    advance_and_yield(Duration::from_millis(50)).await;
    harness.send_key_down(KeyCode::KEY_C).await;
    advance_and_yield(Duration::from_millis(199)).await;
    assert_no_event(&mut harness.consumer);

    advance_and_yield(ONE_MS).await;
    let snapshot = expect_usage(&mut harness.consumer).await;
    assert_eq!(snapshot.key_count.total(), 3);
    assert_eq!(snapshot.active_duration, NOTIFY_RATE_LIMIT_FAST * 2);

    harness.close().await;
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn driver_publishes_drag_duration_from_input_stream() {
    let mut harness = DriverHarness::new(UsageConfig::default()).await;

    harness.send_left_down().await;
    advance_and_yield(Duration::from_millis(50)).await;
    harness.send(harness.label, EventKind::MouseMoveX(3)).await;
    advance_and_yield(Duration::from_millis(50)).await;
    harness.send(harness.label, EventKind::MouseMoveY(-3)).await;
    harness.send_left_up().await;

    advance_and_yield(NOTIFY_RATE_LIMIT_FAST).await;
    let snapshot = expect_usage(&mut harness.consumer).await;
    assert_eq!(snapshot.click_count, 0);
    assert_eq!(snapshot.drag_duration, DragConfig::default().min_duration);
    assert_eq!(snapshot.active_duration, NOTIFY_RATE_LIMIT_FAST);

    harness.close().await;
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn qualified_drag_streams_on_fast_cadence_from_original_down() {
    let mut harness = DriverHarness::new(UsageConfig::default()).await;
    let movement_delay = Duration::from_millis(50);

    harness.send_left_down().await;
    advance_and_yield(movement_delay).await;
    harness
        .send_mouse_move_x(DragConfig::default().min_distance as i32)
        .await;

    advance_and_yield(NOTIFY_RATE_LIMIT_FAST).await;
    let snapshot = expect_usage(&mut harness.consumer).await;
    assert_eq!(snapshot.click_count, 0);
    assert_eq!(
        snapshot.drag_duration,
        movement_delay + NOTIFY_RATE_LIMIT_FAST
    );
    assert_eq!(snapshot.active_duration, NOTIFY_RATE_LIMIT_FAST);

    advance_and_yield(NOTIFY_RATE_LIMIT_FAST).await;
    let snapshot = expect_usage(&mut harness.consumer).await;
    assert_eq!(
        snapshot.drag_duration,
        movement_delay + NOTIFY_RATE_LIMIT_FAST * 2
    );
    assert_eq!(snapshot.active_duration, NOTIFY_RATE_LIMIT_FAST * 2);

    harness.close().await;
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn qualified_drag_backfills_from_original_down_after_long_pause() {
    let mut harness = DriverHarness::new(UsageConfig::default()).await;
    let pause_before_movement = Duration::from_secs(1);

    harness.send_left_down().await;
    advance_and_yield(pause_before_movement).await;
    harness
        .send_mouse_move_x(DragConfig::default().min_distance as i32)
        .await;

    advance_and_yield(NOTIFY_RATE_LIMIT_FAST).await;
    let snapshot = expect_usage(&mut harness.consumer).await;
    assert_eq!(snapshot.click_count, 0);
    assert_eq!(
        snapshot.drag_duration,
        pause_before_movement + NOTIFY_RATE_LIMIT_FAST
    );
    assert_eq!(snapshot.active_duration, NOTIFY_RATE_LIMIT_FAST);

    harness.close().await;
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn qualified_drag_release_after_publish_adds_only_residual_duration() {
    let mut harness = DriverHarness::new(UsageConfig::default()).await;
    let movement_delay = Duration::from_millis(50);
    let residual = Duration::from_millis(40);

    harness.send_left_down().await;
    advance_and_yield(movement_delay).await;
    harness
        .send_mouse_move_x(DragConfig::default().min_distance as i32)
        .await;

    advance_and_yield(NOTIFY_RATE_LIMIT_FAST).await;
    let snapshot = expect_usage(&mut harness.consumer).await;
    assert_eq!(
        snapshot.drag_duration,
        movement_delay + NOTIFY_RATE_LIMIT_FAST
    );

    advance_and_yield(residual).await;
    harness.send_left_up().await;
    assert_no_event(&mut harness.consumer);

    advance_and_yield(NOTIFY_RATE_LIMIT_FAST - residual).await;
    let snapshot = expect_usage(&mut harness.consumer).await;
    assert_eq!(
        snapshot.drag_duration,
        movement_delay + NOTIFY_RATE_LIMIT_FAST + residual
    );
    assert_eq!(snapshot.active_duration, NOTIFY_RATE_LIMIT_FAST * 2);

    harness.close().await;
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn modifier_and_drag_stream_together_on_shared_cadence() {
    let mut harness = DriverHarness::new(UsageConfig::default()).await;
    let movement_delay = Duration::from_millis(50);

    harness.send_key_down(KeyCode::KEY_LEFTSHIFT).await;
    harness.send_left_down().await;
    advance_and_yield(movement_delay).await;
    harness
        .send_mouse_move_x(DragConfig::default().min_distance as i32)
        .await;

    advance_and_yield(NOTIFY_RATE_LIMIT_FAST - movement_delay).await;
    let snapshot = expect_usage(&mut harness.consumer).await;
    assert_eq!(
        snapshot.left_modifier_duration.shift,
        NOTIFY_RATE_LIMIT_FAST
    );
    assert_eq!(snapshot.drag_duration, NOTIFY_RATE_LIMIT_FAST);
    assert_eq!(snapshot.active_duration, NOTIFY_RATE_LIMIT_FAST);

    advance_and_yield(NOTIFY_RATE_LIMIT_FAST).await;
    let snapshot = expect_usage(&mut harness.consumer).await;
    assert_eq!(
        snapshot.left_modifier_duration.shift,
        NOTIFY_RATE_LIMIT_FAST * 2
    );
    assert_eq!(snapshot.drag_duration, NOTIFY_RATE_LIMIT_FAST * 2);
    assert_eq!(snapshot.active_duration, NOTIFY_RATE_LIMIT_FAST * 2);

    harness.close().await;
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn held_modifier_publishes_duration_on_each_fast_window() {
    let mut harness = DriverHarness::new(UsageConfig::default()).await;

    harness.send_key_down(KeyCode::KEY_LEFTSHIFT).await;
    assert_no_event(&mut harness.consumer);

    advance_and_yield(NOTIFY_RATE_LIMIT_FAST).await;
    let snapshot = expect_usage(&mut harness.consumer).await;
    assert_eq!(
        snapshot.left_modifier_duration.shift,
        NOTIFY_RATE_LIMIT_FAST
    );
    assert_eq!(snapshot.active_duration, NOTIFY_RATE_LIMIT_FAST);

    advance_and_yield(NOTIFY_RATE_LIMIT_FAST).await;
    let snapshot = expect_usage(&mut harness.consumer).await;
    assert_eq!(
        snapshot.left_modifier_duration.shift,
        NOTIFY_RATE_LIMIT_FAST * 2
    );
    assert_eq!(snapshot.active_duration, NOTIFY_RATE_LIMIT_FAST * 2);

    harness.close().await;
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn modifier_release_after_published_window_adds_only_residual_duration() {
    let mut harness = DriverHarness::new(UsageConfig::default()).await;
    let residual = Duration::from_millis(50);

    harness.send_key_down(KeyCode::KEY_LEFTSHIFT).await;
    advance_and_yield(NOTIFY_RATE_LIMIT_FAST).await;
    let snapshot = expect_usage(&mut harness.consumer).await;
    assert_eq!(
        snapshot.left_modifier_duration.shift,
        NOTIFY_RATE_LIMIT_FAST
    );
    assert_eq!(snapshot.active_duration, NOTIFY_RATE_LIMIT_FAST);

    advance_and_yield(residual).await;
    harness.send_key_up(KeyCode::KEY_LEFTSHIFT).await;
    assert_no_event(&mut harness.consumer);

    advance_and_yield(NOTIFY_RATE_LIMIT_FAST - residual).await;
    let snapshot = expect_usage(&mut harness.consumer).await;
    assert_eq!(
        snapshot.left_modifier_duration.shift,
        NOTIFY_RATE_LIMIT_FAST + residual
    );
    assert_eq!(snapshot.active_duration, NOTIFY_RATE_LIMIT_FAST * 2);

    harness.close().await;
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn overlapping_modifiers_flush_and_release_independently() {
    let mut harness = DriverHarness::new(UsageConfig::default()).await;

    harness.send_key_down(KeyCode::KEY_LEFTSHIFT).await;
    harness.send_key_down(KeyCode::KEY_LEFTCTRL).await;

    advance_and_yield(NOTIFY_RATE_LIMIT_FAST).await;
    let snapshot = expect_usage(&mut harness.consumer).await;
    assert_eq!(
        snapshot.left_modifier_duration.shift,
        NOTIFY_RATE_LIMIT_FAST
    );
    assert_eq!(snapshot.left_modifier_duration.ctrl, NOTIFY_RATE_LIMIT_FAST);
    assert_eq!(
        snapshot.left_modifier_duration.multi,
        NOTIFY_RATE_LIMIT_FAST
    );
    assert_eq!(snapshot.active_duration, NOTIFY_RATE_LIMIT_FAST);

    advance_and_yield(Duration::from_millis(50)).await;
    harness.send_key_up(KeyCode::KEY_LEFTSHIFT).await;

    advance_and_yield(Duration::from_millis(200)).await;
    let snapshot = expect_usage(&mut harness.consumer).await;
    assert_eq!(
        snapshot.left_modifier_duration.shift,
        NOTIFY_RATE_LIMIT_FAST + Duration::from_millis(50)
    );
    assert_eq!(
        snapshot.left_modifier_duration.ctrl,
        NOTIFY_RATE_LIMIT_FAST * 2
    );
    assert_eq!(
        snapshot.left_modifier_duration.multi,
        NOTIFY_RATE_LIMIT_FAST + Duration::from_millis(50)
    );
    assert_eq!(snapshot.active_duration, NOTIFY_RATE_LIMIT_FAST * 2);

    advance_and_yield(Duration::from_millis(100)).await;
    harness.send_key_up(KeyCode::KEY_LEFTCTRL).await;

    advance_and_yield(Duration::from_millis(150)).await;
    let snapshot = expect_usage(&mut harness.consumer).await;
    assert_eq!(
        snapshot.left_modifier_duration.shift,
        NOTIFY_RATE_LIMIT_FAST + Duration::from_millis(50)
    );
    assert_eq!(
        snapshot.left_modifier_duration.ctrl,
        NOTIFY_RATE_LIMIT_FAST * 2 + Duration::from_millis(100)
    );
    assert_eq!(
        snapshot.left_modifier_duration.multi,
        NOTIFY_RATE_LIMIT_FAST + Duration::from_millis(50)
    );
    assert_eq!(snapshot.active_duration, NOTIFY_RATE_LIMIT_FAST * 3);

    harness.close().await;
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn excluded_label_short_circuits_usage_classification() {
    let mut labels = DeviceLabelStore::new();
    let label = labels.get_or_intern("keyboard");
    let excluded_label = labels.get_or_intern("ignored");

    let mut harness = DriverHarness::new_with_label(
        UsageConfig {
            exclude: vec![excluded_label],
            ..UsageConfig::default()
        },
        label,
    )
    .await;
    harness
        .send(
            excluded_label,
            EventKind::KeyPress {
                key: KeyCode::KEY_A,
                state: ButtonState::Down,
            },
        )
        .await;

    advance_and_yield(BRIDGE_INTERVAL).await;
    let snapshot = expect_activity(&mut harness.consumer).await;
    assert_eq!(snapshot.key_count.total(), 0);
    assert_eq!(snapshot.active_duration, Duration::ZERO);

    harness.close().await;
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn closing_broadcast_exits_driver() {
    let harness = DriverHarness::new(UsageConfig::default()).await;

    harness.close().await;
}

#[test]
fn non_modifier_key_down_increments_key_count_and_key_up_is_ignored() {
    let mut controller = controller_with_defaults();
    let label = label();
    let now = Instant::now();

    assert!(controller.handle_event(&key_event(label, KeyCode::KEY_A, ButtonState::Down), now));
    assert_eq!(controller.snapshot_at(now).key_count.total(), 1);

    assert!(!controller.handle_event(
        &key_event(label, KeyCode::KEY_A, ButtonState::Up),
        now + Duration::from_millis(10)
    ));
    assert_eq!(
        controller
            .snapshot_at(now + Duration::from_millis(10))
            .key_count
            .total(),
        1
    );
}

#[test]
fn non_modifier_key_down_uses_device_specific_classifier() {
    let mut labels = DeviceLabelStore::new();
    let default_label = labels.get_or_intern("keyboard");
    let custom_label = labels.get_or_intern("thumb_board");
    let mut custom_classifier = KeyHandClassifier::none();
    custom_classifier.set(KeyCode::KEY_SPACE, KeyHand::Right);
    let mut controller = Controller::new(
        DragConfig::default(),
        KeyHandUsageConfig {
            default_classifier: KeyHandClassifier::ansi_qwerty(),
            device_classifiers: vec![(custom_label, custom_classifier)],
        },
    );
    let now = Instant::now();

    assert!(controller.handle_event(
        &key_event(default_label, KeyCode::KEY_SPACE, ButtonState::Down),
        now
    ));
    assert!(controller.handle_event(
        &key_event(custom_label, KeyCode::KEY_SPACE, ButtonState::Down),
        now
    ));

    let snapshot = controller.snapshot_at(now);
    assert_eq!(snapshot.key_count.left, 0);
    assert_eq!(snapshot.key_count.right, 1);
    assert_eq!(snapshot.key_count.other, 1);
}

#[test]
fn same_hand_modifier_combo_suppresses_key_count() {
    let mut controller = controller_with_defaults();
    let label = label();
    let now = Instant::now();

    assert!(controller.handle_event(
        &key_event(label, KeyCode::KEY_LEFTSHIFT, ButtonState::Down),
        now
    ));
    assert!(controller.handle_event(&key_event(label, KeyCode::KEY_A, ButtonState::Down), now));
    assert!(controller.handle_event(
        &key_event(label, KeyCode::KEY_RIGHTSHIFT, ButtonState::Down),
        now
    ));
    assert!(controller.handle_event(&key_event(label, KeyCode::KEY_Y, ButtonState::Down), now));

    let snapshot = controller.snapshot_at(now);
    assert_eq!(snapshot.left_modifier_duration.combo, 1);
    assert_eq!(snapshot.right_modifier_duration.combo, 1);
    assert_eq!(snapshot.cross_combo, 0);
    assert_eq!(snapshot.key_count.left, 0);
    assert_eq!(snapshot.key_count.right, 0);
}

#[test]
fn cross_combo_counts_in_both_directions_only_without_same_hand_modifier() {
    let mut controller = controller_with_defaults();
    let label = label();
    let now = Instant::now();

    assert!(controller.handle_event(
        &key_event(label, KeyCode::KEY_LEFTSHIFT, ButtonState::Down),
        now
    ));
    assert!(controller.handle_event(&key_event(label, KeyCode::KEY_Y, ButtonState::Down), now));
    assert!(controller.handle_event(
        &key_event(label, KeyCode::KEY_RIGHTSHIFT, ButtonState::Down),
        now
    ));
    assert!(controller.handle_event(&key_event(label, KeyCode::KEY_U, ButtonState::Down), now));

    let snapshot = controller.snapshot_at(now);
    assert_eq!(snapshot.cross_combo, 1);
    assert_eq!(snapshot.right_modifier_duration.combo, 1);
    assert_eq!(snapshot.key_count.right, 0);

    let mut controller = controller_with_defaults();
    assert!(controller.handle_event(
        &key_event(label, KeyCode::KEY_RIGHTSHIFT, ButtonState::Down),
        now
    ));
    assert!(controller.handle_event(&key_event(label, KeyCode::KEY_A, ButtonState::Down), now));

    let snapshot = controller.snapshot_at(now);
    assert_eq!(snapshot.cross_combo, 1);
    assert_eq!(snapshot.left_modifier_duration.combo, 0);
    assert_eq!(snapshot.key_count.left, 0);
}

#[test]
fn other_key_with_any_modifier_counts_other_combo_once() {
    let mut controller = controller_with_defaults();
    let label = label();
    let now = Instant::now();

    assert!(controller.handle_event(
        &key_event(label, KeyCode::KEY_LEFTSHIFT, ButtonState::Down),
        now
    ));
    assert!(controller.handle_event(
        &key_event(label, KeyCode::KEY_SPACE, ButtonState::Down),
        now
    ));

    let snapshot = controller.snapshot_at(now);
    assert_eq!(snapshot.other_combo, 1);
    assert_eq!(snapshot.key_count.other, 0);
    assert_eq!(snapshot.cross_combo, 0);

    let mut controller = controller_with_defaults();
    assert!(controller.handle_event(
        &key_event(label, KeyCode::KEY_LEFTSHIFT, ButtonState::Down),
        now
    ));
    assert!(controller.handle_event(
        &key_event(label, KeyCode::KEY_RIGHTSHIFT, ButtonState::Down),
        now
    ));
    assert!(controller.handle_event(
        &key_event(label, KeyCode::KEY_SPACE, ButtonState::Down),
        now
    ));

    let snapshot = controller.snapshot_at(now);
    assert_eq!(snapshot.other_combo, 1);
    assert_eq!(snapshot.key_count.other, 0);
    assert_eq!(snapshot.cross_combo, 0);
}

#[test]
fn modifier_down_tracks_until_matching_up_on_correct_side() {
    let mut controller = controller_with_defaults();
    let label = label();
    let start = Instant::now();
    let duration = Duration::from_millis(75);

    assert!(controller.handle_event(
        &key_event(label, KeyCode::KEY_LEFTSHIFT, ButtonState::Down),
        start
    ));
    assert!(controller.handle_event(
        &key_event(label, KeyCode::KEY_LEFTSHIFT, ButtonState::Up),
        start + duration
    ));

    let snapshot = controller.snapshot_at(start + duration);
    assert_eq!(snapshot.left_modifier_duration.shift, duration);
    assert_eq!(snapshot.right_modifier_duration.shift, Duration::ZERO);
    assert_eq!(snapshot.key_count.total(), 0);
}

#[test]
fn duplicate_modifier_down_does_not_reset_start_and_orphan_up_is_ignored() {
    let mut controller = controller_with_defaults();
    let label = label();
    let start = Instant::now();

    assert!(controller.handle_event(
        &key_event(label, KeyCode::KEY_LEFTCTRL, ButtonState::Down),
        start
    ));
    assert!(!controller.handle_event(
        &key_event(label, KeyCode::KEY_LEFTCTRL, ButtonState::Down),
        start + Duration::from_millis(50)
    ));
    assert!(controller.handle_event(
        &key_event(label, KeyCode::KEY_LEFTCTRL, ButtonState::Up),
        start + Duration::from_millis(100)
    ));
    assert!(!controller.handle_event(
        &key_event(label, KeyCode::KEY_LEFTCTRL, ButtonState::Up),
        start + Duration::from_millis(200)
    ));

    assert_eq!(
        controller
            .snapshot_at(start + Duration::from_millis(200))
            .left_modifier_duration
            .ctrl,
        Duration::from_millis(100)
    );
}

#[test]
fn multi_modifier_duration_is_union_time_while_more_than_one_same_side_modifier_is_active() {
    let mut controller = controller_with_defaults();
    let label = label();
    let start = Instant::now();

    assert!(controller.handle_event(
        &key_event(label, KeyCode::KEY_LEFTSHIFT, ButtonState::Down),
        start
    ));
    assert!(!controller.handle_event(
        &key_event(label, KeyCode::KEY_LEFTSHIFT, ButtonState::Down),
        start + Duration::from_millis(10)
    ));
    assert!(controller.handle_event(
        &key_event(label, KeyCode::KEY_LEFTCTRL, ButtonState::Down),
        start + Duration::from_millis(20)
    ));
    assert!(controller.handle_event(
        &key_event(label, KeyCode::KEY_LEFTALT, ButtonState::Down),
        start + Duration::from_millis(50)
    ));
    assert!(controller.handle_event(
        &key_event(label, KeyCode::KEY_LEFTSHIFT, ButtonState::Up),
        start + Duration::from_millis(80)
    ));
    assert!(controller.handle_event(
        &key_event(label, KeyCode::KEY_LEFTCTRL, ButtonState::Up),
        start + Duration::from_millis(110)
    ));
    assert!(controller.handle_event(
        &key_event(label, KeyCode::KEY_LEFTALT, ButtonState::Up),
        start + Duration::from_millis(140)
    ));
    assert!(!controller.handle_event(
        &key_event(label, KeyCode::KEY_LEFTSHIFT, ButtonState::Up),
        start + Duration::from_millis(150)
    ));

    let snapshot = controller.snapshot_at(start + Duration::from_millis(150));
    assert_eq!(
        snapshot.left_modifier_duration.multi,
        Duration::from_millis(90)
    );
    assert_eq!(
        snapshot.left_modifier_duration.shift,
        Duration::from_millis(80)
    );
    assert_eq!(
        snapshot.left_modifier_duration.ctrl,
        Duration::from_millis(90)
    );
    assert_eq!(
        snapshot.left_modifier_duration.alt,
        Duration::from_millis(90)
    );
    assert_eq!(snapshot.right_modifier_duration.multi, Duration::ZERO);
}

#[test]
fn multi_modifier_duration_restarts_when_same_side_modifier_overlap_resumes() {
    let mut controller = controller_with_defaults();
    let label = label();
    let start = Instant::now();

    assert!(controller.handle_event(
        &key_event(label, KeyCode::KEY_LEFTSHIFT, ButtonState::Down),
        start
    ));
    assert!(controller.handle_event(
        &key_event(label, KeyCode::KEY_LEFTCTRL, ButtonState::Down),
        start + Duration::from_millis(10)
    ));
    assert!(controller.handle_event(
        &key_event(label, KeyCode::KEY_LEFTCTRL, ButtonState::Up),
        start + Duration::from_millis(40)
    ));
    assert!(controller.handle_event(
        &key_event(label, KeyCode::KEY_LEFTCTRL, ButtonState::Down),
        start + Duration::from_millis(70)
    ));
    assert!(controller.handle_event(
        &key_event(label, KeyCode::KEY_LEFTSHIFT, ButtonState::Up),
        start + Duration::from_millis(100)
    ));
    assert!(controller.handle_event(
        &key_event(label, KeyCode::KEY_LEFTCTRL, ButtonState::Up),
        start + Duration::from_millis(120)
    ));

    let snapshot = controller.snapshot_at(start + Duration::from_millis(120));
    assert_eq!(
        snapshot.left_modifier_duration.multi,
        Duration::from_millis(60)
    );
    assert_eq!(
        snapshot.left_modifier_duration.shift,
        Duration::from_millis(100)
    );
    assert_eq!(
        snapshot.left_modifier_duration.ctrl,
        Duration::from_millis(80)
    );
}

#[test]
fn modifier_durations_are_tracked_independently_for_each_side_and_kind() {
    let cases = [
        (KeyCode::KEY_LEFTSHIFT, Side::Left, Modifier::Shift),
        (KeyCode::KEY_RIGHTSHIFT, Side::Right, Modifier::Shift),
        (KeyCode::KEY_LEFTCTRL, Side::Left, Modifier::Ctrl),
        (KeyCode::KEY_RIGHTCTRL, Side::Right, Modifier::Ctrl),
        (KeyCode::KEY_LEFTALT, Side::Left, Modifier::Alt),
        (KeyCode::KEY_RIGHTALT, Side::Right, Modifier::Alt),
        (KeyCode::KEY_LEFTMETA, Side::Left, Modifier::Meta),
        (KeyCode::KEY_RIGHTMETA, Side::Right, Modifier::Meta),
    ];

    for (key, side, modifier) in cases {
        let mut controller = controller_with_defaults();
        let label = label();
        let start = Instant::now();
        let duration = Duration::from_millis(40);

        assert!(controller.handle_event(&key_event(label, key, ButtonState::Down), start));
        assert!(controller.handle_event(&key_event(label, key, ButtonState::Up), start + duration));

        let snapshot = controller.snapshot_at(start + duration);
        assert_eq!(modifier_duration(&snapshot, side, modifier), duration);
        assert_eq!(
            modifier_duration(&snapshot, opposite(side), modifier),
            Duration::ZERO
        );
    }
}

#[test]
fn mouse_button_down_is_non_usage_and_release_counts_click_without_qualifying_drag() {
    let mut controller = controller_with_defaults();
    let label = label();
    let start = Instant::now();

    assert!(!controller.handle_event(
        &mouse_event(label, KeyCode::BTN_LEFT, ButtonState::Down),
        start
    ));
    assert!(controller.handle_event(
        &mouse_event(label, KeyCode::BTN_LEFT, ButtonState::Up),
        start + Duration::from_millis(10)
    ));

    let snapshot = controller.snapshot_at(start + Duration::from_millis(10));
    assert_eq!(snapshot.click_count, 1);
    assert_eq!(snapshot.drag_duration, Duration::ZERO);
}

#[test]
fn left_button_drag_records_duration_and_suppresses_click_only_when_thresholds_are_met() {
    let mut controller = controller_with_defaults();
    let label = label();
    let start = Instant::now();

    assert!(!controller.handle_event(
        &mouse_event(label, KeyCode::BTN_LEFT, ButtonState::Down),
        start
    ));
    assert!(!controller.handle_event(
        &Event {
            label,
            kind: EventKind::MouseMoveX(3),
        },
        start + Duration::from_millis(10)
    ));
    assert!(controller.handle_event(
        &Event {
            label,
            kind: EventKind::MouseMoveY(-3),
        },
        start + Duration::from_millis(20)
    ));
    assert!(controller.handle_event(
        &mouse_event(label, KeyCode::BTN_LEFT, ButtonState::Up),
        start + DragConfig::default().min_duration
    ));

    let snapshot = controller.snapshot_at(start + DragConfig::default().min_duration);
    assert_eq!(snapshot.click_count, 0);
    assert_eq!(snapshot.drag_duration, DragConfig::default().min_duration);
}

#[test]
fn movement_without_active_drag_is_non_usage() {
    let mut controller = controller_with_defaults();
    let label = label();
    let now = Instant::now();

    assert!(!controller.handle_event(
        &Event {
            label,
            kind: EventKind::MouseMoveX(20),
        },
        now
    ));
    assert_eq!(controller.snapshot_at(now).drag_duration, Duration::ZERO);
}

#[test]
fn failed_drag_thresholds_right_clicks_and_non_left_buttons_remain_clicks_on_release() {
    let label = label();
    let start = Instant::now();

    let mut short_drag = controller_with_defaults();
    assert!(!short_drag.handle_event(
        &mouse_event(label, KeyCode::BTN_LEFT, ButtonState::Down),
        start
    ));
    assert!(short_drag.handle_event(
        &Event {
            label,
            kind: EventKind::MouseMoveX(10),
        },
        start + Duration::from_millis(10)
    ));
    assert!(short_drag.handle_event(
        &mouse_event(label, KeyCode::BTN_LEFT, ButtonState::Up),
        start + Duration::from_millis(99)
    ));
    assert_eq!(
        short_drag
            .snapshot_at(start + Duration::from_millis(99))
            .click_count,
        1
    );

    let mut right_click = controller_with_defaults();
    assert!(!right_click.handle_event(
        &mouse_event(label, KeyCode::BTN_RIGHT, ButtonState::Down),
        start
    ));
    assert!(right_click.handle_event(
        &mouse_event(label, KeyCode::BTN_RIGHT, ButtonState::Up),
        start + Duration::from_millis(150)
    ));
    assert_eq!(
        right_click
            .snapshot_at(start + Duration::from_millis(150))
            .click_count,
        1
    );

    let mut middle_click = controller_with_defaults();
    assert!(!middle_click.handle_event(
        &mouse_event(label, KeyCode::BTN_MIDDLE, ButtonState::Down),
        start
    ));
    assert!(middle_click.handle_event(
        &mouse_event(label, KeyCode::BTN_MIDDLE, ButtonState::Up),
        start + Duration::from_millis(150)
    ));
    assert_eq!(
        middle_click
            .snapshot_at(start + Duration::from_millis(150))
            .click_count,
        1
    );
}

#[test]
fn scroll_notches_use_absolute_values_and_saturating_addition() {
    let mut controller = controller_with_defaults();
    let label = label();
    let now = Instant::now();

    assert!(!controller.handle_event(
        &Event {
            label,
            kind: EventKind::MouseScrollNotch(0),
        },
        now
    ));
    assert!(controller.handle_event(
        &Event {
            label,
            kind: EventKind::MouseScrollNotch(-2),
        },
        now
    ));
    assert_eq!(controller.snapshot_at(now).scroll_count, 2);

    controller.snapshot.scroll_count = u64::MAX - 1;
    assert!(controller.handle_event(
        &Event {
            label,
            kind: EventKind::MouseScrollNotch(4),
        },
        now
    ));
    assert_eq!(controller.snapshot_at(now).scroll_count, u64::MAX);
    assert!(!controller.handle_event(
        &Event {
            label,
            kind: EventKind::MouseScrollHiRes(120),
        },
        now
    ));
    assert_eq!(controller.snapshot_at(now).scroll_count, u64::MAX);
}

fn modifier_duration(snapshot: &UsageSnapshot, side: Side, modifier: Modifier) -> Duration {
    let modifiers = match side {
        Side::Left => snapshot.left_modifier_duration,
        Side::Right => snapshot.right_modifier_duration,
    };

    match modifier {
        Modifier::Shift => modifiers.shift,
        Modifier::Ctrl => modifiers.ctrl,
        Modifier::Alt => modifiers.alt,
        Modifier::Meta => modifiers.meta,
    }
}

fn opposite(side: Side) -> Side {
    match side {
        Side::Left => Side::Right,
        Side::Right => Side::Left,
    }
}
