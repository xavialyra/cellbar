# Configuration

[README](../README.md) · [Quickstart](quickstart.md)

Cellbar reads `config.toml` from:

1. CLI argument: `cellbar -c <path>` or `cellbar <path>`
2. `$XDG_CONFIG_HOME/cellbar/config.toml`
3. `$HOME/.config/cellbar/config.toml`

Cellbar is configured via named bar definitions `[bar.<id>]` and a top-level list `show = [...]` that controls which bars are initially visible. All defined bars are loaded and can be toggled dynamically at runtime via the control socket. Each bar independently declares its theme file, output monitor(s), screen position (`"top"` or `"bottom"`), margins, and its own `left`, `center`, and `right` widget layouts. Themes can be shared across bars or customized per bar. Surface dimensions are logical pixels; widget widths are character cells.

```toml
# Choose which bars are initially visible (defaults to all defined bars if omitted; use show = [] to start hidden)
show = ["top"]

[bar.top]
theme = "themes/nord.toml"
height = 32                 # Optional: explicit logical height (cells automatically center; clamped by cell_height)
position = "top"
outputs = ["DP-1"]          # Optional: omit to display on all connected monitors
margin = { top = 8, right = 12, bottom = 8, left = 12 }
left = ["[cellbar](@brand)", "[  ]"]
center = ["clock"]
right = ["memory", "[ │ ](@separator)", "cpu"]

[bar.dock]
theme = "themes/dock.toml"  # Can point to the same theme or a different theme
position = "bottom"
outputs = ["DP-1"]
margin = { top = 0, right = 0, bottom = 8, left = 0 }
center = ["media", "[  ]", "volume"]
```

A theme file has this shape:

```toml
[font]
# Font families and optional inline character/range mappings.
# Entries are tried in order; a mapping participates only for matching characters.
families = [
  { "U+E000-U+F8FF" = "Symbols Nerd Font Mono" },
  "JetBrains Mono",
  "Noto Sans CJK SC",
]
size = 12.0
cell_width_adjust = 0
line_height = 1.25

[surface]
background = "base"
padding = { horizontal = 8, vertical = 6 }
radius = 8

[colors]
base = "#181a1f"
text = "#d8dee9"
muted = "#7f8490"
accent = "#88c0d0"

[text]
foreground = "text"

[styles.brand]
foreground = "accent"
bold = true

[styles.clock]
foreground = "text"
bold = true

[styles.muted]
foreground = "muted"
```

Configured font families are ordered: the first is the primary family and the
rest are fallbacks. Regular fallback faces are loaded for character coverage;
bold and italic fallback variants are loaded only when shaped text actually
uses that family with the corresponding style. For example, a bold ASCII clock
does not preload a Chinese fallback's bold face. Loaded variants are retained
for reuse until the renderer is replaced (for example, on configuration reload).
Primary-family variants are still selected from the configured theme styles.
The primary font's `0` is shaped via `swash` and its horizontal advance is
adjusted by `cell_width_adjust` before rounding to the fixed grid width.
The adjustment is a signed number of logical pixels (default `0`); the final
cell width is at least one pixel. Negative values make neighbouring glyphs
overlap. Glyphs are centered
in their assigned cells. One cell is `ceil(font.size * font.line_height)`
logical pixels tall; `font.line_height` defaults to `1.25`, so the default cell
is unchanged. That single cell height is shared by the bar surface (plus twice
`surface.padding.vertical`), the styled background boxes, the image viewport,
and vertical glyph placement, so a glyph cannot drift out of the cell it is
painted over.
Raising `line_height` moves text away from the cell edges, but
`surface.padding.vertical` is usually the better knob for a roomier bar.
Lowering it below the font's own line box starts clipping glyph ink. A taller
cell also stops trimming line-drawing characters: a font's `│` is about
`1.52em` tall, so once the cell exceeds that the separator becomes shorter than
its cell background.
Glyph ink is bounded by the surface horizontally, so a glyph whose ink is wider
than its advance paints its full shape instead of being cut off at the cell
edge; Nerd Font icons rely on this and it matches terminal behaviour.
Vertically it is bounded by the cell, so line- and box-drawing characters such
as `│`, which fonts draw taller than a line box so they can join across rows,
end flush with the cell the way a terminal's own box drawing does.
Horizontal padding sets a minimum inset for the full-row grid, with any
leftover sub-cell pixels distributed to both sides. Both padding dimensions
default to 8 logical pixels. Regions stay at least one cell apart when they
compete for space. Displays within a region are adjacent; add an explicit
space display in the layout to separate them. Character cell widths strictly
follow standard Unicode widths (`unicode-width`): ASCII characters and Private
Use Area (PUA) symbols (including Nerd Font icons and Powerline caps) occupy one
cell, while East Asian fullwidth characters occupy two cells. Because horizontal
glyph ink is bounded by the surface rather than clipped at individual cell
edges, a wider proportional icon still paints in full without being sliced off;
leave an explicit space after wide non-mono icons if needed to prevent visual
overlap.

`bar.margin` controls the external top, right, bottom, and left margins. The
bar is top-anchored; top and side margins position and size the layer surface.
`surface.radius` clips the ARGB buffer to a rounded rectangle; `0` keeps the
bar rectangular. `surface.mode` selects the surface presentation strategy:
`"flat"` (default) paints a continuous bar surface where cell backgrounds act as
inline rectangular highlights, while `"capsules"` renders each display widget as
an independent rounded pill with cell-aligned padding and gaps configured via
`[surface.capsule]` (`background`, `radius`, `padding`, and `gap`, where
`padding` and `gap` are measured in character cells and default to `1`).
Color values accept `#RRGGBB` and `#RRGGBBAA`. Style color
fields can use a hex value or a name from `[colors]`. Supported style attributes are
`foreground`, `background`, `bold`, `italic`, `underline`, and
`strikethrough`. The main configuration and the theme both use the same
markup grammar: every renderable value is a sequence of nodes such as
`[text]` and `[text](@style)`, `#target{ ... }`, `#target(@style){ ... }`,
`#(@style){ ... }`, and `![path](options)`.
Inner nodes without an explicit `(@style)` inherit the enclosing scope's style.
In `"capsules"` mode, a display widget forms a capsule if and only if its
content is fully enclosed within a single root scope with a style (e.g.
`#widget(@pill){ ... }` or `#(@pill){ ... }`). The capsule shell background is
determined strictly and exclusively by that outermost root scope's style
background. Content that is not fully enclosed within a single root scope does
not form a capsule and renders with normal flat cell styling. Inner nodes with
an explicit `(@style)` override render as embedded cell badge highlights inside
the capsule.
Whitespace outside a node is formatting only. A provider emits one markup line
per value; a static `text` display and every non-display layout entry are
parsed as the same markup, so a literal spacer is written `"[  ]"` and a
styled separator `"[ │ ](@separator)"`. Plain text cannot appear outside a
node. `[text]` is the base text style: any span without a
`(@name)` style, or with an unknown one, uses it. It accepts the same
attributes as `[styles.<name>]`, including its own `background` (a highlight
behind the glyphs, distinct from `[surface] background`, which fills the bar),
`inset_y` (vertical inner inset in logical pixels; defaults to 0, accumulating along nested scope stacks),
and `radius` (corner radius in logical pixels for antialiased micro-container pill/badge edges).
In capsule or transparent mode, active capsule and widget boundaries automatically form the Wayland
input region, allowing mouse clicks in hollowed-out gaps or floating margins to seamlessly pass through
to underlying desktop windows.
`[styles.<name>]` entries are purely named overlays referenced from markup.
Cellbar adopts a pure layout architecture for `config.toml`: the configuration file strictly defines
the bar windows, geometry, themes, and `left`/`center`/`right` layouts. Static text and spacers are written
directly inline as markup literals (e.g. `"[  ]"`, `"[ │ ](@separator)"`, or `"[cellbar](@brand)"`).
All dynamic components (such as `workspaces`, `clock`, `volume`, `network`) are completely self-contained;
their settings, templates, update triggers, and actions are configured directly inside their respective
directories under `components/<name>/manifest.toml`.
Text nodes in markup can declare their own layout attributes inside parentheses:
`[content](@style max=40 min=10 left)`.
Text-level constraints are applied directly to the specific text span, truncating long strings with an ellipsis
and padding short strings according to their alignment without disturbing neighbouring icons, spacers, or scope paddings.
Scope containers strictly clip all rendered glyphs, decorations,
and images to their physical bounding box, preventing wide icons or overhang ink from escaping container edges.
Theme and main configuration files reject unknown fields and are validated
together before reload.

The bundled `clock`, `memory`, `cpu`, `battery`, and `backlight` components are
ordinary expression components. They consume the built-in `clock`, `memory`, `cpu`,
`battery`, and `backlight` state sources; no process is started for them. Their
manifests live in the `components/<component-id>/manifest.toml` directory.
`battery` and `backlight` also listen to kernel Netlink `uevent`
notifications for instant updates when AC power is connected or brightness is adjusted.

The built-in source intervals default to 60 seconds for `clock`, 10 seconds for
`memory`, 3 seconds for `cpu`, 30 seconds for `battery`, and 60 seconds for
`backlight`. A component can override its source interval with the
`interval` setting.

## Control

The layer surface always uses Wayland keyboard interactivity `none`; global
shortcuts are managed by your Wayland compositor. Cellbar provides a CLI and
Unix Datagram control socket for runtime inspection and control:

```sh
# Validate configuration without starting the bar
cellbar check
cellbar check /path/to/config.toml

# Query running bar state (outputs, scale, active widgets) as JSON
cellbar status

# List all active and dormant bars in a concise table
cellbar list

# Check bar visibility in scripts (exit code 0 = true, 1 = false)
cellbar is-visible top
cellbar is-hidden dock

# Validate and atomically reload configuration, themes, and providers
cellbar reload

# Hide, show, or toggle all bars or a specific named bar
cellbar hide
cellbar show
cellbar toggle
cellbar toggle dock
cellbar hide top

# Force an immediate refresh of a configured display widget
cellbar refresh clock
cellbar refresh media

# Push ephemeral rich markup directly into a display widget and clear it later
cellbar push window '[ 󰔛 Building release... ](@warning)'
cellbar clear window

# Inject an event payload into active trigger subscriptions matching <event-id>
cellbar emit mpris-properties '{"custom":"data"}'
```

- **`check [PATH]`**: Validates `config.toml`, `theme.toml`, and all referenced
  provider manifests offline.
- **`status`**: Prints a JSON object containing `pid`, `config`, and connected
  `outputs` (each with `name`, `position`, `layer`, `hidden`, `scale`, `configured`, and rendered `widgets`).
- **`list`**: Prints a formatted table showing `BAR`, `OUTPUT`, `POSITION`, `LAYER`, and `STATUS` (`visible` / `hidden`).
- **`is-visible <bar-id>` / `is-hidden <bar-id>`**: Prints `true` and exits with code `0` if the condition holds, or prints `false` and exits with code `1` otherwise (ideal for compositor keybinding scripts).
- **`reload`**: Builds and validates a complete candidate configuration before
  replacing active bars, providers, subscriptions, and sources. Invalid reloads
  keep the existing bar running unchanged.
- **`hide [bar-id]` / `show [bar-id]` / `toggle [bar-id]`**: Unmaps or remaps the
  layer-shell surface and releases or restores its exclusive screen zone. When `[bar-id]`
  is provided, acts only on the targeted bar; otherwise acts on all active bars. Ideal
  for compositor keybindings (e.g. `Super + B` to toggle fullscreen workspace room,
  or `Super + D` to toggle a floating bottom dock).
- **`refresh <widget-id>`**: Immediately re-evaluates or re-runs the provider
  backing `<widget-id>`.
- **`push <widget-id> <markup>` / `clear <widget-id>`**: Overrides `<widget-id>`
  with custom markup (or injects content into an empty `text = ""` placeholder
  widget) until `clear <widget-id>` restores its normal output.
- **`emit <event-id> [data]`**: Dispatches an event to all active provider
  triggers whose trigger `id` equals `<event-id>`.

The control socket is located at `$XDG_RUNTIME_DIR/cellbar/control.sock`.
