use std::path::{Path, PathBuf};

/// Controls which udev devices are monitored.
pub struct DeviceFilter {
    auto_detect: bool,
    include: Vec<DeviceMatcher>,
    exclude: Vec<DeviceMatcher>,
}

impl Default for DeviceFilter {
    fn default() -> Self {
        Self {
            auto_detect: true,
            include: Vec::new(),
            exclude: Vec::new(),
        }
    }
}

impl DeviceFilter {
    pub fn new(
        auto_detect: bool,
        include: Vec<DeviceMatcher>,
        exclude: Vec<DeviceMatcher>,
    ) -> Self {
        Self {
            auto_detect,
            include,
            exclude,
        }
    }

    /// Returns `true` if the device should be monitored.
    pub fn matches(&self, device: &udev::Device) -> bool {
        if device.devnode().is_none() {
            return false;
        }
        let base = self.auto_detect && is_input_device(device);
        let included = self.include.iter().any(|m| matcher_matches(m, device));
        let excluded = self.exclude.iter().any(|m| matcher_matches(m, device));
        (base || included) && !excluded
    }
}

/// Discover input devices filtered by the given `DeviceFilter`.
pub fn find_devices(filter: &DeviceFilter) -> std::io::Result<Vec<udev::Device>> {
    let mut enumerator = udev::Enumerator::new()?;
    enumerator.match_subsystem("input")?;
    let devices: Vec<udev::Device> = enumerator
        .scan_devices()?
        .filter(|d| filter.matches(d))
        .collect();
    Ok(devices)
}

/// Returns `true` if the udev device is a keyboard, mouse, or touchpad.
fn is_input_device(device: &udev::Device) -> bool {
    has_property(device, "ID_INPUT_KEYBOARD")
        || has_property(device, "ID_INPUT_MOUSE")
        || has_property(device, "ID_INPUT_TOUCHPAD")
}

fn has_property(device: &udev::Device, name: &str) -> bool {
    device.property_value(name).is_some_and(|v| v == "1")
}

/// Matches a device by path and/or udev properties. All specified fields must
/// match (AND logic).
pub struct DeviceMatcher {
    pub path: Option<PathBuf>,
    pub model: Option<String>,
    pub model_id: Option<String>,
    pub vendor_id: Option<String>,
    pub serial: Option<String>,
    pub bus: Option<String>,
}

/// Check if a single `DeviceMatcher` matches the given udev device.
/// All specified fields must match (AND logic).
fn matcher_matches(matcher: &DeviceMatcher, device: &udev::Device) -> bool {
    if let Some(path) = &matcher.path
        && !path_matches(path, device)
    {
        return false;
    }
    if let Some(model) = &matcher.model
        && !property_eq(device, "ID_MODEL", model)
    {
        return false;
    }
    if let Some(model_id) = &matcher.model_id
        && !property_eq(device, "ID_MODEL_ID", model_id)
    {
        return false;
    }
    if let Some(vendor_id) = &matcher.vendor_id
        && !property_eq(device, "ID_VENDOR_ID", vendor_id)
    {
        return false;
    }
    if let Some(serial) = &matcher.serial
        && !property_eq(device, "ID_SERIAL", serial)
    {
        return false;
    }
    if let Some(bus) = &matcher.bus
        && !property_eq(device, "ID_BUS", bus)
    {
        return false;
    }
    true
}

/// Check if the configured path matches either DEVNAME or any DEVLINKS entry.
fn path_matches(path: &Path, device: &udev::Device) -> bool {
    if device.devnode().is_some_and(|n| n == path) {
        return true;
    }
    // DEVLINKS is a space-separated list of symlink paths.
    device
        .property_value("DEVLINKS")
        .and_then(|v| v.to_str())
        .is_some_and(|links| links.split(' ').any(|link| Path::new(link) == path))
}

fn property_eq(device: &udev::Device, name: &str, expected: &str) -> bool {
    device.property_value(name).is_some_and(|v| v == expected)
}
