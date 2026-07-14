use super::endpoints::{MidiTransportConfig, TransportConfigs};
use crate::integration::{self, EndpointBinding, EndpointLabelStore};
use crate::transports;
use itertools::Itertools;
use rootcause::prelude::*;
use shared::oe_spawn;
use shared::shutdown::ShutdownSignal;
use shared::spawn::JoinHandle;
use std::rc::Rc;

pub struct TransportsModule {
    midi: Vec<transports::midi::MidiDeviceConfig>,
}

impl TransportsModule {
    /// Spawn one task per configured transport. Skips a transport
    /// entirely when no controls of that type are bound (e.g. with no
    /// MIDI controls we never open an ALSA seq client).
    pub fn start(self, shutdown: ShutdownSignal) -> Vec<JoinHandle<Result<(), Report>>> {
        let mut tasks = Vec::new();
        if !self.midi.is_empty() {
            tasks.push(oe_spawn!(
                "midi-transport",
                transports::midi::run(self.midi, shutdown)
            ));
        }
        tasks
    }
}

/// Build the transports runtime configuration by partitioning
/// `bound_endpoints` by transport. The catalog payload variant tags
/// each entry's transport, so adding HID later means adding a second
/// bucket and arm here.
pub fn init(
    label_store: &'static EndpointLabelStore,
    bound_endpoints: Vec<EndpointBinding<TransportConfigs>>,
) -> TransportsModule {
    let mut midi_bound: Vec<(
        integration::EndpointLabel,
        MidiTransportConfig,
        integration::EndpointIo,
    )> = Vec::with_capacity(bound_endpoints.len());
    for EndpointBinding { label, config, io } in bound_endpoints {
        match config {
            TransportConfigs::Midi(midi) => midi_bound.push((label, midi, io)),
        }
    }
    TransportsModule {
        midi: build_midi_devices(label_store, midi_bound),
    }
}

/// Materialize the MIDI transport's runtime configuration from the
/// already-partitioned MIDI bindings. Each entry's payload owns its
/// per-control config and shares an [`Rc`] of the device key + entry
/// with the other controls on the same device. Grouping by device
/// drops every Rc clone but the last; [`Rc::try_unwrap`] then moves
/// the device key + matchers out without cloning.
fn build_midi_devices(
    label_store: &'static EndpointLabelStore,
    bound: Vec<(
        integration::EndpointLabel,
        MidiTransportConfig,
        integration::EndpointIo,
    )>,
) -> Vec<transports::midi::MidiDeviceConfig> {
    bound
        .into_iter()
        .into_group_map_by(|(_, midi, _)| Rc::as_ptr(&midi.device))
        .into_values()
        .map(|entries| {
            let device_rc = Rc::clone(&entries[0].1.device);
            let controls: Vec<transports::midi::MidiControlDefinition> = entries
                .into_iter()
                .map(|(label, midi, endpoint)| {
                    let MidiTransportConfig { device: _, control } = midi;
                    transports::midi::MidiControlDefinition {
                        label: label_store.resolve(label),
                        message: control.message,
                        channel: control.channel,
                        number: control.number,
                        endpoint,
                    }
                })
                .collect();
            let (device_key, device) =
                Rc::try_unwrap(device_rc).expect("all sibling Rc clones were dropped above");
            transports::midi::MidiDeviceConfig {
                device_key,
                port_match: device.port,
                client_match: device.client,
                controls,
            }
        })
        .collect()
}
