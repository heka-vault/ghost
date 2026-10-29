//! PipeWire plumbing shared by the streams and the ducker: a main loop, a core, round
//! trips, the registry's nodes, and the `Props` and `Format` params read and written as
//! pods. Everything here is synchronous and single-threaded, as `libpipewire` wants; the
//! streams run it on their own thread, the ducker on the caller's.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::fmt;
use std::io::Cursor;
use std::rc::Rc;
use std::sync::Once;
use std::time::Duration;

use pipewire::context::ContextRc;
use pipewire::core::{CoreRc, Listener as CoreListener, PW_ID_CORE};
use pipewire::keys;
use pipewire::main_loop::MainLoopRc;
use pipewire::node::{Node, NodeListener, NodeState as PwNodeState};
use pipewire::properties::{PropertiesBox, properties};
use pipewire::registry::GlobalObject;
use pipewire::spa::param::ParamType;
use pipewire::spa::param::audio::{AudioFormat, AudioInfoRaw};
use pipewire::spa::pod::deserialize::PodDeserializer;
use pipewire::spa::pod::serialize::PodSerializer;
use pipewire::spa::pod::{Object, Pod, Property, Value, ValueArray};
use pipewire::spa::sys as spa_sys;
use pipewire::spa::utils::dict::DictRef;
use pipewire::types::ObjectType;

static INIT: Once = Once::new();

/// `pw_init` once per process.
pub fn init_once() {
    INIT.call_once(pipewire::init);
}

#[derive(Debug, thiserror::Error)]
pub enum PwError {
    #[error("cannot reach PipeWire: {0}")]
    Connect(String),
    #[error("PipeWire: {0}")]
    Call(String),
    #[error("PipeWire did not answer within {0:?}")]
    Timeout(Duration),
}

impl From<pipewire::Error> for PwError {
    fn from(e: pipewire::Error) -> Self {
        PwError::Call(e.to_string())
    }
}

/// A node's state, owned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NodeState {
    Error(String),
    Creating,
    Suspended,
    Idle,
    Running,
}

impl From<PwNodeState<'_>> for NodeState {
    fn from(s: PwNodeState<'_>) -> Self {
        match s {
            PwNodeState::Error(e) => NodeState::Error(e.to_string()),
            PwNodeState::Creating => NodeState::Creating,
            PwNodeState::Suspended => NodeState::Suspended,
            PwNodeState::Idle => NodeState::Idle,
            PwNodeState::Running => NodeState::Running,
        }
    }
}

impl fmt::Display for NodeState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            NodeState::Error(e) => write!(f, "error ({e})"),
            NodeState::Creating => f.write_str("creating"),
            NodeState::Suspended => f.write_str("suspended"),
            NodeState::Idle => f.write_str("idle"),
            NodeState::Running => f.write_str("running"),
        }
    }
}

/// A node's negotiated audio format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NodeFormat {
    pub format: String,
    pub rate: u32,
    pub channels: u32,
}

impl fmt::Display for NodeFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} {}ch {} Hz",
            self.format.to_lowercase(),
            self.channels,
            self.rate
        )
    }
}

/// A node as the registry lists it: its id, `object.serial` (the number `pactl` shows as
/// a sink-input index) and properties.
#[derive(Debug)]
pub struct GlobalNode {
    pub id: u32,
    pub serial: Option<u64>,
    pub props: BTreeMap<String, String>,
    global: GlobalObject<PropertiesBox>,
}

impl GlobalNode {
    fn from_global(g: &GlobalObject<&DictRef>) -> Self {
        let props = g.props.map(dict_to_map).unwrap_or_default();
        let serial = props.get("object.serial").and_then(|s| s.parse().ok());
        Self {
            id: g.id,
            serial,
            props,
            global: g.to_owned(),
        }
    }

    pub fn name(&self) -> &str {
        self.props.get("node.name").map_or("", String::as_str)
    }

    pub fn media_class(&self) -> &str {
        self.props.get("media.class").map_or("", String::as_str)
    }

    /// The serial, or the id where PipeWire is too old to give one.
    pub fn serial_or_id(&self) -> u64 {
        self.serial.unwrap_or(u64::from(self.id))
    }
}

/// What a bound node reports: its state and properties from `info`, its channel volumes
/// from the `Props` param, its format from the `Format` param.
#[derive(Debug, Clone, Default)]
pub struct NodeFacts {
    pub state: Option<NodeState>,
    pub props: BTreeMap<String, String>,
    pub channel_volumes: Option<Vec<f32>>,
    pub format: Option<NodeFormat>,
}

/// A node bound through the registry, with its listener alive.
pub struct BoundNode {
    pub id: u32,
    pub serial: Option<u64>,
    pub props: BTreeMap<String, String>,
    node: Node,
    facts: Rc<RefCell<NodeFacts>>,
    _listener: NodeListener,
}

impl BoundNode {
    pub fn facts(&self) -> NodeFacts {
        self.facts.borrow().clone()
    }

    pub fn name(&self) -> &str {
        self.props.get("node.name").map_or("", String::as_str)
    }

    pub fn serial_or_id(&self) -> u64 {
        self.serial.unwrap_or(u64::from(self.id))
    }

    /// Set the node's `channelVolumes` (linear, one per channel). Takes effect after the
    /// session's next round trip; a refusal arrives as a core error.
    pub fn set_channel_volumes(&self, volumes: &[f32]) -> Result<(), PwError> {
        let bytes = channel_volumes_pod(volumes)?;
        let pod = Pod::from_bytes(&bytes).ok_or_else(|| PwError::Call("bad Props pod".into()))?;
        self.node.set_param(ParamType::Props, 0, pod);
        Ok(())
    }
}

/// A connection to PipeWire on the calling thread: a main loop, a context and a core.
pub struct Session {
    mainloop: MainLoopRc,
    _context: ContextRc,
    core: CoreRc,
    errors: Rc<RefCell<Vec<String>>>,
    _core_listener: CoreListener,
    timeout: Duration,
}

impl Session {
    /// Connect as `app_name`. Fails when PipeWire is not running for this user.
    pub fn connect(app_name: &str) -> Result<Session, PwError> {
        init_once();
        let mainloop = MainLoopRc::new(None).map_err(|e| PwError::Connect(e.to_string()))?;
        let context =
            ContextRc::new(&mainloop, None).map_err(|e| PwError::Connect(e.to_string()))?;
        let props = properties! { *keys::APP_NAME => app_name };
        let core = context
            .connect_rc(Some(props))
            .map_err(|e| PwError::Connect(format!("{e} (is PipeWire running?)")))?;
        let errors = Rc::new(RefCell::new(Vec::new()));
        let core_listener = core
            .add_listener_local()
            .error({
                let errors = errors.clone();
                move |id, _seq, res, message| {
                    errors
                        .borrow_mut()
                        .push(format!("object {id}: {message} ({res})"));
                }
            })
            .register();
        Ok(Session {
            mainloop,
            _context: context,
            core,
            errors,
            _core_listener: core_listener,
            timeout: Duration::from_secs(5),
        })
    }

    pub fn core(&self) -> &CoreRc {
        &self.core
    }

    pub fn mainloop(&self) -> &MainLoopRc {
        &self.mainloop
    }

    /// A sync fence: returns once every method sent so far has had its events handled.
    pub fn roundtrip(&self) -> Result<(), PwError> {
        let pending = self.core.sync(0)?;
        let done = Rc::new(Cell::new(false));
        let timed_out = Rc::new(Cell::new(false));
        let _done_listener = self
            .core
            .add_listener_local()
            .done({
                let done = done.clone();
                let mainloop = self.mainloop.clone();
                move |id, seq| {
                    if id == PW_ID_CORE && seq == pending {
                        done.set(true);
                        mainloop.quit();
                    }
                }
            })
            .register();
        let timer = self.mainloop.loop_().add_timer({
            let timed_out = timed_out.clone();
            let mainloop = self.mainloop.clone();
            move |_| {
                timed_out.set(true);
                mainloop.quit();
            }
        });
        timer.update_timer(Some(self.timeout), None);
        while !done.get() && !timed_out.get() {
            self.mainloop.run();
        }
        if timed_out.get() {
            return Err(PwError::Timeout(self.timeout));
        }
        Ok(())
    }

    /// Errors PipeWire reported since the last call (a refused `set_param`, say).
    pub fn take_errors(&self) -> Vec<String> {
        std::mem::take(&mut *self.errors.borrow_mut())
    }

    /// Every node in the registry, with its properties.
    pub fn nodes(&self) -> Result<Vec<GlobalNode>, PwError> {
        // A fresh registry proxy replays every global, so this works any number of times.
        let registry = self.core.get_registry_rc()?;
        let found = Rc::new(RefCell::new(Vec::new()));
        let _listener = registry
            .add_listener_local()
            .global({
                let found = found.clone();
                move |global| {
                    if global.type_ == ObjectType::Node {
                        found.borrow_mut().push(GlobalNode::from_global(global));
                    }
                }
            })
            .register();
        self.roundtrip()?;
        Ok(found.take())
    }

    /// The node named `name` (`node.name`), if there is one.
    pub fn find_node(&self, name: &str) -> Result<Option<GlobalNode>, PwError> {
        Ok(self.nodes()?.into_iter().find(|n| n.name() == name))
    }

    /// Bind nodes and ask each for its `Props` and `Format`; returns after they answered.
    pub fn bind<'a>(
        &self,
        nodes: impl IntoIterator<Item = &'a GlobalNode>,
    ) -> Result<Vec<BoundNode>, PwError> {
        let registry = self.core.get_registry_rc()?;
        let mut bound = Vec::new();
        for g in nodes {
            let node: Node = registry.bind(&g.global)?;
            let facts = Rc::new(RefCell::new(NodeFacts::default()));
            let listener = node
                .add_listener_local()
                .info({
                    let facts = facts.clone();
                    move |info| {
                        let mut f = facts.borrow_mut();
                        f.state = Some(NodeState::from(info.state()));
                        if let Some(p) = info.props() {
                            f.props = dict_to_map(p);
                        }
                    }
                })
                .param({
                    let facts = facts.clone();
                    move |_seq, ty, _index, _next, pod| {
                        let Some(pod) = pod else { return };
                        match ty {
                            ParamType::Props => {
                                if let Some(v) = parse_channel_volumes(pod) {
                                    facts.borrow_mut().channel_volumes = Some(v);
                                }
                            }
                            ParamType::Format => {
                                if let Some(f) = parse_format(pod) {
                                    facts.borrow_mut().format = Some(f);
                                }
                            }
                            _ => {}
                        }
                    }
                })
                .register();
            node.enum_params(0, Some(ParamType::Props), 0, u32::MAX);
            node.enum_params(0, Some(ParamType::Format), 0, u32::MAX);
            bound.push(BoundNode {
                id: g.id,
                serial: g.serial,
                props: g.props.clone(),
                node,
                facts,
                _listener: listener,
            });
        }
        self.roundtrip()?;
        Ok(bound)
    }
}

fn dict_to_map(d: &DictRef) -> BTreeMap<String, String> {
    d.iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// The `EnumFormat` pod for s16 at `rate`, mono (`MONO` position, which PipeWire mixes
/// down from or up to whatever the device has).
pub fn s16_mono_format_pod(rate: u32) -> Vec<u8> {
    let mut info = AudioInfoRaw::new();
    info.set_format(AudioFormat::S16LE);
    info.set_rate(rate);
    info.set_channels(1);
    let mut position = [0u32; spa_sys::SPA_AUDIO_MAX_CHANNELS as usize];
    position[0] = spa_sys::SPA_AUDIO_CHANNEL_MONO;
    info.set_position(position);
    let obj = Object {
        type_: spa_sys::SPA_TYPE_OBJECT_Format,
        id: spa_sys::SPA_PARAM_EnumFormat,
        properties: info.into(),
    };
    PodSerializer::serialize(Cursor::new(Vec::new()), &Value::Object(obj))
        .expect("a format pod serializes")
        .0
        .into_inner()
}

/// A `Props` pod carrying `channelVolumes`.
pub fn channel_volumes_pod(volumes: &[f32]) -> Result<Vec<u8>, PwError> {
    let obj = Object {
        type_: spa_sys::SPA_TYPE_OBJECT_Props,
        id: spa_sys::SPA_PARAM_Props,
        properties: vec![Property::new(
            spa_sys::SPA_PROP_channelVolumes,
            Value::ValueArray(ValueArray::Float(volumes.to_vec())),
        )],
    };
    PodSerializer::serialize(Cursor::new(Vec::new()), &Value::Object(obj))
        .map(|(cursor, _)| cursor.into_inner())
        .map_err(|e| PwError::Call(format!("Props pod: {e:?}")))
}

/// `channelVolumes` out of a `Props` pod.
pub fn parse_channel_volumes(pod: &Pod) -> Option<Vec<f32>> {
    let (_, value) = PodDeserializer::deserialize_from::<Value>(pod.as_bytes()).ok()?;
    let Value::Object(obj) = value else {
        return None;
    };
    obj.properties
        .into_iter()
        .find(|p| p.key == spa_sys::SPA_PROP_channelVolumes)
        .and_then(|p| match p.value {
            Value::ValueArray(ValueArray::Float(v)) => Some(v),
            _ => None,
        })
}

/// The audio format out of a `Format` pod.
pub fn parse_format(pod: &Pod) -> Option<NodeFormat> {
    let mut info = AudioInfoRaw::new();
    info.parse(pod).ok()?;
    let format = format!("{:?}", info.format());
    Some(NodeFormat {
        format: format.rsplit("::").next().unwrap_or(&format).to_string(),
        rate: info.rate(),
        channels: info.channels(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn channel_volumes_pod_round_trips() {
        let bytes = channel_volumes_pod(&[0.25, 0.5]).unwrap();
        let pod = Pod::from_bytes(&bytes).unwrap();
        assert_eq!(parse_channel_volumes(pod), Some(vec![0.25, 0.5]));
        let empty = channel_volumes_pod(&[]).unwrap();
        assert_eq!(
            parse_channel_volumes(Pod::from_bytes(&empty).unwrap()),
            Some(vec![])
        );
    }

    #[test]
    fn format_pod_parses_back() {
        let bytes = s16_mono_format_pod(16_000);
        let pod = Pod::from_bytes(&bytes).unwrap();
        let f = parse_format(pod).unwrap();
        assert_eq!(
            f,
            NodeFormat {
                format: "S16LE".into(),
                rate: 16_000,
                channels: 1
            }
        );
        assert_eq!(f.to_string(), "s16le 1ch 16000 Hz");
        assert_eq!(
            parse_channel_volumes(pod),
            None,
            "a Format pod has no volumes"
        );
    }

    #[test]
    fn node_state_reads() {
        assert_eq!(NodeState::Running.to_string(), "running");
        assert_eq!(NodeState::Error("x".into()).to_string(), "error (x)");
    }
}
