use bachelor::error::Closed;
use bachelor::watch::{MpmcWatchRefConsumer, MpmcWatchRefProducer, MpmcWatchRefSource, mpmc_watch};
use litemap::LiteMap;
use rootcause::prelude::*;
use smallvec::SmallVec;
use std::rc::Rc;
use std::time::Duration;
use tokio::time::timeout;
use tracing::trace;

pub mod check;
pub mod state;

pub use state::{PainBias, PainLabel, PainLabelStore, PainLiveState, PainState};

/// How long `live` must be quiescent for an entry before the debounce task
/// commits it onto `ratio`.
const DEBOUNCE: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub struct PainSourceSpec {
    pub label: PainLabel,
    pub bias: PainBias,
}

pub struct PainCatalog {
    labels: &'static PainLabelStore,
    bias: LiteMap<PainLabel, PainBias>,
}

impl PainCatalog {
    pub fn resolve(&self, label: PainLabel) -> &'static str {
        self.labels.resolve(label)
    }

    pub fn labels(&self) -> &'static PainLabelStore {
        self.labels
    }

    /// Bias for `label`. Defaults to [`PainBias::Center`] for labels that
    /// exist in state but not in the configured sources (e.g. labels loaded
    /// from persistence whose `[pain.sources]` entry has been removed).
    pub fn bias_of(&self, label: PainLabel) -> PainBias {
        self.bias.get(&label).copied().unwrap_or(PainBias::Center)
    }
}

#[derive(Clone)]
pub struct PainProducer {
    live: MpmcWatchRefProducer<PainLiveState>,
    catalog: Rc<PainCatalog>,
}

impl PainProducer {
    /// Catalog shared with the source/consumer side. Exposed so callers
    /// that already hold a `PainProducer` don't need a second handle to
    /// resolve labels for logging.
    pub fn catalog(&self) -> &PainCatalog {
        &self.catalog
    }

    pub fn set(&self, label: PainLabel, live: f64) -> Result<(), Closed> {
        self.live.update(|state| state.set(label, live))
    }
}

/// The source side of the live pain watch. Hands out [`PainLiveConsumer`]
/// subscribers for listeners that need raw, immediate pain values.
pub struct PainLiveSource {
    inner: MpmcWatchRefSource<PainLiveState>,
    catalog: Rc<PainCatalog>,
}

impl PainLiveSource {
    pub fn subscribe_forward(&self) -> PainLiveConsumer {
        PainLiveConsumer {
            inner: self.inner.subscribe_forward(),
            catalog: Rc::clone(&self.catalog),
        }
    }
}

pub struct PainLiveConsumer {
    inner: MpmcWatchRefConsumer<PainLiveState>,
    catalog: Rc<PainCatalog>,
}

impl PainLiveConsumer {
    pub fn changed(&mut self) -> impl Future<Output = Result<(), Closed>> + Unpin {
        self.inner.changed()
    }

    pub fn view<R>(&self, f: impl FnOnce(&PainLiveState, &PainCatalog) -> R) -> R {
        let catalog = &*self.catalog;
        self.inner.view(|state| f(state, catalog))
    }
}

impl crate::watch_mux::FiniteChanges for PainLiveConsumer {
    fn changed(&mut self) -> impl Future<Output = Result<(), Closed>> + Unpin + '_ {
        PainLiveConsumer::changed(self)
    }
}

/// The source side of the pain watch. Hands out [`PainConsumer`] subscribers.
#[derive(Clone)]
pub struct PainSource {
    inner: MpmcWatchRefSource<PainState>,
    catalog: Rc<PainCatalog>,
}

impl PainSource {
    pub fn subscribe_forward(&self) -> PainConsumer {
        PainConsumer {
            inner: self.inner.subscribe_forward(),
            catalog: Rc::clone(&self.catalog),
        }
    }
}

pub struct PainConsumer {
    inner: MpmcWatchRefConsumer<PainState>,
    catalog: Rc<PainCatalog>,
}

impl PainConsumer {
    pub fn changed(&mut self) -> impl Future<Output = Result<(), Closed>> + Unpin {
        self.inner.changed()
    }

    pub fn view<R>(&self, f: impl FnOnce(&PainState, &PainCatalog) -> R) -> R {
        let catalog = &*self.catalog;
        self.inner.view(|state| f(state, catalog))
    }
}

impl crate::watch_mux::FiniteChanges for PainConsumer {
    fn changed(&mut self) -> impl Future<Output = Result<(), Closed>> + Unpin + '_ {
        PainConsumer::changed(self)
    }
}

/// Build a [`PainCatalog`] from the configured source specs and an already
/// populated [`PainLabelStore`] (which has interned every config-declared
/// label and been finalized into a `&'static` borrow).
pub fn build_catalog(labels: &'static PainLabelStore, specs: &[PainSourceSpec]) -> PainCatalog {
    let bias = specs.iter().map(|s| (s.label, s.bias)).collect();
    PainCatalog { labels, bias }
}

/// Construct the pain watch and a driver future. The driver runs the
/// debounce task that copies `live` values onto `ratio` once they have
/// been quiescent for [`DEBOUNCE`]. External producers (the AnalogIn
/// forwarder spawned by `main`, the CLI command path) feed `live` via
/// the returned [`PainProducer`].
pub fn create(
    catalog: PainCatalog,
    initial: PainState,
) -> (
    PainSource,
    PainLiveSource,
    PainProducer,
    impl Future<Output = Result<(), Report>> + use<>,
) {
    let catalog = Rc::new(catalog);
    let initial_live = PainLiveState::from_committed(&initial);
    let (live_producer, live_source) = mpmc_watch(initial_live);
    let (state_producer, source) = mpmc_watch(initial);
    let producer = PainProducer {
        live: live_producer,
        catalog: Rc::clone(&catalog),
    };
    let source = PainSource {
        inner: source,
        catalog: Rc::clone(&catalog),
    };
    let live_source = PainLiveSource {
        inner: live_source,
        catalog: Rc::clone(&catalog),
    };

    let driver = debounce_loop(live_source.subscribe_forward(), state_producer, catalog);
    (source, live_source, producer, driver)
}

/// Construct a closed, readable committed pain source without a live watch or
/// debounce driver.
pub fn create_inactive(catalog: PainCatalog, initial: PainState) -> PainSource {
    let catalog = Rc::new(catalog);
    let (state_producer, source) = mpmc_watch(initial);
    drop(state_producer);
    PainSource {
        inner: source,
        catalog,
    }
}

/// Background task that copies live values into committed ratios after a quiet
/// window of [`DEBOUNCE`]. When the live watch closes, commits the final live
/// values before returning and dropping the committed producer.
async fn debounce_loop(
    mut live: PainLiveConsumer,
    state: MpmcWatchRefProducer<PainState>,
    catalog: Rc<PainCatalog>,
) -> Result<(), Report> {
    loop {
        if live.changed().await.is_err() {
            break;
        }

        loop {
            match wait(&mut live).await {
                DebounceWake::Changed => {}
                DebounceWake::Closed => {
                    let _ = sync(&live, &state, &catalog);
                    return Ok(());
                }
                DebounceWake::Quiet => {
                    if sync(&live, &state, &catalog).is_err() {
                        return Ok(());
                    }
                    break;
                }
            }
        }
    }

    let _ = sync(&live, &state, &catalog);
    Ok(())
}

enum DebounceWake {
    Changed,
    Closed,
    Quiet,
}

async fn wait(live: &mut PainLiveConsumer) -> DebounceWake {
    match timeout(DEBOUNCE, live.changed()).await {
        Ok(Ok(())) => DebounceWake::Changed,
        Ok(Err(Closed)) => DebounceWake::Closed,
        Err(_) => DebounceWake::Quiet,
    }
}

fn sync(
    live: &PainLiveConsumer,
    state: &MpmcWatchRefProducer<PainState>,
    catalog: &PainCatalog,
) -> Result<(), Closed> {
    let pending = get_pending(live, state);
    if pending.is_empty() {
        Ok(())
    } else {
        commit_pending(state, catalog, &pending)
    }
}

fn get_pending(
    live: &PainLiveConsumer,
    state: &MpmcWatchRefProducer<PainState>,
) -> SmallVec<[(PainLabel, f64); 4]> {
    live.view(|live_state, _catalog| {
        state.view(|committed| {
            live_state
                .entries
                .iter()
                .filter_map(|(label, live_ratio)| {
                    let committed_ratio = committed.entries.get(label).map(|entry| entry.ratio());
                    (committed_ratio != Some(*live_ratio)).then_some((*label, *live_ratio))
                })
                .collect()
        })
    })
}

fn commit_pending(
    state: &MpmcWatchRefProducer<PainState>,
    catalog: &PainCatalog,
    pending: &[(PainLabel, f64)],
) -> Result<(), Closed> {
    state.update(|state| {
        for &(label, ratio) in pending {
            if tracing::enabled!(tracing::Level::TRACE) {
                trace!(
                    label = catalog.resolve(label),
                    ratio, "committing pain change",
                );
            }
            state.commit(label, ratio);
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn catalog_with_label() -> (PainCatalog, PainLabel) {
        let mut labels = PainLabelStore::new();
        let label = labels.get_or_intern("left-hand");
        let labels = labels.finalize();
        (build_catalog(labels, &[]), label)
    }

    #[tokio::test]
    async fn inactive_source_is_closed_but_readable() {
        let (catalog, label) = catalog_with_label();
        let mut initial = PainState::default();
        initial.commit(label, 0.75);

        let source = create_inactive(catalog, initial);
        let mut consumer = source.subscribe_forward();

        consumer.view(|state, catalog| {
            assert_eq!(catalog.resolve(label), "left-hand");
            assert_eq!(state.entries.get(&label).unwrap().ratio(), 0.75);
        });
        assert_eq!(consumer.changed().await, Err(Closed));
    }

    #[tokio::test]
    async fn active_driver_commits_final_state_and_closes() {
        let (catalog, label) = catalog_with_label();
        let (source, _live_source, producer, driver) = create(catalog, PainState::default());
        producer.set(label, 0.5).unwrap();

        drop(producer);
        driver.await.unwrap();

        let mut consumer = source.subscribe_forward();
        consumer.view(|state, _catalog| {
            assert_eq!(state.entries.get(&label).unwrap().ratio(), 0.5);
        });
        assert_eq!(consumer.changed().await, Err(Closed));
    }
}
