use crate::device_events::Event;
use bachelor::broadcast::spmc::SpmcBroadcastConsumer;
use bachelor::channel::spsc;
use bachelor::signal::mpmc_latched::{self, MpmcLatchedSignalSource};
use shared::protocol::DwellServerConfig;
use std::num::NonZeroUsize;

mod event_sink;

enum ControlMessage {
    Pause,
    Resume,
    Reconfigure(DwellServerConfig),
}

/// Creates a new dwell click controller and driver pair
pub fn create(
    events_rx: SpmcBroadcastConsumer<Event>,
) -> (
    controller::Controller,
    MpmcLatchedSignalSource,
    driver::Driver,
) {
    let (control_tx, control_rx) = spsc::channel(NonZeroUsize::new(8).unwrap());
    let (click_tx, click_rx) = mpmc_latched::signal();

    let controller = controller::Controller::new(control_tx);
    let driver = driver::Driver::new(
        DwellServerConfig::default(),
        events_rx,
        control_rx,
        click_tx,
    );
    (controller, click_rx, driver)
}

pub use controller::Controller;

mod controller {
    use super::ControlMessage;
    use bachelor::channel::spsc::SpscChannelProducer;
    use shared::protocol::DwellServerConfig;

    pub struct Controller {
        control_tx: SpscChannelProducer<ControlMessage>,
    }

    impl Controller {
        pub(super) fn new(control_tx: SpscChannelProducer<ControlMessage>) -> Self {
            Self { control_tx }
        }

        pub async fn pause(&mut self) {
            let _ = self.control_tx.send(ControlMessage::Pause).await;
        }

        pub async fn resume(&mut self) {
            let _ = self.control_tx.send(ControlMessage::Resume).await;
        }

        pub async fn reconfigure(&mut self, config: DwellServerConfig) {
            let _ = self
                .control_tx
                .send(ControlMessage::Reconfigure(config))
                .await;
        }
    }
}

pub mod driver {
    use bachelor::broadcast::spmc::SpmcBroadcastConsumer;
    use bachelor::channel::spsc::SpscChannelConsumer;
    use bachelor::signal::mpmc_latched::MpmcLatchedSignalProducer;
    use futures::{
        future::{Either, select},
        pin_mut,
    };
    use rootcause::prelude::*;
    use shared::protocol::DwellServerConfig;
    use std::io;
    use tokio::time::Instant;

    use super::{ControlMessage, event_sink::EventSink};
    use crate::device_events::Event;

    pub struct Driver {
        config: DwellServerConfig,
        events_rx: SpmcBroadcastConsumer<Event>,
        control_rx: SpscChannelConsumer<ControlMessage>,
        click_tx: MpmcLatchedSignalProducer,
        distance: i32,
        paused: bool,
        dwell_deadline: Option<Instant>,
    }

    enum WaitResult {
        Event(Event),
        Control(ControlMessage),
        Closed,
    }

    impl Driver {
        pub(super) fn new(
            config: DwellServerConfig,
            events_rx: SpmcBroadcastConsumer<Event>,
            control_rx: SpscChannelConsumer<ControlMessage>,
            click_tx: MpmcLatchedSignalProducer,
        ) -> Self {
            Self {
                config,
                events_rx,
                control_rx,
                click_tx,
                distance: 0,
                paused: false,
                dwell_deadline: None,
            }
        }

        pub async fn run(mut self) -> Result<(), Report> {
            let mut sink =
                EventSink::new().context("Failed to create dwell click virtual device")?;

            log::info!("created dwell click virtual device");

            loop {
                let outcome = if let Some(deadline) = self.dwell_deadline {
                    self.wait_control_event_or_timeout(deadline).await
                } else {
                    Some(self.wait_control_or_event().await)
                };

                match outcome {
                    Some(WaitResult::Event(event)) => self.handle_event(event),
                    Some(WaitResult::Control(msg)) => self.handle_control(msg),
                    Some(WaitResult::Closed) => return Ok(()),
                    None => {
                        self.handle_dwell_timeout(&mut sink)
                            .context("Click failed")?;
                    }
                }
            }
        }

        async fn wait_control_or_event(&mut self) -> WaitResult {
            let control_fut = self.control_rx.recv();
            let event_fut = self.events_rx.recv();
            pin_mut!(control_fut);
            pin_mut!(event_fut);

            match select(control_fut, event_fut).await {
                Either::Left((Ok(msg), _event_pending)) => WaitResult::Control(msg),
                Either::Left((Err(_), _event_pending)) => WaitResult::Closed,
                Either::Right((Ok(event), _control_pending)) => WaitResult::Event(event),
                Either::Right((Err(_), _control_pending)) => WaitResult::Closed,
            }
        }

        async fn wait_control_event_or_timeout(&mut self, deadline: Instant) -> Option<WaitResult> {
            tokio::time::timeout_at(deadline, self.wait_control_or_event())
                .await
                .ok()
        }

        fn handle_event(&mut self, event: Event) {
            if self.paused {
                return;
            }

            match event {
                Event::MouseMoveX(dx) => {
                    self.add_movement(dx);
                    self.dwell_deadline =
                        Some(Instant::now() + self.config.dwell_duration_threshold);
                }
                Event::MouseMoveY(dy) => {
                    self.add_movement(dy);
                    self.dwell_deadline =
                        Some(Instant::now() + self.config.dwell_duration_threshold);
                }
                Event::MousePress { .. } | Event::KeyPress { .. } | Event::MouseScroll(_) => {
                    self.reset_movement();
                    self.dwell_deadline = None;
                }
            }
        }

        fn handle_control(&mut self, msg: ControlMessage) {
            match msg {
                ControlMessage::Pause => {
                    self.paused = true;
                    self.reset_movement();
                    self.dwell_deadline = None;
                }
                ControlMessage::Resume => {
                    self.paused = false;
                    self.reset_movement();
                    self.dwell_deadline = None;
                }
                ControlMessage::Reconfigure(config) => {
                    self.config = config;
                }
            }
        }

        fn handle_dwell_timeout(&mut self, sink: &mut EventSink) -> io::Result<()> {
            let should_click = self.distance > self.config.movement_threshold;
            self.reset_movement();
            self.dwell_deadline = None;

            if should_click {
                self.perform_click(sink)?;
            }

            Ok(())
        }

        fn reset_movement(&mut self) {
            self.distance = 0;
        }

        fn add_movement(&mut self, delta: i32) {
            self.distance += delta.abs();
        }

        fn perform_click(&mut self, sink: &mut EventSink) -> io::Result<()> {
            log::trace!("sending dwell click now");
            sink.click_left()?;
            self.click_tx.notify();

            Ok(())
        }
    }
}
