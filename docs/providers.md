# Providers and Sources

[README](../README.md) · [Quickstart](quickstart.md) · [Expressions](expressions.md)

## Expression Providers

An expression provider keeps the normal provider manifest and trigger model but
evaluates inside Cellbar instead of starting a process. Expressions are pure:
they can read `context`, `state`, and `settings`, but cannot perform I/O or run
commands. The result is one strict markup value.

```toml
[provider]
expression = 'render(settings.template, { percent: format_percent(context.data.percent) })'

[[triggers.on_source]]
id = "sample"
source = "cpu"
event = "sample"

[settings]
template = "[CPU {percent}](@cpu)"
interval = "3s"
```

`context` is the most recent triggering event. `state` keeps the most recent
context for every trigger ID, so one provider can combine multiple sources.
See [Component context reference](context.md) for event schemas, lifecycle,
missing-data handling, and the differences between expressions and scripts.
Built-in source data includes:

- `cpu.data.percent`
- `memory.data.used_kib`, `memory.data.available_kib`, `memory.data.total_kib`, `memory.data.percent`
- `clock.data.timestamp`
- `battery.data.present`, `battery.data.percent`, `battery.data.status`, `battery.data.plugged`, `battery.data.icon`
- `backlight.data.present`, `backlight.data.percent`, `backlight.data.brightness`, `backlight.data.max_brightness`, `backlight.data.icon`

The available built-in functions are `render`, `format_percent`, `format_size`,
`format_memory`, `strftime`, and `image`. Expression execution is bounded and
rejects invalid expressions at configuration load time.

### Functional Chaining and Lists

Expressions support full functional method chaining over lists, strings, and maps with closures (`|item| ...` or `|item, index| ...`), template strings (`` `...` ``), and conditional expressions (`if / else` and ternary `? :`):

```rust
// Workspaces expression example:
context.message.workspaces
  .filter(|w| !w.hidden)
  .map(|w| if w.active {
    `#ws:{w.id}{ [[{w.name}]](@accent) }`
  } else {
    `#ws:{w.id}{ [{w.name}] }`
  })
  .join("[ ]")

// Window title expression example:
context.message.title != "" ? `[{context.message.title}](@window)` : "[Desktop]"
```

Supported list methods:
- `.map(|item| ...)` / `.map(|item, index| ...)`
- `.filter(|item| ...)`
- `.find(|item| ...)`
- `.any(|item| ...)` / `.all(|item| ...)`
- `.join(separator)`
- `.contains(value)`
- `.len()`, `.is_empty()`, `.first()`, `.last()`, `.reverse()`, `.take(n)`, `.skip(n)`

Supported string methods:
- `.trim()`, `.to_uppercase()`, `.to_lowercase()`
- `.contains(sub)`, `.starts_with(prefix)`, `.ends_with(suffix)`
- `.split(delimiter)`, `.replace(from, to)`, `.render(map)`

## Command Providers

A command provider runs an external process. It uses the same
manifest directory, settings, and trigger model, but its executable is
declared under `[provider]`:

```toml
[provider]
program = "./run.sh"
args = ["--icon", "${setting.icon}", "--context", "${context}"]
timeout = "2s"

[triggers]
on_activate = true
every = "30s"
debounce = "50ms"

[settings]
icon = "[CPU {percent}](@cpu)"
```

Command providers emit one markup line per value. Renderable text uses
`[text]` or `[text](@style)`, interaction regions use `#target{ ... }`, and
images use `![path](options)`. Whitespace outside nodes is formatting only;
other text outside an explicit node is invalid.

`program` is resolved relative to the manifest directory. Commands execute
without a shell and may remain running for streaming output. `${setting.<name>}`
is replaced from validated settings, `${context}` is replaced with the
triggering event as one JSON argument (activation and timer runs receive
`null`), and `${output}` is replaced with the name of the Wayland output
hosting the widget (e.g. `DP-1`, allowing multi-monitor workspace scripts to
query only their own monitor's state). There is one Provider output format:
the strict markup stream described above. Invalid lines are rejected and the
last valid value remains visible.

Component manifests live at `components/<component-id>/manifest.toml`,
where the directory name is the component ID. A component is loaded only when referenced. Component settings
are declared directly under `[settings]` as key-value pairs (for example `interval = "30s"` or `hide_empty = true`).

A provider can be triggered by activation, a timer, or one or more native
source tables:

```toml
[[triggers.on_dbus]]
id = "mpris-properties"
bus = "session"
match_rule = "type='signal',interface='org.freedesktop.DBus.Properties',member='PropertiesChanged'"

[[triggers.on_netlink]]
id = "route-state"
family = "route"

[[triggers.on_uevent]]
id = "power-state"
subsystem = "power_supply"
action = "change"

[[triggers.on_wayland]]
id = "active-window"
event = "toplevel"
```

The supported PipeWire events are `volume`, `default-sink`, `sink-volume`,
`default-source`, and `source-volume`. The supported Wayland events are
`toplevel` and `workspace`. Native triggers are opened only while an active
display needs them. Trigger bursts are coalesced per provider instance.

## Custom Sources

A source is a persistent external process that publishes events. Sources are
not display providers: they never write widget text directly. Their manifests
live at `sources/<source-id>/manifest.toml` and are loaded only when a provider
uses `[[triggers.on_source]]`.

```toml
[source]
program = "./listen.sh"
args = []
restart_after = "1s"
scope = "global"

[[emits]]
event = "changed"
```

A source emits one JSON object per line:

```json
{"version":1,"event":"changed","data":{"player":"spotify","status":"Playing"}}
```

Cellbar verifies the version and declared event, then delivers a normalized
context to matching providers:

```json
{"source":"mpris","event":"changed","data":{"player":"spotify","status":"Playing"}}
```

A provider subscribes to it with:

```toml
[[triggers.on_source]]
id = "mpris-state"
source = "mpris"
event = "changed"
```

`scope = "global"` runs one shared source process. `scope = "output"` runs one
source process per output that has a subscriber. Sources start when their first
subscriber becomes active, stop after their last subscriber disappears, and
restart with capped exponential backoff after exit.

## Provider Output

Expression and command providers share the same strict markup intermediate
representation. Expression strings must return markup such as
`render("[CPU {percent}](@cpu)", { percent: ... })`; command stdout uses the
same grammar one line at a time. Styles and interaction targets are preserved
through parsing, layout, hit-testing, and action routing.

Evaluation and decoding errors keep the last valid value and are reported to
stderr. External sources use the fixed structured event-line format shown
above; that input format is not configurable.

Static image output, mixed text/image segments, and image-producing expressions
are supported as described in [Static Images in the CellFrame](images.md).

## Pointer Actions

Providers can bind pointer interactions in `[actions]`:

```toml
[actions]
"click" = "playerctl play-pause"
"right_click" = "pavucontrol"
"middle_click" = "wpctl set-mute @DEFAULT_AUDIO_SINK@ toggle"
"scroll_up" = "playerctl volume 0.05+"
"scroll_down" = "playerctl volume 0.05-"
```

When a provider's markup defines named interactive scopes with `#target{ ... }`
(for example `#ws:1{ [ 1 ] }` or `#tray:0{ ... }`), actions can match either a
specific target (`"click:ws:1"`) or use a wildcard (`"click:ws:*"`), where `$1`
in the action command is replaced by the matched target suffix.
