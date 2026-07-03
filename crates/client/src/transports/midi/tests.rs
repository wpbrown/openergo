use super::client::MidiDeviceId;
use super::seq_io::{SeqIoResult, *};
use super::*;
use crate::integration::{
    AnalogIn, AnalogOutProducer, Binder, Direction, EndpointCatalog, EndpointConfig, EndpointIo,
    EndpointLabel, EndpointLabelStore,
};
use litemap::LiteMap;
use shared::shutdown::ShutdownSource;
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::future::poll_fn;
use std::rc::Rc;
use std::task::{Poll, Waker};
use tokio::task::JoinHandle;

#[derive(Clone, Copy, Debug)]
struct FakeError {
    function: &'static str,
    errno: i32,
}

impl FakeError {
    fn io() -> Self {
        Self {
            function: "fake-seq-io",
            errno: libc::EIO,
        }
    }

    fn to_alsa(self) -> alsa::Error {
        alsa::Error::new(self.function, self.errno)
    }
}

#[derive(Clone, Debug)]
enum FakeName {
    Ok(String),
    Err(FakeError),
}

impl FakeName {
    fn ok(name: &str) -> Self {
        Self::Ok(name.to_owned())
    }

    fn err() -> Self {
        Self::Err(FakeError::io())
    }

    fn get(&self) -> SeqIoResult<&str> {
        match self {
            Self::Ok(name) => Ok(name.as_str()),
            Self::Err(error) => Err(error.to_alsa()),
        }
    }
}

#[derive(Clone, Debug)]
struct FakeSeqIo {
    state: Rc<RefCell<FakeSeqState>>,
}

#[derive(Debug)]
struct FakeSeqState {
    clients: Vec<FakeClient>,
    events: VecDeque<FakeEventStep>,
    wake_next_event: Option<Waker>,
    ops: Vec<FakeSeqOp>,
    failures: FakeFailures,
}

#[derive(Clone, Debug)]
struct FakeClient {
    client_id: i32,
    client_name: FakeName,
    ports: Vec<FakePort>,
}

#[derive(Clone, Debug)]
struct FakePort {
    addr: Addr,
    port_name: FakeName,
    access: PortAccess,
}

#[derive(Clone, Debug)]
struct FakeClientInfo {
    state: Rc<RefCell<FakeSeqState>>,
    client_id: i32,
    client_name: FakeName,
    ports: Vec<FakePortInfo>,
}

#[derive(Clone, Debug)]
struct FakePortInfo {
    addr: Addr,
    port_name: FakeName,
    access: PortAccess,
}

#[derive(Clone, Debug)]
enum FakeEventStep {
    Event(InputEvent),
    Error(FakeError),
}

#[derive(Clone, Debug, Default)]
struct FakeFailures {
    drop_input: Option<FakeError>,
    subscribe_incoming: HashMap<Addr, FakeError>,
    subscribe_outgoing: HashMap<Addr, FakeError>,
    lookup_client: HashMap<i32, FakeError>,
    lookup_port: HashMap<Addr, FakeError>,
    send_controller: HashMap<Addr, FakeError>,
    send_sysex: HashMap<Addr, FakeError>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum FakeSeqOp {
    EnumerateClients,
    EnumeratePorts {
        client: i32,
    },
    LookupClient {
        client: i32,
    },
    LookupPort {
        addr: Addr,
    },
    SubscribeIncoming {
        remote: Addr,
    },
    SubscribeOutgoing {
        remote: Addr,
    },
    DropInput,
    SendController {
        dest: Addr,
        channel: u8,
        controller: u8,
        value: u8,
    },
    SendSysex {
        dest: Addr,
        bytes: Vec<u8>,
    },
}

impl FakeSeqIo {
    fn new(clients: Vec<FakeClient>) -> Self {
        Self {
            state: Rc::new(RefCell::new(FakeSeqState {
                clients,
                events: VecDeque::new(),
                wake_next_event: None,
                ops: Vec::new(),
                failures: FakeFailures::default(),
            })),
        }
    }

    fn push_event(&self, event: InputEvent) {
        self.push_event_step(FakeEventStep::Event(event));
    }

    fn push_next_event_error(&self, error: FakeError) {
        self.push_event_step(FakeEventStep::Error(error));
    }

    fn set_clients(&self, clients: Vec<FakeClient>) {
        self.state.borrow_mut().clients = clients;
    }

    fn add_port(&self, client_id: i32, port: FakePort) {
        let mut state = self.state.borrow_mut();
        let client = state
            .clients
            .iter_mut()
            .find(|client| client.client_id == client_id)
            .expect("fake client exists");
        client.ports.push(port);
    }

    fn remove_port(&self, addr: Addr) {
        for client in &mut self.state.borrow_mut().clients {
            client.ports.retain(|port| port.addr != addr);
        }
    }

    fn take_ops(&self) -> Vec<FakeSeqOp> {
        std::mem::take(&mut self.state.borrow_mut().ops)
    }

    fn assert_no_new_ops(&self) {
        let ops = self.take_ops();
        assert_eq!(ops, Vec::<FakeSeqOp>::new());
    }

    fn fail_drop_input(&self) {
        self.state.borrow_mut().failures.drop_input = Some(FakeError::io());
    }

    fn fail_subscribe_incoming(&self, addr: Addr) {
        self.state
            .borrow_mut()
            .failures
            .subscribe_incoming
            .insert(addr, FakeError::io());
    }

    fn fail_subscribe_outgoing(&self, addr: Addr) {
        self.state
            .borrow_mut()
            .failures
            .subscribe_outgoing
            .insert(addr, FakeError::io());
    }

    fn fail_lookup_client(&self, client_id: i32) {
        self.state
            .borrow_mut()
            .failures
            .lookup_client
            .insert(client_id, FakeError::io());
    }

    fn clear_lookup_client_failure(&self, client_id: i32) {
        self.state
            .borrow_mut()
            .failures
            .lookup_client
            .remove(&client_id);
    }

    fn fail_lookup_port(&self, addr: Addr) {
        self.state
            .borrow_mut()
            .failures
            .lookup_port
            .insert(addr, FakeError::io());
    }

    fn clear_lookup_port_failure(&self, addr: Addr) {
        self.state.borrow_mut().failures.lookup_port.remove(&addr);
    }

    fn fail_send_controller(&self, addr: Addr) {
        self.state
            .borrow_mut()
            .failures
            .send_controller
            .insert(addr, FakeError::io());
    }

    fn clear_send_controller_failure(&self, addr: Addr) {
        self.state
            .borrow_mut()
            .failures
            .send_controller
            .remove(&addr);
    }

    fn fail_send_sysex(&self, addr: Addr) {
        self.state
            .borrow_mut()
            .failures
            .send_sysex
            .insert(addr, FakeError::io());
    }

    fn push_event_step(&self, step: FakeEventStep) {
        let wake = {
            let mut state = self.state.borrow_mut();
            state.events.push_back(step);
            state.wake_next_event.take()
        };
        if let Some(waker) = wake {
            waker.wake();
        }
    }
}

impl FakeClient {
    fn new(client_id: i32, client_name: &str, ports: Vec<FakePort>) -> Self {
        Self {
            client_id,
            client_name: FakeName::ok(client_name),
            ports,
        }
    }

    fn name_error(client_id: i32, ports: Vec<FakePort>) -> Self {
        Self {
            client_id,
            client_name: FakeName::err(),
            ports,
        }
    }
}

impl FakePort {
    fn new(addr: Addr, port_name: &str, access: PortAccess) -> Self {
        Self {
            addr,
            port_name: FakeName::ok(port_name),
            access,
        }
    }

    fn name_error(addr: Addr, access: PortAccess) -> Self {
        Self {
            addr,
            port_name: FakeName::err(),
            access,
        }
    }
}

impl SeqIo for FakeSeqIo {
    type Client<'seq> = FakeClientInfo;
    type Clients<'seq> = std::vec::IntoIter<FakeClientInfo>;
    type Port<'seq> = FakePortInfo;

    fn clients(&self) -> Self::Clients<'_> {
        let clients = {
            let mut state = self.state.borrow_mut();
            state.ops.push(FakeSeqOp::EnumerateClients);
            state
                .clients
                .iter()
                .map(|client| FakeClientInfo {
                    state: Rc::clone(&self.state),
                    client_id: client.client_id,
                    client_name: client.client_name.clone(),
                    ports: client.ports.iter().map(FakePortInfo::from).collect(),
                })
                .collect::<Vec<_>>()
        };
        clients.into_iter()
    }

    fn client(&self, client_id: i32) -> SeqIoResult<Option<Self::Client<'_>>> {
        let mut state = self.state.borrow_mut();
        state
            .ops
            .push(FakeSeqOp::LookupClient { client: client_id });
        if let Some(error) = state.failures.lookup_client.get(&client_id) {
            return Err(error.to_alsa());
        }
        Ok(state
            .clients
            .iter()
            .find(|client| client.client_id == client_id)
            .map(|client| FakeClientInfo {
                state: Rc::clone(&self.state),
                client_id: client.client_id,
                client_name: client.client_name.clone(),
                ports: client.ports.iter().map(FakePortInfo::from).collect(),
            }))
    }

    fn port(&self, addr: Addr) -> SeqIoResult<Option<Self::Port<'_>>> {
        let mut state = self.state.borrow_mut();
        state.ops.push(FakeSeqOp::LookupPort { addr });
        if let Some(error) = state.failures.lookup_port.get(&addr) {
            return Err(error.to_alsa());
        }
        Ok(state
            .clients
            .iter()
            .flat_map(|client| client.ports.iter())
            .find(|port| port.addr == addr)
            .map(FakePortInfo::from))
    }

    fn subscribe_incoming(&self, remote: Addr) -> SeqIoResult<()> {
        let mut state = self.state.borrow_mut();
        state.ops.push(FakeSeqOp::SubscribeIncoming { remote });
        match state.failures.subscribe_incoming.get(&remote) {
            Some(error) => Err(error.to_alsa()),
            None => Ok(()),
        }
    }

    fn subscribe_outgoing(&self, remote: Addr) -> SeqIoResult<()> {
        let mut state = self.state.borrow_mut();
        state.ops.push(FakeSeqOp::SubscribeOutgoing { remote });
        match state.failures.subscribe_outgoing.get(&remote) {
            Some(error) => Err(error.to_alsa()),
            None => Ok(()),
        }
    }

    fn drop_input(&self) -> SeqIoResult<()> {
        let mut state = self.state.borrow_mut();
        state.ops.push(FakeSeqOp::DropInput);
        match state.failures.drop_input {
            Some(error) => Err(error.to_alsa()),
            None => Ok(()),
        }
    }

    async fn next_event(&mut self) -> SeqIoResult<InputEvent> {
        poll_fn(|context| {
            let mut state = self.state.borrow_mut();
            match state.events.pop_front() {
                Some(FakeEventStep::Event(event)) => Poll::Ready(Ok(event)),
                Some(FakeEventStep::Error(error)) => Poll::Ready(Err(error.to_alsa())),
                None => {
                    state.wake_next_event = Some(context.waker().clone());
                    Poll::Pending
                }
            }
        })
        .await
    }

    fn send_controller(
        &self,
        dest: Addr,
        channel: u8,
        controller: u8,
        value: u8,
    ) -> SeqIoResult<()> {
        let mut state = self.state.borrow_mut();
        state.ops.push(FakeSeqOp::SendController {
            dest,
            channel,
            controller,
            value,
        });
        match state.failures.send_controller.get(&dest) {
            Some(error) => Err(error.to_alsa()),
            None => Ok(()),
        }
    }

    fn send_sysex(&self, dest: Addr, bytes: &[u8]) -> SeqIoResult<()> {
        let mut state = self.state.borrow_mut();
        state.ops.push(FakeSeqOp::SendSysex {
            dest,
            bytes: bytes.to_vec(),
        });
        match state.failures.send_sysex.get(&dest) {
            Some(error) => Err(error.to_alsa()),
            None => Ok(()),
        }
    }
}

impl SeqClientInfo for FakeClientInfo {
    type Port<'seq>
        = FakePortInfo
    where
        Self: 'seq;
    type Ports<'seq>
        = std::vec::IntoIter<FakePortInfo>
    where
        Self: 'seq;

    fn client_id(&self) -> i32 {
        self.client_id
    }

    fn client_name(&self) -> SeqIoResult<&str> {
        self.client_name.get()
    }

    fn ports(&self) -> Self::Ports<'_> {
        self.state.borrow_mut().ops.push(FakeSeqOp::EnumeratePorts {
            client: self.client_id,
        });
        self.ports.clone().into_iter()
    }
}

impl SeqPortInfo for FakePortInfo {
    fn addr(&self) -> Addr {
        self.addr
    }

    fn port_name(&self) -> SeqIoResult<&str> {
        self.port_name.get()
    }

    fn access(&self) -> PortAccess {
        self.access
    }
}

impl From<&FakePort> for FakePortInfo {
    fn from(port: &FakePort) -> Self {
        Self {
            addr: port.addr,
            port_name: port.port_name.clone(),
            access: port.access,
        }
    }
}

#[derive(Clone)]
struct DeviceSpec {
    key: String,
    port_match: Option<String>,
    client_match: Option<String>,
}

#[derive(Clone)]
struct ControlSpec {
    device_idx: usize,
    label: EndpointLabel,
    message: MidiMessage,
    channel: u8,
    number: u8,
    direction: Direction,
    initial: f64,
}

struct TestEndpointConfig {
    direction: Direction,
}

impl EndpointConfig for TestEndpointConfig {
    fn direction(&self) -> Direction {
        self.direction
    }
}

struct MidiTransportFixtureBuilder {
    labels: EndpointLabelStore,
    devices: Vec<DeviceSpec>,
    controls: Vec<ControlSpec>,
}

struct DeviceBuilder<'builder> {
    parent: &'builder mut MidiTransportFixtureBuilder,
    device_idx: usize,
}

struct MidiTransportFixture {
    devices: Vec<MidiDeviceConfig>,
    analog_ins: HashMap<&'static str, AnalogIn>,
    analog_outs: HashMap<&'static str, AnalogOutProducer>,
}

impl MidiTransportFixtureBuilder {
    fn new() -> Self {
        Self {
            labels: EndpointLabelStore::new(),
            devices: Vec::new(),
            controls: Vec::new(),
        }
    }

    fn device(&mut self, key: &str) -> DeviceBuilder<'_> {
        let device_idx = self.devices.len();
        self.devices.push(DeviceSpec {
            key: key.to_owned(),
            port_match: None,
            client_match: None,
        });
        DeviceBuilder {
            parent: self,
            device_idx,
        }
    }

    fn add_control(
        &mut self,
        device_idx: usize,
        label: &str,
        message: MidiMessage,
        channel: u8,
        number: u8,
        direction: Direction,
        initial: f64,
    ) {
        let label = self.labels.get_or_intern(label);
        self.controls.push(ControlSpec {
            device_idx,
            label,
            message,
            channel,
            number,
            direction,
            initial,
        });
    }

    fn finish(self) -> MidiTransportFixture {
        let labels: &'static EndpointLabelStore = Box::leak(Box::new(self.labels));
        let mut by_label = LiteMap::new();
        for control in &self.controls {
            by_label.insert(
                control.label,
                TestEndpointConfig {
                    direction: control.direction,
                },
            );
        }
        let catalog = EndpointCatalog::new(labels, by_label);
        let mut binder = Binder::new(catalog);
        let mut analog_ins = HashMap::new();
        let mut analog_outs = HashMap::new();

        for control in &self.controls {
            let label_text = labels.resolve(control.label);
            if control.direction.allows_in() {
                let analog_in = binder
                    .analog_in(control.label)
                    .expect("test input endpoint binds");
                analog_ins.insert(label_text, analog_in);
            }
            if control.direction.allows_out() {
                let analog_out = binder
                    .analog_out(control.label, control.initial)
                    .expect("test output endpoint binds");
                analog_outs.insert(label_text, analog_out);
            }
        }

        let mut io_by_label: HashMap<EndpointLabel, EndpointIo> = binder
            .complete()
            .into_iter()
            .map(|binding| (binding.label, binding.io))
            .collect();
        let mut devices = self
            .devices
            .into_iter()
            .map(|device| MidiDeviceConfig {
                device_key: device.key,
                port_match: device.port_match,
                client_match: device.client_match,
                controls: Vec::new(),
            })
            .collect::<Vec<_>>();

        for control in self.controls {
            let endpoint = io_by_label
                .remove(&control.label)
                .expect("bound endpoint exists");
            devices[control.device_idx]
                .controls
                .push(MidiControlDefinition {
                    label: labels.resolve(control.label),
                    message: control.message,
                    channel: control.channel,
                    number: control.number,
                    endpoint,
                });
        }

        MidiTransportFixture {
            devices,
            analog_ins,
            analog_outs,
        }
    }
}

impl DeviceBuilder<'_> {
    fn port_match(&mut self, value: &str) -> &mut Self {
        self.parent.devices[self.device_idx].port_match = Some(value.to_owned());
        self
    }

    fn client_match(&mut self, value: &str) -> &mut Self {
        self.parent.devices[self.device_idx].client_match = Some(value.to_owned());
        self
    }

    fn cc_in(&mut self, label: &str, channel: u8, number: u8) -> &mut Self {
        self.parent.add_control(
            self.device_idx,
            label,
            MidiMessage::Cc,
            channel,
            number,
            Direction::In,
            0.0,
        );
        self
    }

    fn cc_out(&mut self, label: &str, channel: u8, number: u8, initial: f64) -> &mut Self {
        self.parent.add_control(
            self.device_idx,
            label,
            MidiMessage::Cc,
            channel,
            number,
            Direction::Out,
            initial,
        );
        self
    }

    fn cc_in_out(&mut self, label: &str, channel: u8, number: u8, initial: f64) -> &mut Self {
        self.parent.add_control(
            self.device_idx,
            label,
            MidiMessage::Cc,
            channel,
            number,
            Direction::InOut,
            initial,
        );
        self
    }

    fn note_in(&mut self, label: &str, channel: u8, number: u8) -> &mut Self {
        self.parent.add_control(
            self.device_idx,
            label,
            MidiMessage::Note,
            channel,
            number,
            Direction::In,
            0.0,
        );
        self
    }

    fn note_out_unsupported(
        &mut self,
        label: &str,
        channel: u8,
        number: u8,
        initial: f64,
    ) -> &mut Self {
        self.parent.add_control(
            self.device_idx,
            label,
            MidiMessage::Note,
            channel,
            number,
            Direction::Out,
            initial,
        );
        self
    }
}

struct RunningTransport {
    fake: FakeSeqIo,
    shutdown: ShutdownSource,
    task: JoinHandle<Result<(), rootcause::Report>>,
    analog_ins: HashMap<&'static str, AnalogIn>,
    analog_outs: HashMap<&'static str, AnalogOutProducer>,
}

async fn spawn_transport(fixture: MidiTransportFixture, fake: FakeSeqIo) -> RunningTransport {
    let MidiTransportFixture {
        devices,
        analog_ins,
        analog_outs,
    } = fixture;
    let shutdown = ShutdownSource::new_manual();
    let task = tokio::task::spawn_local(driver::run_with_seq(
        fake.clone(),
        devices,
        shutdown.signal(),
    ));
    tokio::task::yield_now().await;
    RunningTransport {
        fake,
        shutdown,
        task,
        analog_ins,
        analog_outs,
    }
}

async fn finish_startup() {
    tokio::time::advance(STARTUP_DRAIN_DELAY).await;
    yield_transport().await;
}

async fn yield_transport() {
    for _turn in 0..4 {
        tokio::task::yield_now().await;
    }
}

async fn take_ops_until(
    fake: &FakeSeqIo,
    mut done: impl FnMut(&[FakeSeqOp]) -> bool,
) -> Vec<FakeSeqOp> {
    let mut ops = Vec::new();
    // Keep this comfortably above the current one-output-per-loop behavior;
    // a timeout here means the transport stopped making observable progress.
    for _turn in 0..64 {
        tokio::task::yield_now().await;
        ops.extend(fake.take_ops());
        if done(&ops) {
            return ops;
        }
    }
    panic!("timed out waiting for expected MIDI fake operations; saw {ops:?}");
}

async fn take_n_ops(fake: &FakeSeqIo, expected_len: usize) -> Vec<FakeSeqOp> {
    take_ops_until(fake, |ops| ops.len() >= expected_len).await
}

async fn shutdown_and_expect_ok(running: RunningTransport) {
    running.shutdown.trigger();
    let result = running.task.await.expect("transport task joined");
    assert!(result.is_ok(), "transport returned error: {result:?}");
}

fn addr(client: i32, port: i32) -> Addr {
    Addr { client, port }
}

fn access_in_out() -> PortAccess {
    PortAccess {
        can_source_events: true,
        can_receive_events: true,
    }
}

fn access_in_only() -> PortAccess {
    PortAccess {
        can_source_events: true,
        can_receive_events: false,
    }
}

fn access_out_only() -> PortAccess {
    PortAccess {
        can_source_events: false,
        can_receive_events: true,
    }
}

fn access_none() -> PortAccess {
    PortAccess {
        can_source_events: false,
        can_receive_events: false,
    }
}

fn grid_port(remote: Addr, access: PortAccess) -> FakePort {
    FakePort::new(remote, "Grid Port", access)
}

fn fake_with_ports(ports: Vec<FakePort>) -> FakeSeqIo {
    FakeSeqIo::new(vec![FakeClient::new(20, "Grid MIDI", ports)])
}

fn one_cc_in_out_fixture(initial: f64) -> MidiTransportFixture {
    let mut builder = MidiTransportFixtureBuilder::new();
    builder
        .device("grid")
        .client_match("Grid")
        .port_match("Port")
        .cc_in_out("fader", 0, 7, initial);
    builder.finish()
}

fn one_cc_in_fixture() -> MidiTransportFixture {
    let mut builder = MidiTransportFixtureBuilder::new();
    builder
        .device("grid")
        .client_match("Grid")
        .port_match("Port")
        .cc_in("fader", 0, 7);
    builder.finish()
}

fn one_cc_out_fixture(initial: f64) -> MidiTransportFixture {
    let mut builder = MidiTransportFixtureBuilder::new();
    builder
        .device("grid")
        .client_match("Grid")
        .port_match("Port")
        .cc_out("led", 0, 7, initial);
    builder.finish()
}

fn midi_client_config(devices: Vec<client::MidiDeviceSpec>) -> client::MidiClientConfig {
    client::MidiClientConfig {
        devices,
        startup_drain_delay: std::time::Duration::ZERO,
    }
}

fn midi_device_spec(
    index: usize,
    key: &str,
    wants_input: bool,
    wants_output: bool,
) -> client::MidiDeviceSpec {
    client::MidiDeviceSpec {
        id: MidiDeviceId::from_index(index),
        key: key.to_owned(),
        port_match: Some("Port".to_owned()),
        client_match: Some("Grid".to_owned()),
        wants_input,
        wants_output,
    }
}

fn sysex_op(dest: Addr) -> FakeSeqOp {
    FakeSeqOp::SendSysex {
        dest,
        bytes: FADER_REQUEST_SYSEX.to_vec(),
    }
}

fn cc_op(dest: Addr, channel: u8, controller: u8, value: u8) -> FakeSeqOp {
    FakeSeqOp::SendController {
        dest,
        channel,
        controller,
        value,
    }
}

fn assert_unordered_ops(actual: &[FakeSeqOp], expected: &[FakeSeqOp]) {
    // Use this for operation groups driven by HashSet iteration over
    // sendable MIDI ports; per-port order is intentionally not stable.
    let mut remaining = actual.to_vec();
    for expected_op in expected {
        let position = remaining
            .iter()
            .position(|actual_op| actual_op == expected_op)
            .unwrap_or_else(|| panic!("missing op {expected_op:?} in {actual:?}"));
        remaining.remove(position);
    }
    assert!(remaining.is_empty(), "unexpected ops remain: {remaining:?}");
}

fn assert_no_op_matching(ops: &[FakeSeqOp], matches: impl Fn(&FakeSeqOp) -> bool) {
    assert!(
        !ops.iter().any(matches),
        "unexpected matching operation in {ops:?}"
    );
}

fn analog_value(running: &RunningTransport, label: &str) -> f64 {
    running
        .analog_ins
        .get(label)
        .unwrap_or_else(|| panic!("missing AnalogIn {label}"))
        .get()
}

fn update_out(running: &RunningTransport, label: &str, value: f64) {
    running
        .analog_outs
        .get(label)
        .unwrap_or_else(|| panic!("missing AnalogOutProducer {label}"))
        .update(value)
        .expect("output watch is open");
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn matching_readable_and_writable_port_subscribes_both_directions() {
    let remote = addr(20, 1);
    let fake = fake_with_ports(vec![grid_port(remote, access_in_out())]);
    let running = spawn_transport(one_cc_in_out_fixture(0.42), fake).await;

    assert_eq!(
        running.fake.take_ops(),
        vec![
            FakeSeqOp::EnumerateClients,
            FakeSeqOp::EnumeratePorts { client: 20 },
            FakeSeqOp::SubscribeIncoming { remote },
            FakeSeqOp::SubscribeOutgoing { remote },
        ]
    );

    finish_startup().await;
    shutdown_and_expect_ok(running).await;
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn startup_waits_before_drop_sysex_and_initial_output() {
    let remote = addr(20, 1);
    let fake = fake_with_ports(vec![grid_port(remote, access_in_out())]);
    let running = spawn_transport(one_cc_in_out_fixture(0.42), fake).await;
    let before_delay = running.fake.take_ops();
    assert_eq!(
        before_delay,
        vec![
            FakeSeqOp::EnumerateClients,
            FakeSeqOp::EnumeratePorts { client: 20 },
            FakeSeqOp::SubscribeIncoming { remote },
            FakeSeqOp::SubscribeOutgoing { remote },
        ]
    );
    assert_no_op_matching(&before_delay, |op| {
        matches!(
            op,
            FakeSeqOp::DropInput | FakeSeqOp::SendSysex { .. } | FakeSeqOp::SendController { .. }
        )
    });

    finish_startup().await;

    assert_eq!(
        running.fake.take_ops(),
        vec![
            FakeSeqOp::DropInput,
            sysex_op(remote),
            cc_op(remote, 0, 7, 42)
        ]
    );
    shutdown_and_expect_ok(running).await;
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn one_configured_device_matching_two_startup_ports_is_unavailable() {
    let first = addr(20, 1);
    let second = addr(20, 2);
    let fake = fake_with_ports(vec![
        grid_port(first, access_in_out()),
        grid_port(second, access_in_out()),
    ]);
    let running = spawn_transport(one_cc_out_fixture(0.42), fake).await;
    running.fake.take_ops();

    finish_startup().await;

    assert_eq!(running.fake.take_ops(), vec![FakeSeqOp::DropInput]);
    shutdown_and_expect_ok(running).await;
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn input_only_matching_port_does_not_receive_output_operations() {
    let remote = addr(20, 1);
    let fake = fake_with_ports(vec![grid_port(remote, access_in_only())]);
    let running = spawn_transport(one_cc_in_out_fixture(0.42), fake).await;

    assert_eq!(
        running.fake.take_ops(),
        vec![
            FakeSeqOp::EnumerateClients,
            FakeSeqOp::EnumeratePorts { client: 20 },
            FakeSeqOp::SubscribeIncoming { remote },
        ]
    );
    finish_startup().await;
    assert_eq!(running.fake.take_ops(), vec![FakeSeqOp::DropInput]);
    shutdown_and_expect_ok(running).await;
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn output_only_matching_port_sends_startup_output() {
    let remote = addr(20, 1);
    let fake = fake_with_ports(vec![grid_port(remote, access_out_only())]);
    let running = spawn_transport(one_cc_out_fixture(0.42), fake).await;

    assert_eq!(
        running.fake.take_ops(),
        vec![
            FakeSeqOp::EnumerateClients,
            FakeSeqOp::EnumeratePorts { client: 20 },
            FakeSeqOp::SubscribeOutgoing { remote },
        ]
    );
    finish_startup().await;
    assert_eq!(
        running.fake.take_ops(),
        vec![
            FakeSeqOp::DropInput,
            sysex_op(remote),
            cc_op(remote, 0, 7, 42)
        ]
    );
    shutdown_and_expect_ok(running).await;
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn layer2_reports_multiple_matching_ports_without_subscribing() {
    let first = addr(20, 1);
    let second = addr(20, 2);
    let fake = fake_with_ports(vec![
        grid_port(first, access_in_only()),
        grid_port(second, access_in_only()),
    ]);
    let mut client = client::MidiClient::start(
        fake.clone(),
        midi_client_config(vec![midi_device_spec(0, "grid", true, false)]),
    )
    .await
    .expect("client starts");

    assert_eq!(
        fake.take_ops(),
        vec![
            FakeSeqOp::EnumerateClients,
            FakeSeqOp::EnumeratePorts { client: 20 },
            FakeSeqOp::DropInput,
        ]
    );
    assert_eq!(
        client.next_event().await.expect("pending event"),
        client::MidiClientEvent::DeviceUnavailable {
            device: MidiDeviceId::from_index(0),
            reason: client::DeviceUnavailableReason::MultipleMatchingPorts,
        }
    );
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn layer2_reports_port_claimed_by_multiple_devices_without_subscribing() {
    let remote = addr(20, 1);
    let fake = fake_with_ports(vec![grid_port(remote, access_in_only())]);
    let mut client = client::MidiClient::start(
        fake.clone(),
        midi_client_config(vec![
            midi_device_spec(0, "grid-a", true, false),
            midi_device_spec(1, "grid-b", true, false),
        ]),
    )
    .await
    .expect("client starts");

    assert_eq!(
        fake.take_ops(),
        vec![
            FakeSeqOp::EnumerateClients,
            FakeSeqOp::EnumeratePorts { client: 20 },
            FakeSeqOp::DropInput,
        ]
    );
    assert_eq!(
        client.next_event().await.expect("first pending event"),
        client::MidiClientEvent::DeviceUnavailable {
            device: MidiDeviceId::from_index(0),
            reason: client::DeviceUnavailableReason::PortClaimedByMultipleDevices,
        }
    );
    assert_eq!(
        client.next_event().await.expect("second pending event"),
        client::MidiClientEvent::DeviceUnavailable {
            device: MidiDeviceId::from_index(1),
            reason: client::DeviceUnavailableReason::PortClaimedByMultipleDevices,
        }
    );
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn unmatched_ports_are_ignored() {
    let remote = addr(20, 1);
    let fake = FakeSeqIo::new(vec![FakeClient::new(
        20,
        "Other MIDI",
        vec![FakePort::new(remote, "Other Port", access_in_out())],
    )]);
    let running = spawn_transport(one_cc_in_out_fixture(0.42), fake).await;

    assert_eq!(
        running.fake.take_ops(),
        vec![
            FakeSeqOp::EnumerateClients,
            FakeSeqOp::EnumeratePorts { client: 20 },
        ]
    );
    finish_startup().await;
    assert_eq!(running.fake.take_ops(), vec![FakeSeqOp::DropInput]);
    shutdown_and_expect_ok(running).await;
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn port_name_lookup_errors_during_initial_enumeration_skip_port() {
    let remote = addr(20, 1);
    let fake = fake_with_ports(vec![FakePort::name_error(remote, access_in_out())]);
    let running = spawn_transport(one_cc_in_out_fixture(0.42), fake).await;

    assert_eq!(
        running.fake.take_ops(),
        vec![
            FakeSeqOp::EnumerateClients,
            FakeSeqOp::EnumeratePorts { client: 20 },
        ]
    );
    finish_startup().await;
    assert_eq!(running.fake.take_ops(), vec![FakeSeqOp::DropInput]);
    shutdown_and_expect_ok(running).await;
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn client_name_lookup_errors_still_allow_port_only_matching() {
    let remote = addr(20, 1);
    let fake = FakeSeqIo::new(vec![FakeClient::name_error(
        20,
        vec![FakePort::new(remote, "Grid Port", access_in_out())],
    )]);
    let mut builder = MidiTransportFixtureBuilder::new();
    builder
        .device("grid")
        .port_match("Grid")
        .cc_in_out("fader", 0, 7, 0.42);
    let running = spawn_transport(builder.finish(), fake).await;

    assert_eq!(
        running.fake.take_ops(),
        vec![
            FakeSeqOp::EnumerateClients,
            FakeSeqOp::EnumeratePorts { client: 20 },
            FakeSeqOp::SubscribeIncoming { remote },
            FakeSeqOp::SubscribeOutgoing { remote },
        ]
    );
    finish_startup().await;
    shutdown_and_expect_ok(running).await;
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn matched_ports_with_no_usable_direction_are_ignored() {
    let remote = addr(20, 1);
    let fake = fake_with_ports(vec![grid_port(remote, access_none())]);
    let running = spawn_transport(one_cc_in_fixture(), fake).await;
    running.fake.take_ops();
    finish_startup().await;
    running.fake.take_ops();

    running.fake.push_event(InputEvent::Controller {
        source: remote,
        channel: 0,
        controller: 7,
        value: 90,
    });
    yield_transport().await;

    assert_eq!(analog_value(&running, "fader"), 0.0);
    running.fake.assert_no_new_ops();
    shutdown_and_expect_ok(running).await;
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn controller_event_updates_bound_analog_input() {
    let remote = addr(20, 1);
    let running = spawn_transport(
        one_cc_in_fixture(),
        fake_with_ports(vec![grid_port(remote, access_in_only())]),
    )
    .await;
    running.fake.take_ops();
    finish_startup().await;
    running.fake.take_ops();

    running.fake.push_event(InputEvent::Controller {
        source: remote,
        channel: 0,
        controller: 7,
        value: 64,
    });
    yield_transport().await;

    assert_eq!(analog_value(&running, "fader"), 64.0 / 127.0);
    shutdown_and_expect_ok(running).await;
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn controller_values_clamp_to_midi_range() {
    let remote = addr(20, 1);
    let running = spawn_transport(
        one_cc_in_fixture(),
        fake_with_ports(vec![grid_port(remote, access_in_only())]),
    )
    .await;
    running.fake.take_ops();
    finish_startup().await;
    running.fake.take_ops();

    running.fake.push_event(InputEvent::Controller {
        source: remote,
        channel: 0,
        controller: 7,
        value: i32::MIN,
    });
    yield_transport().await;
    assert_eq!(analog_value(&running, "fader"), 0.0);

    running.fake.push_event(InputEvent::Controller {
        source: remote,
        channel: 0,
        controller: 7,
        value: i32::MAX,
    });
    yield_transport().await;
    assert_eq!(analog_value(&running, "fader"), 1.0);
    shutdown_and_expect_ok(running).await;
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn unmatched_source_channel_or_controller_are_ignored() {
    let remote = addr(20, 1);
    let running = spawn_transport(
        one_cc_in_fixture(),
        fake_with_ports(vec![grid_port(remote, access_in_only())]),
    )
    .await;
    running.fake.take_ops();
    finish_startup().await;
    running.fake.take_ops();

    for event in [
        InputEvent::Controller {
            source: addr(99, 1),
            channel: 0,
            controller: 7,
            value: 100,
        },
        InputEvent::Controller {
            source: remote,
            channel: 1,
            controller: 7,
            value: 100,
        },
        InputEvent::Controller {
            source: remote,
            channel: 0,
            controller: 8,
            value: 100,
        },
    ] {
        running.fake.push_event(event);
        yield_transport().await;
        assert_eq!(analog_value(&running, "fader"), 0.0);
    }
    shutdown_and_expect_ok(running).await;
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn note_on_increments_note_input_and_note_off_is_ignored() {
    let remote = addr(20, 1);
    let mut builder = MidiTransportFixtureBuilder::new();
    builder
        .device("grid")
        .client_match("Grid")
        .port_match("Port")
        .note_in("pad", 0, 60);
    let running = spawn_transport(
        builder.finish(),
        fake_with_ports(vec![grid_port(remote, access_in_only())]),
    )
    .await;
    running.fake.take_ops();
    finish_startup().await;
    running.fake.take_ops();

    for _turn in 0..2 {
        running.fake.push_event(InputEvent::NoteOn {
            source: remote,
            channel: 0,
            note: 60,
            velocity: 100,
        });
        yield_transport().await;
    }
    assert_eq!(analog_value(&running, "pad"), 2.0);

    running.fake.push_event(InputEvent::NoteOff {
        source: remote,
        channel: 0,
        note: 60,
        velocity: 0,
    });
    yield_transport().await;
    assert_eq!(analog_value(&running, "pad"), 2.0);
    shutdown_and_expect_ok(running).await;
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn velocity_zero_note_on_decoded_as_note_off_is_ignored_at_transport_boundary() {
    let remote = addr(20, 1);
    let mut builder = MidiTransportFixtureBuilder::new();
    builder
        .device("grid")
        .client_match("Grid")
        .port_match("Port")
        .note_in("pad", 0, 60);
    let running = spawn_transport(
        builder.finish(),
        fake_with_ports(vec![grid_port(remote, access_in_only())]),
    )
    .await;
    running.fake.take_ops();
    finish_startup().await;
    running.fake.take_ops();

    running.fake.push_event(InputEvent::NoteOff {
        source: remote,
        channel: 0,
        note: 60,
        velocity: 0,
    });
    yield_transport().await;

    assert_eq!(analog_value(&running, "pad"), 0.0);
    shutdown_and_expect_ok(running).await;
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn note_on_does_not_match_cc_binding_with_same_address() {
    let remote = addr(20, 1);
    let running = spawn_transport(
        one_cc_in_fixture(),
        fake_with_ports(vec![grid_port(remote, access_in_only())]),
    )
    .await;
    running.fake.take_ops();
    finish_startup().await;
    running.fake.take_ops();

    running.fake.push_event(InputEvent::NoteOn {
        source: remote,
        channel: 0,
        note: 7,
        velocity: 100,
    });
    yield_transport().await;

    assert_eq!(analog_value(&running, "fader"), 0.0);
    shutdown_and_expect_ok(running).await;
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn one_port_matching_multiple_devices_is_unavailable() {
    let remote = addr(20, 1);
    let mut builder = MidiTransportFixtureBuilder::new();
    builder
        .device("grid-a")
        .client_match("Grid")
        .port_match("Port")
        .cc_in("left", 0, 7);
    builder
        .device("grid-b")
        .client_match("Grid")
        .port_match("Port")
        .cc_in("right", 0, 7);
    let running = spawn_transport(
        builder.finish(),
        fake_with_ports(vec![grid_port(remote, access_in_only())]),
    )
    .await;
    running.fake.take_ops();
    finish_startup().await;
    running.fake.take_ops();

    running.fake.push_event(InputEvent::Controller {
        source: remote,
        channel: 0,
        controller: 7,
        value: 80,
    });
    yield_transport().await;

    assert_eq!(analog_value(&running, "left"), 0.0);
    assert_eq!(analog_value(&running, "right"), 0.0);
    shutdown_and_expect_ok(running).await;
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn duplicate_input_control_keys_on_one_device_use_later_binding() {
    let remote = addr(20, 1);
    let mut builder = MidiTransportFixtureBuilder::new();
    builder
        .device("grid")
        .client_match("Grid")
        .port_match("Port")
        .cc_in("first", 0, 7)
        .cc_in("second", 0, 7);
    let running = spawn_transport(
        builder.finish(),
        fake_with_ports(vec![grid_port(remote, access_in_only())]),
    )
    .await;
    running.fake.take_ops();
    finish_startup().await;
    running.fake.take_ops();

    running.fake.push_event(InputEvent::Controller {
        source: remote,
        channel: 0,
        controller: 7,
        value: 80,
    });
    yield_transport().await;

    assert_eq!(analog_value(&running, "first"), 0.0);
    assert_eq!(analog_value(&running, "second"), 80.0 / 127.0);
    shutdown_and_expect_ok(running).await;
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn output_only_port_does_not_route_fake_input_dispatch() {
    let remote = addr(20, 1);
    let running = spawn_transport(
        one_cc_in_out_fixture(0.42),
        fake_with_ports(vec![grid_port(remote, access_out_only())]),
    )
    .await;
    running.fake.take_ops();
    finish_startup().await;
    running.fake.take_ops();

    running.fake.push_event(InputEvent::Controller {
        source: remote,
        channel: 0,
        controller: 7,
        value: 70,
    });
    yield_transport().await;

    assert_eq!(analog_value(&running, "fader"), 0.0);
    shutdown_and_expect_ok(running).await;
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn analog_out_update_sends_controller_event() {
    let remote = addr(20, 1);
    let running = spawn_transport(
        one_cc_out_fixture(0.42),
        fake_with_ports(vec![grid_port(remote, access_out_only())]),
    )
    .await;
    running.fake.take_ops();
    finish_startup().await;
    running.fake.take_ops();

    update_out(&running, "led", 0.51);

    assert_eq!(
        take_n_ops(&running.fake, 1).await,
        vec![cc_op(remote, 0, 7, 51)]
    );
    shutdown_and_expect_ok(running).await;
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn quantized_duplicate_output_values_are_suppressed() {
    let remote = addr(20, 1);
    let running = spawn_transport(
        one_cc_out_fixture(0.421),
        fake_with_ports(vec![grid_port(remote, access_out_only())]),
    )
    .await;
    running.fake.take_ops();
    finish_startup().await;
    running.fake.take_ops();

    update_out(&running, "led", 0.429);
    yield_transport().await;

    running.fake.assert_no_new_ops();
    shutdown_and_expect_ok(running).await;
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn quantized_output_changes_are_sent() {
    let remote = addr(20, 1);
    let running = spawn_transport(
        one_cc_out_fixture(0.42),
        fake_with_ports(vec![grid_port(remote, access_out_only())]),
    )
    .await;
    running.fake.take_ops();
    finish_startup().await;
    running.fake.take_ops();

    update_out(&running, "led", 0.43);

    assert_eq!(
        take_n_ops(&running.fake, 1).await,
        vec![cc_op(remote, 0, 7, 43)]
    );
    shutdown_and_expect_ok(running).await;
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn output_conversion_uses_floor_times_one_hundred_with_clamping() {
    let remote = addr(20, 1);
    let running = spawn_transport(
        one_cc_out_fixture(0.42),
        fake_with_ports(vec![grid_port(remote, access_out_only())]),
    )
    .await;
    running.fake.take_ops();
    finish_startup().await;
    running.fake.take_ops();

    for (value, expected_cc) in [(-0.2, 0), (1.5, 127), (0.999, 99)] {
        update_out(&running, "led", value);
        assert_eq!(
            take_n_ops(&running.fake, 1).await,
            vec![cc_op(remote, 0, 7, expected_cc)]
        );
    }
    shutdown_and_expect_ok(running).await;
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn closed_output_watch_is_ignored() {
    let remote = addr(20, 1);
    let mut running = spawn_transport(
        one_cc_out_fixture(0.42),
        fake_with_ports(vec![grid_port(remote, access_out_only())]),
    )
    .await;
    running.fake.take_ops();
    finish_startup().await;
    running.fake.take_ops();

    running.analog_outs.remove("led");
    yield_transport().await;

    running.fake.assert_no_new_ops();
    shutdown_and_expect_ok(running).await;
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn note_outputs_are_skipped() {
    let remote = addr(20, 1);
    let mut builder = MidiTransportFixtureBuilder::new();
    builder
        .device("grid")
        .client_match("Grid")
        .port_match("Port")
        .note_out_unsupported("note-led", 0, 60, 0.42);
    let running = spawn_transport(
        builder.finish(),
        fake_with_ports(vec![grid_port(remote, access_out_only())]),
    )
    .await;
    running.fake.take_ops();
    finish_startup().await;

    assert_eq!(
        running.fake.take_ops(),
        vec![FakeSeqOp::DropInput, sysex_op(remote)]
    );
    shutdown_and_expect_ok(running).await;
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn multiple_devices_matching_same_port_do_not_send_output() {
    let remote = addr(20, 1);
    let mut builder = MidiTransportFixtureBuilder::new();
    builder
        .device("grid-a")
        .client_match("Grid")
        .port_match("Port")
        .cc_out("left-led", 0, 7, 0.42);
    builder
        .device("grid-b")
        .client_match("Grid")
        .port_match("Port")
        .cc_out("right-led", 0, 8, 0.51);
    let running = spawn_transport(
        builder.finish(),
        fake_with_ports(vec![grid_port(remote, access_out_only())]),
    )
    .await;
    running.fake.take_ops();
    finish_startup().await;

    assert_eq!(running.fake.take_ops(), vec![FakeSeqOp::DropInput]);
    shutdown_and_expect_ok(running).await;
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn multiple_outputs_on_one_device_are_all_drained() {
    let remote = addr(20, 1);
    let mut builder = MidiTransportFixtureBuilder::new();
    builder
        .device("grid")
        .client_match("Grid")
        .port_match("Port")
        .cc_out("left-led", 0, 7, 0.42)
        .cc_out("right-led", 0, 8, 0.51);
    let running = spawn_transport(
        builder.finish(),
        fake_with_ports(vec![grid_port(remote, access_out_only())]),
    )
    .await;
    running.fake.take_ops();
    finish_startup().await;
    running.fake.take_ops();

    update_out(&running, "left-led", 0.7);
    update_out(&running, "right-led", 0.8);

    assert_unordered_ops(
        &take_n_ops(&running.fake, 2).await,
        &[cc_op(remote, 0, 7, 70), cc_op(remote, 0, 8, 80)],
    );
    shutdown_and_expect_ok(running).await;
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn hotplug_port_start_for_matching_port_subscribes_and_forces_output() {
    let remote = addr(20, 1);
    let fake = fake_with_ports(vec![]);
    let running = spawn_transport(one_cc_in_out_fixture(0.42), fake).await;
    running.fake.take_ops();
    finish_startup().await;
    running.fake.take_ops();

    running
        .fake
        .add_port(20, grid_port(remote, access_in_out()));
    running
        .fake
        .push_event(InputEvent::PortStart { addr: remote });
    yield_transport().await;

    assert_eq!(
        running.fake.take_ops(),
        vec![
            FakeSeqOp::LookupClient { client: 20 },
            FakeSeqOp::LookupPort { addr: remote },
            FakeSeqOp::SubscribeIncoming { remote },
            FakeSeqOp::SubscribeOutgoing { remote },
            sysex_op(remote),
            cc_op(remote, 0, 7, 42),
        ]
    );
    shutdown_and_expect_ok(running).await;
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn hotplug_missing_client_or_port_is_ignored() {
    let missing_client = addr(99, 1);
    let missing_port = addr(20, 2);
    let running = spawn_transport(one_cc_in_out_fixture(0.42), fake_with_ports(vec![])).await;
    running.fake.take_ops();
    finish_startup().await;
    running.fake.take_ops();
    running
        .fake
        .set_clients(vec![FakeClient::new(20, "Grid MIDI", vec![])]);

    running.fake.push_event(InputEvent::PortStart {
        addr: missing_client,
    });
    yield_transport().await;
    assert_eq!(
        running.fake.take_ops(),
        vec![FakeSeqOp::LookupClient { client: 99 }]
    );

    running
        .fake
        .push_event(InputEvent::PortStart { addr: missing_port });
    yield_transport().await;
    assert_eq!(
        running.fake.take_ops(),
        vec![
            FakeSeqOp::LookupClient { client: 20 },
            FakeSeqOp::LookupPort { addr: missing_port },
        ]
    );
    shutdown_and_expect_ok(running).await;
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn hotplug_lookup_errors_are_ignored_conservatively() {
    let remote = addr(20, 1);
    let running = spawn_transport(one_cc_in_out_fixture(0.42), fake_with_ports(vec![])).await;
    running.fake.take_ops();
    finish_startup().await;
    running.fake.take_ops();

    running.fake.fail_lookup_client(20);
    running
        .fake
        .push_event(InputEvent::PortStart { addr: remote });
    yield_transport().await;
    assert_eq!(
        running.fake.take_ops(),
        vec![FakeSeqOp::LookupClient { client: 20 }]
    );

    running.fake.clear_lookup_client_failure(20);
    running
        .fake
        .add_port(20, grid_port(remote, access_in_out()));
    running.fake.fail_lookup_port(remote);
    running
        .fake
        .push_event(InputEvent::PortStart { addr: remote });
    yield_transport().await;
    assert_eq!(
        running.fake.take_ops(),
        vec![
            FakeSeqOp::LookupClient { client: 20 },
            FakeSeqOp::LookupPort { addr: remote },
        ]
    );
    running.fake.clear_lookup_port_failure(remote);
    shutdown_and_expect_ok(running).await;
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn hotplug_input_only_port_does_not_force_output_republish() {
    let remote = addr(20, 1);
    let running = spawn_transport(one_cc_in_out_fixture(0.42), fake_with_ports(vec![])).await;
    running.fake.take_ops();
    finish_startup().await;
    running.fake.take_ops();

    running
        .fake
        .add_port(20, grid_port(remote, access_in_only()));
    running
        .fake
        .push_event(InputEvent::PortStart { addr: remote });
    yield_transport().await;

    assert_eq!(
        running.fake.take_ops(),
        vec![
            FakeSeqOp::LookupClient { client: 20 },
            FakeSeqOp::LookupPort { addr: remote },
            FakeSeqOp::SubscribeIncoming { remote },
        ]
    );
    shutdown_and_expect_ok(running).await;
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn hotplug_second_matching_port_makes_active_device_unavailable() {
    let first = addr(20, 1);
    let second = addr(20, 2);
    let running = spawn_transport(
        one_cc_out_fixture(0.42),
        fake_with_ports(vec![grid_port(first, access_out_only())]),
    )
    .await;
    running.fake.take_ops();
    finish_startup().await;
    running.fake.take_ops();

    running
        .fake
        .add_port(20, grid_port(second, access_out_only()));
    running
        .fake
        .push_event(InputEvent::PortStart { addr: second });
    yield_transport().await;

    assert_eq!(
        running.fake.take_ops(),
        vec![
            FakeSeqOp::LookupClient { client: 20 },
            FakeSeqOp::LookupPort { addr: second },
        ]
    );

    update_out(&running, "led", 0.51);
    yield_transport().await;
    running.fake.assert_no_new_ops();
    shutdown_and_expect_ok(running).await;
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn hotplug_ambiguity_suppresses_input_until_resolution() {
    let first = addr(20, 1);
    let second = addr(20, 2);
    let running = spawn_transport(
        one_cc_in_fixture(),
        fake_with_ports(vec![grid_port(first, access_in_only())]),
    )
    .await;
    running.fake.take_ops();
    finish_startup().await;
    running.fake.take_ops();

    running
        .fake
        .add_port(20, grid_port(second, access_in_only()));
    running
        .fake
        .push_event(InputEvent::PortStart { addr: second });
    yield_transport().await;
    assert_eq!(
        running.fake.take_ops(),
        vec![
            FakeSeqOp::LookupClient { client: 20 },
            FakeSeqOp::LookupPort { addr: second },
        ]
    );

    running.fake.push_event(InputEvent::Controller {
        source: first,
        channel: 0,
        controller: 7,
        value: 70,
    });
    yield_transport().await;
    assert_eq!(analog_value(&running, "fader"), 0.0);

    running.fake.remove_port(second);
    running
        .fake
        .push_event(InputEvent::PortExit { addr: second });
    yield_transport().await;
    running.fake.assert_no_new_ops();

    running.fake.push_event(InputEvent::Controller {
        source: first,
        channel: 0,
        controller: 7,
        value: 80,
    });
    yield_transport().await;
    assert_eq!(analog_value(&running, "fader"), 80.0 / 127.0);
    shutdown_and_expect_ok(running).await;
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn hotplug_ambiguity_resolution_reattaches_remaining_port() {
    let first = addr(20, 1);
    let second = addr(20, 2);
    let running = spawn_transport(
        one_cc_out_fixture(0.42),
        fake_with_ports(vec![grid_port(first, access_out_only())]),
    )
    .await;
    running.fake.take_ops();
    finish_startup().await;
    running.fake.take_ops();

    running
        .fake
        .add_port(20, grid_port(second, access_out_only()));
    running
        .fake
        .push_event(InputEvent::PortStart { addr: second });
    yield_transport().await;

    let ops = running.fake.take_ops();
    assert_eq!(
        ops,
        vec![
            FakeSeqOp::LookupClient { client: 20 },
            FakeSeqOp::LookupPort { addr: second },
        ]
    );

    running.fake.remove_port(second);
    running
        .fake
        .push_event(InputEvent::PortExit { addr: second });
    yield_transport().await;

    assert_eq!(
        running.fake.take_ops(),
        vec![sysex_op(first), cc_op(first, 0, 7, 42)]
    );
    shutdown_and_expect_ok(running).await;
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn duplicate_port_start_is_idempotent() {
    let remote = addr(20, 1);
    let running = spawn_transport(one_cc_out_fixture(0.42), fake_with_ports(vec![])).await;
    running.fake.take_ops();
    finish_startup().await;
    running.fake.take_ops();

    running
        .fake
        .add_port(20, grid_port(remote, access_out_only()));
    running
        .fake
        .push_event(InputEvent::PortStart { addr: remote });
    yield_transport().await;
    assert!(!running.fake.take_ops().is_empty());

    running
        .fake
        .push_event(InputEvent::PortStart { addr: remote });
    yield_transport().await;
    assert_eq!(
        running.fake.take_ops(),
        vec![
            FakeSeqOp::LookupClient { client: 20 },
            FakeSeqOp::LookupPort { addr: remote },
        ]
    );
    shutdown_and_expect_ok(running).await;
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn port_exit_removes_active_input_and_output_routing() {
    let remote = addr(20, 1);
    let running = spawn_transport(
        one_cc_in_out_fixture(0.42),
        fake_with_ports(vec![grid_port(remote, access_in_out())]),
    )
    .await;
    running.fake.take_ops();
    finish_startup().await;
    running.fake.take_ops();

    running.fake.remove_port(remote);
    running
        .fake
        .push_event(InputEvent::PortExit { addr: remote });
    yield_transport().await;
    running.fake.assert_no_new_ops();

    running.fake.push_event(InputEvent::Controller {
        source: remote,
        channel: 0,
        controller: 7,
        value: 100,
    });
    update_out(&running, "fader", 0.6);
    yield_transport().await;

    assert_eq!(analog_value(&running, "fader"), 0.0);
    running.fake.assert_no_new_ops();
    shutdown_and_expect_ok(running).await;
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn replug_after_exit_repeats_attach_behavior_and_receives_current_output() {
    let remote = addr(20, 1);
    let running = spawn_transport(
        one_cc_out_fixture(0.42),
        fake_with_ports(vec![grid_port(remote, access_out_only())]),
    )
    .await;
    running.fake.take_ops();
    finish_startup().await;
    running.fake.take_ops();

    running.fake.remove_port(remote);
    running
        .fake
        .push_event(InputEvent::PortExit { addr: remote });
    yield_transport().await;
    running.fake.assert_no_new_ops();

    running
        .fake
        .add_port(20, grid_port(remote, access_out_only()));
    running
        .fake
        .push_event(InputEvent::PortStart { addr: remote });
    yield_transport().await;

    assert_eq!(
        running.fake.take_ops(),
        vec![
            FakeSeqOp::LookupClient { client: 20 },
            FakeSeqOp::LookupPort { addr: remote },
            FakeSeqOp::SubscribeOutgoing { remote },
            sysex_op(remote),
            cc_op(remote, 0, 7, 42),
        ]
    );
    shutdown_and_expect_ok(running).await;
}

#[test]
fn prepare_devices_returns_none_for_empty_controls_on_every_device() {
    assert!(
        driver::prepare_driver(vec![
            MidiDeviceConfig {
                device_key: "empty".to_owned(),
                port_match: None,
                client_match: None,
                controls: Vec::new(),
            },
            MidiDeviceConfig {
                device_key: "also-empty".to_owned(),
                port_match: Some("Grid".to_owned()),
                client_match: Some("Grid".to_owned()),
                controls: Vec::new(),
            }
        ])
        .is_none()
    );
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn prepare_devices_mixed_empty_and_configured_devices_runs_transport() {
    let mut builder = MidiTransportFixtureBuilder::new();
    builder.device("empty").port_match("Empty");
    builder
        .device("configured")
        .port_match("Configured")
        .cc_out("led", 0, 7, 0.42);
    let fixture = builder.finish();
    let empty = addr(20, 1);
    let configured = addr(20, 2);
    let fake = FakeSeqIo::new(vec![FakeClient::new(
        20,
        "Grid MIDI",
        vec![
            FakePort::new(empty, "Empty Port", access_out_only()),
            FakePort::new(configured, "Configured Port", access_out_only()),
        ],
    )]);
    let running = spawn_transport(fixture, fake).await;

    assert_eq!(
        running.fake.take_ops(),
        vec![
            FakeSeqOp::EnumerateClients,
            FakeSeqOp::EnumeratePorts { client: 20 },
            FakeSeqOp::SubscribeOutgoing { remote: configured },
        ]
    );
    finish_startup().await;
    assert_eq!(
        running.fake.take_ops(),
        vec![
            FakeSeqOp::DropInput,
            sysex_op(configured),
            cc_op(configured, 0, 7, 42),
        ]
    );

    shutdown_and_expect_ok(running).await;
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn shutdown_while_waiting_for_next_midi_event_exits_ok() {
    let remote = addr(20, 1);
    let running = spawn_transport(
        one_cc_in_fixture(),
        fake_with_ports(vec![grid_port(remote, access_in_only())]),
    )
    .await;
    running.fake.take_ops();
    finish_startup().await;
    running.fake.take_ops();

    shutdown_and_expect_ok(running).await;
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn shutdown_while_waiting_for_output_changes_exits_ok() {
    let remote = addr(20, 1);
    let running = spawn_transport(
        one_cc_out_fixture(0.42),
        fake_with_ports(vec![grid_port(remote, access_out_only())]),
    )
    .await;
    running.fake.take_ops();
    finish_startup().await;
    running.fake.take_ops();

    shutdown_and_expect_ok(running).await;
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn shutdown_during_startup_drain_still_finishes_startup_publish_before_exit() {
    let remote = addr(20, 1);
    let running = spawn_transport(
        one_cc_out_fixture(0.42),
        fake_with_ports(vec![grid_port(remote, access_out_only())]),
    )
    .await;
    running.fake.take_ops();

    running.shutdown.trigger();
    finish_startup().await;

    assert_eq!(
        running.fake.take_ops(),
        vec![
            FakeSeqOp::DropInput,
            sysex_op(remote),
            cc_op(remote, 0, 7, 42)
        ]
    );
    let result = running.task.await.expect("transport task joined");
    assert!(result.is_ok(), "transport returned error: {result:?}");
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn drop_input_failure_returns_error() {
    let remote = addr(20, 1);
    let fake = fake_with_ports(vec![grid_port(remote, access_in_only())]);
    fake.fail_drop_input();
    let running = spawn_transport(one_cc_in_fixture(), fake).await;
    running.fake.take_ops();

    finish_startup().await;

    let result = running.task.await.expect("transport task joined");
    assert!(result.is_err());
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn next_event_failure_returns_stream_error_report() {
    let remote = addr(20, 1);
    let running = spawn_transport(
        one_cc_in_fixture(),
        fake_with_ports(vec![grid_port(remote, access_in_only())]),
    )
    .await;
    running.fake.take_ops();
    finish_startup().await;
    running.fake.take_ops();

    running.fake.push_next_event_error(FakeError::io());
    let result = running.task.await.expect("transport task joined");
    let error = result.expect_err("event stream fails");
    assert!(format!("{error:?}").contains("alsa sequencer event stream error"));
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn incoming_subscription_failure_skips_port_entirely() {
    let remote = addr(20, 1);
    let fake = fake_with_ports(vec![grid_port(remote, access_in_out())]);
    fake.fail_subscribe_incoming(remote);
    let running = spawn_transport(one_cc_in_out_fixture(0.42), fake).await;

    assert_eq!(
        running.fake.take_ops(),
        vec![
            FakeSeqOp::EnumerateClients,
            FakeSeqOp::EnumeratePorts { client: 20 },
            FakeSeqOp::SubscribeIncoming { remote },
        ]
    );
    finish_startup().await;
    assert_eq!(running.fake.take_ops(), vec![FakeSeqOp::DropInput]);

    running.fake.push_event(InputEvent::Controller {
        source: remote,
        channel: 0,
        controller: 7,
        value: 90,
    });
    yield_transport().await;
    assert_eq!(analog_value(&running, "fader"), 0.0);
    shutdown_and_expect_ok(running).await;
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn outgoing_subscription_failure_leaves_input_active_but_output_unavailable() {
    let remote = addr(20, 1);
    let fake = fake_with_ports(vec![grid_port(remote, access_in_out())]);
    fake.fail_subscribe_outgoing(remote);
    let running = spawn_transport(one_cc_in_out_fixture(0.42), fake).await;
    running.fake.take_ops();
    finish_startup().await;
    assert_eq!(running.fake.take_ops(), vec![FakeSeqOp::DropInput]);

    running.fake.push_event(InputEvent::Controller {
        source: remote,
        channel: 0,
        controller: 7,
        value: 90,
    });
    update_out(&running, "fader", 0.6);
    yield_transport().await;

    assert_eq!(analog_value(&running, "fader"), 90.0 / 127.0);
    running.fake.assert_no_new_ops();
    shutdown_and_expect_ok(running).await;
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn send_sysex_failure_does_not_stop_task() {
    let remote = addr(20, 1);
    let fake = fake_with_ports(vec![grid_port(remote, access_in_out())]);
    fake.fail_send_sysex(remote);
    let running = spawn_transport(one_cc_in_out_fixture(0.42), fake).await;
    running.fake.take_ops();
    finish_startup().await;
    assert_eq!(
        running.fake.take_ops(),
        vec![
            FakeSeqOp::DropInput,
            sysex_op(remote),
            cc_op(remote, 0, 7, 42)
        ]
    );

    running.fake.push_event(InputEvent::Controller {
        source: remote,
        channel: 0,
        controller: 7,
        value: 90,
    });
    yield_transport().await;
    assert_eq!(analog_value(&running, "fader"), 90.0 / 127.0);
    shutdown_and_expect_ok(running).await;
}

#[tokio::test(flavor = "local", start_paused = true)]
async fn send_controller_failure_does_not_stop_task_and_later_send_can_succeed() {
    let remote = addr(20, 1);
    let fake = fake_with_ports(vec![grid_port(remote, access_out_only())]);
    fake.fail_send_controller(remote);
    let running = spawn_transport(one_cc_out_fixture(0.42), fake).await;
    running.fake.take_ops();
    finish_startup().await;
    assert_eq!(
        running.fake.take_ops(),
        vec![
            FakeSeqOp::DropInput,
            sysex_op(remote),
            cc_op(remote, 0, 7, 42)
        ]
    );

    running.fake.clear_send_controller_failure(remote);
    update_out(&running, "led", 0.6);
    yield_transport().await;

    assert_eq!(running.fake.take_ops(), vec![cc_op(remote, 0, 7, 60)]);
    shutdown_and_expect_ok(running).await;
}
