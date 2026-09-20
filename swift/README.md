# apex in a terminal, with TermKit

An experiment: apex's whole UI — windows, tags, the terminal, the
markdown viewer, the browser, the fuzzy finder, the menus — rewritten
against [TermKit](https://github.com/migueldeicaza/TermKit) so that apex
runs in a terminal instead of in a GPU window.

TermKit is Swift and apex is Rust, so the rewrite is in two pieces that
talk over a pipe:

```
  ┌─────────────────────────────┐          ┌──────────────────────────┐
  │  swift/ApexTUI (TermKit)    │  JSON    │  crates/apex-tui         │
  │  every view, every gesture  │◀────────▶│  apex-tuid: the replica  │
  │  the acme palette, drawing  │  a line  │  laid out on a cell grid │
  └─────────────────────────────┘  each    └───────────┬──────────────┘
                                   way                 │ attach protocol
                                                       ▼
                                              apexd — the session
```

This is the split DESIGN.md §1 already asks for — *state on the server,
rendering on the client* — taken one step further: `apex-tuid` is the
part of the old gpui client that was never about gpui (the replica, the
tiling, what a button means), and `ApexTUI` is the part that was only
ever about drawing.

## Why not one Swift program

A Swift client speaking the attach protocol directly would have to carry
a port of `apex-core` — the state machine, the log, the tiling, the Edit
language, and postcard — about twenty thousand lines that have nothing
to do with the UI. Rewriting *just the UI*, as asked, means the Swift
side gets the drawing and the input and nothing else.

The happy accident that makes this work is that `apex-core`'s tiling is
written against an abstract `Info` — a font height and how many lines a
tag and a body need. Set the font height to **1** and acme's whole
layout runs unchanged, in character cells. Not a line of `tiling.rs`
moved for this.

## What is where

| | |
|---|---|
| `crates/apex-tui/src/cells.rs` | the cell grid: `Info` at one cell a line, and the wrapping that puts a buffer on it |
| `crates/apex-tui/src/model.rs` | the view model: what a frame is |
| `crates/apex-tui/src/ui.rs` | the replica, hit testing, and acme's meaning for B1, B2, B3 and the chords |
| `crates/apex-tui/src/input.rs` | what the UI sends back |
| `swift/ApexTUI/Sources/ApexTUI/` | the UI |

Every component, and the TermKit it is built from:

| component | how |
|---|---|
| window, column, row | `WindowView`, `ColumnView`, `RowView`: custom `View`s laid out by the rectangles acme's tiling chose |
| tag | `TagView`: acme's cyan tag with the layout box, which is also the light that says dirty / a tool's / wanting you |
| text body | `TextBodyView`: acme's yellow paper, its selection, its scrollbar |
| terminal | `TerminalBodyView`: the session's own cell grid, painted (see below) |
| markdown viewer | TermKit's `MarkdownView`, in acme's colours, with acme's scrollbar |
| browser | `BrowserView` + `Html.swift`: a text browser — headings, lists, `<pre>`, and numbered links that B3 follows |
| fuzzy finder | `FinderView`: TermKit's `TextField` and `ListView`, ranked by `Fuzzy.swift` |
| session switcher | `SwitcherView`: TermKit's `ListView` |
| menus | TermKit's `MenuBar` — every entry does exactly what B2 on the same word in a tag does |
| dialogs | TermKit's `MessageBox` |
| theme | `Theme.swift`: plan9port's palette, as RGB where the terminal can and quantised where it cannot |

### The terminal is painted, not emulated

TermKit has a `TerminalView` that drives SwiftTerm over a byte stream.
apex does not have a byte stream to give it: the session runs the
emulator (DESIGN.md §2) and hands clients cells, which is what lets a
client detach and re-attach with the scrollback intact. Running a second
emulator over the first would lose that. So `TerminalBodyView` paints
the grid the session sends — colours as the program asked for them, the
cursor, and acme's scrollbar over the session's scrollback.

### acme's borders

The tiling leaves `BORDER` cells between a tag and its body, between
windows, and between columns — acme's three pixels of black, in cell
units. The UI draws them as rules, which is what they are.

## Building

The Rust half builds with the workspace:

```
cargo build -p apex-tui          # produces apex-tuid
```

The Swift half needs a Swift 6 toolchain:

```
cd swift/ApexTUI && swift build   # produces apex-tui
```

Then, with `apex-tuid` beside it or on `PATH` (or named by `$APEX_TUID`):

```
apex-tui                          # the local daemon, the last session
apex-tui --session work
apex-tui --remote user@host
APEX_THEME=dark apex-tui
```

`apex-tui` passes its arguments straight through to `apex-tuid`, so
`--session`, `--attach`, `--via`, `--remote` and a list of files all mean
what they mean everywhere else in apex.

## Using it

The three buttons are acme's: **B1** selects, **B2** executes what it
sweeps, **B3** looks it up. **B1 then B2** cuts, **B1 then B3** pastes.
The square at a tag's left drags the window; B2 on it grows it, B3 hides
the others. ^P opens the finder, ^Q leaves (the session stays running).

## What is not done

- **The wheel.** TermKit's `UnixDriver` does not decode wheel reports
  (`button & 64`), so on Linux the wheel arrives as a B1 press. Scroll
  with the scrollbar, PageUp/PageDown, or the `CursesDriver` on macOS,
  which does report it. Fixing this belongs upstream in TermKit.
- **Switching sessions** lists the daemon's sessions but does not yet
  re-attach to one; it needs `apex-tuid` to drop its attachment and take
  another, which the gpui client does through its window pool.
- **Web windows** are served with an empty page: `apex-tuid` does not yet
  fetch through the session's I/O plane (WEB.md §2), so only `Body::Html`
  windows — previews, `cmd | apex web` — render. The renderer itself is
  done and works on whatever HTML it is handed.
- **Warping the pointer.** Frames carry acme's warp; a terminal cannot
  move the pointer, so it is ignored.

## What is tested

`cargo test -p apex-tui` runs the view server against a real daemon over
a real socket and checks the frames a UI would draw and the gestures it
would send: a window landing on the grid with its tag and body, B1
sweeping a selection, a double click taking a word, B2 on `Del` closing
the window, B2 on a layout box growing it, typing and backspace, a
resize relaying the row out, long lines wrapping, the finder's
candidates, the clipboard and the snarf buffer, the wheel and the
scrollbar's thumb, B3 looking something up, and a `.md` window being
served as a page.

The Swift half has no tests and has not been compiled: this container
has no Swift toolchain, and `download.swift.org` is blocked by the
environment's egress policy. It is written against TermKit's actual API
(read from the checkout, not from memory), but expect a first `swift
build` to want fixing.
