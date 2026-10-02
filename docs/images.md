# Static Images in the CellFrame (Design)

Status: **implemented for static PNG, JPEG, and WebP**. This document defines
the public syntax and rendering contract. SVG, animation, remote URLs,
terminal escape/Kitty protocol, and embedded/base64 image transfer remain out
of scope. Images extend the single-row topbar without becoming independently
positioned layers.

## Scope and ownership

- Support local static PNG, JPEG, and WebP files. Reject animated inputs rather
  than silently displaying a frame. No SVG, animation, remote URL,
  terminal escape/Kitty protocol, or embedded/base64 image transfer in v1.
- A provider returns ordered text/image content. Layout, display width rules,
  ownership, clipping, and redraw decisions pass through the same `CellFrame`
  used for text. The renderer only converts occupied cells to pixels; it does
  not reserve independent pixel coordinates or manage a separate overlay.
- A provider output replaces its previous value atomically. Invalid output
  keeps the last valid value, consistent with existing provider errors.
- Theme styles still supply the display foreground and background. An image's
  transparent pixels reveal the display background; the bar's rounded corner
  mask still applies. Images are not text backgrounds or text overlays.

## Command provider output

Command providers emit one complete strict markup value per stdout line. There
is no provider protocol field and no JSONL or ANSI rendering branch. Text nodes
use `[text]` or `[text](@style)`, interaction scopes use `#target{ ... }`, and
images use `![path](options)`:

```toml
[provider]
program = "./run.sh"
timeout = "2s"
```

```text
#cover{ ![/home/user/.cache/cover.png](2 cover circle fallback=[art]) [ Playing](@media) }
```

Image options are `width`, `fit`, `shape`, `align`, and `fallback`. Width is
reserved in terminal cells, paths must be local, and relative paths resolve
against the provider manifest directory. Invalid markup or image options keep
the last valid provider value. The existing provider line-size limit applies;
image bytes never travel through stdout.

## Expression and static display syntax

The built-in expression evaluator returns an ordered rich value, never an
encoded control string. String results remain supported unchanged. `+`
concatenates text and image parts in order; `image` performs no I/O inside the
expression evaluator.

```toml
[provider]
expression = 'image(settings.icon, { width: 2, fit: "textmatch", shape: "circle", fallback: "*" }) + " [" + format_percent(context.data.percent) + "](@cpu)"'

[settings]
icon = "assets/battery.png"
```

`image(src)` and `image(src, width)` use the same defaults as markup image
nodes. An options map accepts `width`, `fit`, `shape`, `align`, and `fallback` with
the same validation.
Strings are text components when concatenated with a rich value. Expressions
remain bounded by the built-in expression execution limits; the rich result also
needs explicit limits on segment count and total text length. Relative paths
resolve against the provider manifest directory, as for command output.

A static display can use an image instead of `text` or `provider`:

```toml
[display.logo]
image = "assets/logo.png"
width = 2
fit = "textmatch"
shape = "circle"
image_align = "center"
fallback = "*"
```

Its path resolves against the main config directory. A static image shares
the same `width`, `fit`, `shape`, and `fallback` defaults. Its image-internal horizontal
alignment is `image_align` (default `center`); the existing display `align`
continues to control padding inside `min_width`. The display's `max_width`
and width padding rules still apply. `image`, `text`, and `provider` are mutually
exclusive display sources. Static config errors fail candidate validation; an
unreadable or corrupt file at runtime uses its fallback.

## Fit and placement

Let the complete reserved viewport be `width * cell_width * output_scale` by
the text-line height in physical pixels. Horizontal origin comes from the
`CellFrame` columns, including bar padding/grid centering. All modes center
vertically in the line viewport except `textmatch`:

- `contain`: preserve aspect ratio, show the entire image, leave unused area
  transparent.
- `cover`: preserve aspect ratio, fill the viewport, crop overflow to the
  viewport before applying cell visibility clipping.
- `stretch`: fill the viewport without preserving aspect ratio.
- `scaledown`: like `contain`, but never enlarge above source pixel dimensions
  in physical pixels; center any unused area.
- `textmatch`: `contain` within a cap-height-sized box with its bottom aligned
  to the primary font baseline. Font metrics must be measured from the actual
  selected face; use a documented line-height fallback if cap height cannot
  be determined. No implicit per-image padding outside this box.

`align` controls the horizontal position of a smaller image and the horizontal
crop origin of `cover`. With `shape = "circle"`, the fit viewport is a square
with diameter equal to the smaller side of the complete reserved image box.
The square follows horizontal `align` and is vertically centered; for
`textmatch`, it fits within a cap-height box whose bottom follows the text
baseline. Fit and scaling occur within this square, followed by a circular
alpha mask with 4x4 subpixel coverage at the edge. The mask uses the complete
image placement, even if the visible CellFrame columns clip part of it.
`shape = "rect"` preserves the existing rectangular viewport. Fallback text
is not masked. Source orientation/metadata handling must be consistent
across supported decoders. Zero dimensions are invalid. Round target pixel
bounds consistently to avoid gaps at cell boundaries; output scale changes
invalidate scale-specific raster data.

## CellFrame invariants

Provider values become ordered text/image parts before display-level width
constraints are applied. Each image reserves `width` consecutive columns,
regardless of load state. Each occupied cell records its owner, style, resource
identity, full span width, and slice index. Resource pixel buffers live outside
the frame; adjacent images of the same file remain distinct placements.

The layout can clip an image to visible whole **columns**, including on the
left or right. Unlike an indivisible wide grapheme (currently omitted when
only part fits), the surviving image slices retain their original slice indices
and theoretical full-span origin. Fit is computed against the full viewport;
then pixels are clipped to surviving columns. This avoids stretching a cropped
image to the remaining width. No image pixels may escape a cell owned by that
image. `max_width` can similarly clip an image within a display; `min_width`
adds cells using existing alignment rules, never changing the image's own
declared width. Text grapheme clipping remains unchanged.

For drawing, coalesce consecutive slices of **one placement** into a single
image run. Keep the original placement geometry even when its anchor cell is
clipped; use signed coordinates for off-frame origins. Fill the display cell
background first, then alpha-blend the image, applying the same rounded bar
mask as text. The current SHM canvas stores premultiplied ARGB32 in native byte
order, not straight RGBA bytes. Conversion and blending must match that format
and its existing background/glyph helpers.

Pending/error fallback text is clipped to the reserved columns using normal
grapheme rules. When the decoded image becomes ready, the runtime must mark its
bars dirty and repaint even if the `CellFrame` cells and geometry are unchanged:
the current bar skips identical frames. An image generation/version in the
frame comparison or an explicit raster-dirty flag can supply this invalidation.
A provider update with a different image path refreshes the resource; repeated
output of the same path reuses decoded and scaled pixels. Providers that rewrite
an image at the same path must change the path or request a configuration
reload. The media example uses a content-derived cache path for downloaded art.
Static file replacement is picked up on configuration reload; automatic file
watching is deferred so the calloop path never performs blocking metadata checks.

## Loading, limits, and errors

File I/O, decoding, and resizing must not block the calloop dispatch path.
Use bounded background work and deliver readiness/failure back to the owning
bar(s). Pending images show fallback within their fixed columns. Cache scaled
results by resource version and target physical dimensions; bound total scaled
cache bytes and cap tracked paths. A bounded result channel prevents decoded
pixels from accumulating without limit when the event loop is busy.
An independent decode limit must cap compressed file size, decoded pixel count,
dimensions, concurrent jobs, and total resident bytes. Reject excess inputs
before allocating decoded buffers wherever possible. A failed path is not
retried in a tight loop; a changed image path or configuration reload retries.
Reload clears image references; shutting down the store stops its worker. The
implementation currently limits files
to 8 MiB, dimensions to 2048 pixels per side, decoded images to 2 million
pixels, and scaled cache data to 16 MiB. These limits remain subject to
measurement and may be made configurable later.

No fixed RSS/CPU/binary-size promise is made by this document. Before/after
measurements should cover idle, first decode, steady redraw, many changing
paths, large/corrupt inputs, multi-output scaling, and reload. Supporting
static images creates no animation timer; existing providers can still update
their output on their configured triggers.

## Delivery and tests

The typed content model, strict markup nodes, CellFrame image occupancy,
rich expression results, static display parsing, bounded decoding/cache, and pixel
composition are implemented. Tests cover interleaved text/images, region
competition, clipping, width constraints, markup validation, alpha,
rounded corners, fallback, readiness invalidation, and output-scale-aware
geometry. Resource measurements still need to be collected under real desktop
loads.
