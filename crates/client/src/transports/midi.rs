use crate::integration::{AnalogInProducer, AnalogOut, EndpointIo};
use alsa::seq::{
    Addr, ClientIter, EvCtrl, EvNote, Event, EventType, PortCap, PortIter, PortSubscribe, PortType,
    Seq,
};
use alsa::{Direction as AlsaDirection, PollDescriptors};
use bachelor::error::Closed;
use futures::future::{Either, select, select_all};
use futures::{Stream, StreamExt};
use rootcause::prelude::*;
use shared::shutdown::ShutdownSignal;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::ffi::CString;
use std::os::fd::{AsRawFd, RawFd};
use std::pin::{Pin, pin};
use std::task::{Context, Poll, ready};
use std::time::Duration;
use tokio::io::unix::AsyncFd;
use tracing::{debug, info, trace, warn};

/// After we issue `subscribe_port` for a MIDI device, the source client
/// (e.g. the rawmidi → seq bridge) flushes any events it had buffered
/// while we weren't listening into our input pool. Empirically this
/// happens within a few milliseconds of subscribe; we wait this long
/// and then call `snd_seq_drop_input` to discard the burst before we
/// start the real event loop. The wait is required: calling
/// `drop_input` synchronously right after `subscribe_port` is a no-op
/// because the source hasn't been scheduled yet.
const STARTUP_DRAIN_DELAY: Duration = Duration::from_millis(25);

/// SysEx sent to every subscribed MIDI port after the startup discard
/// period to ask Intech Grid modules (running our `sysexrx_cb`) to re-emit
/// the current state of their faders/potmeters as CC events. Uses the
/// MIDI educational/private manufacturer ID `0x7D`; non-Grid devices
/// should simply ignore it.
const FADER_REQUEST_SYSEX: &[u8] = &[0xF0, 0x7D, 0x01, 0xF7];

/// MIDI message type. Shared between [`crate::app`]'s config parsing
/// and the MIDI transport (which dispatches on it at runtime).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MidiMessage {
    Cc,
    Note,
}

impl MidiMessage {
    pub fn as_str(self) -> &'static str {
        match self {
            MidiMessage::Cc => "cc",
            MidiMessage::Note => "note",
        }
    }
}

/// One MIDI control after binding: addressing info plus its bound
/// endpoint half(s). Built by `app` from the user's config + the
/// [`crate::integration::Binder`]'s output.
pub struct MidiControlDefinition {
    /// Display label; used only for trace/info/warn output.
    pub label: &'static str,
    pub message: MidiMessage,
    /// 0..=15, matching `aseqdump`.
    pub channel: u8,
    /// CC number (when `message = "cc"`) or note number (when
    /// `message = "note"`); 0..=127.
    pub number: u8,
    pub endpoint: EndpointIo,
}

/// Resolved MIDI device configuration owned by the MIDI transport.
/// Built by `app` from the user's config + the bound endpoints from the
/// [`crate::integration::Binder`]; the transport itself is oblivious to
/// the catalog and binder.
pub struct MidiDeviceConfig {
    pub device_key: String,
    pub port_match: Option<String>,
    pub client_match: Option<String>,
    pub controls: Vec<MidiControlDefinition>,
}

/// Holds the alsa seq client together with the cached poll fd. Implementing
/// `AsRawFd` lets us hand the whole thing to [`AsyncFd`] just like
/// `AsyncMonitorSocket` wraps `udev::MonitorSocket`.
struct SeqHolder {
    seq: Seq,
    fd: RawFd,
}

impl AsRawFd for SeqHolder {
    fn as_raw_fd(&self) -> RawFd {
        self.fd
    }
}

/// Async wrapper around an `alsa::seq::Seq` opened in non-blocking capture
/// mode. Each readiness wake yields one decoded event; subsequent
/// `poll_next` calls drain the buffer before re-arming the fd.
pub struct AsyncSeq {
    fd: AsyncFd<SeqHolder>,
}

impl AsyncSeq {
    pub fn new(seq: Seq) -> Result<Self, Report> {
        let pds = (&seq, Some(AlsaDirection::Capture));
        let count = pds.count();
        if count != 1 {
            bail!("alsa seq returned {count} poll fds; expected exactly 1");
        }
        let mut pollfds = [libc::pollfd {
            fd: 0,
            events: 0,
            revents: 0,
        }];
        pds.fill(&mut pollfds)
            .context("Failed to fill alsa seq poll descriptor")?;
        let holder = SeqHolder {
            seq,
            fd: pollfds[0].fd,
        };
        Ok(Self {
            fd: AsyncFd::new(holder).context("Failed to register alsa seq fd")?,
        })
    }

    pub fn seq(&self) -> &Seq {
        &self.fd.get_ref().seq
    }
}

impl Stream for AsyncSeq {
    type Item = Result<Event<'static>, alsa::Error>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        loop {
            let mut guard = ready!(self.fd.poll_read_ready(cx).map_err(|error| {
                alsa::Error::new(
                    "AsyncSeq::poll_read_ready",
                    error.raw_os_error().unwrap_or(libc::EIO),
                )
            }))?;

            let mut input = guard.get_inner().seq.input();
            match input.event_input() {
                Ok(ev) => return Poll::Ready(Some(Ok(ev.into_owned()))),
                Err(e) if e.errno() == libc::EAGAIN => {
                    drop(input);
                    guard.clear_ready();
                }
                Err(e) => return Poll::Ready(Some(Err(e))),
            }
        }
    }
}

/// One configured device after binding: matching info + the per-message,
/// channel, and number dispatch table for incoming controls and the set of currently
/// subscribed-and-sendable port addresses for outgoing CCs.
struct DeviceState {
    device_key: String,
    port_match: Option<String>,
    client_match: Option<String>,
    /// (message, channel, number) → AnalogInProducer. The label is kept
    /// for trace context only. CC values are written directly; note-on
    /// increments the value by one.
    inputs: HashMap<(MidiMessage, u8, u8), (&'static str, AnalogInProducer)>,
    sendable_ports: HashSet<Addr>,
}

/// One AnalogOut bound to this transport, plus the per-control addressing
/// the transport needs to emit it.
struct OutEntry {
    device_idx: usize,
    label: &'static str,
    channel: u8,
    number: u8,
    consumer: AnalogOut,
    last_sent_cc: Option<u8>,
    /// `false` once the watch has closed; the per-iteration `select_all`
    /// skips closed entries.
    open: bool,
}

/// Run the MIDI transport task until `shutdown` fires. Spawned as a
/// single task owning one alsa seq client and all per-port subscriptions.
pub async fn run(
    devices_cfg: Vec<MidiDeviceConfig>,
    mut shutdown: ShutdownSignal,
) -> Result<(), Report> {
    if devices_cfg.iter().all(|d| d.controls.is_empty()) {
        // Nothing to do; just hold the future open until shutdown so the
        // join shape is consistent with the rest of the runtime.
        shutdown.wait().await;
        return Ok(());
    }

    // Build per-device state directly from the resolved controls list.
    // Every `direction = "out"` watch was already seeded with a
    // meaningful initial value by `Binder::analog_out`, so the
    // transport's first publish below is guaranteed to send the
    // persisted utilization state to freshly subscribed devices.
    let mut devices: Vec<DeviceState> = Vec::with_capacity(devices_cfg.len());
    let mut outs: Vec<OutEntry> = Vec::new();
    for device_cfg in devices_cfg {
        let device_idx = devices.len();
        let mut inputs: HashMap<(MidiMessage, u8, u8), (&'static str, AnalogInProducer)> =
            HashMap::new();
        for ctrl in device_cfg.controls {
            let (input, output) = ctrl.endpoint.split();
            if let Some(producer) = input {
                let key = (ctrl.message, ctrl.channel, ctrl.number);
                if let Some((prev_label, _)) = inputs.insert(key, (ctrl.label, producer)) {
                    warn!(
                        "MIDI device '{}': two `in` {} controls share (channel={}, number={}); '{}' overwritten by '{}'",
                        device_cfg.device_key,
                        ctrl.message.as_str(),
                        ctrl.channel,
                        ctrl.number,
                        prev_label,
                        ctrl.label,
                    );
                }
            }
            match ctrl.message {
                MidiMessage::Cc => {
                    if let Some(consumer) = output {
                        outs.push(OutEntry {
                            device_idx,
                            label: ctrl.label,
                            channel: ctrl.channel,
                            number: ctrl.number,
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
                            ctrl.label,
                        );
                    }
                }
            }
        }
        devices.push(DeviceState {
            device_key: device_cfg.device_key,
            port_match: device_cfg.port_match,
            client_match: device_cfg.client_match,
            inputs,
            sendable_ports: HashSet::new(),
        });
    }

    // `None` for direction opens the seq client in duplex mode so we can
    // both receive controller events and send the Grid fader-state SysEx.
    let seq = Seq::open(None, None, true).context("Failed to open alsa sequencer client")?;
    let client_name =
        CString::new("openergo").expect("static client name contains no interior nul");
    seq.set_client_name(&client_name)
        .context("Failed to set alsa client name")?;

    let port_name = CString::new("controls").expect("static port name contains no interior nul");
    let local_port = seq
        .create_simple_port(
            &port_name,
            PortCap::WRITE | PortCap::SUBS_WRITE | PortCap::READ | PortCap::SUBS_READ,
            PortType::APPLICATION | PortType::MIDI_GENERIC,
        )
        .context("Failed to create local seq port")?;
    let our_addr = Addr {
        client: seq.client_id().context("Failed to query alsa client id")?,
        port: local_port,
    };

    // Subscribe to system announce so we receive PortStart/PortExit hot-plug events.
    {
        let sub = PortSubscribe::empty().context("Failed to allocate port subscribe")?;
        sub.set_sender(Addr::system_announce());
        sub.set_dest(our_addr);
        seq.subscribe_port(&sub)
            .context("Failed to subscribe to system announce port")?;
    }

    // Maps each subscribed port to the device indices it matches.
    let mut active_ports: HashMap<Addr, Vec<usize>> = HashMap::new();

    // Initial enumerate.
    for client in ClientIter::new(&seq) {
        let client_id = client.get_client();
        if client_id == our_addr.client {
            continue;
        }
        let client_name = client.get_name().unwrap_or("").to_string();
        for port in PortIter::new(&seq, client_id) {
            let addr = port.addr();
            let Ok(port_name) = port.get_name() else {
                continue;
            };
            try_subscribe(
                &seq,
                our_addr,
                addr,
                port_name,
                &client_name,
                &mut devices,
                &mut active_ports,
            );
        }
    }

    // Wait briefly for the source clients to flush their buffered events
    // into our kernel input pool, then drop everything that's there. This
    // also discards the few `PortSubscribed` announce events generated by
    // the subscriptions above; that's fine because we only use the
    // `system_announce` subscription for future hot-plug events.
    tokio::time::sleep(STARTUP_DRAIN_DELAY).await;
    seq.input()
        .drop_input()
        .context("Failed to drop buffered MIDI startup events")?;

    let mut async_seq = AsyncSeq::new(seq).context("Failed to attach alsa seq to async runtime")?;

    request_fader_state_from_subscribed_ports_all(async_seq.seq(), local_port, &devices);

    // Initial AnalogOut publish: every watch is seeded with the persisted
    // value, so devices come up showing the correct state without waiting
    // for the first domain-side update.
    for out in &mut outs {
        if !out.open {
            continue;
        }
        publish_out(async_seq.seq(), local_port, &devices, out, true);
    }

    loop {
        enum Step {
            Event(Event<'static>),
            OutChanged(usize, Result<(), Closed>),
            Shutdown,
        }

        let step = {
            let next_event = async_seq.next();
            // Build per-iteration `changed()` futures only for still-open
            // outs. If every out has closed, fall back to a pending future
            // so the select waits for ALSA events / shutdown only.
            let (active_indices, waits): (Vec<usize>, Vec<_>) = outs
                .iter_mut()
                .enumerate()
                .filter(|(_, o)| o.open)
                .map(|(i, o)| (i, Box::pin(o.consumer.changed())))
                .unzip();
            type AnyOut<'a> = Pin<Box<dyn Future<Output = (Result<(), Closed>, usize)> + 'a>>;
            let any_out: AnyOut<'_> = if waits.is_empty() {
                Box::pin(async {
                    std::future::pending::<()>().await;
                    unreachable!()
                })
            } else {
                Box::pin(async move {
                    let (res, fired_idx, _rem) = select_all(waits).await;
                    (res, active_indices[fired_idx])
                })
            };
            let wait_shutdown = shutdown.wait();
            match select(select(pin!(next_event), any_out), pin!(wait_shutdown)).await {
                Either::Left((Either::Left((Some(Ok(ev)), _)), _)) => Step::Event(ev),
                Either::Left((Either::Left((Some(Err(e)), _)), _)) => {
                    return Err(report!(e)
                        .context("alsa sequencer event stream error")
                        .into_dynamic());
                }
                Either::Left((Either::Left((None, _)), _)) => {
                    bail!("alsa sequencer event stream ended unexpectedly")
                }
                Either::Left((Either::Right(((res, idx), _)), _)) => Step::OutChanged(idx, res),
                Either::Right(_) => Step::Shutdown,
            }
        };

        match step {
            Step::Shutdown => return Ok(()),
            Step::OutChanged(idx, Ok(())) => {
                publish_out(async_seq.seq(), local_port, &devices, &mut outs[idx], false);
            }
            Step::OutChanged(idx, Err(Closed)) => {
                let label = outs[idx].label;
                outs[idx].open = false;
                debug!(
                    "AnalogOut watch for '{}' closed; ignoring further updates",
                    label,
                );
            }
            Step::Event(event) => {
                let mut newly_subscribed_devices: HashSet<usize> = HashSet::new();
                handle_event(
                    event,
                    async_seq.seq(),
                    our_addr,
                    &mut devices,
                    &mut active_ports,
                    &mut newly_subscribed_devices,
                );

                // If a hot-plug attached a new sendable port to one or
                // more devices, re-send the SysEx fader request and
                // republish every AnalogOut for those devices so the
                // freshly attached device starts with the correct state.
                if !newly_subscribed_devices.is_empty() {
                    for &device_idx in &newly_subscribed_devices {
                        let device = &devices[device_idx];
                        request_fader_state_from_subscribed_ports(
                            async_seq.seq(),
                            local_port,
                            &device.sendable_ports,
                        );
                    }
                    for out in &mut outs {
                        if !out.open || !newly_subscribed_devices.contains(&out.device_idx) {
                            continue;
                        }
                        publish_out(async_seq.seq(), local_port, &devices, out, true);
                    }
                }
            }
        }
    }
}

fn publish_out(seq: &Seq, our_port: i32, devices: &[DeviceState], out: &mut OutEntry, force: bool) {
    let cc_value = ratio_to_cc(out.consumer.get());
    if !force && out.last_sent_cc == Some(cc_value) {
        return;
    }

    let device = &devices[out.device_idx];
    trace!(
        "AnalogOut '{}' -> CC {} ch={} value={} (device '{}')",
        out.label, out.number, out.channel, cc_value, device.device_key,
    );
    send_cc(
        seq,
        our_port,
        &device.sendable_ports,
        out.channel,
        out.number,
        cc_value,
    );
    out.last_sent_cc = Some(cc_value);
}

fn handle_event(
    event: Event<'static>,
    seq: &Seq,
    our_addr: Addr,
    devices: &mut [DeviceState],
    active_ports: &mut HashMap<Addr, Vec<usize>>,
    newly_subscribed_devices: &mut HashSet<usize>,
) {
    match event.get_type() {
        EventType::Controller => {
            let Some(ctrl) = event.get_data::<EvCtrl>() else {
                return;
            };
            let source = event.get_source();
            let Some(device_indices) = active_ports.get(&source) else {
                return;
            };
            let value = (ctrl.value.clamp(0, 127) as f64) / 127.0;
            let Ok(number) = u8::try_from(ctrl.param) else {
                return;
            };
            for &device_idx in device_indices {
                let device = &devices[device_idx];
                if let Some((label, producer)) =
                    device.inputs.get(&(MidiMessage::Cc, ctrl.channel, number))
                {
                    trace!(
                        "MIDI controller {} ch={} on port {}:{} -> control '{}' = {} (device '{}')",
                        ctrl.param,
                        ctrl.channel,
                        source.client,
                        source.port,
                        label,
                        value,
                        device.device_key,
                    );
                    let _ = producer.set(value);
                }
            }
        }
        EventType::Noteon => {
            let Some(note) = event.get_data::<EvNote>() else {
                return;
            };
            if note.velocity == 0 {
                return;
            }
            let source = event.get_source();
            let Some(device_indices) = active_ports.get(&source) else {
                return;
            };
            for &device_idx in device_indices {
                let device = &devices[device_idx];
                if let Some((label, producer)) =
                    device
                        .inputs
                        .get(&(MidiMessage::Note, note.channel, note.note))
                {
                    trace!(
                        "MIDI note-on {} ch={} vel={} on port {}:{} -> control '{}' += 1 (device '{}')",
                        note.note,
                        note.channel,
                        note.velocity,
                        source.client,
                        source.port,
                        label,
                        device.device_key,
                    );
                    let _ = producer.update(|value| *value += 1.0);
                }
            }
        }
        EventType::PortStart => {
            let Some(addr) = event.get_data::<Addr>() else {
                return;
            };
            if addr.client == our_addr.client {
                return;
            }
            let Ok(port_info) = seq.get_any_port_info(addr) else {
                return;
            };
            let Ok(port_name) = port_info.get_name() else {
                return;
            };
            let client_name = seq
                .get_any_client_info(addr.client)
                .ok()
                .and_then(|c| c.get_name().ok().map(str::to_string))
                .unwrap_or_default();
            let sendable_before: Vec<HashSet<Addr>> =
                devices.iter().map(|d| d.sendable_ports.clone()).collect();
            try_subscribe(
                seq,
                our_addr,
                addr,
                port_name,
                &client_name,
                devices,
                active_ports,
            );
            for (idx, device) in devices.iter().enumerate() {
                if device.sendable_ports.len() > sendable_before[idx].len() {
                    newly_subscribed_devices.insert(idx);
                }
            }
        }
        EventType::PortExit => {
            let Some(addr) = event.get_data::<Addr>() else {
                return;
            };
            for device in devices.iter_mut() {
                device.sendable_ports.remove(&addr);
            }
            if active_ports.remove(&addr).is_some() {
                info!("MIDI port {}:{} exited", addr.client, addr.port);
            }
        }
        _ => {}
    }
}

/// Send the fader-state SysEx to every sendable port across every device.
/// Used at startup; for hot-plug, the per-device variant below is used.
fn request_fader_state_from_subscribed_ports_all(
    seq: &Seq,
    our_port: i32,
    devices: &[DeviceState],
) {
    for device in devices {
        request_fader_state_from_subscribed_ports(seq, our_port, &device.sendable_ports);
    }
}

/// Send the Grid fader-state request SysEx to every port we subscribed to
/// after the startup discard window. Grid modules running our `sysexrx_cb`
/// will respond by re-emitting the current value of each fader as a CC
/// event, which is then picked up by the normal `handle_event` path so
/// AnalogIn state reflects the controller's true position before any user
/// movement.
fn request_fader_state_from_subscribed_ports(
    seq: &Seq,
    our_port: i32,
    sendable_ports: &HashSet<Addr>,
) {
    for &dest in sendable_ports {
        let mut event = Event::new_ext(EventType::Sysex, Cow::Borrowed(FADER_REQUEST_SYSEX));
        event.set_source(our_port);
        event.set_dest(dest);
        event.set_direct();
        match seq.event_output_direct(&mut event) {
            Ok(_) => debug!(
                "Sent fader-state request SysEx to MIDI port {}:{}",
                dest.client, dest.port,
            ),
            Err(e) => warn!(
                "Failed to send fader-state request SysEx to MIDI port {}:{}: {e}",
                dest.client, dest.port,
            ),
        }
    }
}

/// Convert a 0..=1.0 utilization ratio to a CC byte. Clamps to 0..=127;
/// negatives shouldn't occur but are defended against.
fn ratio_to_cc(ratio: f64) -> u8 {
    let scaled = (ratio * 100.0).floor() as i64;
    scaled.clamp(0, 127) as u8
}

fn send_cc(
    seq: &Seq,
    our_port: i32,
    sendable_ports: &HashSet<Addr>,
    channel: u8,
    cc: u8,
    value: u8,
) {
    for &dest in sendable_ports {
        let ctrl = EvCtrl {
            channel,
            param: cc as u32,
            value: value as i32,
        };
        let mut event = Event::new(EventType::Controller, &ctrl);
        event.set_source(our_port);
        event.set_dest(dest);
        event.set_direct();
        match seq.event_output_direct(&mut event) {
            Ok(_) => trace!(
                "Sent CC {cc}={value} ch={channel} to MIDI port {}:{}",
                dest.client, dest.port,
            ),
            Err(e) => warn!(
                "Failed to send CC {cc} to MIDI port {}:{}: {e}",
                dest.client, dest.port,
            ),
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn try_subscribe(
    seq: &Seq,
    our_addr: Addr,
    source: Addr,
    port_name: &str,
    client_name: &str,
    devices: &mut [DeviceState],
    active_ports: &mut HashMap<Addr, Vec<usize>>,
) {
    if active_ports.contains_key(&source) {
        return;
    }
    let matched: Vec<usize> = devices
        .iter()
        .enumerate()
        .filter(|(_, d)| port_matches(d, port_name, client_name))
        .map(|(i, _)| i)
        .collect();
    if matched.is_empty() {
        return;
    }
    let sub = match PortSubscribe::empty() {
        Ok(s) => s,
        Err(e) => {
            warn!("Failed to allocate port subscribe: {e}");
            return;
        }
    };
    sub.set_sender(source);
    sub.set_dest(our_addr);
    if let Err(e) = seq.subscribe_port(&sub) {
        warn!(
            "Failed to subscribe to MIDI port {}:{} '{port_name}': {e}",
            source.client, source.port,
        );
        return;
    }

    // If the device port also accepts writes (i.e. has both WRITE and
    // SUBS_WRITE capability), set up the reverse subscription so we can
    // send the Grid fader-state request SysEx and outbound CCs without
    // ENODEV.
    let mut sendable = false;
    if let Ok(info) = seq.get_any_port_info(source) {
        let caps = info.get_capability();
        if caps.contains(PortCap::WRITE | PortCap::SUBS_WRITE) {
            match PortSubscribe::empty() {
                Ok(send_sub) => {
                    send_sub.set_sender(our_addr);
                    send_sub.set_dest(source);
                    match seq.subscribe_port(&send_sub) {
                        Ok(()) => {
                            sendable = true;
                        }
                        Err(e) => warn!(
                            "Failed to subscribe send path to MIDI port {}:{} '{port_name}': {e}",
                            source.client, source.port,
                        ),
                    }
                }
                Err(e) => {
                    warn!("Failed to allocate send port subscribe: {e}");
                }
            }
        }
    }
    for &device_idx in &matched {
        let device = &mut devices[device_idx];
        if sendable {
            device.sendable_ports.insert(source);
        }
        let cc_count = device
            .inputs
            .keys()
            .filter(|(message, _, _)| *message == MidiMessage::Cc)
            .count();
        let note_count = device
            .inputs
            .keys()
            .filter(|(message, _, _)| *message == MidiMessage::Note)
            .count();
        let in_labels: Vec<&str> = device.inputs.values().map(|(label, _)| *label).collect();
        info!(
            "Subscribed MIDI port {}:{} '{port_name}' (client '{client_name}') for device '{}' ({cc_count} CC in, {note_count} note in: {})",
            source.client,
            source.port,
            device.device_key,
            in_labels.join(", "),
        );
    }
    active_ports.insert(source, matched);
}

fn port_matches(device: &DeviceState, port_name: &str, client_name: &str) -> bool {
    let port_ok = device
        .port_match
        .as_ref()
        .is_none_or(|m| port_name.contains(m));
    let client_ok = device
        .client_match
        .as_ref()
        .is_none_or(|m| client_name.contains(m));
    port_ok && client_ok
}
