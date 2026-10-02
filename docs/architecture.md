# Architecture

Cellbar is a single-threaded Wayland topbar runtime. Wayland dispatch, control
commands, timers, native event sources, expression evaluation, command provider
stdout, and custom source stdout are coordinated by one calloop event loop. Blocking D-Bus and PipeWire
work remains behind dedicated dispatchers and feeds normalized events back into
that loop.

```text
Wayland / native events / custom sources
                |
                v
          normalized context
                |
                v
      provider trigger and debounce
                |
                v
       expression or command output
                |
                v
         widget value -> CellFrame -> renderer
```

## Configuration ownership

The main configuration owns runtime and layout structure:

- the relative path to the theme file;
- bar geometry and fixed top-layer placement;
- left, center, and right layout order;
- stable display IDs;
- display text, provider selection, settings, and display width constraints.

The separate theme file owns presentation:

- font selection and size;
- named colors;
- bar surface background, padding, and corner radius;
- the base `[text]` style (foreground, background, and glyph attributes) plus
named styles referenced from markup; every renderable value, including static
text and literal layout entries, is the same markup.

The loader resolves the theme relative to the main configuration, validates all
color and style references, and stores an immutable resolved theme in the
runtime. Layout and display definitions remain separate: layout is the
human-readable outline of a bar; display definitions provide stable identities
for control commands, diagnostics, and reloads.

Provider packages own display behavior. An expression provider declares one
bounded in-process expression; a command provider declares an executable,
arguments, output protocol, and timeout. Expressions read the current event,
per-provider trigger state, and settings. Command providers run externally and
are supervised by the runtime.

Source packages own long-lived external event adapters. A source emits
versioned JSON Lines event envelopes and is shared by subscribers according to
its declared scope. Sources do not write widget text.

## Runtime model

Every connected output owns one `Bar`. A bar owns one widget instance for every
layout entry. Display instances use `(OutputId, WidgetId)` runtime keys.

Provider state is per display instance. Command providers receive output,
widget, and scale environment values. A global custom source is shared across
bars; an output-scoped source has one process for each subscribing output.

```text
Output: Discovered -> Ready -> Removing -> Removed
Bar:    Created -> AwaitingConfigure -> Mapped -> Closing -> Closed
```

Bar creation uses `wlr-layer-shell-unstable-v1` with top, left, and right
anchors, and a height derived from the font and vertical padding.

## Triggers

Provider triggers are normalized internally into a source kind, stable trigger
ID, selector, and optional filters. The public manifest avoids a generic
all-fields subscription table and instead uses source-specific tables:

- `triggers.on_dbus`: session or system bus plus a D-Bus match rule;
- `triggers.on_netlink`: Linux route family;
- `triggers.on_uevent`: optional subsystem and action filters;
- `triggers.on_pipewire`: a supported PipeWire event;
- `triggers.on_wayland`: `toplevel` or `workspace`;
- `triggers.on_source`: a named custom source event.

Native listeners are started only while an active display uses the matching
trigger. Trigger bursts retain the newest context and are debounced per
provider instance. The latest event is available as `context`; the most recent event for every
trigger ID is available through `state`. Command providers receive `context` as
one JSON argv value.

## Custom sources

A custom source has a persistent child process and writes JSON Lines to stdout:

```json
{"version":1,"event":"changed","data":{"value":"example"}}
```

The runtime checks the version and that the source declared `changed`, then
adds the source ID to the context before routing it to matching provider
triggers. Source output is bounded to 64 KiB per line. Invalid JSON, unknown
fields, unsupported versions, and undeclared events are logged and ignored.

Source process groups use parent-death notification and a dedicated reaper.
They restart with capped exponential backoff after exit. A source is stopped
immediately when its last subscriber disappears after output removal or reload.

## Rendering

Each output builds a bounded, single-row `CellFrame` over the available
horizontal surface width, then places left, center, and right display values
into it. The primary font's `0` is shaped with `font.cell_width_adjust` as
letter spacing; its resulting advance sets the fixed logical-pixel cell width
(at least one pixel). Use a monospace primary
font to avoid clipping proportional glyphs. Glyphs are centered in their cells.
The grid is centered between the horizontal surface padding. The bar height is
`ceil(font.size * font.line_height) + 2 * padding.vertical`. The center region is centered
in the grid; left and right grow inward. When regions compete for space, right
has priority over left, then center is clipped; visible regions remain at least
one cell apart. Displays within each region are adjacent unless a space display
is explicitly configured.

Display values are padded to `min_width` or truncated to `max_width` in
terminal cell widths without changing bar height. `align` controls placement
inside the padded text. Grapheme clusters occupy their standard Unicode cell
width: ASCII and Private Use Area (PUA) icons occupy one cell, while CJK
fullwidth characters occupy two cells with continuation cells and are omitted
if only part fits. Each cell carries its resolved style and display identity.
Empty grid cells are unowned. Control characters display as spaces rather than
introducing rows. The renderer uses font fallback and integer output scaling,
placing glyphs in their assigned cells while allowing natural horizontal ink
overhang across adjacent cells without clipping. Display backgrounds cover their
cells and the full bar height, while the bar background and rounded corners
remain pixel based.

Expression and command providers return one strict markup value per update.
Text, style, image, and interaction metadata share the same parsed
intermediate representation through layout and hit-testing. External sources
use versioned JSON Lines for structured event data; source events are not
rendered directly as widget output. Unsupported terminal control sequences are
not interpreted.

## Reload and shutdown

Reload builds a complete candidate before applying it:

1. read the main configuration, its theme file, and only referenced
   provider/source manifests;
2. deserialize with unknown-field rejection;
3. apply defaults, validate theme references, settings, trigger selectors,
   expressions, command placeholders, and source references;
4. compile widget definitions, resolved styles, and resolved command arguments;
5. replace active bars only after validation succeeds.

Apply stops prior provider children and no-longer-needed sources, rebuilds
trigger registrations, reconciles sources, then redraws bars. Spawn failure is a
runtime failure and does not roll back a valid configuration.

Shutdown stops accepting new work, stops provider and source process groups,
unmaps bars, releases event resources, and removes the user-only control socket.

## Testing boundaries

Unit tests cover configuration rejection and defaults, expression evaluation,
command placeholder and output decoding, source event envelopes, trigger
compilation, Unicode clipping, and deadline selection. Integration testing under
a nested compositor should cover output hotplug, native trigger routing,
custom-source fanout, provider cleanup, focus neutrality, and transactional
reload.
