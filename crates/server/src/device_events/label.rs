use lasso::{Rodeo, Spur};

/// Cheap, copyable handle for an interned device label. Equality on
/// `DeviceLabel` is a single integer compare; resolving back to a `&str`
/// requires the originating [`DeviceLabelStore`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct DeviceLabel(Spur);

/// Owns the interned strings backing every [`DeviceLabel`]. Mutated only
/// during config processing; afterwards it is wrapped in [`std::rc::Rc`] and
/// shared read-only between async tasks on the local set.
pub struct DeviceLabelStore {
    rodeo: Rodeo,
    auto_detect: DeviceLabel,
}

/// Synthetic label assigned to devices matched via auto-detect (i.e. they did
/// not match an explicit include rule). The angle brackets ensure this can
/// never collide with a user-supplied label.
const AUTO_DETECT_LABEL: &str = "<auto>";

impl DeviceLabelStore {
    pub fn new() -> Self {
        let mut rodeo = Rodeo::new();
        let auto_detect = DeviceLabel(rodeo.get_or_intern_static(AUTO_DETECT_LABEL));
        Self { rodeo, auto_detect }
    }

    /// The pre-interned label used for auto-detected devices.
    pub fn auto_detect(&self) -> DeviceLabel {
        self.auto_detect
    }

    /// Intern `label` (or return the existing handle).
    pub fn get_or_intern(&mut self, label: &str) -> DeviceLabel {
        DeviceLabel(self.rodeo.get_or_intern(label))
    }

    /// Look up an already-interned label without interning it. Returns `None`
    /// if `label` is unknown to this store.
    pub fn get(&self, label: &str) -> Option<DeviceLabel> {
        self.rodeo.get(label).map(DeviceLabel)
    }

    /// Resolve a `DeviceLabel` back to its string form. Panics if the label
    /// originated from a different store.
    pub fn resolve(&self, label: DeviceLabel) -> &str {
        self.rodeo.resolve(&label.0)
    }
}

impl Default for DeviceLabelStore {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_detect_resolves_to_angle_bracketed_auto() {
        let store = DeviceLabelStore::new();
        assert_eq!(store.resolve(store.auto_detect()), AUTO_DETECT_LABEL);
    }

    #[test]
    fn intern_is_idempotent() {
        let mut store = DeviceLabelStore::new();
        let a = store.get_or_intern("keyboard");
        let b = store.get_or_intern("keyboard");
        let c = store.get_or_intern("mouse");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(store.resolve(a), "keyboard");
        assert_eq!(store.resolve(c), "mouse");
    }
}
