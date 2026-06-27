use alsa::seq::{
    ClientInfo, ClientIter, EvCtrl, EvNote, Event, EventType, PortCap, PortInfo, PortIter,
    PortSubscribe, PortType, Seq,
};
use alsa::{Direction as AlsaDirection, PollDescriptors};
use std::borrow::Cow;
use std::ffi::CStr;
use std::os::fd::{AsRawFd, RawFd};
use tokio::io::unix::AsyncFd;
use tracing::debug;

pub(super) use alsa::seq::Addr;

pub(super) type SeqIoResult<T> = alsa::Result<T>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PortAccess {
    pub can_source_events: bool,
    pub can_receive_events: bool,
}

pub(super) trait SeqClientInfo {
    type Port<'a>: SeqPortInfo + 'a
    where
        Self: 'a;

    type Ports<'a>: Iterator<Item = Self::Port<'a>> + 'a
    where
        Self: 'a;

    fn client_id(&self) -> i32;
    fn client_name(&self) -> SeqIoResult<&str>;
    fn ports(&self) -> Self::Ports<'_>;
}

pub(super) trait SeqPortInfo {
    fn addr(&self) -> Addr;
    fn port_name(&self) -> SeqIoResult<&str>;
    fn access(&self) -> PortAccess;
}

#[derive(Clone, Debug, PartialEq)]
pub(super) enum InputEvent {
    Controller {
        source: Addr,
        channel: u8,
        controller: u8,
        value: i32,
    },
    NoteOn {
        source: Addr,
        channel: u8,
        note: u8,
        velocity: u8,
    },
    NoteOff {
        source: Addr,
        channel: u8,
        note: u8,
        velocity: u8,
    },
    PortStart {
        addr: Addr,
    },
    PortExit {
        addr: Addr,
    },
}

pub(super) trait SeqIo {
    type Client<'a>: SeqClientInfo + 'a
    where
        Self: 'a;

    type Clients<'a>: Iterator<Item = Self::Client<'a>> + 'a
    where
        Self: 'a;

    type Port<'a>: SeqPortInfo + 'a
    where
        Self: 'a;

    fn clients(&self) -> Self::Clients<'_>;
    fn client(&self, client_id: i32) -> SeqIoResult<Option<Self::Client<'_>>>;
    fn port(&self, addr: Addr) -> SeqIoResult<Option<Self::Port<'_>>>;
    fn subscribe_incoming(&self, remote: Addr) -> SeqIoResult<()>;
    fn subscribe_outgoing(&self, remote: Addr) -> SeqIoResult<()>;
    fn drop_input(&self) -> SeqIoResult<()>;
    async fn next_event(&mut self) -> SeqIoResult<InputEvent>;
    fn send_controller(
        &self,
        dest: Addr,
        channel: u8,
        controller: u8,
        value: u8,
    ) -> SeqIoResult<()>;
    fn send_sysex(&self, dest: Addr, bytes: &[u8]) -> SeqIoResult<()>;
}

pub(super) struct AlsaSeqIo {
    fd: AsyncFd<SeqHolder>,
    local: Addr,
}

struct SeqHolder {
    seq: Seq,
    fd: RawFd,
}

impl AsRawFd for SeqHolder {
    fn as_raw_fd(&self) -> RawFd {
        self.fd
    }
}

pub(super) struct AlsaClientInfo<'seq> {
    seq: &'seq Seq,
    client: ClientInfo,
}

pub(super) struct AlsaPortInfo {
    port: PortInfo,
}

pub(super) struct AlsaClients<'seq> {
    seq: &'seq Seq,
    iter: ClientIter<'seq>,
    local_client: i32,
}

pub(super) struct AlsaPorts<'seq> {
    iter: PortIter<'seq>,
}

impl AlsaSeqIo {
    pub(super) fn open(client_name: &CStr, port_name: &CStr) -> SeqIoResult<Self> {
        let seq = Seq::open(None, None, true)?;
        seq.set_client_name(client_name)?;

        let local_port = seq.create_simple_port(
            port_name,
            PortCap::WRITE | PortCap::SUBS_WRITE | PortCap::READ | PortCap::SUBS_READ,
            PortType::APPLICATION | PortType::MIDI_GENERIC,
        )?;
        let local = Addr {
            client: seq.client_id()?,
            port: local_port,
        };

        subscribe_port(&seq, Addr::system_announce(), local)?;

        let pds = (&seq, Some(AlsaDirection::Capture));
        let count = pds.count();
        if count != 1 {
            return Err(alsa::Error::new("AlsaSeqIo::open", libc::EIO));
        }
        let mut pollfds = [libc::pollfd {
            fd: 0,
            events: 0,
            revents: 0,
        }];
        pds.fill(&mut pollfds)?;
        let holder = SeqHolder {
            seq,
            fd: pollfds[0].fd,
        };
        let fd = AsyncFd::new(holder).map_err(io_to_alsa_error)?;

        Ok(Self { fd, local })
    }

    fn seq(&self) -> &Seq {
        &self.fd.get_ref().seq
    }
}

impl SeqIo for AlsaSeqIo {
    type Client<'a> = AlsaClientInfo<'a>;

    type Clients<'a> = AlsaClients<'a>;

    type Port<'a> = AlsaPortInfo;

    fn clients(&self) -> Self::Clients<'_> {
        AlsaClients {
            seq: self.seq(),
            iter: ClientIter::new(self.seq()),
            local_client: self.local.client,
        }
    }

    fn client(&self, client_id: i32) -> SeqIoResult<Option<Self::Client<'_>>> {
        if is_local_client(self.local.client, client_id) {
            return Ok(None);
        }

        match self.seq().get_any_client_info(client_id) {
            Ok(client) => Ok(Some(AlsaClientInfo {
                seq: self.seq(),
                client,
            })),
            Err(error) => {
                debug!(client_id, %error, "failed to inspect MIDI client");
                Ok(None)
            }
        }
    }

    fn port(&self, addr: Addr) -> SeqIoResult<Option<Self::Port<'_>>> {
        if is_local_client(self.local.client, addr.client) {
            return Ok(None);
        }

        match self.seq().get_any_port_info(addr) {
            Ok(port) => Ok(Some(AlsaPortInfo { port })),
            Err(error) => {
                debug!(client = addr.client, port = addr.port, %error, "failed to inspect MIDI port");
                Ok(None)
            }
        }
    }

    fn subscribe_incoming(&self, remote: Addr) -> SeqIoResult<()> {
        subscribe_port(self.seq(), remote, self.local)
    }

    fn subscribe_outgoing(&self, remote: Addr) -> SeqIoResult<()> {
        subscribe_port(self.seq(), self.local, remote)
    }

    fn drop_input(&self) -> SeqIoResult<()> {
        self.seq().input().drop_input()
    }

    async fn next_event(&mut self) -> SeqIoResult<InputEvent> {
        loop {
            let mut guard = self.fd.readable().await.map_err(io_to_alsa_error)?;
            let mut input = guard.get_inner().seq.input();
            loop {
                match input.event_input() {
                    Ok(event) => {
                        if let Some(decoded) = decode_event(self.local.client, &event) {
                            return Ok(decoded);
                        }
                    }
                    Err(error) if error.errno() == libc::EAGAIN => {
                        drop(input);
                        guard.clear_ready();
                        break;
                    }
                    Err(error) => return Err(error),
                }
            }
        }
    }

    fn send_controller(
        &self,
        dest: Addr,
        channel: u8,
        controller: u8,
        value: u8,
    ) -> SeqIoResult<()> {
        let ctrl = EvCtrl {
            channel,
            param: u32::from(controller),
            value: i32::from(value),
        };
        let mut event = Event::new(EventType::Controller, &ctrl);
        event.set_source(self.local.port);
        event.set_dest(dest);
        event.set_direct();
        self.seq().event_output_direct(&mut event).map(|_| ())
    }

    fn send_sysex(&self, dest: Addr, bytes: &[u8]) -> SeqIoResult<()> {
        let mut event = Event::new_ext(EventType::Sysex, Cow::Borrowed(bytes));
        event.set_source(self.local.port);
        event.set_dest(dest);
        event.set_direct();
        self.seq().event_output_direct(&mut event).map(|_| ())
    }
}

impl<'seq> Iterator for AlsaClients<'seq> {
    type Item = AlsaClientInfo<'seq>;

    fn next(&mut self) -> Option<Self::Item> {
        for client in self.iter.by_ref() {
            if is_local_client(self.local_client, client.get_client()) {
                continue;
            }

            return Some(AlsaClientInfo {
                seq: self.seq,
                client,
            });
        }

        None
    }
}

impl SeqClientInfo for AlsaClientInfo<'_> {
    type Port<'a>
        = AlsaPortInfo
    where
        Self: 'a;

    type Ports<'a>
        = AlsaPorts<'a>
    where
        Self: 'a;

    fn client_id(&self) -> i32 {
        self.client.get_client()
    }

    fn client_name(&self) -> SeqIoResult<&str> {
        self.client.get_name()
    }

    fn ports(&self) -> Self::Ports<'_> {
        AlsaPorts {
            iter: PortIter::new(self.seq, self.client_id()),
        }
    }
}

impl<'seq> Iterator for AlsaPorts<'seq> {
    type Item = AlsaPortInfo;

    fn next(&mut self) -> Option<Self::Item> {
        self.iter.next().map(|port| AlsaPortInfo { port })
    }
}

impl SeqPortInfo for AlsaPortInfo {
    fn addr(&self) -> Addr {
        self.port.addr()
    }

    fn port_name(&self) -> SeqIoResult<&str> {
        self.port.get_name()
    }

    fn access(&self) -> PortAccess {
        port_access_from_caps(self.port.get_capability())
    }
}

fn subscribe_port(seq: &Seq, sender: Addr, dest: Addr) -> SeqIoResult<()> {
    let sub = PortSubscribe::empty()?;
    sub.set_sender(sender);
    sub.set_dest(dest);
    seq.subscribe_port(&sub)
}

fn decode_event(local_client: i32, event: &Event<'_>) -> Option<InputEvent> {
    match event.get_type() {
        EventType::Controller => {
            let ctrl = event.get_data::<EvCtrl>()?;
            let controller = u8::try_from(ctrl.param).ok()?;
            Some(InputEvent::Controller {
                source: event.get_source(),
                channel: ctrl.channel,
                controller,
                value: ctrl.value,
            })
        }
        EventType::Noteon => {
            let note = event.get_data::<EvNote>()?;
            if note.velocity == 0 {
                Some(InputEvent::NoteOff {
                    source: event.get_source(),
                    channel: note.channel,
                    note: note.note,
                    velocity: note.velocity,
                })
            } else {
                Some(InputEvent::NoteOn {
                    source: event.get_source(),
                    channel: note.channel,
                    note: note.note,
                    velocity: note.velocity,
                })
            }
        }
        EventType::Noteoff => {
            let note = event.get_data::<EvNote>()?;
            Some(InputEvent::NoteOff {
                source: event.get_source(),
                channel: note.channel,
                note: note.note,
                velocity: note.velocity,
            })
        }
        EventType::PortStart => {
            let addr = event.get_data::<Addr>()?;
            (!is_local_client(local_client, addr.client)).then_some(InputEvent::PortStart { addr })
        }
        EventType::PortExit => {
            let addr = event.get_data::<Addr>()?;
            (!is_local_client(local_client, addr.client)).then_some(InputEvent::PortExit { addr })
        }
        _ => None,
    }
}

fn port_access_from_caps(caps: PortCap) -> PortAccess {
    PortAccess {
        can_source_events: caps.contains(PortCap::READ | PortCap::SUBS_READ),
        can_receive_events: caps.contains(PortCap::WRITE | PortCap::SUBS_WRITE),
    }
}

fn is_local_client(local_client: i32, client_id: i32) -> bool {
    client_id == local_client
}

fn io_to_alsa_error(error: std::io::Error) -> alsa::Error {
    alsa::Error::new(
        "AlsaSeqIo::async_fd",
        error.raw_os_error().unwrap_or(libc::EIO),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOCAL_CLIENT: i32 = 12;
    const REMOTE: Addr = Addr { client: 0, port: 1 };

    #[test]
    fn port_access_requires_read_and_subs_read_for_source_events() {
        let access = port_access_from_caps(PortCap::READ | PortCap::SUBS_READ);
        assert!(access.can_source_events);

        let access = port_access_from_caps(PortCap::READ);
        assert!(!access.can_source_events);

        let access = port_access_from_caps(PortCap::SUBS_READ);
        assert!(!access.can_source_events);
    }

    #[test]
    fn port_access_requires_write_and_subs_write_for_receive_events() {
        let access = port_access_from_caps(PortCap::WRITE | PortCap::SUBS_WRITE);
        assert!(access.can_receive_events);

        let access = port_access_from_caps(PortCap::WRITE);
        assert!(!access.can_receive_events);

        let access = port_access_from_caps(PortCap::SUBS_WRITE);
        assert!(!access.can_receive_events);
    }

    #[test]
    fn decodes_controller_events() {
        let ctrl = EvCtrl {
            channel: 3,
            param: 74,
            value: 91,
        };
        let mut event = Event::new(EventType::Controller, &ctrl);
        event.set_source(REMOTE.port);

        assert_eq!(
            decode_event(LOCAL_CLIENT, &event),
            Some(InputEvent::Controller {
                source: REMOTE,
                channel: 3,
                controller: 74,
                value: 91,
            })
        );
    }

    #[test]
    fn ignores_controller_numbers_outside_u8() {
        let ctrl = EvCtrl {
            channel: 3,
            param: 256,
            value: 91,
        };
        let event = Event::new(EventType::Controller, &ctrl);

        assert_eq!(decode_event(LOCAL_CLIENT, &event), None);
    }

    #[test]
    fn decodes_note_on_events() {
        let note = EvNote {
            channel: 2,
            note: 60,
            velocity: 100,
            duration: 0,
            off_velocity: 0,
        };
        let mut event = Event::new(EventType::Noteon, &note);
        event.set_source(REMOTE.port);

        assert_eq!(
            decode_event(LOCAL_CLIENT, &event),
            Some(InputEvent::NoteOn {
                source: REMOTE,
                channel: 2,
                note: 60,
                velocity: 100,
            })
        );
    }

    #[test]
    fn decodes_zero_velocity_note_on_as_note_off() {
        let note = EvNote {
            channel: 2,
            note: 60,
            velocity: 0,
            duration: 0,
            off_velocity: 0,
        };
        let mut event = Event::new(EventType::Noteon, &note);
        event.set_source(REMOTE.port);

        assert_eq!(
            decode_event(LOCAL_CLIENT, &event),
            Some(InputEvent::NoteOff {
                source: REMOTE,
                channel: 2,
                note: 60,
                velocity: 0,
            })
        );
    }

    #[test]
    fn decodes_note_off_events() {
        let note = EvNote {
            channel: 2,
            note: 60,
            velocity: 64,
            duration: 0,
            off_velocity: 0,
        };
        let mut event = Event::new(EventType::Noteoff, &note);
        event.set_source(REMOTE.port);

        assert_eq!(
            decode_event(LOCAL_CLIENT, &event),
            Some(InputEvent::NoteOff {
                source: REMOTE,
                channel: 2,
                note: 60,
                velocity: 64,
            })
        );
    }

    #[test]
    fn decodes_remote_port_start_and_exit() {
        let start = Event::new(EventType::PortStart, &REMOTE);
        let exit = Event::new(EventType::PortExit, &REMOTE);

        assert_eq!(
            decode_event(LOCAL_CLIENT, &start),
            Some(InputEvent::PortStart { addr: REMOTE })
        );
        assert_eq!(
            decode_event(LOCAL_CLIENT, &exit),
            Some(InputEvent::PortExit { addr: REMOTE })
        );
    }

    #[test]
    fn ignores_local_port_start_and_exit() {
        let local = Addr {
            client: LOCAL_CLIENT,
            port: 1,
        };
        let start = Event::new(EventType::PortStart, &local);
        let exit = Event::new(EventType::PortExit, &local);

        assert_eq!(decode_event(LOCAL_CLIENT, &start), None);
        assert_eq!(decode_event(LOCAL_CLIENT, &exit), None);
    }

    #[test]
    fn ignores_unknown_events() {
        let event = Event::new_ext(EventType::Sysex, Cow::Borrowed(&[] as &[u8]));

        assert_eq!(decode_event(LOCAL_CLIENT, &event), None);
    }

    #[test]
    fn production_io_implements_seq_io() {
        fn assert_seq_io<T: SeqIo>() {}

        assert_seq_io::<AlsaSeqIo>();
    }
}
