use std::time::Instant;

use cursor_icon::CursorIcon;
use smithay_client_toolkit::{
    compositor::CompositorHandler,
    delegate_registry,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        Capability, SeatHandler, SeatState,
        pointer::{PointerEvent, PointerEventKind, PointerHandler, ThemeSpec},
    },
    shell::{
        WaylandSurface,
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
    },
    shm::{Shm, ShmHandler, slot::SlotPool},
};
use wayland_client::{
    Connection, Dispatch, Proxy, QueueHandle,
    protocol::{wl_output, wl_pointer::WlPointer, wl_seat::WlSeat, wl_surface},
};
use wayland_protocols::ext::workspace::v1::client::{
    ext_workspace_group_handle_v1::{self, ExtWorkspaceGroupHandleV1},
    ext_workspace_handle_v1::{self, ExtWorkspaceHandleV1},
    ext_workspace_manager_v1::{self, ExtWorkspaceManagerV1},
};
use wayland_protocols_wlr::foreign_toplevel::v1::client::{
    zwlr_foreign_toplevel_handle_v1::{self, ZwlrForeignToplevelHandleV1},
    zwlr_foreign_toplevel_manager_v1::{self, ZwlrForeignToplevelManagerV1},
};

use crate::{
    config::{BarLayer, BarMargins, BarPosition, EventSourceKind},
    events::{
        self,
        wayland::{
            self as wayland_events, ToplevelTracker, ToplevelWindow, WorkspaceData,
            WorkspaceGroupData, WorkspaceTracker,
        },
    },
    interaction::{InteractionEvent, MouseAxis, MouseButton},
    runtime::{Runtime, bar::Bar, model::WidgetKey},
};

pub(super) fn apply_bar_surface_config(
    layer: &LayerSurface,
    position: BarPosition,
    height: u32,
    margin: BarMargins,
    exclusive: bool,
) {
    let v_anchor = match position {
        BarPosition::Top => Anchor::TOP,
        BarPosition::Bottom => Anchor::BOTTOM,
    };
    layer.set_anchor(v_anchor | Anchor::LEFT | Anchor::RIGHT);
    layer.set_margin(
        i32::try_from(margin.top).unwrap_or(i32::MAX),
        i32::try_from(margin.right).unwrap_or(i32::MAX),
        i32::try_from(margin.bottom).unwrap_or(i32::MAX),
        i32::try_from(margin.left).unwrap_or(i32::MAX),
    );
    layer.set_size(0, height);
    let exclusive_margin = match position {
        BarPosition::Top => margin.bottom,
        BarPosition::Bottom => margin.top,
    };
    let exclusive_zone = if exclusive {
        height.saturating_add(exclusive_margin)
    } else {
        0
    };
    layer.set_exclusive_zone(i32::try_from(exclusive_zone).unwrap_or(i32::MAX));
}

impl Runtime {
    pub(super) fn start_toplevel_listener(&mut self) {
        if self.toplevel_tracker.is_some() {
            return;
        }
        match self
            .globals
            .bind::<ZwlrForeignToplevelManagerV1, _, _>(&self.queue_handle, 1..=3, ())
        {
            Ok(manager) => {
                self.toplevel_tracker = Some(ToplevelTracker::new(manager));
            }
            Err(error) => {
                eprintln!(
                    "cellbar: compositor does not support zwlr_foreign_toplevel_manager_v1: {error}"
                );
            }
        }
    }

    pub(super) fn stop_toplevel_listener(&mut self) {
        if let Some(mut tracker) = self.toplevel_tracker.take() {
            tracker.stop();
        }
    }

    pub(super) fn start_workspace_listener(&mut self) {
        if self.workspace_tracker.is_some() {
            return;
        }
        match self
            .globals
            .bind::<ExtWorkspaceManagerV1, _, _>(&self.queue_handle, 1..=1, ())
        {
            Ok(manager) => {
                self.workspace_tracker = Some(WorkspaceTracker::new(manager));
            }
            Err(error) => {
                eprintln!("cellbar: compositor does not support ext_workspace_manager_v1: {error}");
            }
        }
    }

    pub(super) fn stop_workspace_listener(&mut self) {
        if let Some(mut tracker) = self.workspace_tracker.take() {
            tracker.stop();
        }
    }

    pub(super) fn dispatch_wayland_toplevel_event(
        &mut self,
        title: &str,
        app_id: &str,
        activated: bool,
    ) {
        let targets: Vec<_> = self
            .subscriptions
            .iter()
            .filter(|(_, entry)| {
                entry.kind == EventSourceKind::Wayland && entry.event == "toplevel"
            })
            .map(|(sub_id, entry)| (*sub_id, entry.manifest_id.clone()))
            .collect();
        for (sub_id, manifest_id) in targets {
            let context =
                wayland_events::format_toplevel_context(&manifest_id, title, app_id, activated);
            self.handle_event(events::Event {
                subscription_id: sub_id,
                context,
            });
        }
    }

    pub(super) fn dispatch_wayland_workspace_event(&mut self) {
        let Some(tracker) = &self.workspace_tracker else {
            return;
        };
        let workspaces = tracker.collect_workspaces(&self.output_state);
        let targets: Vec<_> = self
            .subscriptions
            .iter()
            .filter(|(_, entry)| {
                entry.kind == EventSourceKind::Wayland && entry.event == "workspace"
            })
            .map(|(sub_id, entry)| (*sub_id, entry.manifest_id.clone()))
            .collect();
        for (sub_id, manifest_id) in targets {
            let context = wayland_events::format_workspace_context(&manifest_id, &workspaces);
            self.handle_event(events::Event {
                subscription_id: sub_id,
                context,
            });
        }
    }

    pub(super) fn create_bar(
        &mut self,
        queue_handle: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        let info = self.output_state.info(&output);
        let output_name = info
            .as_ref()
            .and_then(|info| info.name.clone())
            .unwrap_or_else(|| format!("output-{}", output.id().protocol_id()));
        let scale = info.as_ref().map_or(1, |info| info.scale_factor).max(1);

        let bar_specs = self.bar_specs.clone();
        let mut created_any = false;
        for spec in bar_specs {
            if !spec.outputs.is_empty()
                && !spec.outputs.iter().any(|o| o == &output_name || o == "*")
            {
                continue;
            }
            if self.bars.iter().any(|b| {
                b.output == output
                    && b.name == spec.name
                    && b.position == spec.position
                    && b.layer_level == spec.layer
            }) {
                continue;
            }

            let surface = self.compositor.create_surface(queue_handle);
            surface.set_buffer_scale(scale);
            let shell_layer = match spec.layer {
                BarLayer::Background => Layer::Background,
                BarLayer::Bottom => Layer::Bottom,
                BarLayer::Top => Layer::Top,
                BarLayer::Overlay => Layer::Overlay,
            };
            let layer = self.layer_shell.create_layer_surface(
                queue_handle,
                surface,
                shell_layer,
                Some("cellbar"),
                Some(&output),
            );
            let bar_height = spec.bar_height();
            layer.set_keyboard_interactivity(KeyboardInteractivity::None);
            apply_bar_surface_config(
                &layer,
                spec.position,
                bar_height,
                spec.margin,
                if spec.initially_hidden {
                    false
                } else {
                    spec.exclusive
                },
            );
            layer.commit();

            match SlotPool::new(4, &self.shm) {
                Ok(pool) => {
                    let bar_title = match &spec.name {
                        Some(name) => format!("{name} on {output_name}"),
                        None => output_name.clone(),
                    };
                    eprintln!("cellbar: creating bar {bar_title} at scale {scale}");
                    let bar_id = self.next_bar_id;
                    self.next_bar_id = self.next_bar_id.saturating_add(1);
                    let widgets =
                        self.instantiate_bar_widgets(&spec.widget_definitions, &output_name);
                    self.bars.push(Bar {
                        id: bar_id,
                        name: spec.name.clone(),
                        height: spec.height,
                        position: spec.position,
                        layer_level: spec.layer,
                        margin: spec.margin,
                        exclusive: spec.exclusive,
                        theme: Some(spec.theme.clone()),
                        widget_definitions: spec.widget_definitions.clone(),
                        output: output.clone(),
                        output_name: output_name.clone(),
                        layer,
                        pool,
                        logical_width: 0,
                        logical_height: bar_height,
                        scale,
                        configured: false,
                        hidden: spec.initially_hidden,
                        auto_hide: spec.auto_hide,
                        auto_hide_deadline: if !spec.initially_hidden {
                            spec.auto_hide.map(|dur| Instant::now() + dur)
                        } else {
                            None
                        },
                        pointer_inside: false,
                        dirty: false,
                        frame_pending: false,
                        widgets,
                        frame_scratch: Default::default(),
                        last_frame: None,
                        last_geometry: None,
                        last_image_generation: 0,
                        static_spans: Vec::new(),
                        widget_ranges: Vec::new(),
                    });
                    let index = self.bars.len() - 1;
                    self.register_bar_subscriptions(index);
                    created_any = true;
                }
                Err(error) => {
                    self.failure =
                        Some(format!("cannot create SHM pool for {output_name}: {error}"));
                }
            }
        }

        if created_any {
            self.rebuild_widget_indices();
            self.rebuild_interaction_registry();
            self.sync_sources();
            self.refresh_providers();
            self.start_due_processes();
            self.rearm_tick();
        }
    }

    pub(super) fn update_bar_scale(&mut self, output: &wl_output::WlOutput) {
        let Some(scale) = self
            .output_state
            .info(output)
            .map(|info| info.scale_factor.max(1))
        else {
            return;
        };
        let Some(index) = self.bars.iter().position(|bar| &bar.output == output) else {
            return;
        };
        let configured = {
            let bar = &mut self.bars[index];
            if bar.scale == scale {
                return;
            }
            bar.scale = scale;
            bar.layer.wl_surface().set_buffer_scale(scale);
            bar.configured
        };
        if configured {
            self.mark_bar_dirty(index);
        }
    }

    pub(super) fn remove_bar_for_output(&mut self, output: &wl_output::WlOutput) {
        let mut removed_any = false;
        while let Some(index) = self.bars.iter().position(|bar| &bar.output == output) {
            let bar = self.bars.swap_remove(index);
            eprintln!("cellbar: removed bar from {}", bar.output_name);
            removed_any = true;
        }
        if removed_any {
            self.unregister_unused_providers();
            self.sync_sources();
            self.rebuild_widget_indices();
            self.rebuild_interaction_registry();
        }
    }

    pub(super) fn set_cursor(&mut self, conn: &Connection, icon: CursorIcon, force: bool) {
        if !force && self.cursor_icon == icon {
            return;
        }
        let Some(pointer) = self.pointer.as_ref() else {
            return;
        };
        match pointer.set_cursor(conn, icon) {
            Ok(()) => self.cursor_icon = icon,
            Err(error) => eprintln!("cellbar: failed to set cursor {icon:?}: {error}"),
        }
    }

    pub(super) fn cursor_for_position(
        &self,
        surface: &wl_surface::WlSurface,
        surface_x: f64,
    ) -> CursorIcon {
        let Some(bar) = self
            .bars
            .iter()
            .find(|bar| bar.layer.wl_surface() == surface)
        else {
            return CursorIcon::Default;
        };
        let origin = self.theme.padding.horizontal;
        let cell_width = self.renderer.cell_width();
        let Some((widget_index, target)) = bar.hit_test_widget(surface_x, origin, cell_width)
        else {
            return CursorIcon::Default;
        };
        let key = WidgetKey {
            bar_id: bar.id,
            widget_index,
        };
        if self
            .interaction_registry
            .is_interactive(key, target.as_deref())
        {
            CursorIcon::Pointer
        } else {
            CursorIcon::Default
        }
    }
}

impl CompositorHandler for Runtime {
    fn scale_factor_changed(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        new_factor: i32,
    ) {
        let Some(index) = self
            .bars
            .iter()
            .position(|bar| bar.layer.wl_surface() == surface)
        else {
            return;
        };
        let new_factor = new_factor.max(1);
        let configured = {
            let bar = &mut self.bars[index];
            if bar.scale == new_factor {
                return;
            }
            bar.scale = new_factor;
            surface.set_buffer_scale(new_factor);
            bar.configured
        };
        if configured {
            self.mark_bar_dirty(index);
        }
    }

    fn transform_changed(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _new_transform: wl_output::Transform,
    ) {
    }

    fn frame(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        surface: &wl_surface::WlSurface,
        _time: u32,
    ) {
        let Some(index) = self
            .bars
            .iter()
            .position(|bar| bar.layer.wl_surface() == surface)
        else {
            return;
        };
        let should_redraw = {
            let bar = &mut self.bars[index];
            bar.frame_pending = false;
            bar.configured && bar.dirty
        };
        if should_redraw {
            self.mark_bar_dirty(index);
        }
    }

    fn surface_enter(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }

    fn surface_leave(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        _surface: &wl_surface::WlSurface,
        _output: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for Runtime {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(
        &mut self,
        _connection: &Connection,
        queue_handle: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        self.create_bar(queue_handle, output);
    }

    fn update_output(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        self.update_bar_scale(&output);
    }

    fn output_destroyed(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        output: wl_output::WlOutput,
    ) {
        self.remove_bar_for_output(&output);
    }
}

impl LayerShellHandler for Runtime {
    fn closed(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        layer: &LayerSurface,
    ) {
        if let Some(index) = self.bars.iter().position(|bar| &bar.layer == layer) {
            let bar = self.bars.swap_remove(index);
            self.unregister_unused_providers();
            self.sync_sources();
            self.rebuild_widget_indices();
            self.rebuild_interaction_registry();
            eprintln!("cellbar: compositor closed bar on {}", bar.output_name);
        }
    }

    fn configure(
        &mut self,
        _connection: &Connection,
        _queue_handle: &QueueHandle<Self>,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        let Some(index) = self.bars.iter().position(|bar| &bar.layer == layer) else {
            return;
        };
        let bar = &mut self.bars[index];
        if bar.hidden {
            return;
        }
        let previous_size = (bar.logical_width, bar.logical_height);
        if configure.new_size.0 > 0 {
            bar.logical_width = configure.new_size.0;
        }
        if configure.new_size.1 > 0 {
            bar.logical_height = configure.new_size.1;
        }
        if previous_size != (bar.logical_width, bar.logical_height) {
            bar.last_frame = None;
            bar.last_geometry = None;
        }
        if bar.logical_width == 0 || bar.logical_height == 0 {
            return;
        }

        let first_configure = !bar.configured;
        bar.configured = true;
        if first_configure || bar.dirty || previous_size != (bar.logical_width, bar.logical_height)
        {
            bar.frame_pending = false;
            bar.last_frame = None;
            bar.last_geometry = None;
            if first_configure {
                self.refresh_providers();
            }
            if let Err(error) = self.redraw_bar(index) {
                self.failure = Some(error);
                return;
            }
        }
        if first_configure {
            let bar = &self.bars[index];
            eprintln!(
                "cellbar: mapped bar on {} ({}x{} logical, scale {})",
                bar.output_name, bar.logical_width, bar.logical_height, bar.scale
            );
            #[cfg(target_os = "linux")]
            unsafe {
                libc::malloc_trim(0);
            }
        }
    }
}

impl ShmHandler for Runtime {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl SeatHandler for Runtime {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }

    fn new_seat(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _seat: WlSeat) {}

    fn new_capability(
        &mut self,
        _conn: &Connection,
        qh: &QueueHandle<Self>,
        seat: WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer && self.pointer.is_none() {
            let cursor_surface = self.compositor.create_surface(qh);
            match self.seat_state.get_pointer_with_theme::<Runtime, ()>(
                qh,
                &seat,
                self.shm.wl_shm(),
                cursor_surface,
                ThemeSpec::System,
            ) {
                Ok(pointer) => {
                    self.pointer = Some(pointer);
                }
                Err(error) => {
                    eprintln!("cellbar: failed to get themed pointer: {error}");
                }
            }
        }
    }

    fn remove_capability(
        &mut self,
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
        _seat: WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer {
            self.pointer = None;
        }
    }

    fn remove_seat(&mut self, _conn: &Connection, _qh: &QueueHandle<Self>, _seat: WlSeat) {}
}

impl PointerHandler for Runtime {
    fn pointer_frame(
        &mut self,
        conn: &Connection,
        _qh: &QueueHandle<Self>,
        _pointer: &WlPointer,
        events: &[PointerEvent],
    ) {
        for event in events {
            let surface = &event.surface;
            let (surface_x, _surface_y) = event.position;
            match &event.kind {
                PointerEventKind::Enter { .. } => {
                    let icon = self.cursor_for_position(surface, surface_x);
                    self.set_cursor(conn, icon, true);
                }
                PointerEventKind::Motion { .. } => {
                    let icon = self.cursor_for_position(surface, surface_x);
                    self.set_cursor(conn, icon, false);
                }
                PointerEventKind::Leave { .. } => {
                    self.set_cursor(conn, CursorIcon::Default, true);
                }
                _ => {}
            }

            let Some(bar_index) = self
                .bars
                .iter()
                .position(|b| b.layer.wl_surface() == surface)
            else {
                continue;
            };

            match &event.kind {
                PointerEventKind::Enter { .. } => {
                    let bar = &mut self.bars[bar_index];
                    bar.pointer_inside = true;
                    if bar.auto_hide.is_some() {
                        bar.auto_hide_deadline = None;
                    }
                }
                PointerEventKind::Leave { .. } => {
                    let bar = &mut self.bars[bar_index];
                    bar.pointer_inside = false;
                    if let Some(dur) = bar.auto_hide
                        && !bar.hidden
                    {
                        bar.auto_hide_deadline = Some(Instant::now() + dur);
                    }
                    self.rearm_tick();
                }
                _ => {}
            }

            let bar = &self.bars[bar_index];
            let bar_id = bar.id;
            let origin = self.theme.padding.horizontal;
            let cell_width = self.renderer.cell_width();

            let interaction_events: Vec<InteractionEvent> = match &event.kind {
                PointerEventKind::Press { button, .. } => {
                    vec![InteractionEvent::Click(MouseButton::from_linux_code(
                        *button,
                    ))]
                }
                PointerEventKind::Axis {
                    horizontal,
                    vertical,
                    ..
                } => {
                    let mut list = Vec::new();
                    if vertical.absolute < -1.0 || vertical.discrete < 0 || vertical.value120 < 0 {
                        list.push(InteractionEvent::Scroll(MouseAxis::ScrollUp));
                    } else if vertical.absolute > 1.0
                        || vertical.discrete > 0
                        || vertical.value120 > 0
                    {
                        list.push(InteractionEvent::Scroll(MouseAxis::ScrollDown));
                    }
                    if horizontal.absolute < -1.0
                        || horizontal.discrete < 0
                        || horizontal.value120 < 0
                    {
                        list.push(InteractionEvent::Scroll(MouseAxis::ScrollLeft));
                    } else if horizontal.absolute > 1.0
                        || horizontal.discrete > 0
                        || horizontal.value120 > 0
                    {
                        list.push(InteractionEvent::Scroll(MouseAxis::ScrollRight));
                    }
                    list
                }
                _ => Vec::new(),
            };

            if !interaction_events.is_empty()
                && let Some((widget_index, sub_target)) =
                    bar.hit_test_widget(surface_x, origin, cell_width)
            {
                let key = WidgetKey {
                    bar_id,
                    widget_index,
                };
                for evt in interaction_events {
                    if let Some((binding, target_val)) = self
                        .interaction_registry
                        .lookup(key, evt, sub_target.as_deref())
                        .map(|(b, t)| (b.clone(), t))
                        && self
                            .interaction_registry
                            .should_dispatch(binding.id, binding.debounce)
                    {
                        let resolved = binding.resolve_action(target_val.as_deref());
                        self.execute_action(&resolved);
                    }
                }
            }
        }
    }
}

delegate_registry!(Runtime);

impl ProvidesRegistryState for Runtime {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }

    registry_handlers![OutputState, SeatState];
}

smithay_client_toolkit::delegate_dispatch2!(Runtime);

impl Dispatch<ZwlrForeignToplevelManagerV1, ()> for Runtime {
    fn event(
        runtime: &mut Self,
        _manager: &ZwlrForeignToplevelManagerV1,
        event: zwlr_foreign_toplevel_manager_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_foreign_toplevel_manager_v1::Event::Toplevel { toplevel } => {
                if let Some(tracker) = &mut runtime.toplevel_tracker {
                    let id = toplevel.id();
                    tracker
                        .windows
                        .insert(id.clone(), ToplevelWindow::default());
                    tracker.handles.insert(id, toplevel);
                }
            }
            zwlr_foreign_toplevel_manager_v1::Event::Finished => {
                runtime.stop_toplevel_listener();
            }
            _ => {}
        }
    }

    wayland_client::event_created_child!(Runtime, ZwlrForeignToplevelManagerV1, [
        0 => (ZwlrForeignToplevelHandleV1, ()),
    ]);
}

impl Dispatch<ZwlrForeignToplevelHandleV1, ()> for Runtime {
    fn event(
        runtime: &mut Self,
        handle: &ZwlrForeignToplevelHandleV1,
        event: zwlr_foreign_toplevel_handle_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        let id = handle.id();

        match event {
            zwlr_foreign_toplevel_handle_v1::Event::Title { title } => {
                if let Some(tracker) = &mut runtime.toplevel_tracker
                    && let Some(win) = tracker.windows.get_mut(&id)
                {
                    win.title = title;
                }
            }
            zwlr_foreign_toplevel_handle_v1::Event::AppId { app_id } => {
                if let Some(tracker) = &mut runtime.toplevel_tracker
                    && let Some(win) = tracker.windows.get_mut(&id)
                {
                    win.app_id = app_id;
                }
            }
            zwlr_foreign_toplevel_handle_v1::Event::State { state } => {
                let is_active = state
                    .chunks_exact(4)
                    .any(|c| u32::from_ne_bytes(c.try_into().unwrap_or_default()) == 2);
                if let Some(tracker) = &mut runtime.toplevel_tracker
                    && let Some(win) = tracker.windows.get_mut(&id)
                {
                    win.activated = is_active;
                }
            }
            zwlr_foreign_toplevel_handle_v1::Event::Done => {
                let Some(tracker) = &mut runtime.toplevel_tracker else {
                    return;
                };
                let Some(win) = tracker.windows.get(&id).cloned() else {
                    return;
                };

                let mut should_emit = false;
                if win.activated {
                    if tracker.active_window.as_ref() != Some(&id) {
                        tracker.active_window = Some(id.clone());
                        should_emit = true;
                    } else {
                        should_emit = true;
                    }
                } else if tracker.active_window.as_ref() == Some(&id) {
                    tracker.active_window = None;
                    should_emit = true;
                }

                if should_emit {
                    let title = if win.activated { &win.title } else { "" };
                    let app_id = if win.activated { &win.app_id } else { "" };
                    runtime.dispatch_wayland_toplevel_event(title, app_id, win.activated);
                }
            }
            zwlr_foreign_toplevel_handle_v1::Event::Closed => {
                let was_active = if let Some(tracker) = &mut runtime.toplevel_tracker {
                    let was_active = tracker.active_window.as_ref() == Some(&id);
                    if was_active {
                        tracker.active_window = None;
                    }
                    tracker.windows.remove(&id);
                    tracker.handles.remove(&id);
                    was_active
                } else {
                    false
                };
                if was_active {
                    runtime.dispatch_wayland_toplevel_event("", "", false);
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<ExtWorkspaceManagerV1, ()> for Runtime {
    fn event(
        runtime: &mut Self,
        _manager: &ExtWorkspaceManagerV1,
        event: ext_workspace_manager_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        match event {
            ext_workspace_manager_v1::Event::WorkspaceGroup { workspace_group } => {
                if let Some(tracker) = &mut runtime.workspace_tracker {
                    let id = workspace_group.id();
                    tracker
                        .groups
                        .insert(id.clone(), WorkspaceGroupData::default());
                    tracker.group_handles.insert(id, workspace_group);
                }
            }
            ext_workspace_manager_v1::Event::Workspace { workspace } => {
                if let Some(tracker) = &mut runtime.workspace_tracker {
                    let id = workspace.id();
                    tracker
                        .workspaces
                        .insert(id.clone(), WorkspaceData::default());
                    tracker.workspace_handles.insert(id, workspace);
                }
            }
            ext_workspace_manager_v1::Event::Done => {
                runtime.dispatch_wayland_workspace_event();
            }
            ext_workspace_manager_v1::Event::Finished => {
                runtime.stop_workspace_listener();
            }
            _ => {}
        }
    }

    wayland_client::event_created_child!(Runtime, ExtWorkspaceManagerV1, [
        0 => (ExtWorkspaceGroupHandleV1, ()),
        1 => (ExtWorkspaceHandleV1, ()),
    ]);
}

impl Dispatch<ExtWorkspaceGroupHandleV1, ()> for Runtime {
    fn event(
        runtime: &mut Self,
        group: &ExtWorkspaceGroupHandleV1,
        event: ext_workspace_group_handle_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        let id = group.id();
        let Some(tracker) = &mut runtime.workspace_tracker else {
            return;
        };
        match event {
            ext_workspace_group_handle_v1::Event::OutputEnter { output } => {
                if let Some(group_data) = tracker.groups.get_mut(&id)
                    && !group_data.outputs.iter().any(|o| o == &output)
                {
                    group_data.outputs.push(output);
                }
            }
            ext_workspace_group_handle_v1::Event::OutputLeave { output } => {
                if let Some(group_data) = tracker.groups.get_mut(&id) {
                    group_data.outputs.retain(|o| o != &output);
                }
            }
            ext_workspace_group_handle_v1::Event::WorkspaceEnter { workspace } => {
                if let Some(group_data) = tracker.groups.get_mut(&id) {
                    group_data.workspaces.insert(workspace.id());
                }
            }
            ext_workspace_group_handle_v1::Event::WorkspaceLeave { workspace } => {
                if let Some(group_data) = tracker.groups.get_mut(&id) {
                    group_data.workspaces.remove(&workspace.id());
                }
            }
            ext_workspace_group_handle_v1::Event::Removed => {
                tracker.groups.remove(&id);
                if let Some(handle) = tracker.group_handles.remove(&id) {
                    handle.destroy();
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<ExtWorkspaceHandleV1, ()> for Runtime {
    fn event(
        runtime: &mut Self,
        workspace: &ExtWorkspaceHandleV1,
        event: ext_workspace_handle_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        let id = workspace.id();
        let Some(tracker) = &mut runtime.workspace_tracker else {
            return;
        };
        match event {
            ext_workspace_handle_v1::Event::Id { id: ws_id } => {
                if let Some(ws) = tracker.workspaces.get_mut(&id) {
                    ws.id = Some(ws_id);
                }
            }
            ext_workspace_handle_v1::Event::Name { name } => {
                if let Some(ws) = tracker.workspaces.get_mut(&id) {
                    ws.name = name;
                }
            }
            ext_workspace_handle_v1::Event::Coordinates { coordinates } => {
                let coords = coordinates
                    .chunks_exact(4)
                    .map(|chunk| u32::from_ne_bytes(chunk.try_into().unwrap_or_default()))
                    .collect();
                if let Some(ws) = tracker.workspaces.get_mut(&id) {
                    ws.coordinates = coords;
                }
            }
            ext_workspace_handle_v1::Event::State { state } => {
                if let Some(ws) = tracker.workspaces.get_mut(&id) {
                    match state {
                        wayland_client::WEnum::Value(s) => {
                            ws.active = s.contains(ext_workspace_handle_v1::State::Active);
                            ws.urgent = s.contains(ext_workspace_handle_v1::State::Urgent);
                            ws.hidden = s.contains(ext_workspace_handle_v1::State::Hidden);
                        }
                        wayland_client::WEnum::Unknown(v) => {
                            ws.active = (v & ext_workspace_handle_v1::State::Active.bits()) != 0;
                            ws.urgent = (v & ext_workspace_handle_v1::State::Urgent.bits()) != 0;
                            ws.hidden = (v & ext_workspace_handle_v1::State::Hidden.bits()) != 0;
                        }
                    }
                }
            }
            ext_workspace_handle_v1::Event::Removed => {
                tracker.workspaces.remove(&id);
                if let Some(handle) = tracker.workspace_handles.remove(&id) {
                    handle.destroy();
                }
                for group in tracker.groups.values_mut() {
                    group.workspaces.remove(&id);
                }
            }
            _ => {}
        }
    }
}
