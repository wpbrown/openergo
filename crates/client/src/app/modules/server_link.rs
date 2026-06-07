use crate::activity::ActivityProducer;
use crate::client;
use crate::credit::CreditCalculator;
use crate::usage::UsageRawProducer;
use bachelor::error::Closed;
use futures::SinkExt;
use rootcause::prelude::*;
use shared::oe_spawn;
use shared::protocol::server::UsageIncrement;
use shared::shutdown::ShutdownSignal;
use shared::spawn::JoinHandle;
use std::future::ready;
use std::path::PathBuf;

/// Spawn the reconnect loop that connects to the upstream server and
/// pushes each `(UsageIncrement, CreditIncrement)` pair onto the usage
/// broadcast. When the returned join handle resolves the broadcast
/// producer drops, closing the broadcast and letting the usage
/// drivers exit cleanly.
pub fn start(
    socket_path: PathBuf,
    usage_raw_producer: UsageRawProducer,
    activity_producer: ActivityProducer,
    mut calculator: CreditCalculator,
    shutdown: ShutdownSignal,
) -> JoinHandle<Result<(), Report>> {
    let producer = usage_raw_producer.with(move |increment: UsageIncrement| {
        let credit = calculator.calculate(&increment);
        ready(Ok::<_, Closed>((increment, credit)))
    });
    oe_spawn!(
        "server-reconnect",
        client::reconnect_loop(socket_path, producer, activity_producer, shutdown)
    )
}
