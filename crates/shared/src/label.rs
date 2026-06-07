use lasso::{Rodeo, Spur};

/// Cheap, copyable handle for an interned label string. Equality on `Label`
/// is a single integer compare; resolving back to a `&str` requires the
/// originating [`LabelStore`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Label(Spur);

/// Owns the interned strings backing every [`Label`]. Mutated only during
/// config processing; afterwards it is wrapped in [`std::rc::Rc`] and shared
/// read-only between async tasks on the local set.
pub struct LabelStore {
    rodeo: Rodeo,
}

impl LabelStore {
    pub fn new() -> Self {
        Self {
            rodeo: Rodeo::new(),
        }
    }

    /// Intern `label` (or return the existing handle).
    pub fn get_or_intern(&mut self, label: &str) -> Label {
        Label(self.rodeo.get_or_intern(label))
    }

    /// Intern a `&'static str`. Slightly cheaper than `get_or_intern` for
    /// compile-time-known labels.
    pub fn get_or_intern_static(&mut self, label: &'static str) -> Label {
        Label(self.rodeo.get_or_intern_static(label))
    }

    /// Look up an already-interned label without interning it. Returns `None`
    /// if `label` is unknown to this store.
    pub fn get(&self, label: &str) -> Option<Label> {
        self.rodeo.get(label).map(Label)
    }

    /// Resolve a `Label` back to its string form. Panics if the label
    /// originated from a different store.
    pub fn resolve(&self, label: Label) -> &str {
        self.rodeo.resolve(&label.0)
    }
}

impl Default for LabelStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intern_is_idempotent() {
        let mut store = LabelStore::new();
        let a = store.get_or_intern("left");
        let b = store.get_or_intern("left");
        let c = store.get_or_intern("right");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(store.resolve(a), "left");
        assert_eq!(store.resolve(c), "right");
    }

    #[test]
    fn get_returns_none_for_unknown() {
        let store = LabelStore::new();
        assert!(store.get("missing").is_none());
    }
}
