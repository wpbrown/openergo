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
    /// debounce task in [`crate::pain::create`] when the live input has been
    /// quiescent for the debounce window.
    ratio: f64,
    /// Timestamp of the last `ratio` commit.
    last_updated: Timestamp,
}

impl PainEntry {
    pub fn ratio(&self) -> f64 {
        self.ratio
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
    /// Set the committed ratio for `label` and refresh `last_updated`.
    /// Intended to be called only by the debounce task in
    /// [`crate::pain::create`].
    pub(super) fn commit(&mut self, label: PainLabel, ratio: f64) {
        if let Some(entry) = self.entries.get_mut(&label) {
            entry.ratio = ratio;
            entry.last_updated = Timestamp::now();
        } else {
            self.entries.insert(
                label,
                PainEntry {
                    ratio,
                    last_updated: Timestamp::now(),
                },
            );
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct PainLiveState {
    pub entries: LiteMap<PainLabel, f64>,
}

impl PainLiveState {
    pub fn from_committed(committed: &PainState) -> Self {
        let entries = committed
            .entries
            .iter()
            .map(|(label, entry)| (*label, entry.ratio()))
            .collect();
        Self { entries }
    }

    pub fn set(&mut self, label: PainLabel, ratio: f64) {
        self.entries.insert(label, ratio);
    }
}
