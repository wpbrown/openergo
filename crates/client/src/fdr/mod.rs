pub mod record;
pub mod writer;

mod builder;
mod feeders;
mod schema;

use crate::activity::ActivityStateConsumer;
use crate::credit::limit::CreditLimitConsumer;
use crate::credit::utilization::{CreditEventConsumer, CreditUtilizationConsumer};
use crate::pain::PainConsumer;
use crate::persistence::AppStateIdentity;
use crate::usage::{AllUsageConsumer, UsageRawConsumer};
use bachelor::channel::mpsc::channel as mpsc_channel;
use directories::ProjectDirs;
use futures::future::{Either, join_all, select};
use record::{FdrRecord, FdrSession};
use rootcause::prelude::*;
use shared::oe_spawn;
use std::num::NonZeroUsize;
use std::path::PathBuf;
use tracing::debug;
use writer::writer_task;

/// Bounded capacity of the internal record channel. Generous: the writer
/// drains it continuously, so it only buffers brief bursts between polls.
const RECORD_CHANNEL_CAPACITY: NonZeroUsize =
    NonZeroUsize::new(32).expect("broadcast capacity must be non-zero");

/// Default path for the Flight Data Recorder SQLite database.
///
/// Prefers `$XDG_STATE_HOME/openergo/fdr.db` on Linux, falling back to
/// the data directory if no state directory is available.
pub fn default_db_path() -> PathBuf {
    let dir = ProjectDirs::from("", "", "openergo")
        .map(|dirs| {
            dirs.state_dir()
                .unwrap_or_else(|| dirs.data_dir())
                .to_path_buf()
        })
        .unwrap_or_else(|| PathBuf::from("."));
    dir.join("fdr.db")
}

/// The full set of inputs the recorder needs. The credit-event feeder takes
/// its own utilization subscription for the per-event utilization context.
pub struct FdrConsumers {
    pub usage: AllUsageConsumer,
    pub usage_raw: UsageRawConsumer,
    pub pain: PainConsumer,
    pub limits: CreditLimitConsumer,
    pub activity: ActivityStateConsumer,
    pub events: CreditEventConsumer,
    pub events_utilization: CreditUtilizationConsumer,
}

pub async fn create(identity: AppStateIdentity, consumers: FdrConsumers) -> Result<(), Report> {
    let FdrConsumers {
        usage,
        usage_raw,
        pain,
        limits,
        activity,
        events,
        events_utilization,
    } = consumers;

    let (records_tx, records_rx) = mpsc_channel::<FdrRecord>(RECORD_CHANNEL_CAPACITY);
    let writer = oe_spawn!("fdr-writer", writer_task(records_rx));

    // Initial session record. The activity total baseline is read before the
    // activity consumer moves into its feeder.
    let session = activity.view(|state| FdrSession::new(state, &identity));
    let _ = records_tx.send(FdrRecord::Session(Box::new(session))).await;

    let usage_feeder = oe_spawn!(
        "fdr-usage-bucket",
        feeders::usage_bucket(usage_raw, records_tx.clone())
    );
    let activity_feeder = oe_spawn!(
        "fdr-activity",
        feeders::activity_sampler(activity, records_tx.clone())
    );
    let window_feeder = oe_spawn!(
        "fdr-credit-window",
        feeders::credit_window(usage, records_tx.clone())
    );
    let pain_feeder = oe_spawn!("fdr-pain", feeders::pain(pain, records_tx.clone()));
    let limit_feeder = oe_spawn!(
        "fdr-credit-limit",
        feeders::credit_limit(limits, records_tx.clone())
    );
    let event_feeder = oe_spawn!(
        "fdr-credit-event",
        feeders::credit_event(events, events_utilization, records_tx.clone(),)
    );

    // Drop the coordinator's own sender so the channel closes once every
    // feeder has finished and dropped its clone.
    drop(records_tx);

    debug!("fdr waiting for feeder input closure or writer completion");
    let feeders = join_all([
        usage_feeder,
        activity_feeder,
        window_feeder,
        pain_feeder,
        limit_feeder,
        event_feeder,
    ]);

    let result = match select(writer, feeders).await {
        Either::Left((result, _)) => result,
        Either::Right((_feeder_results, writer)) => {
            debug!("fdr shutting down writer");
            writer.await
        }
    };

    debug!("fdr writer shut down");
    result
}
