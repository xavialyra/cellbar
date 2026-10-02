# System Tray Provider for Cellbar

A lightweight, standard-compliant StatusNotifierItem (SNI / AppIndicator) system tray provider for Cellbar.

## Features

- **Standard Protocol Implementation**:
  - Implements the standard `org.kde.StatusNotifierWatcher` and `org.kde.StatusNotifierItem` D-Bus specifications.
  - Supports `org.ayatana.common` text labels (`XAyatanaLabel`).
  - Supports in-memory `IconPixmap` and system theme `IconName` (PNG and SVG).
  - Clean fallback to category/text glyphs when no image is available.
- **Event-Driven & Low Latency**:
  - Listens to D-Bus signals for items registering, unregistering, icon changes, status updates, and title changes.
- **Interactive Routing**:
  - Supports left-click (`Activate`), right-click (`ContextMenu` / `SecondaryActivate`), middle-click (`SecondaryActivate`), and mouse wheel scrolling (`Scroll`).
- **Decoupled High-Performance Streaming Daemon**:
  - `cellbar-tray` is a self-contained, high-performance C daemon that natively registers `org.kde.StatusNotifierWatcher`.
  - Converts item pixmaps directly into PNGs, dynamically resolves XDG desktop icons, and directly streams updated markup to stdout (`#tray:0{...}\n`).
  - True event-driven zero CPU usage at rest (sleeps in `sd_bus_wait`), taking under ~250 KB private dirty memory with no external Python runtime or script overhead.

## Settings

- `icon_mode`: `"image"` (default, uses high-fidelity images with text fallback) or `"font"` (pure text/glyph mode).
- `icon_width`: Reserved cell width for tray images (default: `2`).
- `icon_fit`: Image fitting mode: `"contain"` (default), `"scaledown"`, or `"textmatch"`.
- `spacing`: Markup spacer string between tray icons (default: `"[ ]"` for a 1-cell gap). Bare strings are automatically wrapped in `[...]`.
- `hide_passive`: Hide items whose `Status` is `"Passive"` (default: `true`).

## Actions

- Left click: `Activate(0, 0)`
- Right click: `ContextMenu(0, 0)` (fallback to `SecondaryActivate`)
- Middle click: `SecondaryActivate(0, 0)`
- Wheel scroll: `Scroll(delta, orientation)`
