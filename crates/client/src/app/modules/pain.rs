use crate::integration::{AnalogIn, Binder, EndpointConfig, EndpointLabel, EndpointLabelStore};
use crate::pain::{
    self, PainBias, PainCatalog, PainLabel, PainLabelStore, PainLiveSource, PainProducer,
    PainSource, PainSourceSpec, PainState,
};
use rootcause::prelude::*;
use shared::oe_spawn;
use shared::spawn::JoinHandle;

pub struct Config {
    pub sources: Vec<SourceConfig>,
}

pub struct SourceConfig {
    pub name: String,
    pub source: String,
    pub bias: PainBias,
}

/// Pre-startup view of the `[pain]` configuration: the resolved
/// catalog (label store + per-label bias) plus the per-source
/// `(pain-label, control-label)` pairs the binder later consumes to
/// hand back one `AnalogIn` per configured source.
pub struct PainModule {
    catalog: PainCatalog,
    sources: Vec<(PainLabel, EndpointLabel)>,
}

pub struct PainRuntime {
    pub source: PainSource,
    pub active: Option<ActivePainRuntime>,
}

pub struct ActivePainRuntime {
    pub live_source: PainLiveSource,
    pub producer: PainProducer,
    pub driver_task: JoinHandle<Result<(), Report>>,
}

impl PainModule {
    /// Catalog reference for callers that need to resolve labels
    /// before the pain driver has started (e.g. `persistence::load`).
    pub fn catalog(&self) -> &PainCatalog {
        &self.catalog
    }

    /// Resolve the configured `(pain-label, control-label)` pairs into
    /// one `AnalogIn` per source via `binder`. The returned vector is
    /// owned by `app::run`, which passes it to the
    /// `pain_input_forwarder` connector module.
    pub fn bind_sources<T: EndpointConfig>(
        &self,
        binder: &mut Binder<T>,
    ) -> Result<Vec<(PainLabel, AnalogIn)>, Report> {
        let mut out = Vec::with_capacity(self.sources.len());
        for (pain_label, source_label) in &self.sources {
            let analog_in = binder
                .analog_in(*source_label)
                .context("Failed to bind pain source as input")?;
            out.push((*pain_label, analog_in));
        }
        Ok(out)
    }

    /// Start active pain tracking when sources are configured, otherwise
    /// retain only a closed, readable committed snapshot.
    pub fn start(self, initial: PainState) -> PainRuntime {
        if self.sources.is_empty() {
            return PainRuntime {
                source: pain::create_inactive(self.catalog, initial),
                active: None,
            };
        }

        let (source, live_source, producer, driver) = pain::create(self.catalog, initial);
        PainRuntime {
            source,
            active: Some(ActivePainRuntime {
                live_source,
                producer,
                driver_task: oe_spawn!("pain-driver", driver),
            }),
        }
    }
}

/// Resolve the `[pain]` configuration into a [`PainModule`]. Each
/// source's control name is looked up in `endpoint_labels` (the
/// already-populated label store from the endpoint catalog); an
/// unknown control is reported as a startup error.
pub fn init(
    cfg: Option<Config>,
    endpoint_labels: &'static EndpointLabelStore,
) -> Result<PainModule, Report> {
    let mut pain_label_store = PainLabelStore::new();
    let mut specs: Vec<PainSourceSpec> = Vec::new();
    let mut sources: Vec<(PainLabel, EndpointLabel)> = Vec::new();
    if let Some(cfg) = cfg {
        specs.reserve(cfg.sources.len());
        sources.reserve(cfg.sources.len());
        for SourceConfig { name, source, bias } in cfg.sources {
            let source_label = endpoint_labels.get(&source).ok_or_else(|| {
                report!(
                    "pain source '{name}' references unknown control '{}'",
                    source
                )
            })?;
            let label = pain_label_store.get_or_intern(&name);
            specs.push(PainSourceSpec { label, bias });
            sources.push((label, source_label));
        }
    }
    // Finalize the label store (leaks it) so the catalog can borrow
    // the interned strings as `&'static`.
    let pain_label_store: &'static PainLabelStore = pain_label_store.finalize();
    let catalog = pain::build_catalog(pain_label_store, &specs);
    Ok(PainModule { catalog, sources })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::task::LocalSet;

    fn pain_module(active: bool) -> PainModule {
        let mut pain_labels = PainLabelStore::new();
        let pain_label = pain_labels.get_or_intern("left-hand");
        let catalog = pain::build_catalog(pain_labels.finalize(), &[]);
        let sources = if active {
            let mut endpoint_labels = EndpointLabelStore::new();
            vec![(pain_label, endpoint_labels.get_or_intern("pain-control"))]
        } else {
            Vec::new()
        };
        PainModule { catalog, sources }
    }

    #[tokio::test]
    async fn inactive_start_has_only_a_closed_readable_source() {
        let runtime = pain_module(false).start(PainState::default());
        assert!(runtime.active.is_none());

        let mut consumer = runtime.source.subscribe_forward();
        consumer.view(|state, _catalog| assert!(state.entries.is_empty()));
        assert_eq!(consumer.changed().await, Err(bachelor::error::Closed));
    }

    #[tokio::test]
    async fn active_start_returns_live_handles_and_joinable_driver() {
        LocalSet::new()
            .run_until(async {
                let runtime = pain_module(true).start(PainState::default());
                let active = runtime.active.expect("configured pain should be active");
                let _live_consumer = active.live_source.subscribe_forward();

                drop(active.producer);
                active.driver_task.await.unwrap();

                let mut consumer = runtime.source.subscribe_forward();
                assert_eq!(consumer.changed().await, Err(bachelor::error::Closed));
            })
            .await;
    }
}
