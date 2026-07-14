use litemap::LiteMap;
use shared::label::{Label, LabelStore};

/// Cheap, copyable handle for an interned endpoint label. Equality is a
/// single integer compare; resolving to a `&str` requires the
/// originating [`EndpointLabelStore`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct EndpointLabel(pub(super) Label);

/// Owns the interned strings backing every [`EndpointLabel`]. Mutated
/// only during config processing; once populated it is leaked to obtain
/// a `&'static EndpointLabelStore` so labels resolved off it are
/// `&'static str` and can be stored cheaply on transport-side domain
/// types used for tracing.
pub struct EndpointLabelStore {
    inner: LabelStore,
}

impl EndpointLabelStore {
    pub fn new() -> Self {
        Self {
            inner: LabelStore::new(),
        }
    }

    pub fn get_or_intern(&mut self, label: &str) -> EndpointLabel {
        EndpointLabel(self.inner.get_or_intern(label))
    }

    pub fn get(&self, label: &str) -> Option<EndpointLabel> {
        self.inner.get(label).map(EndpointLabel)
    }

    pub fn resolve(&self, label: EndpointLabel) -> &str {
        self.inner.resolve(label.0)
    }
}

impl Default for EndpointLabelStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Direction of an endpoint relative to the application: `In` means the
/// device produces values, `Out` means the application sends values to
/// the device, `InOut` means both.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Direction {
    #[default]
    In,
    Out,
    InOut,
}

impl Direction {
    pub fn allows_in(self) -> bool {
        matches!(self, Direction::In | Direction::InOut)
    }

    pub fn allows_out(self) -> bool {
        matches!(self, Direction::Out | Direction::InOut)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Direction::In => "in",
            Direction::Out => "out",
            Direction::InOut => "inout",
        }
    }
}

/// The minimum the [`super::Binder`] needs to know about a per-endpoint
/// catalog entry. Implementors carry whatever transport-specific
/// addressing they like; the binder only inspects the direction.
pub trait EndpointConfig {
    fn direction(&self) -> Direction;
}

/// Read-only catalog mapping every configured endpoint label to its
/// transport-specific configuration `T`. Built by the endpoints app module
/// and held on the application stack; the label store is leaked
/// separately so resolved labels can be `&'static str`.
pub struct EndpointCatalog<T> {
    labels: &'static EndpointLabelStore,
    by_label: LiteMap<EndpointLabel, T>,
}

impl<T> EndpointCatalog<T> {
    /// Construct a catalog from a leaked label store and a label-keyed
    /// map of endpoint configurations.
    pub fn new(labels: &'static EndpointLabelStore, by_label: LiteMap<EndpointLabel, T>) -> Self {
        Self { labels, by_label }
    }

    pub fn lookup(&self, label: EndpointLabel) -> Option<&T> {
        self.by_label.get(&label)
    }

    /// Remove and return the entry for `label`, leaving the slot
    /// empty. Used by [`super::Binder::complete`] to hand owned
    /// per-endpoint configurations to the caller. Subsequent
    /// [`Self::lookup`] calls for the same label return `None`.
    pub fn take(&mut self, label: EndpointLabel) -> Option<T> {
        self.by_label.remove(&label)
    }

    pub fn labels(&self) -> &'static EndpointLabelStore {
        self.labels
    }
}
