use crate::activity::ActivityStateConsumer;
use crate::credit::utilization::CreditUtilizationConsumer;
use crate::pain::{PainConsumer, PainLabelStore};
use crate::persistence::{self, AppSnapshot, AppStateIdentity, PersistBaggage};
use crate::usage::AllUsageConsumer;
use rootcause::prelude::*;
use shared::oe_spawn;
use shared::spawn::JoinHandle;
use std::path::PathBuf;

/// Pre-startup view of the persistence layer: owns the resolved
/// state file path and the [`PersistBaggage`] returned from
/// [`crate::persistence::load`]. The loaded [`AppSnapshot`] is
/// returned separately by [`init`] so callers can destructure it
/// into per-driver initial state.
pub struct PersistenceModule {
    state_path: PathBuf,
    identity: AppStateIdentity,
    baggage: PersistBaggage,
}

impl PersistenceModule {
    /// The persisted app-state identity loaded at startup. Copied out so the
    /// FDR can record it in its session row before this module's identity
    /// moves into the persistence driver.
    pub fn identity(&self) -> AppStateIdentity {
        self.identity
    }

    /// Spawn the persistence driver. Consumes the module: the
    /// state path and baggage move into the driver, which owns
    /// them for the rest of the process.
    pub fn start(
        self,
        sources: AllUsageConsumer,
        pain: PainConsumer,
        utilization: CreditUtilizationConsumer,
        activity: ActivityStateConsumer,
    ) -> JoinHandle<Result<(), Report>> {
        let Self {
            state_path,
            identity,
            baggage,
        } = self;
        oe_spawn!(
            "persistence",
            persistence::create(
                state_path,
                sources,
                pain,
                utilization,
                activity,
                identity,
                baggage,
            )
        )
    }
}

/// Resolve the default state path and load any persisted snapshot
/// and baggage. The snapshot is returned alongside the module so
/// the caller can destructure it into per-driver initial state
/// before spawning the driver via [`PersistenceModule::start`].
pub async fn init(
    labels: &PainLabelStore,
) -> Result<(PersistenceModule, Option<AppSnapshot>), Report> {
    let state_path = persistence::default_state_path();
    let (snapshot, identity, baggage) = persistence::load(&state_path, labels).await?;
    Ok((
        PersistenceModule {
            state_path,
            identity,
            baggage,
        },
        snapshot,
    ))
}
