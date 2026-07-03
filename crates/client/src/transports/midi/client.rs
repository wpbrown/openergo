use super::seq_io::{Addr, InputEvent, PortAccess, SeqClientInfo, SeqIo, SeqPortInfo};
use rootcause::prelude::*;
use std::collections::{HashMap, HashSet, VecDeque};
use std::time::Duration;
use tracing::{debug, info, warn};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MidiDeviceId(usize);

impl MidiDeviceId {
    pub fn from_index(index: usize) -> Self {
        Self(index)
    }

    pub fn index(self) -> usize {
        self.0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MidiControlAddress {
    Cc { channel: u8, number: u8 },
    Note { channel: u8, number: u8 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MidiInputValue {
    Controller { value: i32 },
    NoteOn { velocity: u8 },
    NoteOff { velocity: u8 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MidiInputEvent {
    pub device: MidiDeviceId,
    pub control: MidiControlAddress,
    pub value: MidiInputValue,
}

pub struct MidiClient<S: SeqIo> {
    seq: S,
    devices: Vec<ClientDeviceState>,
    ports: HashMap<Addr, ClientPortState>,
    pending: VecDeque<MidiClientEvent>,
}

pub struct MidiClientConfig {
    pub devices: Vec<MidiDeviceSpec>,
    pub startup_drain_delay: Duration,
}

pub struct MidiDeviceSpec {
    pub id: MidiDeviceId,
    pub key: String,
    pub port_match: Option<String>,
    pub client_match: Option<String>,
    pub wants_input: bool,
    pub wants_output: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MidiClientEvent {
    DeviceAttached {
        device: MidiDeviceId,
        input: bool,
        output: bool,
    },
    DeviceDetached {
        device: MidiDeviceId,
    },
    DeviceUnavailable {
        device: MidiDeviceId,
        reason: DeviceUnavailableReason,
    },
    Input(MidiInputEvent),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceUnavailableReason {
    NoMatchingPort,
    MultipleMatchingPorts,
    PortClaimedByMultipleDevices,
    NoUsableDirection,
    IncomingSubscriptionFailed,
    OutgoingSubscriptionFailed,
}

#[derive(Clone, Copy, Debug)]
pub enum MidiSendError {
    DeviceInactive,
    OutputUnavailable,
    Seq(alsa::Error),
}

struct ClientPortState {
    access: PortAccess,
    matched_devices: Vec<MidiDeviceId>,
    active_device: Option<MidiDeviceId>,
    incoming_subscribed: bool,
    outgoing_subscribed: bool,
    incoming_failed: bool,
    outgoing_failed: bool,
}

struct ClientDeviceState {
    spec: MidiDeviceSpec,
    matching_ports: HashSet<Addr>,
    active: Option<ActiveDeviceBinding>,
    last_reported: DeviceAvailability,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ActiveDeviceBinding {
    addr: Addr,
    input: bool,
    output: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DeviceAvailability {
    Unknown,
    Attached { input: bool, output: bool },
    Unavailable(DeviceUnavailableReason),
}

enum BindingResolution {
    Active(ActiveDeviceBinding),
    Unavailable(DeviceUnavailableReason),
}

impl<S: SeqIo> MidiClient<S> {
    pub async fn start(seq: S, config: MidiClientConfig) -> Result<Self, Report> {
        let specs = config.devices;
        let mut observed_ports = Vec::new();

        for client in seq.clients() {
            let client_name = client.client_name().unwrap_or("");
            for port in client.ports() {
                let addr = port.addr();
                let Ok(port_name) = port.port_name() else {
                    continue;
                };
                let matched_devices = matching_devices(&specs, port_name, client_name);
                observed_ports.push((addr, port.access(), matched_devices));
            }
        }

        let devices = specs
            .into_iter()
            .map(|spec| ClientDeviceState {
                spec,
                matching_ports: HashSet::new(),
                active: None,
                last_reported: DeviceAvailability::Unknown,
            })
            .collect();

        let mut client = Self {
            seq,
            devices,
            ports: HashMap::new(),
            pending: VecDeque::new(),
        };

        for (addr, access, matched_devices) in observed_ports {
            client.observe_port(addr, access, matched_devices);
        }
        client.reconcile_all();

        tokio::time::sleep(config.startup_drain_delay).await;
        client
            .seq
            .drop_input()
            .context("Failed to drop buffered MIDI startup events")?;

        Ok(client)
    }

    pub async fn next_event(&mut self) -> Result<MidiClientEvent, Report> {
        loop {
            if let Some(event) = self.pending.pop_front() {
                return Ok(event);
            }

            match self.seq.next_event().await {
                Ok(InputEvent::Controller {
                    source,
                    channel,
                    controller,
                    value,
                }) => self.handle_input(
                    source,
                    MidiControlAddress::Cc {
                        channel,
                        number: controller,
                    },
                    MidiInputValue::Controller { value },
                ),
                Ok(InputEvent::NoteOn {
                    source,
                    channel,
                    note,
                    velocity,
                }) => self.handle_input(
                    source,
                    MidiControlAddress::Note {
                        channel,
                        number: note,
                    },
                    MidiInputValue::NoteOn { velocity },
                ),
                Ok(InputEvent::NoteOff {
                    source,
                    channel,
                    note,
                    velocity,
                }) => self.handle_input(
                    source,
                    MidiControlAddress::Note {
                        channel,
                        number: note,
                    },
                    MidiInputValue::NoteOff { velocity },
                ),
                Ok(InputEvent::PortStart { addr }) => self.handle_port_start(addr),
                Ok(InputEvent::PortExit { addr }) => self.handle_port_exit(addr),
                Err(error) => {
                    return Err(report!(error)
                        .context("alsa sequencer event stream error")
                        .into_dynamic());
                }
            }
        }
    }

    pub fn send_cc(
        &self,
        device: MidiDeviceId,
        channel: u8,
        number: u8,
        value: u8,
    ) -> Result<(), MidiSendError> {
        let binding = self.output_binding(device)?;
        self.seq
            .send_controller(binding.addr, channel, number, value)
            .map_err(MidiSendError::Seq)
    }

    pub fn send_sysex(&self, device: MidiDeviceId, bytes: &[u8]) -> Result<(), MidiSendError> {
        let binding = self.output_binding(device)?;
        self.seq
            .send_sysex(binding.addr, bytes)
            .map_err(MidiSendError::Seq)
    }

    fn output_binding(&self, device: MidiDeviceId) -> Result<ActiveDeviceBinding, MidiSendError> {
        let Some(binding) = self
            .devices
            .get(device.index())
            .and_then(|device| device.active)
        else {
            return Err(MidiSendError::DeviceInactive);
        };
        if !binding.output {
            return Err(MidiSendError::OutputUnavailable);
        }
        Ok(binding)
    }

    fn observe_port(
        &mut self,
        addr: Addr,
        access: PortAccess,
        matched_devices: Vec<MidiDeviceId>,
    ) -> bool {
        if let Some(port) = self.ports.get_mut(&addr) {
            if port.access == access && port.matched_devices == matched_devices {
                return false;
            }
            port.access = access;
            port.matched_devices = matched_devices;
            return true;
        }

        self.ports.insert(
            addr,
            ClientPortState {
                access,
                matched_devices,
                active_device: None,
                incoming_subscribed: false,
                outgoing_subscribed: false,
                incoming_failed: false,
                outgoing_failed: false,
            },
        );
        true
    }

    fn handle_port_start(&mut self, addr: Addr) {
        let Some((access, matched_devices)) = (|| {
            let Ok(Some(client)) = self.seq.client(addr.client) else {
                return None;
            };
            let Ok(Some(port)) = self.seq.port(addr) else {
                return None;
            };
            let Ok(port_name) = port.port_name() else {
                return None;
            };
            let client_name = client.client_name().unwrap_or("");
            Some((port.access(), self.matching_devices(port_name, client_name)))
        })() else {
            return;
        };

        if self.observe_port(addr, access, matched_devices) {
            self.reconcile_all();
        }
    }

    fn handle_port_exit(&mut self, addr: Addr) {
        if self.ports.remove(&addr).is_none() {
            return;
        }
        info!("MIDI port {}:{} exited", addr.client, addr.port);
        self.reconcile_all();
    }

    fn handle_input(&mut self, source: Addr, control: MidiControlAddress, value: MidiInputValue) {
        let Some(device_id) = self.ports.get(&source).and_then(|port| port.active_device) else {
            return;
        };
        let Some(binding) = self.devices[device_id.index()].active else {
            return;
        };
        if !binding.input {
            return;
        }
        self.pending
            .push_back(MidiClientEvent::Input(MidiInputEvent {
                device: device_id,
                control,
                value,
            }));
    }

    fn reconcile_all(&mut self) {
        for device in &mut self.devices {
            device.matching_ports.clear();
        }
        for port in self.ports.values_mut() {
            port.active_device = None;
        }

        for (addr, port) in &self.ports {
            for &device_id in &port.matched_devices {
                self.devices[device_id.index()].matching_ports.insert(*addr);
            }
        }

        for idx in 0..self.devices.len() {
            let resolution = self.resolve_binding(idx);
            let new_reported = match resolution {
                BindingResolution::Active(binding) => {
                    let device_id = self.devices[idx].spec.id;
                    self.devices[idx].active = Some(binding);
                    if let Some(port) = self.ports.get_mut(&binding.addr) {
                        port.active_device = Some(device_id);
                    }
                    DeviceAvailability::Attached {
                        input: binding.input,
                        output: binding.output,
                    }
                }
                BindingResolution::Unavailable(reason) => {
                    self.devices[idx].active = None;
                    DeviceAvailability::Unavailable(reason)
                }
            };
            self.queue_availability_transition(idx, new_reported);
        }
    }

    fn resolve_binding(&mut self, idx: usize) -> BindingResolution {
        let port_count = self.devices[idx].matching_ports.len();
        if port_count == 0 {
            return BindingResolution::Unavailable(DeviceUnavailableReason::NoMatchingPort);
        }
        if port_count > 1 {
            return BindingResolution::Unavailable(DeviceUnavailableReason::MultipleMatchingPorts);
        }

        let addr = *self.devices[idx]
            .matching_ports
            .iter()
            .next()
            .expect("one matching port exists");
        let Some(port) = self.ports.get(&addr) else {
            return BindingResolution::Unavailable(DeviceUnavailableReason::NoMatchingPort);
        };
        if port.matched_devices.len() > 1 {
            return BindingResolution::Unavailable(
                DeviceUnavailableReason::PortClaimedByMultipleDevices,
            );
        }

        self.try_activate(idx, addr)
    }

    fn try_activate(&mut self, idx: usize, addr: Addr) -> BindingResolution {
        let wants_input = self.devices[idx].spec.wants_input;
        let wants_output = self.devices[idx].spec.wants_output;
        let port = self
            .ports
            .get_mut(&addr)
            .expect("candidate port exists during reconciliation");

        let mut input = false;
        let mut output = false;

        if wants_input && port.access.can_source_events {
            if !port.incoming_subscribed && !port.incoming_failed {
                match self.seq.subscribe_incoming(addr) {
                    Ok(()) => port.incoming_subscribed = true,
                    Err(error) => {
                        warn!(
                            "Failed to subscribe to MIDI port {}:{}: {error}",
                            addr.client, addr.port,
                        );
                        port.incoming_failed = true;
                        return BindingResolution::Unavailable(
                            DeviceUnavailableReason::IncomingSubscriptionFailed,
                        );
                    }
                }
            }
            if port.incoming_subscribed {
                input = true;
            } else if port.incoming_failed {
                return BindingResolution::Unavailable(
                    DeviceUnavailableReason::IncomingSubscriptionFailed,
                );
            }
        }

        if wants_output && port.access.can_receive_events {
            if !port.outgoing_subscribed && !port.outgoing_failed {
                match self.seq.subscribe_outgoing(addr) {
                    Ok(()) => port.outgoing_subscribed = true,
                    Err(error) => {
                        warn!(
                            "Failed to subscribe send path to MIDI port {}:{}: {error}",
                            addr.client, addr.port,
                        );
                        port.outgoing_failed = true;
                    }
                }
            }
            if port.outgoing_subscribed {
                output = true;
            }
        }

        match (input, output, port.outgoing_failed) {
            (true, _, _) | (_, true, _) => {
                let direction = match (input, output) {
                    (true, true) => "input+output",
                    (true, false) => "input-only",
                    (false, true) => "output-only",
                    (false, false) => "none",
                };
                debug!(
                    "MIDI device '{}' active on port {}:{} ({direction})",
                    self.devices[idx].spec.key, addr.client, addr.port,
                );
                BindingResolution::Active(ActiveDeviceBinding {
                    addr,
                    input,
                    output,
                })
            }
            (false, false, true) => {
                BindingResolution::Unavailable(DeviceUnavailableReason::OutgoingSubscriptionFailed)
            }
            (false, false, false) => {
                BindingResolution::Unavailable(DeviceUnavailableReason::NoUsableDirection)
            }
        }
    }

    fn queue_availability_transition(&mut self, idx: usize, new_reported: DeviceAvailability) {
        let old_reported = self.devices[idx].last_reported;
        if old_reported == new_reported {
            return;
        }

        let device = self.devices[idx].spec.id;
        match (old_reported, new_reported) {
            (DeviceAvailability::Attached { .. }, DeviceAvailability::Unavailable(reason)) => {
                self.pending
                    .push_back(MidiClientEvent::DeviceDetached { device });
                self.pending
                    .push_back(MidiClientEvent::DeviceUnavailable { device, reason });
            }
            (_, DeviceAvailability::Attached { input, output }) => {
                self.pending.push_back(MidiClientEvent::DeviceAttached {
                    device,
                    input,
                    output,
                });
            }
            (_, DeviceAvailability::Unavailable(reason)) => {
                self.pending
                    .push_back(MidiClientEvent::DeviceUnavailable { device, reason });
            }
            (_, DeviceAvailability::Unknown) => {}
        }
        self.devices[idx].last_reported = new_reported;
    }

    fn matching_devices(&self, port_name: &str, client_name: &str) -> Vec<MidiDeviceId> {
        matching_devices(
            self.devices.iter().map(|device| &device.spec),
            port_name,
            client_name,
        )
    }
}

fn matching_devices<'a>(
    specs: impl IntoIterator<Item = &'a MidiDeviceSpec>,
    port_name: &str,
    client_name: &str,
) -> Vec<MidiDeviceId> {
    specs
        .into_iter()
        .filter(|spec| port_matches(spec, port_name, client_name))
        .map(|spec| spec.id)
        .collect()
}

fn port_matches(spec: &MidiDeviceSpec, port_name: &str, client_name: &str) -> bool {
    let port_ok = spec
        .port_match
        .as_ref()
        .is_none_or(|matcher| port_name.contains(matcher));
    let client_ok = spec
        .client_match
        .as_ref()
        .is_none_or(|matcher| client_name.contains(matcher));
    port_ok && client_ok
}
