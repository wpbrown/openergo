use crate::integration::EndpointIo;
use rootcause::prelude::*;
use shared::shutdown::ShutdownSignal;
use std::ffi::CString;
use std::time::Duration;

mod client;
mod driver;
mod seq_io;

#[cfg(test)]
mod tests;

use seq_io::AlsaSeqIo;

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

/// Runtime MIDI message type used by the transport driver.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

/// Run the MIDI transport task until `shutdown` fires. Spawned as a
/// single task owning one alsa seq client and all per-port subscriptions.
pub async fn run(
    devices_cfg: Vec<MidiDeviceConfig>,
    mut shutdown: ShutdownSignal,
) -> Result<(), Report> {
    let Some(prepared) = driver::prepare_driver(devices_cfg) else {
        // Nothing to do; just hold the future open until shutdown so the
        // join shape is consistent with the rest of the runtime.
        shutdown.wait().await;
        return Ok(());
    };

    let client_name =
        CString::new("openergo").expect("static client name contains no interior nul");
    let port_name = CString::new("controls").expect("static port name contains no interior nul");
    let seq = AlsaSeqIo::open(&client_name, &port_name)
        .context("Failed to open alsa sequencer client")?;

    driver::run_prepared_with_seq(seq, prepared, shutdown).await
}
