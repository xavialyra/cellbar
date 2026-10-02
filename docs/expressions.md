# Expression Language Reference

[README](../README.md) · [Component Context](context.md) · [Providers](providers.md)

Cellbar expressions provide an in-process, zero-overhead computation engine for
transforming source events and settings into formatted markup. They evaluate
synchronously in microseconds without spawning external processes or shells, and
guarantee safety against infinite loops and unauthorized I/O.

---

## Evaluation Model & Scope

Expressions are purely functional transformations executed on event delivery:
`Result<DynValue, String>`.

Every expression operates within an environment populated with three standard variables:

| Variable | Description |
| --- | --- |
| `context` | The latest event payload delivered to this component instance (or `null` before first arrival). |
| `settings` | A map of configuration options defined in the component's `[settings]` block. |
| `state` | A map storing the latest event payload received for each configured trigger ID (e.g., `state["tick"]`). |

Local variables can be declared using `let`:
```javascript
let title = context?.message?.title ?? "Desktop";
title != "" ? `[${title}](@window)` : ""
```

---

## Syntax and Operators

### Literals & Data Types

- **Numbers**: Integers and floating-point values (e.g., `42`, `3.14`).
- **Booleans**: `true`, `false`.
- **Null**: `null`.
- **Strings**: Double-quoted strings with standard escapes (`"Hello \"world\"\n"`).
- **Template Literals**: Backtick strings supporting `${expr}` and `{ident}` interpolation:
  ```javascript
  `#ws:${w.id}{ [${w.name}](@accent) }`
  ```
- **Lists**: Square-bracketed arrays: `[1, 2, "three", true]`.
- **Maps**: Key-value mappings: `{ key: value, other: 123 }`.
  - **Shorthand properties**: `{ title, count }` expands to `{ title: title, count: count }`.
  - **String keys**: `{ "complex-key": 42 }`.

### Navigation & Safe Access

Expressions provide robust safe navigation operators to handle optional or asynchronous Wayland and hardware events cleanly:

#### Optional Chaining (`?.`)
Safely navigates properties, indexing, or method calls. If the target is `null`, evaluation immediately short-circuits to `null` without error:
- **Field access**: `context?.message?.title`
- **Indexed access**: `context?.workspaces?.[0]`
- **Method invocation**: `context?.title?.replace("foo", "bar")`

#### Nullish Coalescing (`??`)
Returns the right-hand operand only when the left-hand operand evaluates to `null`:
```javascript
let title = context?.message?.title ?? "Desktop";
```
*Note*: Unlike the logical OR operator (`||`), nullish coalescing does not fall back on empty strings (`""`), `0`, or `false`:
- `"" ?? "default"` &rarr; `""` (preserves explicit empty string)
- `null ?? "default"` &rarr; `"default"`

#### Membership (`in` / `not in`)
Tests whether an item or key is contained within a collection or substring:
- **Lists**: `item in [1, 2, 3]` (element equality check)
- **Maps**: `"title" in context?.message` (checks key existence)
- **Strings**: `"bar" in "foobarbaz"` (substring inclusion)
- **Negation**: `item not in list` or `!(item in list)`

### Operators & Precedence

Listed from highest to lowest precedence:

1. **Member Access / Postfix**: `.`, `?.`, `[...]`, `?.[...]`, `func(...)`, `?.func(...)`
2. **Unary**: `!`, `-`, `not`
3. **Multiplicative / Additive**: `+`, `-`
4. **Comparison & Membership**: `<`, `<=`, `>`, `>=`, `in`, `not in`
5. **Equality**: `==`, `!=`
6. **Logical AND**: `&&`
7. **Logical OR**: `||`
8. **Nullish Coalescing**: `??`
9. **Conditional / Control Flow**: `if { ... } else { ... }`, `condition ? then_expr : else_expr`
10. **Assignment / Declaration**: `let name = expr`

---

## Control Flow & Functional Chaining

### Conditionals

Both ternary operators and Rust-style block `if / else` expressions are supported:

```javascript
// Ternary
let label = is_active ? "[Active](@accent)" : "[Idle](@muted)";

// Block if/else (evaluates to a value)
let icon = if percent > 80 {
    "󰂂"
} else if percent > 20 {
    "󰁿"
} else {
    "󰂃"
};
```

### Closures & Functional Methods

Collections support chainable functional methods with closures in either pipe (`|x| ...`) or arrow (`x => ...`) syntax:

```javascript
context?.message?.workspaces
    .filter(|w| !w.hidden)
    .map(|w| if w.active {
        `#ws:${w.id}{ [${w.name}](@accent) }`
    } else {
        `#ws:${w.id}{ [${w.name}](@muted) }`
    })
    .join("[ ]")
```

#### Supported Collection Methods
- `.filter(|item| predicate)`: Keeps items matching the predicate.
- `.map(|item| transform)`: Transforms each item.
- `.join(separator)`: Joins a list of strings into a single string.
- `.len()`: Returns length of string or list.
- `.replace(from, to)`: Replaces substrings.
- `.trim()`: Trims whitespace from both ends of a string.
- `.starts_with(prefix)` / `.ends_with(suffix)`: String prefix/suffix checks.

---

## Built-in Functions

### Template & Markup Rendering

#### `render(template, map)`
Replaces `{key}` placeholders in `template` with values from `map`. String values are automatically sanitized with `escape_markup_text` to prevent breaking bar layout formatting:
```javascript
render(settings.template, { time: strftime(context.data.timestamp, settings.strftime) })
```

#### `image(src, options)`
Generates an image element token for Cellbar's renderer:
```javascript
image(settings.icon, { width: 2, fit: "contain", shape: "circle" })
```
- `width`: Logical cell width (integer).
- `fit`: `"contain"` (default) or `"cover"`.
- `shape`: `"rect"` (default) or `"circle"`.

### Formatter Functions

- **`strftime(unix_timestamp, format_string)`**: Formats a Unix timestamp (seconds) into local time.
- **`format_percent(number)`**: Formats numbers into a percentage string (e.g. `42` &rarr; `"42%"`).
- **`format_size(kibibytes)`**: Formats KiB values into human-readable units (e.g., `8192000` &rarr; `"7.8G"`).
- **`format_memory(template, used_kib, total_kib, avail_kib, percent)`**: Substitutes `{used}`, `{total}`, `{available}`, and `{percent}` placeholders in memory templates.

---

## Practical Examples

### 1. Active Window Title with Safe Navigation
Guards against compositor startup latency or empty title states:
```toml
[provider]
expression = '''
let title = context?.message?.title ?? "";
title != "" ? render(settings.template, { title }) : ""
'''

[settings]
template = "[{title}](@window max=50 min=20 left)"
```

### 2. Clock Provider
```toml
[provider]
expression = 'render(settings.template, { time: strftime(context.data.timestamp, settings.strftime) })'

[settings]
strftime = "%H:%M"
template = "[{time}](@clock)"
```

### 3. CPU Statistics
```toml
[provider]
expression = 'render(settings.template, { percent: format_percent(context.data.percent) })'

[settings]
template = "[CPU {percent}](@cpu)"
```

### 4. Memory Statistics
```toml
[provider]
expression = 'render(settings.template, { used: format_size(context.data.used_kib), percent: format_percent(context.data.percent) })'

[settings]
template = "[MEM {used}](@memory)"
```
