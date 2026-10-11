# CADCraft control protocol

Start the desktop app with `--control PORT` (or `CADCRAFT_CONTROL_PORT=PORT`). It listens on
`127.0.0.1:PORT` for JSON lines: one request per line, one reply per line.

```json
{"id": 1, "method": "cmdline.input", "params": {"text": "circle 0,0 5"}}
{"id": 1, "ok": true, "result": {"prompt": "Command:", "output": ["Command: CIRCLE"], ...}}
```

Errors come back as `{"id": …, "ok": false, "error": "message"}`.

The server waits up to 60 s for the app to answer a request. After that it replies with a timeout, and
`state` tells whether sending the request again is safe:

- `{"id": …, "ok": false, "error": "timeout", "state": "not-run"}`: the app had not started the request.
  It is cancelled and will never run, so sending it again is safe.
- `{"id": …, "ok": false, "error": "timeout", "state": "may-have-run"}`: the app had already started it.
  It may still finish and change the drawing, and its late result is discarded. Inspect the drawing
  (`drawing.inspect`) before sending it again.

| Method | Params | What it does |
|---|---|---|
| `cmdline.input` | `{text}` | Type a line at the command line exactly like a user. Starts commands by name/alias and answers prompts (points `x,y`, `@dx,dy`, `@d<a`, distances, keywords, empty = Enter). Spaces act as Enter except at text prompts. |
| `cmdline.script` | `{text}` | Run a multi-line script (like `.scr` files). |
| `cmdline.key` | `{key: "enter"\|"escape"}` | Press Enter or Escape. Replies like `cmdline.input`: the state, `output`, and `error` when Enter was refused. |
| `cmdline.state` | | Current prompt, keywords, accepted input kinds, history tail. |
| `engine.execute` | `{command, params}` | Run any command with JSON parameters. Never opens a dialog. |
| `engine.commands` | | Every command: id, label, menu path, shortcut, aliases, params doc, enabled. |
| `drawing.inspect` | `{entities?, limit?}` | Drawing summary (counts, extents, layers, styles, blocks, layouts, selection, undo) and entities with handles and geometry. |
| `ui.inspect` | | UI state, canvas rect, view, cursor and snap, performance counters; `closePrompt`: the drawing a "Save changes?" prompt is asking about (null when none); and actual egui keyboard focus (`focus.id`, `focus.label`, `focus.role`). |
| `ui.elements` | `{label?, role?, focused?}` | Observed toolbar, tab, drafting and layer-palette controls from the latest UI frame. Filters match exactly. Each entry has its real egui `id`, label, role, rectangle `[x,y,width,height]`, enabled/visible/selected state and current focus. |
| `ui.menu.list` / `ui.menu.invoke` | `{command}` | The menu tree; invoke an item like a click (interactive). |
| `ui.pointer` | `{x, y, space?: "world"\|"screen", button?, action?: "click"\|"move"}` | Click in the drawing area (world coordinates by default). Right button = Enter. |
| `ui.click` / `ui.move` | `{x, y, button?, shift?}` | Real egui pointer events in screen points (reach every widget). |
| `ui.drag` | `{x, y, to: [x, y], button?}` | Real press, move and release (drag the command line's top edge, the ViewCube ring…). |
| `ui.key` / `ui.text` | `{key, cmd?, shift?, alt?}` / `{text}` | Synthetic keyboard input. |
| `ui.set` | UiState fields | Show/hide palettes, toolbars, command line, ViewCube…; `theme: "system"\|"light"\|"dark"` (also `engine.execute` with `ui.theme {theme}`); `ui.inspect` reports the theme shown (`theme`: light/dark). `saveFormat: "dxf"\|"dwg"` (also `ui.saveformat {format}`) is the format Save As suggests for new drawings. |
| `ui.resize` | `{width, height}` | Resize the window. |
| `ui.focus` | | Activate the native window. Does not assign focus to a widget. |
| `ui.screenshot` | `{path?}` | PNG of the window (needs a presented frame). |
| `ui.render` | `{path?, width?, height?, fit?}` | Headless render. `fit` (default true) frames the whole drawing; `false` uses the current view. Replies `{pngBase64, width, height, bytes, path?}`. |
| `app.open` / `app.save` / `app.quit` | `{path}` | File operations. |

The MCP server (`cadcraft-cli mcp --connect 127.0.0.1:PORT`) wraps this protocol; headless
(`cadcraft-cli mcp`) it serves the same methods from an in-process session.

`ui.key` queues real egui key events; its reply acknowledges the queue. Poll `ui.inspect` or
`ui.elements` for the resulting focus or state before sending the next action. Use Tab and
Shift-Tab to traverse controls and Enter/Space to activate them. The observed registry is not
the operating system's accessibility tree and does not establish physical input or screen-reader
acceptance. Unregistered controls still report their actual focus ID, with a null label/role.

The native host restores layout choices from `workspace.json` and saves changes atomically.
It stores palette/toolset visibility, selected toolset, collapsed groups, command-line history
height and related chrome choices. It does not store drawings, command buffers, dialogs or the
current Start tab. The default directory is `~/Library/Application Support/CADCraft` on macOS,
`%APPDATA%/CADCraft` on Windows, and `$XDG_CONFIG_HOME/cadcraft` or `~/.config/cadcraft` elsewhere.
Set `CADCRAFT_CONFIG_DIR` to a private directory for an isolated profile; an explicitly empty
value disables workspace reads and writes. An invalid, oversized or newer-version file is
reported and left untouched, with persistence disabled for that launch. Repair or move that
file before restarting to resume saving workspace choices.

## Panel docking

Tool Sets, Layers, and Properties can be regrouped, split, floated within the app window, closed, and reopened. The drawing canvas stays visible and cannot be closed or turned into a tab. The document/session view is protected. Layouts, active tabs, panel visibility, and floating rectangles are saved with the app's existing UI preferences. These operations do not edit document contents.

Panel IDs: `toolSets`, `layers`, `properties`. The protected center is `canvas`.

`ui.dock` accepts `operation`; basic per-panel operations also require `panel`. The advanced `resizeSplit` operation below uses a split `path` instead of a panel. Basic operations are `open`, `close`, `activate`, `float` (with finite positive `rect: [x, y, width, height]`), or `move` (with `target` and `zone: center|left|right|top|bottom`). Center groups with the target; side zones split beside it. `ui.dock.reset` restores the default arrangement. Invalid moves return an error without changing the layout.

Invoke these UI commands through `engine.execute`, for example `{"command":"ui.dock","params":{"operation":"float","panel":"layers","rect":[100,100,320,440]}}`.

Advanced `ui.dock` operations use the same validated transaction as pointer gestures:
`moveFloating` updates `rect`, `resizeSplit` accepts a boolean child `path` and
`size` (`{"Ratio":0.5}`, `{"FixedFirst":280}`, or `{"FixedSecond":280}`),
`setStackOpen` takes `open`, and `resizeStack` takes a point `height` or null.
A center `move` can include `before` (panel ID or null to append) for tab order.
The equivalent `{"action": ...}` form accepts a serialized shared docking action;
this is also the form emitted by the renderer. Protected-canvas and app-specific panel
rules apply equally to both forms.

## Command-line replies

The app and the headless session reply with the same fields.

`cmdline.state`, `cmdline.key` and `cmdline.input` return the command-line state:

| Field | Value |
|---|---|
| `prompt` | The prompt text (`"Command:"` when idle). |
| `running` | The id of the running command, or `null`. |
| `keywords` | The current prompt's options. |
| `accept` | The kinds of input the current prompt accepts, or `null` when idle. |
| `buffer` | Text typed in the command line but not yet submitted (always `""` headless). |
| `history` | The last 20 history lines. |
| `historyExpanded` | Whether the history is expanded (always `false` headless). |

`cmdline.input` adds:

- `output`: the history lines the input added. This includes the error message when the line was refused.
- `error`: the error message, only when the line was refused (unknown command, invalid input…).

A refused line still replies `"ok": true`, because the text was typed. Check `error`.

`app.open` without a string `path` fails with `"missing path"` before any command runs.

## Interface language

Select a language with `engine.execute` and the UI command `ui.language`:

```json
{"id":1,"method":"engine.execute","params":{"command":"ui.language","params":{"lang":"uk"}}}
```

Supported values are `auto`, `en` and `uk`. `ui.set` also accepts
`{"interfaceLanguage":"uk"}`; `ui.inspect` reports the saved choice in its UI state.
Invalid choices leave the current language unchanged. Menus, command labels and prompt text
returned by the control channel retain their English source text. See [localization.md](localization.md).
