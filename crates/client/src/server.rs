use crate::credit::limit::{CreditLimitConsumer, CreditLimitSource};
use crate::pain::{PainConsumer, PainSource};
use crate::usage::{AllUsageConsumer, AllUsageSources, UsageSource};
use bachelor::error::Closed;
use futures::future::{Either, select};
use futures::{SinkExt, StreamExt, pin_mut};
use rootcause::prelude::*;
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
    pain_source: PainSource,
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
    pain_source: PainSource,
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
                    let listener_pain = self.pain_source.subscribe_forward();
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

async fn handle_listener(
    mut stream: UnixStream,
    mut sources: AllUsageConsumer,
    mut pain: PainConsumer,
    mut limits: CreditLimitConsumer,
) {
    if let Err(e) = write_protocol_version(&mut stream, PROTOCOL_VERSION).await {
        debug!("Failed to send protocol version to CLI listener: {e}");
        return;
    }

    let codec = ClientServerCodec::default();
    let mut framed = Framed::new(stream, codec);

    // Snapshot limits up front. We keep a local cache so a single limit
    // change only re-sends the messages whose limit actually moved.
    let mut last_limits = limits.view(|state| *state);

    // Send initial totals so the listener has a snapshot before waiting for
    // the next change.
    let (rest, brk, day) = sources.view(|_all, rest, breaks, day| {
        (
            rest.credit().total(),
            breaks.credit().total(),
            day.credit().total(),
        )
    });
    let initial_pain = pain.view(|state, catalog| {
        state
            .entries
            .iter()
            .map(|(label, entry)| (catalog.resolve(*label), entry.live()))
            .collect::<Vec<(&'static str, f64)>>()
    });
    let initial = [
        ClientMessage::Rest(rest, last_limits.rest),
        ClientMessage::Break(brk, last_limits.breaks),
        ClientMessage::Day(day, last_limits.day),
    ]
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
        let outcome = match next_outcome(&mut sources, &mut pain, &mut limits, &mut framed).await {
            Ok(o) => o,
            Err(report) => {
                error!("client handler failed: {report}");
                return;
            }
        };

        let msgs_to_send: Vec<ClientMessage> = match outcome {
            Outcome::SourceGone(name) => {
                debug!("{name} source closed, disconnecting client");
                return;
            }
            Outcome::ClientGone => {
                info!("client disconnected");
                return;
            }
            Outcome::State(UsageSource::All) => continue, // intentionally not forwarded
            Outcome::State(UsageSource::Rest) => {
                let total = sources.view(|_, rest, _, _| rest.credit().total());
                vec![ClientMessage::Rest(total, last_limits.rest)]
            }
            Outcome::State(UsageSource::Break) => {
                let total = sources.view(|_, _, breaks, _| breaks.credit().total());
                vec![ClientMessage::Break(total, last_limits.breaks)]
            }
            Outcome::State(UsageSource::Day) => {
                let total = sources.view(|_, _, _, day| day.credit().total());
                vec![ClientMessage::Day(total, last_limits.day)]
            }
            Outcome::Pain => pain.view(|state, catalog| {
                state
                    .entries
                    .iter()
                    .map(|(label, entry)| ClientMessage::Pain {
                        label: CowStr::borrowed(catalog.resolve(*label)),
                        ratio: entry.live(),
                    })
                    .collect()
            }),
            Outcome::Limits => {
                let new_limits = limits.view(|state| *state);
                let mut msgs = Vec::with_capacity(3);
                if new_limits.rest != last_limits.rest {
                    let total = sources.view(|_, rest, _, _| rest.credit().total());
                    msgs.push(ClientMessage::Rest(total, new_limits.rest));
                }
                if new_limits.breaks != last_limits.breaks {
                    let total = sources.view(|_, _, breaks, _| breaks.credit().total());
                    msgs.push(ClientMessage::Break(total, new_limits.breaks));
                }
                if new_limits.day != last_limits.day {
                    let total = sources.view(|_, _, _, day| day.credit().total());
                    msgs.push(ClientMessage::Day(total, new_limits.day));
                }
                last_limits = new_limits;
                if msgs.is_empty() {
                    continue;
                }
                msgs
            }
        };

        for msg in msgs_to_send {
            if let Err(e) = framed.send(msg).await {
                debug!("Failed to send message to CLI listener: {e}");
                return;
            }
        }
    }
}

/// Inputs that can drive the listener loop.
enum Outcome {
    /// One of the four usage state sources reported a change.
    State(UsageSource),
    /// The pain state changed.
    Pain,
    /// The credit-limit state changed.
    Limits,
    /// One of the upstream watch sources closed. Named so the log line can
    /// identify which one. Generally happens during client shutdown.
    SourceGone(&'static str),
    /// The downstream CLI disconnected its socket.
    ClientGone,
}

/// Race the listener's input streams. Decoding errors from the CLI socket
/// surface as `Err`; everything else (including upstream/downstream
/// disconnects) is reported via [`Outcome`] so the caller can decide how
/// loudly to log it.
async fn next_outcome(
    sources: &mut AllUsageConsumer,
    pain: &mut PainConsumer,
    limits: &mut CreditLimitConsumer,
    framed: &mut Framed<UnixStream, ClientServerCodec>,
) -> Result<Outcome, Report> {
    let next_state = sources.changed();
    let next_pain = pain.changed();
    let next_limits = limits.changed();
    let next_command = framed.next();
    let state_or_pain = select(next_state, next_pain);
    let state_pain_or_limits = select(state_or_pain, next_limits);
    Ok(match select(state_pain_or_limits, next_command).await {
        Either::Left((Either::Left((Either::Left((Ok(source), _)), _)), _)) => {
            Outcome::State(source)
        }
        Either::Left((Either::Left((Either::Left((Err(Closed), _)), _)), _)) => {
            Outcome::SourceGone("usage")
        }
        Either::Left((Either::Left((Either::Right((Ok(()), _)), _)), _)) => Outcome::Pain,
        Either::Left((Either::Left((Either::Right((Err(Closed), _)), _)), _)) => {
            Outcome::SourceGone("pain")
        }
        Either::Left((Either::Right((Ok(()), _)), _)) => Outcome::Limits,
        Either::Left((Either::Right((Err(Closed), _)), _)) => Outcome::SourceGone("credit-limit"),
        Either::Right((Some(Ok(cmd)), _)) => match cmd {},
        Either::Right((Some(Err(e)), _)) => {
            return Err(e).context("Failed to decode CLI command")?;
        }
        Either::Right((None, _)) => Outcome::ClientGone,
    })
}
