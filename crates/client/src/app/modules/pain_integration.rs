use crate::integration::AnalogIn;
use crate::pain::{PainLabel, PainProducer};
use bachelor::error::Closed;
use rootcause::prelude::*;
use shared::oe_spawn;
use shared::select_small::select_small_once;
use shared::spawn::JoinHandle;
use tracing::trace;

/// Spawn the pain analog-in forwarder. `analog_ins` must be
/// non-empty.
pub fn start(
    analog_ins: Vec<(PainLabel, AnalogIn)>,
    producer: PainProducer,
) -> JoinHandle<Result<(), Report>> {
    oe_spawn!("pain-integration", run(analog_ins, producer),)
}

/// Forward each bound [`AnalogIn`] onto its [`PainLabel`] via
/// [`PainProducer::set`]. Exits on stage-1 shutdown, when the producer
/// drops, or when every `AnalogIn` watch has closed.
async fn run(
    mut analog_ins: Vec<(PainLabel, AnalogIn)>,
    producer: PainProducer,
) -> Result<(), Report> {
    loop {
        let (fired_res, fired_idx) =
            select_small_once::<_, 4>(analog_ins.iter_mut().map(|(_, ain)| ain.changed())).await;

        match fired_res {
            Ok(()) => {
                let (label, ain) = &analog_ins[fired_idx];
                let value = ain.get();
                trace!(
                    label = producer.catalog().resolve(*label),
                    value, "forwarding",
                );
                producer.set(*label, value);
            }
            Err(Closed) => {
                analog_ins.swap_remove(fired_idx);
                if analog_ins.is_empty() {
                    return Ok(());
                }
            }
        }
    }
}
