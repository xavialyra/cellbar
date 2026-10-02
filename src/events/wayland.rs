use std::collections::{HashMap, HashSet};

use smithay_client_toolkit::output::OutputState;
use wayland_client::backend::ObjectId;
use wayland_client::protocol::wl_output::WlOutput;
use wayland_protocols::ext::workspace::v1::client::{
    ext_workspace_group_handle_v1::ExtWorkspaceGroupHandleV1,
    ext_workspace_handle_v1::ExtWorkspaceHandleV1, ext_workspace_manager_v1::ExtWorkspaceManagerV1,
};
use wayland_protocols_wlr::foreign_toplevel::v1::client::{
    zwlr_foreign_toplevel_handle_v1::ZwlrForeignToplevelHandleV1,
    zwlr_foreign_toplevel_manager_v1::ZwlrForeignToplevelManagerV1,
};

#[derive(Default, Clone, Debug, PartialEq, Eq)]
pub struct ToplevelWindow {
    pub title: String,
    pub app_id: String,
    pub activated: bool,
}

pub struct ToplevelTracker {
    pub manager: ZwlrForeignToplevelManagerV1,
    pub windows: HashMap<ObjectId, ToplevelWindow>,
    pub handles: HashMap<ObjectId, ZwlrForeignToplevelHandleV1>,
    pub active_window: Option<ObjectId>,
}

impl ToplevelTracker {
    pub fn new(manager: ZwlrForeignToplevelManagerV1) -> Self {
        Self {
            manager,
            windows: HashMap::new(),
            handles: HashMap::new(),
            active_window: None,
        }
    }

    pub fn stop(&mut self) {
        self.manager.stop();
        for (_, handle) in self.handles.drain() {
            handle.destroy();
        }
        self.windows.clear();
        self.active_window = None;
    }
}

#[derive(Default, Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceData {
    pub id: Option<String>,
    pub name: String,
    pub coordinates: Vec<u32>,
    pub active: bool,
    pub urgent: bool,
    pub hidden: bool,
}

#[derive(Default, Clone, Debug)]
pub struct WorkspaceGroupData {
    pub outputs: Vec<WlOutput>,
    pub workspaces: HashSet<ObjectId>,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WorkspaceInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    pub name: String,
    pub active: bool,
    pub urgent: bool,
    pub hidden: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub outputs: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub coordinates: Vec<u32>,
}

pub struct WorkspaceTracker {
    pub manager: ExtWorkspaceManagerV1,
    pub workspaces: HashMap<ObjectId, WorkspaceData>,
    pub workspace_handles: HashMap<ObjectId, ExtWorkspaceHandleV1>,
    pub groups: HashMap<ObjectId, WorkspaceGroupData>,
    pub group_handles: HashMap<ObjectId, ExtWorkspaceGroupHandleV1>,
}

impl WorkspaceTracker {
    pub fn new(manager: ExtWorkspaceManagerV1) -> Self {
        Self {
            manager,
            workspaces: HashMap::new(),
            workspace_handles: HashMap::new(),
            groups: HashMap::new(),
            group_handles: HashMap::new(),
        }
    }

    pub fn stop(&mut self) {
        self.manager.stop();
        for (_, handle) in self.group_handles.drain() {
            handle.destroy();
        }
        for (_, handle) in self.workspace_handles.drain() {
            handle.destroy();
        }
        self.workspaces.clear();
        self.groups.clear();
    }

    pub fn collect_workspaces(&self, output_state: &OutputState) -> Vec<WorkspaceInfo> {
        let mut list: Vec<WorkspaceInfo> = self
            .workspaces
            .iter()
            .map(|(ws_id, ws)| {
                let mut outputs = Vec::new();
                for group in self.groups.values() {
                    if group.workspaces.contains(ws_id) {
                        for output in &group.outputs {
                            if let Some(name) = output_state.info(output).and_then(|info| info.name)
                                && !outputs.contains(&name)
                            {
                                outputs.push(name);
                            }
                        }
                    }
                }
                outputs.sort();

                WorkspaceInfo {
                    id: ws.id.clone(),
                    name: ws.name.clone(),
                    active: ws.active,
                    urgent: ws.urgent,
                    hidden: ws.hidden,
                    outputs,
                    coordinates: ws.coordinates.clone(),
                }
            })
            .collect();

        list.sort_by(|a, b| {
            if !a.coordinates.is_empty() && !b.coordinates.is_empty() {
                return a.coordinates.cmp(&b.coordinates);
            }
            match (a.name.parse::<u32>(), b.name.parse::<u32>()) {
                (Ok(an), Ok(bn)) => an.cmp(&bn),
                (Ok(_), Err(_)) => std::cmp::Ordering::Less,
                (Err(_), Ok(_)) => std::cmp::Ordering::Greater,
                (Err(_), Err(_)) => a.name.cmp(&b.name),
            }
        });

        list
    }
}

pub fn format_toplevel_context(
    subscription_id: &str,
    title: &str,
    app_id: &str,
    activated: bool,
) -> String {
    serde_json::json!({
        "version": 1,
        "subscription": subscription_id,
        "source": "wayland",
        "event": "toplevel",
        "message": {
            "title": title,
            "app_id": app_id,
            "activated": activated,
        }
    })
    .to_string()
}

pub fn format_workspace_context(subscription_id: &str, workspaces: &[WorkspaceInfo]) -> String {
    let active: Vec<&str> = workspaces
        .iter()
        .filter(|w| w.active)
        .map(|w| w.name.as_str())
        .collect();
    let current = active.first().copied();

    serde_json::json!({
        "version": 1,
        "subscription": subscription_id,
        "source": "wayland",
        "event": "workspace",
        "message": {
            "current": current,
            "active": active,
            "workspaces": workspaces,
        }
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_toplevel_context_json() {
        let json_str =
            format_toplevel_context("active-win", "termway — Cargo.toml", "Alacritty", true);
        let parsed: serde_json::Value = serde_json::from_str(&json_str).expect("valid json");
        assert_eq!(parsed["version"], 1);
        assert_eq!(parsed["subscription"], "active-win");
        assert_eq!(parsed["source"], "wayland");
        assert_eq!(parsed["event"], "toplevel");
        assert_eq!(parsed["message"]["title"], "termway — Cargo.toml");
        assert_eq!(parsed["message"]["app_id"], "Alacritty");
        assert_eq!(parsed["message"]["activated"], true);
    }

    #[test]
    fn formats_workspace_context_json() {
        let workspaces = vec![
            WorkspaceInfo {
                id: Some("ws-1".into()),
                name: "1".into(),
                active: true,
                urgent: false,
                hidden: false,
                outputs: vec!["DP-1".into()],
                coordinates: vec![1],
            },
            WorkspaceInfo {
                id: Some("ws-2".into()),
                name: "2".into(),
                active: false,
                urgent: true,
                hidden: false,
                outputs: vec!["DP-1".into()],
                coordinates: vec![2],
            },
        ];
        let json_str = format_workspace_context("my-ws", &workspaces);
        let parsed: serde_json::Value = serde_json::from_str(&json_str).expect("valid json");
        assert_eq!(parsed["version"], 1);
        assert_eq!(parsed["subscription"], "my-ws");
        assert_eq!(parsed["source"], "wayland");
        assert_eq!(parsed["event"], "workspace");
        assert_eq!(parsed["message"]["current"], "1");
        assert_eq!(parsed["message"]["active"], serde_json::json!(["1"]));
        assert_eq!(parsed["message"]["workspaces"].as_array().unwrap().len(), 2);
        assert_eq!(parsed["message"]["workspaces"][0]["name"], "1");
        assert_eq!(parsed["message"]["workspaces"][0]["active"], true);
        assert_eq!(parsed["message"]["workspaces"][1]["name"], "2");
        assert_eq!(parsed["message"]["workspaces"][1]["urgent"], true);
    }
}
