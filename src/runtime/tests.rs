use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use super::{
    MAX_TICK_DELAY,
    bar::Bar,
    model::*,
    next_tick_delay_for,
    sources::SourceMessage,
};
use crate::{
    config::{Config, EventSourceKind, ProviderConfig, Trigger},
    interaction::{CallAction, InteractionEvent, InteractionRegistry, MouseButton},
};

#[test]
fn maps_expression_provider_triggers() {
    let mut config: Config = toml::from_str(
        r##"
[bar.main]
theme = "theme.toml"
left = ["network"]
"##,
    )
    .expect("valid layout configuration");
    config.providers.insert(
        "network".into(),
        ProviderConfig {
            program: None,
            expression: Some("settings.icon + context.event".into()),
            args: vec![],
            timeout: None,
            triggers: vec![Trigger {
                id: "route-state".into(),
                kind: EventSourceKind::Netlink,
                event: "route".into(),
                bus: None,
                match_rule: None,
                subsystem: None,
                action: None,
                source: None,
            }],
            on_activate: true,
            every: Some(Duration::from_secs(30)),
            debounce: Duration::from_millis(50),
            settings: BTreeMap::from([(
                "icon".into(),
                toml::Value::String("net".into()),
            )]),
            actions: BTreeMap::new(),
        },
    );
    let definitions = WidgetDefinition::from_config(&config);
    let WidgetDefinitionContent::Provider(ProviderDefinition::Expression(expression)) =
        &definitions[0].content
    else {
        panic!("expected expression provider definition");
    };
    assert_eq!(expression.triggers.len(), 1);
    assert_eq!(expression.triggers[0].kind, EventSourceKind::Netlink);
    assert_eq!(expression.settings["icon"], "net");
}

#[test]
fn maps_command_provider_definition() {
    let mut config: Config = toml::from_str(
        r##"
[bar.main]
theme = "themes/theme.toml"
left = ["command"]
"##,
    )
    .expect("valid layout configuration");
    config.config_dir = PathBuf::from("/tmp/cellbar-config");
    config.providers.insert(
        "command".into(),
        ProviderConfig {
            program: Some("run.sh".into()),
            expression: None,
            args: vec!["${setting.icon}".into(), "${context}".into()],
            timeout: None,
            triggers: vec![],
            on_activate: true,
            every: None,
            debounce: Duration::from_millis(50),
            settings: BTreeMap::from([(
                "icon".into(),
                toml::Value::String("!".into()),
            )]),
            actions: BTreeMap::new(),
        },
    );
    let definitions = WidgetDefinition::from_config(&config);
    let WidgetDefinitionContent::Provider(ProviderDefinition::Command(command)) =
        &definitions[0].content
    else {
        panic!("expected command provider definition");
    };
    assert_eq!(command.command, ["run.sh", "!", "${context}"]);
    assert_eq!(
        command.base,
        Path::new("/tmp/cellbar-config/components/command")
    );
}

#[test]
fn decodes_custom_source_envelope() {
    let message: SourceMessage =
        serde_json::from_str(r#"{"version":1,"event":"changed","data":{"title":"Song"}}"#)
            .expect("valid source message");
    assert_eq!(message.event, "changed");
    assert_eq!(message.data["title"], "Song");
}

#[test]
fn schedules_the_nearest_runtime_deadline() {
    let now = Instant::now();
    assert_eq!(next_tick_delay_for(now, []), MAX_TICK_DELAY);
    assert_eq!(
        next_tick_delay_for(now, [now + DEFAULT_TICK_DELAY]),
        DEFAULT_TICK_DELAY
    );
    assert_eq!(
        next_tick_delay_for(now, [now + Duration::from_millis(500)]),
        Duration::from_millis(500)
    );
    assert_eq!(
        next_tick_delay_for(now, [now + MAX_TICK_DELAY + Duration::from_secs(1)]),
        MAX_TICK_DELAY
    );
}

#[test]
fn test_workspaces_memory_cpu_click_interactions() {
    let config_path = Path::new("examples/flat/config.toml");
    let config = Config::load(config_path).expect("valid config");
    let definitions = WidgetDefinition::from_config(&config);
    let mut widgets = WidgetState::instantiate(&definitions);

    // Locate indices for workspaces, memory, and cpu widgets
    let ws_index = widgets.iter().position(|w| w.id == "workspaces").unwrap();
    let mem_index = widgets.iter().position(|w| w.id == "memory").unwrap();
    let cpu_index = widgets.iter().position(|w| w.id == "cpu").unwrap();

    // Mock provider contents
    // workspaces mocks actual run.sh output (using explicit [ ] spacer)
    if let WidgetContent::Provider { state, .. } = &mut widgets[ws_index].content {
        let mut provider = state.borrow_mut();
        match &mut *provider {
            ProviderState::Command(state) => {
                state.value = "#ws:1{ [1] }[ ]#ws:2{ [[2]](@accent) }".into();
                state.rich = Some(crate::markup::parse_markup(&state.value, Path::new(".")).unwrap());
            }
            ProviderState::Expression(state) => {
                state.value = "#ws:1{ [1] }[ ]#ws:2{ [[2]](@accent) }".into();
                state.rich = Some(crate::markup::parse_markup(&state.value, Path::new(".")).unwrap());
            }
        }
    }

    // Mock memory output
    if let WidgetContent::Provider { state, .. } = &mut widgets[mem_index].content {
        let mut provider = state.borrow_mut();
        if let ProviderState::Expression(state) = &mut *provider {
            state.value = "[ 4.2G](@memory)".into();
            state.rich = Some(crate::markup::parse_markup(&state.value, Path::new(".")).unwrap());
        }
    }

    // Mock cpu output
    if let WidgetContent::Provider { state, .. } = &mut widgets[cpu_index].content {
        let mut provider = state.borrow_mut();
        if let ProviderState::Expression(state) = &mut *provider {
            state.value = "[ 12%](@cpu)".into();
            state.rich = Some(crate::markup::parse_markup(&state.value, Path::new(".")).unwrap());
        }
    }

    let mut bar = unsafe {
        let mut b: std::mem::MaybeUninit<Bar> = std::mem::MaybeUninit::uninit();
        let p = b.as_mut_ptr();
        std::ptr::addr_of_mut!((*p).id).write(0);
        std::ptr::addr_of_mut!((*p).name).write(None);
        std::ptr::addr_of_mut!((*p).position).write(crate::config::BarPosition::Top);
        std::ptr::addr_of_mut!((*p).margin).write(crate::config::BarMargins::default());
        std::ptr::addr_of_mut!((*p).exclusive).write(true);
        std::ptr::addr_of_mut!((*p).theme).write(None);
        std::ptr::addr_of_mut!((*p).widget_definitions).write(definitions.clone());
        std::ptr::addr_of_mut!((*p).widgets).write(widgets);
        std::ptr::addr_of_mut!((*p).frame_scratch).write(crate::cell_frame::CellFrame::new(
            200,
            config.theme.default_style,
        ));
        std::ptr::addr_of_mut!((*p).static_spans).write(Vec::new());
        std::ptr::addr_of_mut!((*p).widget_ranges).write(Vec::new());
        b.assume_init()
    };

    let region_strings = bar.region_strings(&config.theme);
    crate::cell_frame::layout_rich_regions(
        &mut bar.frame_scratch,
        crate::cell_frame::RichRegions {
            left: &region_strings.left,
            center: &region_strings.center,
            right: &region_strings.right,
        },
    );

    // Populate interaction_registry
    let mut registry = InteractionRegistry::new();
    let default_debounce = Duration::from_millis(150);
    for (w_idx, _widget) in bar.widgets.iter().enumerate() {
        let key = WidgetKey {
            bar_id: 0,
            widget_index: w_idx,
        };
        if let Some(definition) = definitions.get(w_idx) {
            for (pattern, event, spec) in &definition.actions {
                let (mut command_args, refresh, debounce) = spec.to_command_args();
                if !command_args.is_empty() {
                    let program = command_args.remove(0);
                    let action = CallAction::Command {
                        program,
                        args: command_args,
                        refresh_target: refresh,
                    };
                    registry.register(
                        key,
                        pattern.clone(),
                        *event,
                        action,
                        debounce.unwrap_or(default_debounce),
                        None,
                    );
                }
            }
        }
    }

    // Verify interaction hits for workspaces, memory, and cpu
    let origin = config.theme.padding.horizontal;
    let cell_width = 8;
    let mut found_ws1 = false;
    let mut found_mem = false;
    let mut found_cpu = false;

    for col in 0..200 {
        if let Some((owner, target)) = bar.hit_test_widget(
            (origin + (col as u32) * cell_width + 2) as f64,
            origin,
            cell_width,
        ) {
            let key = WidgetKey {
                bar_id: 0,
                widget_index: owner,
            };
            let click_evt = InteractionEvent::Click(MouseButton::Left);
            if let Some((binding, target_val)) =
                registry.lookup(key, click_evt, target.as_deref())
            {
                let resolved = binding.resolve_action(target_val.as_deref());
                if owner == ws_index
                    && target.as_deref() == Some("ws:1")
                    && let CallAction::Command { args, .. } = &resolved
                    && args.iter().any(|arg| arg.contains("view,1"))
                {
                    found_ws1 = true;
                }
                if owner == mem_index
                    && let CallAction::Command { args, .. } = &resolved
                    && args.iter().any(|arg| arg.contains("filter mem"))
                {
                    found_mem = true;
                }
                if owner == cpu_index
                    && let CallAction::Command { args, .. } = &resolved
                    && args.iter().any(|arg| arg.contains("filter cpu"))
                {
                    found_cpu = true;
                }
            }
        }
    }

    assert!(
        found_ws1,
        "workspaces ws:1 click interaction should be resolved"
    );
    assert!(found_mem, "memory click interaction should be resolved");
    assert!(found_cpu, "cpu click interaction should be resolved");

    assert_eq!(bar.widgets[ws_index].text(), "#ws:1{ [1] }[ ]#ws:2{ [[2]](@accent) }");
    bar.widgets[ws_index].pushed_markup =
        Some(crate::markup::parse_markup("[hh]", Path::new(".")).unwrap());
    assert_eq!(bar.widgets[ws_index].text(), "hh");
    assert!(matches!(
        bar.widgets[ws_index].content,
        WidgetContent::Provider { .. }
    ));
    bar.widgets[ws_index].pushed_markup = None;
    assert_eq!(bar.widgets[ws_index].text(), "#ws:1{ [1] }[ ]#ws:2{ [[2]](@accent) }");

    std::mem::forget(bar);
}

#[test]
fn test_multi_bar_shares_provider_instances() {
    use std::rc::Rc;
    use crate::runtime::model::{ProcessDefinition, ProviderDefinition, WidgetDefinition, WidgetDefinitionContent, WidgetContent, ProviderState};

    let tray_def = WidgetDefinition {
        id: "tray".into(),
        region: crate::config::Region::Right,
        min_width: None,
        align: crate::config::DisplayAlign::Left,
        max_width: None,
        content: WidgetDefinitionContent::Provider(ProviderDefinition::Command(ProcessDefinition {
            id: "tray".into(),
            command: vec!["./cellbar-tray".into()],
            base: PathBuf::from("."),
            timeout: None,
            source_interval: None,
            on_activate: true,
            every: None,
            debounce: Duration::from_millis(50),
            triggers: Vec::new(),
        })),
        actions: Vec::new(),
    };

    let ws_def = WidgetDefinition {
        id: "workspaces".into(),
        region: crate::config::Region::Left,
        min_width: None,
        align: crate::config::DisplayAlign::Left,
        max_width: None,
        content: WidgetDefinitionContent::Provider(ProviderDefinition::Command(ProcessDefinition {
            id: "workspaces".into(),
            command: vec!["./ws.sh".into(), "${output}".into()],
            base: PathBuf::from("."),
            timeout: None,
            source_interval: None,
            on_activate: true,
            every: None,
            debounce: Duration::from_millis(50),
            triggers: Vec::new(),
        })),
        actions: Vec::new(),
    };

    let defs = vec![tray_def, ws_def];

    let mut runtime = unsafe {
        let mut r: std::mem::MaybeUninit<crate::runtime::Runtime> = std::mem::MaybeUninit::uninit();
        let p = r.as_mut_ptr();
        std::ptr::addr_of_mut!((*p).providers).write(std::collections::HashMap::new());
        r.assume_init()
    };

    let bar1_widgets = runtime.instantiate_bar_widgets(&defs, "DP-1");
    let bar2_widgets = runtime.instantiate_bar_widgets(&defs, "DP-1");
    let bar3_widgets = runtime.instantiate_bar_widgets(&defs, "HDMI-A-1");

    // 1. Verify global widget (tray) shares the exact same Rc instance across ALL bars
    let (WidgetContent::Provider { state: tray1, .. }, WidgetContent::Provider { state: tray2, .. }, WidgetContent::Provider { state: tray3, .. }) =
        (&bar1_widgets[0].content, &bar2_widgets[0].content, &bar3_widgets[0].content) else { panic!() };

    assert!(Rc::ptr_eq(tray1, tray2), "tray on bar1 and bar2 must share same instance");
    assert!(Rc::ptr_eq(tray1, tray3), "tray across outputs must also share same instance");

    // Modifying tray through bar1 must immediately reflect on bar2 and bar3
    if let ProviderState::Command(proc) = &mut *tray1.borrow_mut() {
        proc.value = "[tray-icon]".into();
    }
    assert_eq!(bar1_widgets[0].text(), "[tray-icon]");
    assert_eq!(bar2_widgets[0].text(), "[tray-icon]");
    assert_eq!(bar3_widgets[0].text(), "[tray-icon]");

    // 2. Verify output-scoped widget (workspaces with ${output})
    let (WidgetContent::Provider { key: ws1_key, state: ws1 }, WidgetContent::Provider { key: ws2_key, state: ws2 }, WidgetContent::Provider { key: ws3_key, state: ws3 }) =
        (&bar1_widgets[1].content, &bar2_widgets[1].content, &bar3_widgets[1].content) else { panic!() };

    assert_eq!(ws1_key.output.as_deref(), Some("DP-1"));
    assert_eq!(ws2_key.output.as_deref(), Some("DP-1"));
    assert_eq!(ws3_key.output.as_deref(), Some("HDMI-A-1"));

    // bar1 and bar2 on DP-1 share the same workspaces instance
    assert!(Rc::ptr_eq(ws1, ws2), "workspaces on same output DP-1 must be shared");
    // bar3 on HDMI-A-1 has its own instance
    assert!(!Rc::ptr_eq(ws1, ws3), "workspaces on HDMI-A-1 must have separate instance");

    std::mem::forget(runtime);
}

#[test]
fn test_build_bar_specs_initially_hidden() {
    let text = r#"
show = ["active"]

[bar.active]
theme = "theme.toml"

[bar.drawer]
theme = "theme.toml"
"#;
    let config = Config::parse(text).expect("valid config");
    let specs = super::build_bar_specs(&config);
    assert_eq!(specs.len(), 2);
    let active_spec = specs.iter().find(|s| s.name.as_deref() == Some("active")).unwrap();
    let drawer_spec = specs.iter().find(|s| s.name.as_deref() == Some("drawer")).unwrap();
    assert!(!active_spec.initially_hidden);
    assert!(drawer_spec.initially_hidden);
}

#[test]
fn test_build_bar_specs_all_hidden_when_show_empty() {
    let text = r#"
show = []

[bar.bar-a]
theme = "theme.toml"

[bar.bar-b]
theme = "theme.toml"
"#;
    let config = Config::parse(text).expect("valid config");
    let specs = super::build_bar_specs(&config);
    assert_eq!(specs.len(), 2);
    assert!(specs.iter().all(|s| s.initially_hidden));
}

#[test]
fn test_build_bar_specs_all_visible_when_show_omitted() {
    let text = r#"
[bar.bar-a]
theme = "theme.toml"

[bar.bar-b]
theme = "theme.toml"
"#;
    let config = Config::parse(text).expect("valid config");
    let specs = super::build_bar_specs(&config);
    assert_eq!(specs.len(), 2);
    assert!(specs.iter().all(|s| !s.initially_hidden));
}

#[test]
fn test_dormant_bars_exclude_provider_keys() {
    let tray_def = WidgetDefinition {
        id: "tray".into(),
        region: crate::config::Region::Right,
        min_width: None,
        align: crate::config::DisplayAlign::Right,
        max_width: None,
        content: WidgetDefinitionContent::Provider(ProviderDefinition::Command(ProcessDefinition {
            id: "tray".into(),
            command: vec!["./tray.sh".into()],
            base: PathBuf::from("."),
            timeout: None,
            source_interval: None,
            on_activate: true,
            every: None,
            debounce: Duration::from_millis(50),
            triggers: Vec::new(),
        })),
        actions: Vec::new(),
    };
    let defs = vec![tray_def];

    let mut runtime = unsafe {
        let mut r: std::mem::MaybeUninit<crate::runtime::Runtime> = std::mem::MaybeUninit::uninit();
        let p = r.as_mut_ptr();
        std::ptr::addr_of_mut!((*p).bars).write(vec![]);
        std::ptr::addr_of_mut!((*p).providers).write(std::collections::HashMap::new());
        r.assume_init()
    };

    let widgets = runtime.instantiate_bar_widgets(&defs, "DP-1");

    let bar = unsafe {
        let mut b = std::mem::MaybeUninit::<Bar>::uninit();
        let p = b.as_mut_ptr();
        std::ptr::addr_of_mut!((*p).hidden).write(true);
        std::ptr::addr_of_mut!((*p).widgets).write(widgets);
        b.assume_init()
    };

    runtime.bars.push(bar);
    assert!(runtime.visible_provider_keys().is_empty());

    // When shown, the key appears in visible_provider_keys
    runtime.bars[0].hidden = false;
    assert_eq!(runtime.visible_provider_keys().len(), 1);

    if let Some(bar) = runtime.bars.pop() {
        std::mem::forget(bar);
    }
    std::mem::forget(runtime);
}

#[test]
fn test_control_list_is_visible_and_is_hidden() {
    let mut runtime = unsafe {
        let mut r: std::mem::MaybeUninit<crate::runtime::Runtime> = std::mem::MaybeUninit::uninit();
        let p = r.as_mut_ptr();
        std::ptr::addr_of_mut!((*p).bars).write(vec![]);
        std::ptr::addr_of_mut!((*p).bar_specs).write(vec![]);
        r.assume_init()
    };

    let bar_top = unsafe {
        let mut b = std::mem::MaybeUninit::<Bar>::uninit();
        let p = b.as_mut_ptr();
        std::ptr::addr_of_mut!((*p).name).write(Some("top".into()));
        std::ptr::addr_of_mut!((*p).output_name).write("DP-1".into());
        std::ptr::addr_of_mut!((*p).position).write(crate::config::BarPosition::Top);
        std::ptr::addr_of_mut!((*p).layer_level).write(crate::config::BarLayer::Top);
        std::ptr::addr_of_mut!((*p).hidden).write(false);
        b.assume_init()
    };

    let bar_hud = unsafe {
        let mut b = std::mem::MaybeUninit::<Bar>::uninit();
        let p = b.as_mut_ptr();
        std::ptr::addr_of_mut!((*p).name).write(Some("hud".into()));
        std::ptr::addr_of_mut!((*p).output_name).write("DP-1".into());
        std::ptr::addr_of_mut!((*p).position).write(crate::config::BarPosition::Bottom);
        std::ptr::addr_of_mut!((*p).layer_level).write(crate::config::BarLayer::Overlay);
        std::ptr::addr_of_mut!((*p).hidden).write(true);
        b.assume_init()
    };

    runtime.bars.push(bar_top);
    runtime.bars.push(bar_hud);

    let list_resp: serde_json::Value =
        serde_json::from_str(&runtime.handle_control_command(b"list")).unwrap();
    assert_eq!(list_resp["ok"], true);
    assert_eq!(list_resp["bars"].as_array().unwrap().len(), 2);

    let vis_top: serde_json::Value =
        serde_json::from_str(&runtime.handle_control_command(b"is-visible top")).unwrap();
    assert_eq!(vis_top["ok"], true);
    assert_eq!(vis_top["match"], true);

    let hid_top: serde_json::Value =
        serde_json::from_str(&runtime.handle_control_command(b"is-hidden top")).unwrap();
    assert_eq!(hid_top["ok"], true);
    assert_eq!(hid_top["match"], false);

    let vis_hud: serde_json::Value =
        serde_json::from_str(&runtime.handle_control_command(b"is-visible hud")).unwrap();
    assert_eq!(vis_hud["ok"], true);
    assert_eq!(vis_hud["match"], false);

    let hid_hud: serde_json::Value =
        serde_json::from_str(&runtime.handle_control_command(b"is-hidden hud")).unwrap();
    assert_eq!(hid_hud["ok"], true);
    assert_eq!(hid_hud["match"], true);

    let unknown: serde_json::Value =
        serde_json::from_str(&runtime.handle_control_command(b"is-visible missing")).unwrap();
    assert_eq!(unknown["ok"], false);

    while let Some(bar) = runtime.bars.pop() {
        std::mem::forget(bar);
    }
    std::mem::forget(runtime);
}
