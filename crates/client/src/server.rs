use std::pin::pin;

use crate::credit::limit::{CreditLimitConsumer, CreditLimitSource};
use crate::pain::{PainLiveConsumer, PainLiveSource};
use crate::usage::{AllUsageConsumer, AllUsageSources};
use crate::watch_mux::{WatchMux, define_watch_mux};
use bachelor::error::Closed;
use futures::future::{Either, select};
use futures::{SinkExt, StreamExt, pin_mut};
use rootcause::prelude::*;
use shared::model::{Credit, CreditLimit};
use shared::oe_spawn;
use shared::protocol::client::{ClientMessage, ClientServerCodec, CowStr, PROTOCOL_VERSION};
use shared::protocol::write_protocol_version;
use shared::shutdown::ShutdownSignal;
use tokio::net::{UnixListener, UnixStream};
use tokio_util::codec::Framed;
use tracing::{debug, error, info};

/// Creates the client server.
pub fn create(
    listener: UnixListener,
    sources: AllUsageSources,
    pain_source: Option<PainLiveSource>,
    credit_limits: CreditLimitSource,
) -> ClientServer {
    ClientServer {
        listener,
        sources,
        pain_source,
        credit_limits,
    }
}

pub struct ClientServer {
    listener: UnixListener,
    sources: AllUsageSources,
    pain_source: Option<PainLiveSource>,
    credit_limits: CreditLimitSource,
}

impl ClientServer {
    /// Run the server until `shutdown` fires, then return.
    pub async fn run(self, mut shutdown: ShutdownSignal) {
        loop {
            let accept = self.listener.accept();
            let wait = shutdown.wait();
            pin_mut!(accept, wait);
            let event = select(accept, wait).await;
            match event {
                Either::Left((Ok((stream, _)), _)) => {
                    let listener_sources = self.sources.subscribe_forward();
                    let listener_pain = self
                        .pain_source
                        .as_ref()
                        .map(PainLiveSource::subscribe_forward);
                    let listener_limits = self.credit_limits.subscribe_forward();
                    // Intentionally detached: each listener is bounded by
                    // client disconnect or by closure of the watched sources
                    // during app shutdown, so the IPC accept loop does not
                    // maintain a child task registry.
                    oe_spawn!("client-ipc-listener", async move {
                        handle_listener(stream, listener_sources, listener_pain, listener_limits)
                            .await;
                    });
                    info!("new client connected");
                }
                Either::Left((Err(e), _)) => {
                    error!("Failed to accept client connection: {}", e);
                }
                Either::Right(((), _)) => {
                    info!("Client server shutting down");
                    return;
                }
            }
        }
    }
}

define_watch_mux! {
    struct ListenerInputs;
    flags ListenerInput;
    usage: AllUsageConsumer => USAGE,
    pain: Option<PainLiveConsumer> => PAIN,
    limits: CreditLimitConsumer => LIMITS,
}

#[derive(Clone, Copy, PartialEq)]
struct OutboundSnapshot {
    rest: (Credit, CreditLimit),
    breaks: (Credit, CreditLimit),
    day: (Credit, CreditLimit),
}

impl OutboundSnapshot {
    fn read(inputs: &ListenerInputs) -> Self {
        let limits = inputs.limits.view(|state| *state);
        inputs.usage.view(|_all, rest, breaks, day| Self {
            rest: (rest.credit().total(), limits.rest),
            breaks: (breaks.credit().total(), limits.breaks),
            day: (day.credit().total(), limits.day),
        })
    }

    fn messages_since(self, previous: Option<Self>) -> Vec<ClientMessage> {
        let mut messages = Vec::with_capacity(3);
        if previous.is_none_or(|previous| previous.rest != self.rest) {
            messages.push(ClientMessage::Rest(self.rest.0, self.rest.1));
        }
        if previous.is_none_or(|previous| previous.breaks != self.breaks) {
            messages.push(ClientMessage::Break(self.breaks.0, self.breaks.1));
        }
        if previous.is_none_or(|previous| previous.day != self.day) {
            messages.push(ClientMessage::Day(self.day.0, self.day.1));
        }
        messages
    }
}

async fn handle_listener(
    mut stream: UnixStream,
    usage: AllUsageConsumer,
    pain: Option<PainLiveConsumer>,
    limits: CreditLimitConsumer,
) {
    if let Err(e) = write_protocol_version(&mut stream, PROTOCOL_VERSION).await {
        debug!("Failed to send protocol version to CLI listener: {e}");
        return;
    }

    let codec = ClientServerCodec::default();
    let mut framed = Framed::new(stream, codec);
    let mut inputs = WatchMux::new(ListenerInputs {
        usage,
        pain,
        limits,
    });

    let mut last_snapshot = OutboundSnapshot::read(inputs.get());
    let initial_pain = inputs
        .get()
        .pain
        .as_ref()
        .map(|pain| {
            pain.view(|state, catalog| {
                state
                    .entries
                    .iter()
                    .map(|(label, ratio)| (catalog.resolve(*label), *ratio))
                    .collect::<Vec<(&'static str, f64)>>()
            })
        })
        .unwrap_or_default();
    let initial =
        last_snapshot
            .messages_since(None)
            .into_iter()
            .chain(
                initial_pain
                    .into_iter()
                    .map(|(label, ratio)| ClientMessage::Pain {
                        label: CowStr::borrowed(label),
                        ratio,
                    }),
            );
    for msg in initial {
        if let Err(e) = framed.send(msg).await {
            error!("failed to send initial state to client: {e}");
            return;
        }
    }

    loop {
        let event = match next_event(&mut inputs, &mut framed).await {
            Ok(event) => event,
            Err(report) => {
                error!("client handler failed: {report}");
                return;
            }
        };

        let msgs_to_send = match event {
            ListenerEvent::InputsClosed => {
                debug!("all application sources closed, disconnecting client");
                return;
            }
            ListenerEvent::ClientGone => {
                info!("client disconnected");
                return;
            }
            ListenerEvent::Input(ListenerInput::USAGE | ListenerInput::LIMITS) => {
                let snapshot = OutboundSnapshot::read(inputs.get());
                let messages = snapshot.messages_since(Some(last_snapshot));
                last_snapshot = snapshot;
                messages
            }
            ListenerEvent::Input(ListenerInput::PAIN) => inputs
                .get()
                .pain
                .as_ref()
                .map(|pain| {
                    pain.view(|state, catalog| {
                        state
                            .entries
                            .iter()
                            .map(|(label, ratio)| ClientMessage::Pain {
                                label: CowStr::borrowed(catalog.resolve(*label)),
                                ratio: *ratio,
                            })
                            .collect()
                    })
                })
                .unwrap_or_default(),
            ListenerEvent::Input(_) => continue,
        };

        for msg in msgs_to_send {
            if let Err(e) = framed.send(msg).await {
                debug!("Failed to send message to CLI listener: {e}");
                return;
            }
        }
    }
}

enum ListenerEvent {
    Input(ListenerInput),
    InputsClosed,
    ClientGone,
}

async fn next_event(
    inputs: &mut WatchMux<ListenerInputs>,
    framed: &mut Framed<UnixStream, ClientServerCodec>,
) -> Result<ListenerEvent, Report> {
    let input = pin!(inputs.changed());
    let command = framed.next();
    Ok(match select(input, command).await {
        Either::Left((Ok(input), _)) => ListenerEvent::Input(input),
        Either::Left((Err(Closed), _)) => ListenerEvent::InputsClosed,
        Either::Right((Some(Ok(cmd)), _)) => match cmd {},
        Either::Right((Some(Err(e)), _)) => {
            return Err(e).context("Failed to decode CLI command")?;
        }
        Either::Right((None, _)) => ListenerEvent::ClientGone,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::credit::limit::{self, CreditLimitProducer, CreditLimitState};
    use crate::pain::{self, PainLabelStore, PainState};
    use crate::usage::all::AllState;
    use crate::usage::breaks::BreakState;
    use crate::usage::daily::DayState;
    use crate::usage::rest::RestState;
    use bachelor::watch::{MpmcWatchRefProducer, mpmc_watch};
    use futures::future::{join, join3};
    use shared::protocol::client::CliCodec;
    use shared::protocol::read_protocol_version;
    use std::time::Duration;
    use tokio::time::timeout;

    const TEST_TIMEOUT: Duration = Duration::from_secs(2);
    const NO_MESSAGE_TIMEOUT: Duration = Duration::from_millis(20);

    struct UsageProducers {
        _all: MpmcWatchRefProducer<AllState>,
        rest: MpmcWatchRefProducer<RestState>,
        _breaks: MpmcWatchRefProducer<BreakState>,
        _day: MpmcWatchRefProducer<DayState>,
    }

    fn usage_fixture() -> (AllUsageConsumer, UsageProducers) {
        let (all, all_source) = mpmc_watch(AllState::default());
        let (rest, rest_source) = mpmc_watch(RestState::default());
        let (breaks, break_source) = mpmc_watch(BreakState::default());
        let (day, day_source) = mpmc_watch(DayState::default());
        let sources = AllUsageSources::new(all_source, rest_source, break_source, day_source);
        (
            sources.subscribe_forward(),
            UsageProducers {
                _all: all,
                rest,
                _breaks: breaks,
                _day: day,
            },
        )
    }

    async fn connect_cli(stream: &mut UnixStream) -> Framed<&mut UnixStream, CliCodec> {
        assert_eq!(
            read_protocol_version(stream, PROTOCOL_VERSION)
                .await
                .unwrap(),
            None
        );
        Framed::new(stream, CliCodec::default())
    }

    async fn next_message(framed: &mut Framed<&mut UnixStream, CliCodec>) -> ClientMessage {
        framed.next().await.unwrap().unwrap()
    }

    fn assert_initial_usage(messages: [ClientMessage; 3]) {
        assert!(matches!(
            messages[0],
            ClientMessage::Rest(Credit::ZERO, CreditLimit::ZERO)
        ));
        assert!(matches!(
            messages[1],
            ClientMessage::Break(Credit::ZERO, CreditLimit::ZERO)
        ));
        assert!(matches!(
            messages[2],
            ClientMessage::Day(Credit::ZERO, CreditLimit::ZERO)
        ));
    }

    #[tokio::test]
    async fn absent_pain_survives_partial_closure_and_exits_after_all_inputs_close() {
        let (usage, usage_producers) = usage_fixture();
        let (limit_source, limit_producer) = limit::create(CreditLimitState::default());
        let limits = limit_source.subscribe_forward();
        let (mut client_stream, server_stream) = UnixStream::pair().unwrap();

        let client = async move {
            let mut framed = connect_cli(&mut client_stream).await;
            let initial = [
                next_message(&mut framed).await,
                next_message(&mut framed).await,
                next_message(&mut framed).await,
            ];
            assert_initial_usage(initial);

            usage_producers.rest.update(|_| {}).unwrap();
            assert!(timeout(NO_MESSAGE_TIMEOUT, framed.next()).await.is_err());

            limit_producer.update(|state| state.rest = CreditLimit::new(12.0));
            assert!(matches!(
                next_message(&mut framed).await,
                ClientMessage::Rest(Credit::ZERO, limit) if limit == CreditLimit::new(12.0)
            ));

            drop(limit_producer);
            assert!(timeout(NO_MESSAGE_TIMEOUT, framed.next()).await.is_err());

            drop(usage_producers);
            assert!(framed.next().await.is_none());
        };

        timeout(
            TEST_TIMEOUT,
            join(handle_listener(server_stream, usage, None, limits), client),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn active_pain_preserves_initial_order_and_full_map_updates() {
        let (usage, _usage_producers) = usage_fixture();
        let (limit_source, limit_producer): (_, CreditLimitProducer) =
            limit::create(CreditLimitState::default());
        let limits = limit_source.subscribe_forward();

        let mut labels = PainLabelStore::new();
        let left = labels.get_or_intern("left");
        let right = labels.get_or_intern("right");
        let catalog = pain::build_catalog(labels.finalize(), &[]);
        let (_committed, live_source, pain_producer, driver) =
            pain::create(catalog, PainState::default());
        pain_producer.set(left, 0.25).unwrap();
        pain_producer.set(right, 0.5).unwrap();
        let pain = live_source.subscribe_forward();
        let (mut client_stream, server_stream) = UnixStream::pair().unwrap();

        let client = async move {
            let mut framed = connect_cli(&mut client_stream).await;
            let initial = [
                next_message(&mut framed).await,
                next_message(&mut framed).await,
                next_message(&mut framed).await,
            ];
            assert_initial_usage(initial);
            assert!(matches!(
                next_message(&mut framed).await,
                ClientMessage::Pain { label, ratio }
                    if label.as_str() == "left" && ratio == 0.25
            ));
            assert!(matches!(
                next_message(&mut framed).await,
                ClientMessage::Pain { label, ratio }
                    if label.as_str() == "right" && ratio == 0.5
            ));

            pain_producer.set(left, 0.75).unwrap();
            assert!(matches!(
                next_message(&mut framed).await,
                ClientMessage::Pain { label, ratio }
                    if label.as_str() == "left" && ratio == 0.75
            ));
            assert!(matches!(
                next_message(&mut framed).await,
                ClientMessage::Pain { label, ratio }
                    if label.as_str() == "right" && ratio == 0.5
            ));

            drop(pain_producer);

            limit_producer.update(|state| state.rest = CreditLimit::new(9.0));
            assert!(matches!(
                next_message(&mut framed).await,
                ClientMessage::Rest(Credit::ZERO, limit) if limit == CreditLimit::new(9.0)
            ));
            limit_producer.update(|state| state.rest = CreditLimit::new(9.0));
            assert!(timeout(NO_MESSAGE_TIMEOUT, framed.next()).await.is_err());

            drop(framed);
        };

        let (_, driver_result, ()) = timeout(
            TEST_TIMEOUT,
            join3(
                handle_listener(server_stream, usage, Some(pain), limits),
                driver,
                client,
            ),
        )
        .await
        .unwrap();
        driver_result.unwrap();
    }

    #[tokio::test]
    async fn socket_closure_ends_listener_while_application_inputs_are_open() {
        let (usage, _usage_producers) = usage_fixture();
        let (limit_source, _limit_producer) = limit::create(CreditLimitState::default());
        let limits = limit_source.subscribe_forward();
        let (mut client_stream, server_stream) = UnixStream::pair().unwrap();

        let client = async move {
            let mut framed = connect_cli(&mut client_stream).await;
            for _ in 0..3 {
                let _ = next_message(&mut framed).await;
            }
            drop(framed);
        };

        timeout(
            TEST_TIMEOUT,
            join(handle_listener(server_stream, usage, None, limits), client),
        )
        .await
        .unwrap();
    }

    #[test]
    fn outbound_snapshot_compares_complete_pairs_in_protocol_order() {
        let previous = OutboundSnapshot {
            rest: (Credit::new(1.0), CreditLimit::new(10.0)),
            breaks: (Credit::new(2.0), CreditLimit::new(20.0)),
            day: (Credit::new(3.0), CreditLimit::new(30.0)),
        };
        let current = OutboundSnapshot {
            rest: (Credit::new(4.0), CreditLimit::new(10.0)),
            breaks: (Credit::new(2.0), CreditLimit::new(25.0)),
            day: previous.day,
        };

        let messages = current.messages_since(Some(previous));
        assert_eq!(messages.len(), 2);
        assert!(matches!(messages[0], ClientMessage::Rest(_, _)));
        assert!(matches!(messages[1], ClientMessage::Break(_, _)));
        assert!(current.messages_since(Some(current)).is_empty());
    }
}
