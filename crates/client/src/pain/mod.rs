use bachelor::error::Closed;
use bachelor::watch::{MpmcWatchRefConsumer, MpmcWatchRefProducer, MpmcWatchRefSource, mpmc_watch};
use futures::future::{Either, select};
use futures::pin_mut;
use litemap::LiteMap;
use rootcause::prelude::*;
use shared::shutdown::ShutdownSignal;
use std::rc::Rc;
use std::time::Duration;
use tokio::time::{Instant, sleep_until};
use tracing::trace;

pub mod state;

pub use state::{PainBias, PainLabel, PainLabelStore, PainState};

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
    inner: MpmcWatchRefProducer<PainState>,
    catalog: Rc<PainCatalog>,
}

impl PainProducer {
    /// Catalog shared with the source/consumer side. Exposed so callers
    /// that already hold a `PainProducer` don't need a second handle to
    /// resolve labels for logging.
    pub fn catalog(&self) -> &PainCatalog {
        &self.catalog
    }

    /// Set `label`'s pain live value. The watch is never closed while at
    /// least one consumer or source is alive, but any send error is silently
    /// dropped (consistent with how other drivers in this codebase treat
    /// closed watches).
    pub fn set(&self, label: PainLabel, live: f64) {
        let _ = self.inner.update(|state| state.set(label, live));
    }

    /// Commit `live` onto `ratio` for every label in `labels`. Used only by
    /// the debounce task in [`create`].
    fn commit(&self, labels: &[PainLabel]) -> Result<(), Closed> {
        self.inner.update(|state| {
            for label in labels {
                if tracing::enabled!(tracing::Level::TRACE)
                    && let Some(entry) = state.entries.get(label)
                {
                    trace!(
                        label = self.catalog.resolve(*label),
                        live = entry.live(),
                        ratio = entry.ratio(),
                        "committing pain change",
                    );
                }
                state.commit(*label);
            }
        })
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
    shutdown: ShutdownSignal,
) -> (
    PainSource,
    PainProducer,
    impl Future<Output = Result<(), Report>> + use<>,
) {
    let catalog = Rc::new(catalog);
    let (producer, source) = mpmc_watch(initial);
    let producer = PainProducer {
        inner: producer,
        catalog: Rc::clone(&catalog),
    };
    let source = PainSource {
        inner: source,
        catalog,
    };

    let driver = debounce_loop(source.subscribe_forward(), producer.clone(), shutdown);
    (source, producer, driver)
}

/// Background task that copies `live` over `ratio` after a quiet window of
/// [`DEBOUNCE`]. Tracks pending labels in a [`LiteMap`] keyed by the
/// most recent observed change. On shutdown, commits any still-pending
/// entries before returning.
async fn debounce_loop(
    mut consumer: PainConsumer,
    producer: PainProducer,
    mut shutdown: ShutdownSignal,
) -> Result<(), Report> {
    let mut pending: LiteMap<PainLabel, Instant> = LiteMap::new();

    loop {
        // Refresh `pending` with any entries whose `live` no longer matches
        // `ratio`. Any commit by this task itself will land here as a no-op
        // (live == ratio after commit) so the loop converges on quiescence.
        let now = Instant::now();
        consumer.view(|state, _catalog| {
            for (label, entry) in &state.entries {
                if entry.live() != entry.ratio() {
                    pending.insert(*label, now);
                }
            }
        });

        let next_due = pending.iter().map(|(_, t)| *t + DEBOUNCE).min();

        let timer = async {
            match next_due {
                Some(deadline) => sleep_until(deadline).await,
                None => std::future::pending::<()>().await,
            }
        };

        let changed = consumer.changed();
        let shut = shutdown.wait();
        pin_mut!(timer, changed, shut);

        match select(select(changed, timer), shut).await {
            // Watch saw a new value; loop will rescan.
            Either::Left((Either::Left((Ok(()), _)), _)) => {}
            // Source closed; commit anything pending and exit.
            Either::Left((Either::Left((Err(Closed), _)), _)) => {
                commit_pending(&producer, &mut pending);
                return Ok(());
            }
            // Debounce window elapsed; commit due entries.
            Either::Left((Either::Right(((), _)), _)) => {
                let now = Instant::now();
                let due: Vec<PainLabel> = pending
                    .iter()
                    .filter_map(|(l, t)| (*t + DEBOUNCE <= now).then_some(*l))
                    .collect();
                if !due.is_empty() {
                    if producer.commit(&due).is_err() {
                        return Ok(());
                    }
                    for l in &due {
                        pending.remove(l);
                    }
                }
            }
            // Shutdown: flush everything and exit.
            Either::Right(((), _)) => {
                commit_pending(&producer, &mut pending);
                return Ok(());
            }
        }
    }
}

fn commit_pending(producer: &PainProducer, pending: &mut LiteMap<PainLabel, Instant>) {
    if pending.is_empty() {
        return;
    }
    let labels: Vec<PainLabel> = pending.iter().map(|(l, _)| *l).collect();
    let _ = producer.commit(&labels);
    pending.clear();
}
