# Component context reference

[Component execution](providers.md) · [Expression syntax](expressions.md) · [Configuration](configuration.md)

## Three expression inputs

A component's `[provider] expression` can read:

| Name | Meaning |
| --- | --- |
| `settings` | Direct values from this component's `[settings]` table |
| `context` | The last event delivered to this expression instance |
| `state` | Last delivered event for each subscribed trigger ID |

`context` is structured event data, not a template placeholder and not the
component's rendered output. There is **no single universal event schema**:
read `data` for built-in/custom sources, and `message` for native events.
Fields such as `version`, `source`, `event`, and `subscription` are not present
in every event. Do not assume they are always available.

## Lifecycle and missing data

- Before any event arrives, expression `context` is `null` and `state` is empty.
- Evaluating on activation, a timer, or manual refresh does not manufacture
  source data. Expressions keep the last successfully decoded context.
- Built-in source sampling (such as a CPU sample) is an actual event and does
  update context.
- Events are coalesced per trigger before evaluation. `state` retains the most
  recent event for each trigger; `context` becomes the last pending event
  processed. This is not an event history or a reliable cross-source ordering.
- State belongs to a runtime component instance, not to all bars globally; do
  not rely on it surviving instance recreation or a reload.
- Guard optional data, particularly window information and inactive hardware.

```toml
[provider]
expression = '''
let title = context.message && context.message.title != "" ? context.message.title : "";
title != "" ? render(settings.template, { title }) : ""
'''
```

Return `""` to hide component content when there is no useful data. Returning
plain text instead of markup is not a substitute for a valid text node.

## Built-in and custom sources: `context.data`

Subscriptions use `[[triggers.on_source]]`:

```toml
[[triggers.on_source]]
id = "cpu-sample"
source = "cpu"
event = "sample"
```

The event is normalized to:

```json
{"source":"cpu","event":"sample","data":{"percent":17}}
```

Use `context.data.percent`, not `context.cpu.data.percent`.

| Source | Event | Useful `data` fields |
| --- | --- | --- |
| `clock` | `tick` | `timestamp` (Unix seconds) |
| `cpu` | `sample` | `percent` |
| `memory` | `sample` | `used_kib`, `available_kib`, `total_kib`, `percent` |
| `battery` | `sample` | `present`, `percent`, `status`, `plugged`, `icon` |
| `backlight` | `sample` | `present`, `percent`, `brightness`, `max_brightness`, `device`, `icon` |

Custom source events have the same `source`/`event`/`data` envelope. Their `data`
fields are defined by the source program. The source's input-line `version`
is validated by Cellbar but is not copied into this normalized context.

## Combining sources: `state`

Keys in `state` are the manifest's **trigger IDs**, not source names (unless
you deliberately use the same name):

```toml
[provider]
expression = '''
state.cpu_sample && state.memory_sample
    ? render("[CPU {cpu} | MEM {mem}]", {
        cpu: format_percent(state.cpu_sample.data.percent),
        mem: format_percent(state.memory_sample.data.percent)
      })
    : ""
'''

[[triggers.on_source]]
id = "cpu_sample"
source = "cpu"
event = "sample"

[[triggers.on_source]]
id = "memory_sample"
source = "memory"
event = "sample"
```

Each state entry contains the complete context object for that trigger, not
just its `data`. Check that both entries exist before reading them.

## Exact context contracts

The following contracts are the stable fields currently emitted by each source.
All names are case-sensitive. A source can still be unavailable on a machine;
in that case no useful sample is produced and the provider should handle an
empty or unchanged display.

### Built-in system sources

```json
{"source":"clock","event":"tick","data":{"timestamp":1710000000}}
{"source":"cpu","event":"sample","data":{"percent":17}}
{"source":"memory","event":"sample","data":{"used_kib":4000000,"available_kib":12000000,"total_kib":16000000,"percent":25}}
{"source":"battery","event":"sample","data":{"present":true,"percent":82,"status":"Discharging","plugged":false,"icon":"󰁹"}}
{"source":"backlight","event":"sample","data":{"present":true,"percent":60,"brightness":600,"max_brightness":1000,"device":"intel_backlight","icon":"󰃠"}}
```

The numeric fields are numbers, `present` and `plugged` are booleans, and
missing hardware is represented by `present = false` rather than a different
schema. The battery field is `plugged`, not `charging`.

### Wayland sources

`toplevel` uses:

```json
{"source":"wayland","event":"toplevel","message":{"title":"Terminal","app_id":"org.example.Terminal","activated":true}}
```

`workspace` uses:

```json
{"source":"wayland","event":"workspace","message":{"current":"2","active":["2"],"workspaces":[{"name":"1","active":false,"urgent":false,"hidden":false},{"name":"2","active":true,"urgent":false,"hidden":false}]}}
```

Native workspace records always contain `name` (string), `active`, `urgent`,
and `hidden` (booleans). Optional `id` is a string, `outputs` is a string array,
and `coordinates` is an integer array. Native workspace events do not contain
`client_count`; compositor IPC scripts may use their own richer records.

### Notification-style native sources

These sources are notifications rather than snapshots:

```json
{"source":"pipewire","event":"volume","subscription":"volume"}
{"source":"netlink","family":"route","message":{"type":"link","action":"new","interface_index":2,"interface_name":"wlan0"},"subscription":"route-state"}
{"source":"uevent","message":{"action":"change","devpath":"/devices/...","subsystem":"power_supply","devname":"BAT0","sequence":42},"subscription":"power-state"}
```

PipeWire currently does not include a volume number or device state in this
context. D-Bus similarly provides signal metadata only:

```json
{"message":{"type":"signal","sender":"org.example","destination":"org.example.Client","path":"/org/example","interface":"org.example.Properties","member":"Changed","body":null},"subscription":"properties"}
```

Use these events to trigger a refresh; do not treat them as complete state
objects. A command provider can query the current state after receiving
`${context}`.



These JSON examples omit `version: 1` for brevity on native events; built-in
source events have no version field. D-Bus events have no `source` field.

## Wayland: `context.message`

For `[[triggers.on_wayland]] event = "toplevel"`:

```json
{"version":1,"subscription":"active-window","source":"wayland","event":"toplevel","message":{"title":"Terminal","app_id":"org.example.Terminal","activated":true}}
```

For `event = "workspace"`, `message` contains `current` (a name or null),
`active` (an array of names), and `workspaces` (workspace records). No supported
compositor protocol means there may be no events; do not assume a title or
workspace is always present.

## Other native event shapes

- **D-Bus**: `version`, `subscription`, and `message` with `type`, `sender`,
  `destination`, `path`, `interface`, `member`, and `body`. **The current native
  backend sets `body` to null**; it does not decode signal payloads. Use the
  signal as a refresh notification and query properties in a command component
  if needed. Do not depend on `context.message.body[1]` holding MPRIS metadata.
- **PipeWire**: `version`, `source = "pipewire"`, `subscription`, and `event`
  (`volume`, `default-sink`, `sink-volume`, `default-source`, or `source-volume`).
  This is a change notification, not a numeric volume snapshot.
- **Netlink**: `version`, `subscription`, `source = "netlink"`, `family = "route"`,
  and `message` with `type`, `action`, and optional `interface_index` /
  `interface_name`. There is no universal `event` field in this envelope.
- **Uevent**: `version`, `subscription`, `source = "uevent"`, and `message` with
  `action`, `devpath`, and optional `subsystem`, `devname`, `sequence`.

## Command components: `${context}`

Scripts do not receive expression variables. Pass the event as a single JSON
argument explicitly:

```toml
[provider]
program = "./run.sh"
args = ["--event", "${context}"]
```

Arguments are passed directly without shell evaluation. A pending event is
substituted into `${context}`. Without a pending event (typically activation,
timer, or manual refresh), the argument is the literal JSON `null`. Unlike
expression context, this is not a persistent snapshot cache. Scripts needing
current state should query it when the event is absent or incomplete.

Other substitutions are separate concepts:

- `${setting.name}`: a manifest setting serialized for a command argument.
- `${output}`: the output name for an output-specific component instance.
- `{name}` in `render(settings.template, { name: value })`: a rendering
  placeholder supplied by the expression, not automatically read from context.

Never evaluate event JSON as shell code. Parse it as data (for example with
`jq`) and escape external text when constructing markup.
