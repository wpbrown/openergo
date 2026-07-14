use crate::integration::{Direction, EndpointCatalog, EndpointConfig, EndpointLabelStore};
use crate::transports::midi::MidiMessage;
use std::collections::HashMap;
use std::rc::Rc;

pub enum TransportConfigs {
    Midi(MidiTransportConfig),
}

impl EndpointConfig for TransportConfigs {
    fn direction(&self) -> Direction {
        match self {
            Self::Midi(midi) => midi.control.direction,
        }
    }
}

pub struct MidiTransportConfig {
    pub device: Rc<(String, MidiDeviceConfig)>,
    pub control: MidiControlConfig,
}

pub enum DeviceConfig {
    Midi(MidiDeviceConfig),
}

#[derive(Debug)]
pub struct MidiDeviceConfig {
    pub port: Option<String>,
    pub client: Option<String>,
    pub controls: HashMap<String, MidiControlConfig>,
}

#[derive(Debug)]
pub struct MidiControlConfig {
    pub message: MidiMessage,
    pub channel: u8,
    pub number: u8,
    pub direction: Direction,
}

/// Build an [`EndpointCatalog`] from instantiated device configuration.
/// Consumes `devices` so every per-device and per-control entry is
/// owned by the catalog and (eventually) the bindings the binder hands
/// back. Creates a fresh label store, interns each device's control
/// labels into it, and leaks the populated store so labels resolved
/// off the catalog are `&'static str`.
pub fn init(devices: HashMap<String, DeviceConfig>) -> EndpointCatalog<TransportConfigs> {
    let mut labels = EndpointLabelStore::new();
    let mut by_label = litemap::LiteMap::new();

    let mut devices: Vec<(String, DeviceConfig)> = devices.into_iter().collect();
    devices.sort_by(|a, b| a.0.cmp(&b.0));
    for (device_key, device) in devices {
        match device {
            DeviceConfig::Midi(mut midi) => {
                let mut controls: Vec<(String, MidiControlConfig)> =
                    std::mem::take(&mut midi.controls).into_iter().collect();
                controls.sort_by(|a, b| a.0.cmp(&b.0));
                let device = Rc::new((device_key, midi));
                for (label, control) in controls {
                    let endpoint = labels.get_or_intern(&label);
                    by_label.insert(
                        endpoint,
                        TransportConfigs::Midi(MidiTransportConfig {
                            device: Rc::clone(&device),
                            control,
                        }),
                    );
                }
            }
        }
    }

    let labels: &'static EndpointLabelStore = Box::leak(Box::new(labels));
    EndpointCatalog::new(labels, by_label)
}
