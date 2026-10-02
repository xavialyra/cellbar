use std::{cell::RefCell, collections::HashMap, rc::Rc, thread, thread::JoinHandle};

use calloop::channel::Sender;
use pipewire::{
    self as pw,
    metadata::{Metadata, MetadataListener},
    node::{Node, NodeListener},
    registry::{Listener as RegistryListener, RegistryRc},
    types::ObjectType,
};
use serde_json::json;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    DefaultSink,
    SinkVolume,
    DefaultSource,
    SourceVolume,
}

impl Event {
    pub fn name(self) -> &'static str {
        match self {
            Self::DefaultSink => "default-sink",
            Self::SinkVolume => "sink-volume",
            Self::DefaultSource => "default-source",
            Self::SourceVolume => "source-volume",
        }
    }
}

enum Command {
    Shutdown,
}

pub struct Dispatcher {
    commands: pw::channel::Sender<Command>,
    thread: Option<JoinHandle<()>>,
}

pub fn start(event_sender: Sender<Event>) -> Dispatcher {
    let (commands, receiver) = pw::channel::channel();
    let thread = thread::Builder::new()
        .name("cellbar-pipewire".to_owned())
        .stack_size(128 * 1024)
        .spawn(move || run(receiver, event_sender))
        .expect("failed to start PipeWire dispatcher thread");
    Dispatcher {
        commands,
        thread: Some(thread),
    }
}

impl Drop for Dispatcher {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct State {
    nodes: HashMap<u32, AudioNode>,
    metadata: HashMap<u32, AudioMetadata>,
}

struct AudioNode {
    event: Event,
    _node: Node,
    _listener: NodeListener,
}

struct AudioMetadata {
    _metadata: Metadata,
    _listener: MetadataListener,
}

fn run(commands: pw::channel::Receiver<Command>, event_sender: Sender<Event>) {
    let main_loop = match pw::main_loop::MainLoopRc::new(None) {
        Ok(main_loop) => main_loop,
        Err(error) => {
            eprintln!("cellbar: cannot initialize PipeWire: {error}");
            return;
        }
    };
    let context = match pw::context::ContextRc::new(&main_loop, None) {
        Ok(context) => context,
        Err(error) => {
            eprintln!("cellbar: cannot create PipeWire context: {error}");
            return;
        }
    };
    let core = match context.connect_rc(None) {
        Ok(core) => core,
        Err(error) => {
            eprintln!("cellbar: cannot connect to PipeWire: {error}");
            return;
        }
    };
    let registry = match core.get_registry_rc() {
        Ok(registry) => registry,
        Err(error) => {
            eprintln!("cellbar: cannot get PipeWire registry: {error}");
            return;
        }
    };

    let shutdown_loop = main_loop.clone();
    let _commands = commands.attach(main_loop.loop_(), move |command| match command {
        Command::Shutdown => shutdown_loop.quit(),
    });

    let notify = Rc::new(move |event| {
        let _ = event_sender.send(event);
    });
    let state = Rc::new(RefCell::new(State {
        nodes: HashMap::new(),
        metadata: HashMap::new(),
    }));
    let _registry_listener = register_registry_listener(registry, Rc::clone(&state), notify);

    main_loop.run();
}

fn register_registry_listener(
    registry: RegistryRc,
    state: Rc<RefCell<State>>,
    notify: Rc<dyn Fn(Event)>,
) -> RegistryListener {
    let listener_registry = registry.clone();
    let listener_state = Rc::clone(&state);
    let listener_notify = Rc::clone(&notify);
    registry
        .add_listener_local()
        .global(move |object| match object.type_ {
            ObjectType::Node => {
                let Some(event) = object.props.and_then(audio_node_event) else {
                    return;
                };
                let Ok(node) = listener_registry.bind::<Node, _>(object) else {
                    return;
                };
                let node_notify = Rc::clone(&listener_notify);
                let listener = node
                    .add_listener_local()
                    .param(move |_, parameter, _, _, _| {
                        if parameter == pw::spa::param::ParamType::Props {
                            node_notify(event);
                        }
                    })
                    .register();
                node.subscribe_params(&[pw::spa::param::ParamType::Props]);
                listener_state.borrow_mut().nodes.insert(
                    object.id,
                    AudioNode {
                        event,
                        _node: node,
                        _listener: listener,
                    },
                );
                listener_notify(event);
            }
            ObjectType::Metadata => {
                let Ok(metadata) = listener_registry.bind::<Metadata, _>(object) else {
                    return;
                };
                let metadata_notify = Rc::clone(&listener_notify);
                let listener = metadata
                    .add_listener_local()
                    .property(move |_, key, _, _| {
                        if let Some(event) = metadata_event(key) {
                            metadata_notify(event);
                        }
                        0
                    })
                    .register();
                listener_state.borrow_mut().metadata.insert(
                    object.id,
                    AudioMetadata {
                        _metadata: metadata,
                        _listener: listener,
                    },
                );
            }
            _ => {}
        })
        .global_remove(move |id| {
            let mut state = state.borrow_mut();
            let event = state.nodes.remove(&id).map(|node| node.event);
            let metadata_removed = state.metadata.remove(&id).is_some();
            drop(state);
            if let Some(event) = event {
                notify(event);
            }
            if metadata_removed {
                notify(Event::DefaultSink);
                notify(Event::DefaultSource);
            }
        })
        .register()
}

fn audio_node_event(props: &pw::spa::utils::dict::DictRef) -> Option<Event> {
    match props.get("media.class") {
        Some("Audio/Sink") => Some(Event::SinkVolume),
        Some("Audio/Source") => Some(Event::SourceVolume),
        _ => None,
    }
}

fn metadata_event(key: Option<&str>) -> Option<Event> {
    match key {
        Some("default.audio.sink") => Some(Event::DefaultSink),
        Some("default.audio.source") => Some(Event::DefaultSource),
        _ => None,
    }
}

pub fn subscription_matches(subscription: &str, event: Event) -> bool {
    match subscription {
        "volume" => matches!(event, Event::DefaultSink | Event::SinkVolume),
        _ => subscription == event.name(),
    }
}

pub fn format_event_context(subscription: &str, event: Event) -> String {
    json!({
        "version": 1,
        "source": "pipewire",
        "event": event.name(),
        "subscription": subscription,
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_audio_nodes_and_metadata() {
        assert_eq!(
            metadata_event(Some("default.audio.sink")),
            Some(Event::DefaultSink)
        );
        assert_eq!(
            metadata_event(Some("default.audio.source")),
            Some(Event::DefaultSource)
        );
        assert_eq!(metadata_event(Some("other")), None);
    }

    #[test]
    fn matches_aggregate_and_specific_subscriptions() {
        assert!(subscription_matches("volume", Event::DefaultSink));
        assert!(subscription_matches("volume", Event::SinkVolume));
        assert!(!subscription_matches("volume", Event::SourceVolume));
        assert!(subscription_matches("source-volume", Event::SourceVolume));
    }

    #[test]
    fn formats_event_context() {
        assert_eq!(
            format_event_context("volume", Event::DefaultSink),
            r#"{"event":"default-sink","source":"pipewire","subscription":"volume","version":1}"#
        );
    }
}
