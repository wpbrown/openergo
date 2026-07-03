use super::client::{
    DeviceUnavailableReason, MidiClient, MidiClientConfig, MidiClientEvent, MidiControlAddress,
    MidiDeviceId, MidiDeviceSpec, MidiInputValue, MidiSendError,
};
use super::seq_io::SeqIo;
use super::{FADER_REQUEST_SYSEX, MidiDeviceConfig, MidiMessage, STARTUP_DRAIN_DELAY};
use crate::integration::{AnalogInProducer, AnalogOut};
use bachelor::error::Closed;
use futures::FutureExt;
use futures::future::{Either, select};
use rootcause::prelude::*;
use shared::select_small::select_small_once;
use shared::shutdown::ShutdownSignal;
use std::collections::HashMap;
use std::pin::pin;
use tracing::{debug, info, trace, warn};

pub struct PreparedMidiDriver {
    client_config: MidiClientConfig,
    devices: Vec<DriverDevice>,
    outs: Vec<DriverOut>,
}

pub struct MidiDriver<S: SeqIo> {
    client: MidiClient<S>,
    devices: Vec<DriverDevice>,
    outs: Vec<DriverOut>,
}

struct DriverDevice {
    key: String,
    inputs: HashMap<MidiControlAddress, (&'static str, AnalogInProducer)>,
    request_fader_state: bool,
    active: DriverDeviceIoState,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct DriverDeviceIoState {
    input: bool,
    output: bool,
}

impl DriverDeviceIoState {
    fn inactive() -> Self {
        Self::default()
    }
}

struct DriverOut {
    device: MidiDeviceId,
    label: &'static str,
    channel: u8,
    number: u8,
    consumer: AnalogOut,
    last_sent_cc: Option<u8>,
    open: bool,
}

enum DriverStep {
    Midi(MidiClientEvent),
    OutChanged(usize, Result<(), Closed>),
    Shutdown,
}

pub fn prepare_driver(devices_cfg: Vec<MidiDeviceConfig>) -> Option<PreparedMidiDriver> {
    if devices_cfg.iter().all(|device| device.controls.is_empty()) {
        return None;
    }

    let mut device_specs = Vec::with_capacity(devices_cfg.len());
    let mut devices = Vec::with_capacity(devices_cfg.len());
    let mut outs = Vec::new();

    for (index, device_cfg) in devices_cfg.into_iter().enumerate() {
        let id = MidiDeviceId::from_index(index);
        let mut inputs = HashMap::new();
        let has_bound_control = !device_cfg.controls.is_empty();

        for control in device_cfg.controls {
            let (input, output) = control.endpoint.split();
            let address = match control.message {
                MidiMessage::Cc => MidiControlAddress::Cc {
                    channel: control.channel,
                    number: control.number,
                },
                MidiMessage::Note => MidiControlAddress::Note {
                    channel: control.channel,
                    number: control.number,
                },
            };

            if let Some(producer) = input
                && let Some((prev_label, _)) = inputs.insert(address, (control.label, producer))
            {
                warn!(
                    "MIDI device '{}': two `in` {} controls share (channel={}, number={}); '{}' overwritten by '{}'",
                    device_cfg.device_key,
                    control.message.as_str(),
                    control.channel,
                    control.number,
                    prev_label,
                    control.label,
                );
            }

            match control.message {
                MidiMessage::Cc => {
                    if let Some(consumer) = output {
                        outs.push(DriverOut {
                            device: id,
                            label: control.label,
                            channel: control.channel,
                            number: control.number,
                            consumer,
                            last_sent_cc: None,
                            open: true,
                        });
                    }
                }
                MidiMessage::Note => {
                    if output.is_some() {
                        warn!(
                            "MIDI control '{}' has message=note with an output endpoint; note outputs are not supported (skipping output)",
                            control.label,
                        );
                    }
                }
            }
        }

        let wants_input = !inputs.is_empty();
        device_specs.push(MidiDeviceSpec {
            id,
            key: device_cfg.device_key.clone(),
            port_match: device_cfg.port_match,
            client_match: device_cfg.client_match,
            wants_input,
            wants_output: has_bound_control,
        });
        devices.push(DriverDevice {
            key: device_cfg.device_key,
            inputs,
            request_fader_state: has_bound_control,
            active: DriverDeviceIoState::inactive(),
        });
    }

    Some(PreparedMidiDriver {
        client_config: MidiClientConfig {
            devices: device_specs,
            startup_drain_delay: STARTUP_DRAIN_DELAY,
        },
        devices,
        outs,
    })
}

#[cfg(test)]
pub async fn run_with_seq<S: SeqIo>(
    seq: S,
    devices_cfg: Vec<MidiDeviceConfig>,
    mut shutdown: ShutdownSignal,
) -> Result<(), Report> {
    let Some(prepared) = prepare_driver(devices_cfg) else {
        shutdown.wait().await;
        return Ok(());
    };
    run_prepared_with_seq(seq, prepared, shutdown).await
}

pub async fn run_prepared_with_seq<S: SeqIo>(
    seq: S,
    prepared: PreparedMidiDriver,
    shutdown: ShutdownSignal,
) -> Result<(), Report> {
    MidiDriver::start(seq, prepared).await?.run(shutdown).await
}

impl<S: SeqIo> MidiDriver<S> {
    async fn start(seq: S, prepared: PreparedMidiDriver) -> Result<Self, Report> {
        let client = MidiClient::start(seq, prepared.client_config).await?;
        Ok(Self {
            client,
            devices: prepared.devices,
            outs: prepared.outs,
        })
    }

    async fn run(mut self, mut shutdown: ShutdownSignal) -> Result<(), Report> {
        loop {
            match self.next_step(&mut shutdown).await? {
                DriverStep::Midi(event) => self.handle_midi_event(event),
                DriverStep::OutChanged(idx, Ok(())) => self.publish_out(idx, false),
                DriverStep::OutChanged(idx, Err(Closed)) => self.close_out(idx),
                DriverStep::Shutdown => return Ok(()),
            }
        }
    }

    async fn next_step(&mut self, shutdown: &mut ShutdownSignal) -> Result<DriverStep, Report> {
        let midi = pin!(self.client.next_event());
        let any_out = if self.outs.iter().any(|out| out.open) {
            Either::Left(select_small_once::<_, 8>(
                self.outs
                    .iter_mut()
                    .enumerate()
                    .filter(|(_, out)| out.open)
                    .map(|(idx, out)| out.consumer.changed().map(move |res| (res, idx))),
            ))
        } else {
            Either::Right(std::future::pending())
        };
        let wait_shutdown = shutdown.wait();

        match select(select(midi, any_out), wait_shutdown).await {
            Either::Left((Either::Left((Ok(event), _)), _)) => Ok(DriverStep::Midi(event)),
            Either::Left((Either::Left((Err(error), _)), _)) => Err(error),
            Either::Left((Either::Right((((res, idx), _), _)), _)) => {
                Ok(DriverStep::OutChanged(idx, res))
            }
            Either::Right(_) => Ok(DriverStep::Shutdown),
        }
    }

    fn handle_midi_event(&mut self, event: MidiClientEvent) {
        match event {
            MidiClientEvent::DeviceAttached {
                device,
                input,
                output,
            } => self.attach_device(device, input, output),
            MidiClientEvent::DeviceDetached { device } => self.detach_device(device),
            MidiClientEvent::DeviceUnavailable { device, reason } => {
                self.mark_device_unavailable(device, reason);
            }
            MidiClientEvent::Input(event) => {
                self.handle_input(event.device, event.control, event.value)
            }
        }
    }

    fn attach_device(&mut self, device: MidiDeviceId, input: bool, output: bool) {
        let driver_device = &mut self.devices[device.index()];
        driver_device.active = DriverDeviceIoState { input, output };
        info!(
            "MIDI device '{}' attached (input={}, output={})",
            driver_device.key, input, output,
        );

        if output && driver_device.request_fader_state {
            match self.client.send_sysex(device, FADER_REQUEST_SYSEX) {
                Ok(()) => debug!(
                    "Sent fader-state request SysEx to MIDI device '{}'",
                    driver_device.key,
                ),
                Err(error) => warn_midi_send_error(
                    error,
                    "Failed to send fader-state request SysEx",
                    &driver_device.key,
                ),
            }
        }

        if output {
            for idx in 0..self.outs.len() {
                if self.outs[idx].open && self.outs[idx].device == device {
                    self.publish_out(idx, true);
                }
            }
        }
    }

    fn detach_device(&mut self, device: MidiDeviceId) {
        let driver_device = &mut self.devices[device.index()];
        driver_device.active = DriverDeviceIoState::inactive();
        info!("MIDI device '{}' detached", driver_device.key);
    }

    fn mark_device_unavailable(&mut self, device: MidiDeviceId, reason: DeviceUnavailableReason) {
        let driver_device = &mut self.devices[device.index()];
        driver_device.active = DriverDeviceIoState::inactive();
        debug!(
            "MIDI device '{}' unavailable: {:?}",
            driver_device.key, reason,
        );
    }

    fn handle_input(
        &mut self,
        device: MidiDeviceId,
        control: MidiControlAddress,
        value: MidiInputValue,
    ) {
        let driver_device = &self.devices[device.index()];
        if !driver_device.active.input {
            return;
        }

        match value {
            MidiInputValue::Controller { value } => {
                let Some((label, producer)) = driver_device.inputs.get(&control) else {
                    return;
                };
                let value = (value.clamp(0, 127) as f64) / 127.0;
                trace!(
                    "MIDI controller input for '{}' -> control '{}' = {}",
                    driver_device.key, label, value,
                );
                let _ = producer.set(value);
            }
            MidiInputValue::NoteOn { velocity } => {
                let Some((label, producer)) = driver_device.inputs.get(&control) else {
                    return;
                };
                trace!(
                    "MIDI note-on input for '{}' vel={} -> control '{}' += 1",
                    driver_device.key, velocity, label,
                );
                let _ = producer.update(|value| *value += 1.0);
            }
            MidiInputValue::NoteOff { .. } => {}
        }
    }

    fn publish_out(&mut self, idx: usize, force: bool) {
        let cc_value = ratio_to_cc(self.outs[idx].consumer.get());
        if !force && self.outs[idx].last_sent_cc == Some(cc_value) {
            return;
        }

        let device = self.outs[idx].device;
        let driver_device = &self.devices[device.index()];
        if !driver_device.active.output {
            return;
        }

        trace!(
            "AnalogOut '{}' -> CC {} ch={} value={} (device '{}')",
            self.outs[idx].label,
            self.outs[idx].number,
            self.outs[idx].channel,
            cc_value,
            driver_device.key,
        );
        match self.client.send_cc(
            device,
            self.outs[idx].channel,
            self.outs[idx].number,
            cc_value,
        ) {
            Ok(()) => self.outs[idx].last_sent_cc = Some(cc_value),
            Err(MidiSendError::Seq(error)) => {
                warn!(
                    "Failed to send CC {} for MIDI device '{}': {error}",
                    self.outs[idx].number, driver_device.key,
                );
                self.outs[idx].last_sent_cc = Some(cc_value);
            }
            Err(error) => warn_midi_send_error(error, "Failed to send CC", &driver_device.key),
        }
    }

    fn close_out(&mut self, idx: usize) {
        let label = self.outs[idx].label;
        self.outs[idx].open = false;
        debug!(
            "AnalogOut watch for '{}' closed; ignoring further updates",
            label,
        );
    }
}

fn ratio_to_cc(ratio: f64) -> u8 {
    let scaled = (ratio * 100.0).floor() as i64;
    scaled.clamp(0, 127) as u8
}

fn warn_midi_send_error(error: MidiSendError, action: &str, device_key: &str) {
    match error {
        MidiSendError::DeviceInactive => {
            warn!("{action} for MIDI device '{device_key}': device inactive")
        }
        MidiSendError::OutputUnavailable => {
            warn!("{action} for MIDI device '{device_key}': output unavailable")
        }
        MidiSendError::Seq(error) => warn!("{action} for MIDI device '{device_key}': {error}"),
    }
}
