use bachelor::error::Closed;
use bachelor::watch::{
    MpmcWatchRefConsumer, MpmcWatchRefObserver, MpmcWatchRefProducer, MpmcWatchRefSource,
    mpmc_watch,
};
use futures::FutureExt;

/// Future returned by [`AnalogIn::changed`] / [`AnalogOut::changed`].
/// Resolves the next time the underlying watch sees an update, or with
/// [`Closed`] once every producer has dropped.
pub struct AnalogIoChanged<'a>(MpmcWatchRefObserver<'a, 'a>);

impl Future for AnalogIoChanged<'_> {
    type Output = Result<(), Closed>;

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        self.0.poll_unpin(cx)
    }
}

/// Domain-side endpoint handle for an `In` control. The transport task
/// writes values; the domain reads them.
pub struct AnalogIn {
    inner: MpmcWatchRefConsumer<f64>,
}

impl AnalogIn {
    pub fn changed(&mut self) -> AnalogIoChanged<'_> {
        AnalogIoChanged(self.inner.changed())
    }

    pub fn get(&self) -> f64 {
        self.inner.get()
    }
}

/// Transport-side handle for an `In` control. The transport task writes
/// the device's most recent value here.
pub struct AnalogInProducer {
    inner: MpmcWatchRefProducer<f64>,
}

impl AnalogInProducer {
    pub fn set(&self, value: f64) -> Result<(), Closed> {
        self.inner.set(value)
    }
}

/// Transport-side handle for an `Out` control. The transport task reads
/// each new value and sends it to the device.
pub struct AnalogOut {
    inner: MpmcWatchRefConsumer<f64>,
}

impl AnalogOut {
    pub fn changed(&mut self) -> AnalogIoChanged<'_> {
        AnalogIoChanged(self.inner.changed())
    }

    pub fn get(&self) -> f64 {
        self.inner.get()
    }
}

/// Domain-side handle for an `Out` control. The domain writes values
/// that the transport task forwards to the device.
pub struct AnalogOutProducer {
    inner: MpmcWatchRefProducer<f64>,
}

impl AnalogOutProducer {
    pub fn update(&self, value: f64) -> Result<(), Closed> {
        self.inner.set(value)
    }
}

/// Transport-side bundle of pre-bound endpoint halves for one control.
/// Produced by [`super::Binder::complete`] and consumed by transport
/// modules; the variants mirror the catalog's [`super::Direction`] but
/// carry the actual watch halves.
pub enum EndpointIo {
    In(AnalogInProducer),
    Out(AnalogOut),
    InOut {
        input: AnalogInProducer,
        output: AnalogOut,
    },
}

impl EndpointIo {
    /// Decompose into optional input and output halves, in that order.
    pub fn split(self) -> (Option<AnalogInProducer>, Option<AnalogOut>) {
        match self {
            EndpointIo::In(input) => (Some(input), None),
            EndpointIo::Out(output) => (None, Some(output)),
            EndpointIo::InOut { input, output } => (Some(input), Some(output)),
        }
    }
}

/// Subscription source for an `In` control. Held by the binder so
/// multiple domain consumers (e.g. two pain sources referencing the
/// same control label) can each get their own [`AnalogIn`] subscribed
/// to a single shared [`AnalogInProducer`]. Discarded once binding
/// finishes; the watch stays alive as long as the producer or any
/// consumer is alive.
pub(super) struct AnalogInSource {
    inner: MpmcWatchRefSource<f64>,
}

impl AnalogInSource {
    pub(super) fn subscribe(&self) -> AnalogIn {
        AnalogIn {
            inner: self.inner.subscribe_forward(),
        }
    }
}

/// Allocate a fresh AnalogIn watch seeded with `0.0` and return the
/// transport-side producer plus a subscription source. The caller is
/// responsible for subscribing at least one [`AnalogIn`] consumer via
/// [`AnalogInSource::subscribe`]; the source supports fan-out across
/// multiple consumers.
pub(super) fn analog_in_pair() -> (AnalogInProducer, AnalogInSource) {
    let (producer, source) = mpmc_watch::<f64>(0.0);
    (
        AnalogInProducer { inner: producer },
        AnalogInSource { inner: source },
    )
}

/// Allocate a fresh AnalogOut watch seeded with `initial` and return
/// both halves: the producer for the domain to write into and the
/// consumer for the transport task to read.
pub(super) fn analog_out_pair(initial: f64) -> (AnalogOutProducer, AnalogOut) {
    let (producer, source) = mpmc_watch::<f64>(initial);
    let consumer = source.subscribe_forward();
    (
        AnalogOutProducer { inner: producer },
        AnalogOut { inner: consumer },
    )
}
