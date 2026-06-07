use jiff::Timestamp;
use litemap::LiteMap;
use serde::{Deserialize, Serialize};
use shared::label::{Label, LabelStore};

/// Cheap, copyable handle for an interned pain label. Equality is a single
/// integer compare; resolving to a `&str` requires the originating
/// [`PainLabelStore`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PainLabel(Label);

/// Side bias for a pain source. Surfaces as a telemetry attribute and is
/// the hook for future credit/strain weighting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PainBias {
    Left,
    Right,
    Center,
}

impl PainBias {
    pub fn as_str(self) -> &'static str {
        match self {
            PainBias::Left => "left",
            PainBias::Right => "right",
            PainBias::Center => "center",
        }
    }
}

pub struct PainLabelStore {
    inner: LabelStore,
}

impl PainLabelStore {
    pub fn new() -> Self {
        Self {
            inner: LabelStore::new(),
        }
    }

    /// Intern `label` (or return the existing handle).
    pub fn get_or_intern(&mut self, label: &str) -> PainLabel {
        PainLabel(self.inner.get_or_intern(label))
    }

    /// Look up an already-interned label without interning it. Returns `None`
    /// if `label` is unknown to this store.
    pub fn get(&self, label: &str) -> Option<PainLabel> {
        self.inner.get(label).map(PainLabel)
    }

    /// Resolve a `PainLabel` back to its string form. Panics if the label
    /// originated from a different store.
    pub fn resolve(&self, label: PainLabel) -> &str {
        self.inner.resolve(label.0)
    }

    pub fn finalize(self) -> &'static Self {
        Box::leak(Box::new(self))
    }
}

impl Default for PainLabelStore {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct PainEntry {
    /// Debounced ratio used for downstream computation. Updated only by the
    /// debounce task in [`crate::pain::create`] when [`live`] has been
    /// quiescent for the debounce window.
    ratio: f64,
    /// Latest raw value as reported by the producer (MIDI / CLI). Provides
    /// instant feedback to UI consumers. Not persisted; on load it is
    /// initialized to match `ratio` via [`PainState::initialize_live_from_ratio`].
    #[serde(skip)]
    live: f64,
    /// Timestamp of the last `ratio` commit.
    last_updated: Timestamp,
}

impl PainEntry {
    pub fn ratio(&self) -> f64 {
        self.ratio
    }

    pub fn live(&self) -> f64 {
        self.live
    }

    pub fn last_updated(&self) -> Timestamp {
        self.last_updated
    }
}

#[derive(Debug, Default, Clone)]
pub struct PainState {
    pub entries: LiteMap<PainLabel, PainEntry>,
}

impl PainState {
    /// Set the live value for `label`. The committed `ratio` is updated
    /// asynchronously by the debounce task; for new entries `ratio`
    /// initializes to `0.0` until the debounce window elapses.
    pub fn set(&mut self, label: PainLabel, live: f64) {
        if let Some(entry) = self.entries.get_mut(&label) {
            entry.live = live;
        } else {
            self.entries.insert(
                label,
                PainEntry {
                    ratio: 0.0,
                    live,
                    last_updated: Timestamp::now(),
                },
            );
        }
    }

    /// Copy `live` over `ratio` for `label` and refresh `last_updated`.
    /// Intended to be called only by the debounce task in
    /// [`crate::pain::create`].
    pub(super) fn commit(&mut self, label: PainLabel) {
        if let Some(entry) = self.entries.get_mut(&label) {
            entry.ratio = entry.live;
            entry.last_updated = Timestamp::now();
        }
    }

    /// After loading from persistence, initialize each entry's `live`
    /// (which is not persisted and would otherwise default to `0.0`) to
    /// match its `ratio`.
    pub(crate) fn initialize_live_from_ratio(&mut self) {
        for (_, entry) in self.entries.iter_mut() {
            entry.live = entry.ratio;
        }
    }
}
