use bachelor::channel::mpsc;
use futures::Stream;
use std::num::NonZeroUsize;
use std::pin::Pin;
use std::task::{Context, Poll};

mod discovery;
mod event_source;
mod label;

pub use event_source::{ButtonState, Event, EventKind, Key};
pub use label::{DeviceLabel, DeviceLabelStore};

const DEVICE_CHANNEL_CAPACITY: NonZeroUsize =
    NonZeroUsize::new(16).expect("device channel capacity must be non-zero");

/// A stream of device events backed by an mpsc channel.
pub struct DeviceEventsSource {
    inner: bachelor::channel::mpsc::MpscChannelConsumer<Event>,
}

impl Stream for DeviceEventsSource {
    type Item = Event;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.inner).poll_next(cx)
    }
}

pub use discovery::{DeviceFilter, DeviceMatcher};

/// Creates a device event source and driver pair.
pub fn create(
    filter: DeviceFilter,
    label_store: std::rc::Rc<DeviceLabelStore>,
) -> (DeviceEventsSource, driver::Driver) {
    let (events_tx, events_rx) = mpsc::channel(DEVICE_CHANNEL_CAPACITY);
    let source = DeviceEventsSource { inner: events_rx };
    let driver = driver::Driver::new(events_tx, filter, label_store);
    (source, driver)
}

pub mod driver {
    use super::Event;
    use super::async_udev::AsyncMonitorSocket;
    use super::discovery::{self, DeviceFilter};
    use super::label::{DeviceLabel, DeviceLabelStore};
    use crate::device_events::event_source::{open_event_stream, translate_event_stream};
    use bachelor::channel::mpsc::MpscChannelProducer;
    use evdev::EventStream;
    use futures::StreamExt;
    use futures::{SinkExt, TryStreamExt};
    use rootcause::prelude::*;
    use std::io;
    use std::ops::Deref;
    use std::path::PathBuf;
    use std::rc::Rc;
    use tokio::task::spawn_local;

    pub struct Driver {
        events_tx: MpscChannelProducer<Event>,
        filter: Rc<DeviceFilter>,
        label_store: Rc<DeviceLabelStore>,
    }

    impl Driver {
        pub(super) fn new(
            events_tx: MpscChannelProducer<Event>,
            filter: DeviceFilter,
            label_store: Rc<DeviceLabelStore>,
        ) -> Self {
            Self {
                events_tx,
                filter: Rc::new(filter),
                label_store,
            }
        }

        pub async fn run(self) -> Result<(), Report> {
            let label_store = self.label_store.clone();

            // Enumerate existing devices and spawn tasks for each
            let devices = discovery::find_devices(&self.filter)
                .context("Failed to enumerate input devices")?;
            for (device, label) in devices {
                spawn_local(run_device(
                    InputDevice::Enumerated(device),
                    label,
                    label_store.clone(),
                    self.events_tx.clone(),
                ));
            }

            // Monitor for hot-plugged devices
            let monitor = udev::MonitorBuilder::new()
                .and_then(|b| b.match_subsystem("input"))
                .and_then(|b| b.listen())
                .context("Failed to create udev monitor")?;

            let mut monitor =
                AsyncMonitorSocket::new(monitor).context("Failed to create async monitor")?;

            while let Some(result) = monitor.next().await {
                match result {
                    Ok(event) => {
                        if event.event_type() != udev::EventType::Add {
                            continue;
                        }
                        let Some(label) = self.filter.matches(&event) else {
                            continue;
                        };
                        spawn_local(run_device(
                            InputDevice::HotPlugged(event),
                            label,
                            label_store.clone(),
                            self.events_tx.clone(),
                        ));
                    }
                    Err(error) => {
                        log::warn!("udev monitor error: {error}");
                    }
                }
            }

            bail!("udev monitor stopped unexpectedly");
        }
    }

    /// Holds either a `udev::Device` or `udev::Event`, providing access to the underlying device.
    enum InputDevice {
        Enumerated(udev::Device),
        HotPlugged(udev::Event),
    }

    impl Deref for InputDevice {
        type Target = udev::Device;

        fn deref(&self) -> &Self::Target {
            match self {
                InputDevice::Enumerated(d) => d,
                InputDevice::HotPlugged(e) => e,
            }
        }
    }

    async fn run_device(
        device: InputDevice,
        label: DeviceLabel,
        label_store: Rc<DeviceLabelStore>,
        tx: MpscChannelProducer<Event>,
    ) {
        let label_str = label_store.resolve(label);
        let Some(devnode) = device.devnode() else {
            log::warn!("[{label_str}] device has no devnode, skipping");
            return;
        };

        let stream = match open_event_stream(devnode) {
            Ok(device) => device,
            Err(error) => {
                log::warn!(
                    "{}",
                    report!(error)
                        .context("Failed to open device")
                        .attach(format!("label: {label_str}"))
                        .attach(format!("devnode: {}", devnode.display()))
                );
                return;
            }
        };

        let devnode = PathBuf::from(devnode);
        drop(device);

        let evdev_device = stream.device();
        log::info!(
            "attached [{label_str}]: {}\n  name: {}\n  physical path: {}",
            devnode.display(),
            evdev_device.name().unwrap_or("unknown"),
            evdev_device.physical_path().unwrap_or("unknown"),
        );
        match stream_device_events(stream, label, tx).await {
            Ok(()) => {
                log::info!(
                    "finished streaming events for removed device [{label_str}] {}",
                    devnode.display()
                );
            }
            Err(report) => {
                log::warn!(
                    "{}",
                    report
                        .attach(format!("label: {label_str}"))
                        .attach(format!("devnode: {}", devnode.display()))
                );
            }
        }

        log::info!("detached [{label_str}]: {}", devnode.display());
    }

    async fn stream_device_events(
        stream: EventStream,
        label: DeviceLabel,
        tx: MpscChannelProducer<Event>,
    ) -> Result<(), Report> {
        enum StreamError {
            Device(io::Error),
            Channel,
        }

        let result = translate_event_stream(stream, label)
            .map_err(StreamError::Device)
            .forward(tx.into_sink().sink_map_err(|_| StreamError::Channel))
            .await;

        match result {
            Ok(()) => Ok(()),
            Err(StreamError::Device(e)) if e.raw_os_error() == Some(libc::ENODEV) => Ok(()),
            Err(StreamError::Device(e)) => Err(report!(e).context("Device error").into_dynamic()),
            Err(StreamError::Channel) => Err(report!("Event channel closed").into_dynamic()),
        }
    }
}

mod async_udev {
    use futures::Stream;
    use std::io;
    use std::pin::Pin;
    use std::task::{Context, Poll};
    use tokio::io::unix::AsyncFd;
    use udev::MonitorSocket;

    pub struct AsyncMonitorSocket {
        fd: AsyncFd<MonitorSocket>,
    }

    impl AsyncMonitorSocket {
        pub fn new(monitor: MonitorSocket) -> io::Result<Self> {
            Ok(Self {
                fd: AsyncFd::new(monitor)?,
            })
        }
    }

    impl Stream for AsyncMonitorSocket {
        type Item = Result<udev::Event, io::Error>;

        fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
            loop {
                let mut guard = match self.fd.poll_read_ready_mut(cx) {
                    Poll::Ready(Ok(guard)) => guard,
                    Poll::Ready(Err(err)) => {
                        return Poll::Ready(Some(Err(err)));
                    }
                    Poll::Pending => return Poll::Pending,
                };

                if let Some(event) = guard.get_inner_mut().iter().next() {
                    return Poll::Ready(Some(Ok(event)));
                }

                guard.clear_ready();
            }
        }
    }
}
