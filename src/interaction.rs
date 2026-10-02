use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use crate::runtime::model::WidgetKey;

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Other(u32),
}

impl MouseButton {
    pub fn from_linux_code(code: u32) -> Self {
        // Standard Linux evdev codes:
        // BTN_LEFT = 0x110 (272)
        // BTN_RIGHT = 0x111 (273)
        // BTN_MIDDLE = 0x112 (274)
        match code {
            272 => Self::Left,
            273 => Self::Right,
            274 => Self::Middle,
            other => Self::Other(other),
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum MouseAxis {
    ScrollUp,
    ScrollDown,
    ScrollLeft,
    ScrollRight,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum InteractionEvent {
    Click(MouseButton),
    Scroll(MouseAxis),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct CallId(pub u32);

#[derive(Clone, Debug, PartialEq)]
pub enum CallAction {
    Command {
        program: String,
        args: Vec<String>,
        refresh_target: Option<String>,
    },
    RefreshWidget(String),
}

#[derive(Clone, Debug, PartialEq)]
pub struct CallBinding {
    pub id: CallId,
    pub source_widget: WidgetKey,
    pub target_pattern: Option<String>,
    pub action: CallAction,
    pub debounce: Duration,
    pub linked_provider: Option<WidgetKey>,
}

impl CallBinding {
    pub fn resolve_action(&self, target_val: Option<&str>) -> CallAction {
        let val = target_val.unwrap_or("");
        match &self.action {
            CallAction::Command {
                program,
                args,
                refresh_target,
            } => {
                let resolved_program = program.replace("${target}", val);
                let resolved_args = args
                    .iter()
                    .map(|arg| arg.replace("${target}", val))
                    .collect();
                let resolved_refresh = refresh_target.as_ref().map(|r| r.replace("${target}", val));
                CallAction::Command {
                    program: resolved_program,
                    args: resolved_args,
                    refresh_target: resolved_refresh,
                }
            }
            CallAction::RefreshWidget(target) => {
                CallAction::RefreshWidget(target.replace("${target}", val))
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct CallState {
    pub last_invoked: Instant,
}

type InteractionRoutes = HashMap<(WidgetKey, InteractionEvent), Vec<(Option<String>, CallId)>>;

#[derive(Default)]
pub struct InteractionRegistry {
    pub bindings: HashMap<CallId, CallBinding>,
    pub routes: InteractionRoutes,
    pub states: HashMap<CallId, CallState>,
    next_id: u32,
}

impl InteractionRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register(
        &mut self,
        source_widget: WidgetKey,
        target_pattern: Option<String>,
        event: InteractionEvent,
        action: CallAction,
        debounce: Duration,
        linked_provider: Option<WidgetKey>,
    ) -> CallId {
        let id = CallId(self.next_id);
        self.next_id = self.next_id.saturating_add(1);

        let binding = CallBinding {
            id,
            source_widget,
            target_pattern: target_pattern.clone(),
            action,
            debounce,
            linked_provider,
        };
        self.bindings.insert(id, binding);
        self.routes
            .entry((source_widget, event))
            .or_default()
            .push((target_pattern, id));
        id
    }

    /// Looks up a call binding for a widget event, with support for sub-targets and wildcards.
    /// Returns the matched binding and the extracted target parameter (if any).
    pub fn lookup(
        &self,
        source_widget: WidgetKey,
        event: InteractionEvent,
        sub_target: Option<&str>,
    ) -> Option<(&CallBinding, Option<String>)> {
        let list = self.routes.get(&(source_widget, event))?;

        // 1. Exact sub-target match
        if let Some(target) = sub_target {
            for (pattern, id) in list {
                if let Some(pat) = pattern
                    && pat == target
                {
                    return self.bindings.get(id).map(|b| (b, Some(target.to_owned())));
                }
            }

            // 2. Wildcard match (e.g. pattern "ws:*" matches target "ws:2")
            for (pattern, id) in list {
                if let Some(pat) = pattern
                    && let Some(prefix) = pat.strip_suffix('*')
                    && target.starts_with(prefix)
                {
                    let matched_val = target[prefix.len()..].to_owned();
                    return self.bindings.get(id).map(|b| (b, Some(matched_val)));
                }
            }
        }

        // 3. Fallback to widget-level default (pattern is None)
        for (pattern, id) in list {
            if pattern.is_none() {
                return self
                    .bindings
                    .get(id)
                    .map(|b| (b, sub_target.map(str::to_owned)));
            }
        }

        None
    }

    /// Returns whether a widget/target has any mouse action, regardless of
    /// which button or scroll direction will eventually dispatch it.
    pub fn is_interactive(&self, source_widget: WidgetKey, sub_target: Option<&str>) -> bool {
        [
            InteractionEvent::Click(MouseButton::Left),
            InteractionEvent::Click(MouseButton::Right),
            InteractionEvent::Click(MouseButton::Middle),
            InteractionEvent::Scroll(MouseAxis::ScrollUp),
            InteractionEvent::Scroll(MouseAxis::ScrollDown),
            InteractionEvent::Scroll(MouseAxis::ScrollLeft),
            InteractionEvent::Scroll(MouseAxis::ScrollRight),
        ]
        .into_iter()
        .any(|event| self.lookup(source_widget, event, sub_target).is_some())
    }

    /// Checks if a call should be dispatched according to a debounce window.
    /// Returns true and updates state if allowed, or false if debounced.
    pub fn should_dispatch(&mut self, id: CallId, debounce: Duration) -> bool {
        let now = Instant::now();
        if let Some(state) = self.states.get_mut(&id) {
            if now.duration_since(state.last_invoked) < debounce {
                return false;
            }
            state.last_invoked = now;
            true
        } else {
            self.states.insert(id, CallState { last_invoked: now });
            true
        }
    }

    pub fn clear(&mut self) {
        self.bindings.clear();
        self.routes.clear();
        self.states.clear();
        self.next_id = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mouse_button_from_linux_codes() {
        assert_eq!(MouseButton::from_linux_code(272), MouseButton::Left);
        assert_eq!(MouseButton::from_linux_code(273), MouseButton::Right);
        assert_eq!(MouseButton::from_linux_code(274), MouseButton::Middle);
        assert_eq!(MouseButton::from_linux_code(275), MouseButton::Other(275));
    }

    #[test]
    fn registers_and_routes_interactions() {
        let mut registry = InteractionRegistry::new();
        let key = WidgetKey {
            bar_id: 1,
            widget_index: 2,
        };
        let action = CallAction::Command {
            program: "pactl".into(),
            args: vec![
                "set-sink-mute".into(),
                "@DEFAULT_SINK@".into(),
                "toggle".into(),
            ],
            refresh_target: None,
        };

        let call_id = registry.register(
            key,
            None,
            InteractionEvent::Click(MouseButton::Left),
            action.clone(),
            Duration::from_millis(50),
            Some(key),
        );

        assert_eq!(call_id, CallId(0));
        let (binding, target_val) = registry
            .lookup(key, InteractionEvent::Click(MouseButton::Left), None)
            .expect("binding found");
        assert_eq!(binding.id, CallId(0));
        assert_eq!(binding.source_widget, key);
        assert_eq!(binding.action, action);
        assert_eq!(binding.linked_provider, Some(key));
        assert_eq!(target_val, None);

        // Different button should not route
        assert!(
            registry
                .lookup(key, InteractionEvent::Click(MouseButton::Right), None)
                .is_none()
        );

        let scroll_action = CallAction::Command {
            program: "wpctl".into(),
            args: vec!["set-volume".into(), "5%+".into()],
            refresh_target: Some("volume".into()),
        };
        let scroll_id = registry.register(
            key,
            None,
            InteractionEvent::Scroll(MouseAxis::ScrollUp),
            scroll_action.clone(),
            Duration::from_millis(150),
            None,
        );
        assert_eq!(scroll_id, CallId(1));
        let (scroll_binding, _) = registry
            .lookup(key, InteractionEvent::Scroll(MouseAxis::ScrollUp), None)
            .expect("scroll binding found");
        assert_eq!(scroll_binding.action, scroll_action);
        assert_eq!(scroll_binding.debounce, Duration::from_millis(150));

        // Test sub-target and wildcard routing
        let ws_action = CallAction::Command {
            program: "hyprctl".into(),
            args: vec!["dispatch".into(), "workspace".into(), "${target}".into()],
            refresh_target: None,
        };
        registry.register(
            key,
            Some("ws:*".into()),
            InteractionEvent::Click(MouseButton::Left),
            ws_action,
            Duration::from_millis(50),
            None,
        );

        // Click with sub_target "ws:3" matches "ws:*" with parameter "3"
        let (ws_binding, target_val) = registry
            .lookup(
                key,
                InteractionEvent::Click(MouseButton::Left),
                Some("ws:3"),
            )
            .expect("matched wildcard target");
        assert_eq!(target_val, Some("3".into()));
        assert!(registry.is_interactive(key, Some("ws:3")));
        assert!(!registry.is_interactive(
            WidgetKey {
                bar_id: 9,
                widget_index: 9,
            },
            None,
        ));

        let resolved = ws_binding.resolve_action(target_val.as_deref());
        match resolved {
            CallAction::Command { args, .. } => {
                assert_eq!(args, vec!["dispatch", "workspace", "3"]);
            }
            _ => panic!("expected command action"),
        }
    }

    #[test]
    fn should_dispatch_rate_limits_within_debounce() {
        let mut registry = InteractionRegistry::new();
        let id = CallId(1);
        let debounce = Duration::from_millis(50);

        assert!(registry.should_dispatch(id, debounce));
        // Immediate next call is debounced
        assert!(!registry.should_dispatch(id, debounce));
    }
}
