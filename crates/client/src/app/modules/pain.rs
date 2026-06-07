use super::super::config;
use crate::integration::{AnalogIn, Binder, EndpointConfig, EndpointLabel, EndpointLabelStore};
use crate::pain::{
    self, PainBias, PainCatalog, PainLabel, PainLabelStore, PainLiveSource, PainProducer,
    PainSource, PainSourceSpec, PainState,
};
use rootcause::prelude::*;
use shared::oe_spawn;
use shared::spawn::JoinHandle;

/// Pre-startup view of the `[pain]` configuration: the resolved
/// catalog (label store + per-label bias) plus the per-source
/// `(pain-label, control-label)` pairs the binder later consumes to
/// hand back one `AnalogIn` per configured source.
pub struct PainModule {
    catalog: PainCatalog,
    sources: Vec<(PainLabel, EndpointLabel)>,
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
            let source_label_str = binder.labels().resolve(*source_label);
            let analog_in = binder.analog_in(source_label_str).map_err(|e| {
                report!("pain source binding failed for control '{source_label_str}': {e}")
            })?;
            out.push((*pain_label, analog_in));
        }
        Ok(out)
    }

    /// Spawn the pain debounce driver. Consumes the module: after
    /// `start` the catalog lives inside the returned [`PainSource`]
    /// / [`PainLiveSource`] / [`PainProducer`] handles (shared via `Rc`). Returns the
    /// spawned driver's join handle alongside the source/producer
    /// pair.
    pub fn start(
        self,
        initial: PainState,
    ) -> (
        PainSource,
        PainLiveSource,
        PainProducer,
        JoinHandle<Result<(), Report>>,
    ) {
        let (source, live_source, producer, driver) = pain::create(self.catalog, initial);
        let task = oe_spawn!("pain-driver", driver);
        (source, live_source, producer, task)
    }
}

/// Resolve the `[pain]` configuration into a [`PainModule`]. Each
/// source's control name is looked up in `endpoint_labels` (the
/// already-populated label store from the endpoint catalog); an
/// unknown control is reported as a startup error.
pub fn init(
    cfg: Option<config::PainConfig>,
    endpoint_labels: &'static EndpointLabelStore,
) -> Result<PainModule, Report> {
    let mut pain_label_store = PainLabelStore::new();
    let mut specs: Vec<PainSourceSpec> = Vec::new();
    let mut sources: Vec<(PainLabel, EndpointLabel)> = Vec::new();
    if let Some(cfg) = cfg {
        specs.reserve(cfg.sources.len());
        sources.reserve(cfg.sources.len());
        for (name, source) in cfg.sources {
            let bias = match source.bias {
                config::PainBiasConfig::Left => PainBias::Left,
                config::PainBiasConfig::Right => PainBias::Right,
                config::PainBiasConfig::Center => PainBias::Center,
            };
            let source_label = endpoint_labels.get(&source.source).ok_or_else(|| {
                report!(
                    "pain source '{name}' references unknown control '{}'",
                    source.source
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
