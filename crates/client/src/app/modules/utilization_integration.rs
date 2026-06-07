use super::credit::CreditSinks;
use crate::credit::utilization::CreditUtilizationConsumer;
use bachelor::error::Closed;
use rootcause::prelude::*;
use shared::oe_spawn;
use shared::spawn::JoinHandle;

/// Spawn the utilization sink forwarder. Callers should check
/// [`CreditSinks::any`] before calling; otherwise the spawned task
/// has nothing to push to.
pub fn start(
    consumer: CreditUtilizationConsumer,
    sinks: CreditSinks,
) -> JoinHandle<Result<(), Report>> {
    oe_spawn!("credit-utilization-out-forwarder", run(consumer, sinks),)
}

async fn run(mut consumer: CreditUtilizationConsumer, sinks: CreditSinks) -> Result<(), Report> {
    loop {
        match consumer.changed().await {
            Err(Closed) => return Ok(()),
            Ok(()) => {
                let (rest, breaks, day) = consumer.view(|s| {
                    let u = s.last_published();
                    (u.rest, u.breaks, u.day)
                });
                if let Some(sink) = sinks.rest.as_ref() {
                    let _ = sink.update(rest);
                }
                if let Some(sink) = sinks.breaks.as_ref() {
                    let _ = sink.update(breaks);
                }
                if let Some(sink) = sinks.day.as_ref() {
                    let _ = sink.update(day);
                }
            }
        }
    }
}
