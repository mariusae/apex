# Sessions, Tabs and Window Chrome

Everything the apex client draws *around* acme's tiled columns lives in a set of small modules in `crates/apex-client/src`. These modules cover the tabs and the pool of sessions behind them, the session picker, the sidebar and title bar, the ctrl-tab walk and the ⌘⇧\ overview, the stash shelf and column strips, window glides, notifications and the Dock, diagnostic toasts, process pills, the window's standing with its daemon, and restarting a daemon of another build. Most of these modules are `impl Acme` blocks that add methods to the one big client type described on [The UI Client (apex-ui)](client.md). A few hold app-wide state as gpui globals: `Pool` and the attention queue.

None of this chrome changes the replicated model, with two exceptions. Stashing goes through the node into the layout shard. Killing a process and taking a notification go to the server. Everything else is read from the session's replica (`Node`) every frame and painted. The tiling these pieces decorate is on [Tiling and Layout](tiling-and-layout.md). Leases, fencing and attachments, which the "standing" chip reports on, are on [Sessions, Shards and Leadership](sessions-and-replication.md). Input inside the windows is on [Mouse, Keyboard and Look](client-input.md), and the other pickers are on [Pickers and Overlays](client-overlays.md). The window statuses that the sidebar shows come from [Program Status Protocol (OSC 7501)](program-status.md).

## Module map

| Module | What it owns |
|---|---|
| `pool.rs` | `Pool` global: the app's tabs (`TabId`, `Tab`, `State`), the parked sessions, recency order, launching links in the background |
| `app.rs` (parts) | `switch_to`, `park`, `adopt_parked`, `leave`, `close_current_session`, `tab_word`, `tab_notified`, `other_badge`, `next_notification`, `tabs_tick`, the 100 ms tick |
| `shell.rs` (parts) | The key bindings, `SIDEBAR_W`, `pjw`, and the session picker (`Selector`, `Row`, `Host`, `Loading`), known hosts and sessions, recent sessions |
| `titlebar.rs` | Session name (click to rename), the chevron's session menu |
| `sidebar.rs` | Vertical tabs per host, the shown session's windows, stash and processes, the status lines of windows in every session, and the sidebar button's glyph and badge |
| `switcher.rs` | ctrl-tab walk with a slide, the ⌘⇧\ overview grid, sidebar row previews |
| `miniature.rs` | `Mini`: a session, window or column drawn small and live from a replica |
| `shelf.rs` | The stash as a fan of cards in the title bar, with a live preview |
| `strips.rs` | Stashed columns (sheet edges) and minimized columns (slim cards with handles); `dot_in`, the handle's dot for any replica |
| `glide.rs` | Animating windows and columns between layout positions |
| `attention.rs` | Cross-session notification queue, Dock badge and bounce, `NSBeep` |
| `toasts.rs` | Toasts for new text in diagnostic windows |
| `procs.rs`, `titlefit.rs` | Process pills and their hover card; fitting path and pills to the bar |
| `standing.rs` | Leading / watching / stalled / offline / coming; chip, banner, card, auto-reconnect |
| `restart.rs` | Offering and doing a restart of a mismatched local server or remote daemon |

Sources: [pool.rs:1-18](crates/apex-client/src/pool.rs#L1-L18), [standing.rs:1-17](crates/apex-client/src/standing.rs#L1-L17), [switcher.rs:1-18](crates/apex-client/src/switcher.rs#L1-L18), [shelf.rs:1-10](crates/apex-client/src/shelf.rs#L1-L10), [toasts.rs:1-17](crates/apex-client/src/toasts.rs#L1-L17), [titlefit.rs:1-12](crates/apex-client/src/titlefit.rs#L1-L12), [sidebar.rs:1-13](crates/apex-client/src/sidebar.rs#L1-L13), [strips.rs:213-219](crates/apex-client/src/strips.rs#L213-L219)

## Tabs and the session pool

### Tabs are the app's, sessions are the host's

A session's identity cannot be relied on to name a tab. If you end a session and attach to its label again, the host makes a new session with a new id. The pool therefore gives each tab its own `TabId(u64)`, which never changes and is never reused, and keeps the session's `SessionUrl` beside it in `Tab::url`, correcting it as attaches report more. Everything in the UI (keys, clicks, notifications) refers to a tab by `TabId`. `Pool::open` is the only place a URL is turned into a tab. It is also the only caller of `one_session`, which treats two URLs as the same session when they are equal, or when they have the same provider, host argument and label (so a re-made session, or a rename one side has not heard about yet, keeps its tab).

```rust
pub struct Tab {
    pub id: TabId,
    pub url: SessionUrl,
    pub state: State,   // Coming(Why) | Up | Down(String)
}
```

`State::word` gives the short word shown after a tab's name: "connecting…" or "restoring…" for `Coming(Why::Attaching | Why::Restoring)`, "offline" for `Down`, nothing for `Up`. `Why::sentence` gives the longer line shown on the blank page while a tab waits.

Tabs persist across launches. `Pool::install` reads one URL per line from the `open-sessions` file beside the client's state file. It keeps only URLs that carry an identity, and makes each a tab in state `Down("not attached")`, so the bar is right from the first frame. `save_tabs` rewrites the file whenever the tab list or a tab's URL changes. Parked sessions do not persist: "Nothing is parked across a launch."

Sources: [pool.rs:39-117](crates/apex-client/src/pool.rs#L39-L117), [pool.rs:186-238](crates/apex-client/src/pool.rs#L186-L238), [pool.rs:269-305](crates/apex-client/src/pool.rs#L269-L305)

### Parked sessions

apex has one window. When it switches away from a session, `Acme::park` takes the remote `Link`, `Log` and `Node` off the window, puts a blank in-process stand-in in their place, and returns a `Parked`. It returns `None` when the window is not on a connected remote link, since there is nothing to keep. The `Parked` holds the link, replica, URL, wake target, previews, live pages, snarfouts, a pending goto, and a `Seen` record. `Seen` stores the diagnostic text and notification times the window had already seen, so that showing the session again toasts and pings only what came in meanwhile. Parking also clears the window's toasts and pulled stash cards, which belong to the session. `Pool::park` stores the `Parked` under its tab and redirects the link's wake to the pool through `WakeTarget`: the reader thread holds a forwarding closure whose target can be swapped.

A parked session is still the leader of its shards. The pool's tending task (`Pool::tend`) runs whenever a parked link wakes it, and does the following:

- It polls each link, which applies incoming entries.
- It discards shows, and opens gotos in the first column (URLs via `Proposal::open_url`, files via `ClientMsg::OpenFile`).
- It answers the rules' client asks. Only `open` can be answered; anything else is refused with "the session is parked, nobody sees it".
- It clears pending I/O, then flushes and catches up.

After that it copies each parked session's metalog label onto its tab's URL, so a rename made elsewhere shows. A link that has ended leaves its tab in place with `Down("the link ended")`. Because a parked session keeps its replica, the sidebar can read its windows' statuses and notifications through `Pool::parked_nodes` while it is parked.

At most `CAP = 8` sessions stay parked. Beyond that, the least recently parked link is closed. Its tab stays, marked `Down("let go: 8 sessions are as many as stay attached")`, and showing it again attaches again. `park` also refuses to park a tab the window is already showing: a second UI attachment would take the lead (the daemon lets the latest UI lead) and leave the window fenced.

Sources: [pool.rs:119-184](crates/apex-client/src/pool.rs#L119-L184), [pool.rs:474](crates/apex-client/src/pool.rs#L474), [pool.rs:499-539](crates/apex-client/src/pool.rs#L499-L539), [pool.rs:586-659](crates/apex-client/src/pool.rs#L586-L659), [app.rs:821](crates/apex-client/src/app.rs#L821), [app.rs:905](crates/apex-client/src/app.rs#L905)

### Bringing a tab up

`Pool::start` marks a tab `Coming(why)` and runs `Acme::connect_existing_targeted` (for tabs restored at launch, which must already exist) or `Acme::connect_blocking` on the background executor. A tab that is already coming is left alone. When the attempt finishes, `Pool::landed` decides where the link goes:

- **The tab was closed meanwhile.** The link is closed.
- **Success.** The session's real identity (`app::identified`) is written to the tab, which goes `Up`. If a window is sitting on that tab unconnected (`waiting_on`), the window `adopt`s the link. Otherwise the link is parked.
- **Failure.** An `ErrorKind::Unsupported` error on a local URL means the server is from another version, and the app offers a restart (see below). A waiting window shows the error. With no window waiting, a "no session…" error drops the tab, since there is nothing to come back to, and any other error marks the tab `Down(why)`.

At launch, `main.rs` opens the window and then calls `Pool::restore`, which starts every tab the window is not showing with `Why::Restoring` and `existing = true`. The exception is when the launch target is the picker (`Target::Choose`); then the picker is opened and the tabs wait in the bar.

```mermaid
stateDiagram-v2
    [*] --> Down: "install (from open-sessions) or Pool::open"
    Down --> Coming: "Pool::start"
    Coming --> Up: "landed Ok (adopted or parked)"
    Coming --> Down: "landed Err"
    Coming --> [*]: "landed Err 'no session' and no window waiting"
    Up --> Down: "link ended / evicted past CAP"
    Up --> [*]: "Pool::let_go"
    Down --> [*]: "Pool::let_go"
```

Sources: [pool.rs:307-435](crates/apex-client/src/pool.rs#L307-L435), [main.rs:875-892](crates/apex-client/src/main.rs#L875-L892)

### Switching, closing and recency

`Acme::switch_to(id)` is the single path for showing a tab. It parks the current session, sets `self.tab`, and records the tab as settled unless a ctrl-tab walk is in progress. If the pool has the session parked, `adopt_parked` swaps the link, log and node back in at once and restores `Seen`. Otherwise the window shows a blank page with the tab's `Why::sentence` (or the pool's own reason, when the tab is already coming), and `Pool::start` makes the link. Nothing waits on the UI thread, so moving on before the link lands loses nothing: the link parks itself when it arrives.

The pool keeps two orders. `tabs` is the order the user arranged. `settled` holds tabs the window stopped on, most recent first. `by_recency` returns every settled tab first, followed by parked or coming tabs that were never settled on, most recently parked first. `most_recent` drives ⌘⇧K (`previous_session`) and `next_tab`.

Three things end a tab's life in the window:

- **The current tab's ×** calls `close_current_session`, which switches to `next_tab` (the most recent other tab, else a neighbour) and then calls `Pool::let_go` on the old tab. If it was the last tab, it sets `close_requested`, so the window closes as a browser's does and the session stays.
- **A session ended under the window** (a `Ended` report during sync, or `End` finishing in `end_session`) sets `leave_requested`. `leave` then lets the tab go and switches to `next_tab`, or closes the window with `close_now` when nothing else is connected.
- **Closing the window** parks the session through `park_into_pool`.

Other keyboard paths are cmd-1…9 (`go_to_tab`) and ⌘⇧[ / ⌘⇧] (`cycle_tab`, wrapping); ⌘⇧W is bound to `CloseTab`.

```mermaid
sequenceDiagram
    participant UI as "Acme (window)"
    participant P as "Pool (global)"
    participant BG as "background executor"
    UI->>UI: "park() current link, log, node"
    UI->>P: "Pool::park(old tab, Parked)"
    UI->>P: "Pool::take(new tab)"
    alt parked
        P-->>UI: "Parked"
        UI->>UI: "adopt_parked (instant)"
    else not parked
        UI->>UI: "wait(sentence) blank page"
        UI->>P: "Pool::start(id)"
        P->>BG: "connect_blocking / connect_existing_targeted"
        BG-->>P: "landed(result)"
        P->>UI: "adopt(link) if window still waiting on tab, else park"
    end
```

Sources: [app.rs:905-1010](crates/apex-client/src/app.rs#L905-L1010), [app.rs:1043-1110](crates/apex-client/src/app.rs#L1043-L1110), [shell.rs:272-275](crates/apex-client/src/shell.rs#L272-L275), [shell.rs:280-291](crates/apex-client/src/shell.rs#L280-L291), [pool.rs:541-584](crates/apex-client/src/pool.rs#L541-L584)

## The session picker

⌘T, the sidebar's "New Session" row and the title menu's "New Session…" all call `open_selector`, which calls `open_picker`. At launch with nothing to reopen, `main.rs` opens it too. The picker's state is a `Selector` holding:

- the open tabs, and recently closed sessions not open here (from the `recent-sessions` file, one per host and label);
- the known hosts, with this Mac first (`known_hosts` reads the `known-hosts` file, and `open_picker` adds the window's own host if missing);
- each host's sessions as a `Loading`: `Seeded` from the `known-sessions` cache at once, `Ready` when the host answers, `Failed(last, why)` when it does not.

Each host is asked in the background by `ask_host`. A remote host with no apex yet gets one via `providers::deploy` and is asked again. An `epoch` counter drops answers meant for an earlier opening of the picker.

`Selector::rows` builds one list, filtered by the typed text matched (case-insensitively, as a substring) against session label and host argument. The list is the same whatever the way in:

| Row | Meaning |
|---|---|
| `GoTo(TabId, url)` | A tab open here, other than the current one |
| `Open(url)` | A session a host has, or one recently closed, and not already listed |
| `Create(url, host)` | Make a session with the typed name, on each host that lacks one by that name (only if `valid_label` accepts it) |
| `NewHost` | The "Add a host…" form (`Connect`: a provider from `providers::available()`, and a host) |
| `Rename(name)` | Shown when opened with `open_rename` (right-click on a sidebar row) |
| `Note(text)` | Unpickable: an invalid label |

`choose` dispatches the picked row. `GoTo` calls `switch_to`, `Open` and `Create` call `switch_to_url` (and so `Pool::open`), and `Rename` calls `rename_session` for this session or `rename_other`, which runs the daemon's rename locally or `apex rename-session` remotely and then `renamed_recent`. Picking a session saves the open tabs with `save_open`. `keeping` keeps the cursor on its row while answers arrive and rows move, and until the user moves or types (`moved`), the cursor lands on the window's own session once its host answers.

Sources: [shell.rs:805-840](crates/apex-client/src/shell.rs#L805-L840), [shell.rs:884-916](crates/apex-client/src/shell.rs#L884-L916), [shell.rs:1119-1224](crates/apex-client/src/shell.rs#L1119-L1224), [shell.rs:1265-1415](crates/apex-client/src/shell.rs#L1265-L1415), [shell.rs:1654-1691](crates/apex-client/src/shell.rs#L1654-L1691), [main.rs:857-868](crates/apex-client/src/main.rs#L857-L868)

## Title bar and sidebar

### The title bar

`title_bar` in `main.rs` builds the bar as a Mac app's. Its height is `title_h()`: the tag line height plus a border, and at least 38 px. From left to right it holds:

1. With the sidebar hidden: room for AppKit's traffic lights (`LIGHTS_W`, or 8 px in full screen), the sidebar button, then the session's name and chevron (`session_title`) followed by the standing chip. With the sidebar shown, the name lives in the sidebar, so the chip alone leads the bar, and the sidebar's card extends up around the lights, where the same sidebar button now sits on the card (`title-sidebar-card`).
2. The session's directory crumbs and the process pills, both fitted by `titlefit`.
3. acme's top tag (`ViewId::Top`), still editable.
4. Room for the stash shelf at the right end (`shelf_room`).

Bare parts of the bar move the window on drag and zoom it on double-click.

The sidebar button is a 24 px square holding `sidebar_glyph` (a window with a panel down its left) and, over its corner, `sidebar_badge`. The badge is how another session wanting the user is shown in the bar. It is built from `Acme::badge_shown`, a copy of `other_badge` kept by `tabs_tick` (see below). It is drawn on the button in both layouts, so it is visible whether or not the sidebar is out. The badge is one of two marks:

- the icon of the window's status, in the stale gold mixed toward the text colour, on a small disc of the button's ground, when that session's window has a status that asks for the user (`Done`, `Blocked`, `Error`);
- otherwise a small blue accent dot ringed by the button's ground, when the notification says no more than that.

`titlebar.rs` draws the session's name. Clicking it turns it into a `LineEdit` field with everything selected: return renames the session if the new name is non-empty and has no whitespace, and escape cancels. The chevron toggles a menu of every tab. The menu marks the current tab with ✓, shows a remote tab's host, shows `tab_word` (for example "watching" or "offline"), and puts the pjw glyph on other tabs with notifications. It ends with "New Session…". The chevron itself no longer carries a badge; that is the sidebar button's job. pjw is never used for the session being shown.

Sources: [main.rs:1144-1146](crates/apex-client/src/main.rs#L1144-L1146), [main.rs:1193](crates/apex-client/src/main.rs#L1193), [main.rs:1274-1423](crates/apex-client/src/main.rs#L1274-L1423), [titlebar.rs:73-158](crates/apex-client/src/titlebar.rs#L73-L158), [titlebar.rs:213](crates/apex-client/src/titlebar.rs#L213), [sidebar.rs:673-707](crates/apex-client/src/sidebar.rs#L673-L707), [app.rs:2044-2056](crates/apex-client/src/app.rs#L2044-L2056)

### The sidebar

When the sidebar is toggled on (⌃⌘S, `theme::sidebar()`), it is a 236 px column (`SIDEBAR_W`) holding a card inset 6 px (`sidebar::INSET`). `sidebar_entries` groups rows by host: this Mac first, then known hosts, then any other host a tab is on. Under each host it lists the app's tabs (`Entry::Tab`), then that host's other sessions in a fainter style (`Entry::Known`; clicking one calls `switch_to_url`). Host headers appear only when there is more than one host, and say "unreachable" when the last ask failed. `sidebar_refresh` asks every host for its sessions in the background when the sidebar shows and again each minute; a host that fails keeps what it had before as `Loading::Failed(before, why)`.

A tab row shows an initial avatar (accent-coloured for the current tab), the name, and a second line: the tab word, or else the host for a remote session. Hovering shows an × that closes the current session or calls `Pool::let_go` on another; otherwise a notified tab other than the current one shows pjw. Hovering a row also draws `session_preview`, a 320 px live miniature of a parked session beside the row. Right-click opens the rename picker.

#### Window rows and statuses

Under the current tab, `window_rows` lists the session's windows column by column, then a "Stashed" section (latest first), then "Processes". Windows of *other* sessions appear too, but only those that have something to say: under each parked tab's row (from `Pool::parked_nodes`, never the shown tab), `status_windows` picks the windows, laid out first and then stashed, whose status has text (`Node::window_status(w)` with `text()` non-empty). A status is the one a program reported by [OSC 7501](program-status.md).

Every window row is built by `win_row` from a `WinLine`, which `win_line` fills from whichever replica the window belongs to:

| Part | Where it comes from |
|---|---|
| Handle dot | `Acme::window_dot` for the shown session, `strips::dot_in(node, w, None)` for a parked one: unsaved, live, working, progress and, for the shown session, notification age |
| Name and folder | `names(node, w)`: the window's label, else the last part of its path, else a kind such as "Terminal", "Errors" or "X preview"; paths relative to the session's directory via `shown` |
| Status line | Under the name when there is one: the state's icon (`text_element::status_icon`), the text cut to 120 characters by `shortened`, and the age the status has stood (`age`: "now", then `Nm`, `Nh`, `Nd`) |

A status in the state `Done`, `Blocked` or `Error` (`asks`) is drawn in the stale gold mixed toward the text colour, the same hue as its pill in the window's tag. Other states use the dim text colour. Rows with a status get a little extra vertical padding and align their dot to the first line.

Clicking a row in the shown session calls `reveal_window`, or `unstash` for a stashed window. Clicking a row under another tab calls `go_in_tab(Some(tab), Loc { name: window id, … })`, which switches to that tab and lands on the window once the session is shown. `proc_row` draws each process under "Processes" with the same actions as its pill (× kills, B1 goes to its output, B3 to where it was run from).

Since the statuses and their ages change without input, `tabs_tick` (see below) includes every session's status texts, plus the current minute when there are any, in what it compares, so the sidebar redraws when one changes and the ages tick over.

Sources: [sidebar.rs:34-313](crates/apex-client/src/sidebar.rs#L34-L313), [sidebar.rs:319-363](crates/apex-client/src/sidebar.rs#L319-L363), [sidebar.rs:368-424](crates/apex-client/src/sidebar.rs#L368-L424), [sidebar.rs:436-503](crates/apex-client/src/sidebar.rs#L436-L503), [sidebar.rs:506-561](crates/apex-client/src/sidebar.rs#L506-L561), [sidebar.rs:568-611](crates/apex-client/src/sidebar.rs#L568-L611), [sidebar.rs:741-838](crates/apex-client/src/sidebar.rs#L741-L838), [shell.rs:124](crates/apex-client/src/shell.rs#L124), [shell.rs:293](crates/apex-client/src/shell.rs#L293), [switcher.rs:475-516](crates/apex-client/src/switcher.rs#L475-L516), [finder.rs:311](crates/apex-client/src/finder.rs#L311)

## ctrl-tab, the overview and miniatures

### Miniatures

`miniature::snapshot(node, theme)` turns a replica into a `Mini`, a list of coloured rectangles and text lines in the session's own pixel coordinates. It includes the top tag, each shown column's tag and body background, each visible window's tag (path, label, then dim tag text), and the window body:

- a text window's lines from its body view's origin, with tabs expanded;
- a terminal's grid, in runs of the cell colours;
- a page as plain paper with its name, since a page's native WKWebView belongs only to the shown session.

`snapshot_window` draws a single window and `snapshot_column_at` a single column. `Mini::paint` scales the result to fit a width. `paint_tilted` takes a `Tilt` that can lean a card back with a slope. Because each card is rebuilt from the replica every frame, the cards are live.

Sources: [miniature.rs:1-80](crates/apex-client/src/miniature.rs#L1-L80), [miniature.rs:82-185](crates/apex-client/src/miniature.rs#L82-L185), [miniature.rs:187-307](crates/apex-client/src/miniature.rs#L187-L307)

### The ctrl-tab walk

On the first ctrl-tab, `switcher_step` freezes an order in a `Switcher`: this tab, then `Pool::by_recency`. Each press steps the index (ctrl-shift-tab goes back) and calls `slide_to`. `slide_to` snapshots the outgoing session as a `Mini`, calls `switch_to`, and starts a 220 ms `SwitchSlide`. During the slide, the new content is offset by `dir * width * (1 - ease(t))` while the outgoing miniature slides out beside it. Because `switch_to` skips `note_settled` while a `Switcher` exists, sessions passed on the way are not counted as settled. Releasing control calls `switcher_commit`, which settles on the current tab. The 100 ms tick also commits if control has come up without gpui seeing it, for example while a page had the keys. Escape (`close_switcher`) slides back to the first entry.

### The overview (⌘⇧\)

`toggle_overview` opens an `Overview` (bound to ⌘⇧\, ⌘| and ⌘'). `overview_grid` picks the column count that gives the largest cards with the layout's aspect ratio inside 48 px padding, capped at 45% of the window's width. `overview_overlay` animates the current session's card shrinking from the full window into its grid slot over 260 ms (`OVERVIEW_RISE`) while the others fade in from 92% size. An accent ring shows the focus. Hover and the arrow keys move the focus (`overview_key`), and return or a click picks a card. Escape, a click off the cards, or ⌘⇧\ again picks the current tab. The picked card grows to fill the window over 240 ms (`OVERVIEW_PICK`), and then `overview_tick` calls `switch_to` and `note_settled`. `session_card` draws a session that is neither shown nor parked as its state word, or "not connected".

Sources: [switcher.rs:20-166](crates/apex-client/src/switcher.rs#L20-L166), [switcher.rs:168-473](crates/apex-client/src/switcher.rs#L168-L473), [shell.rs:300-302](crates/apex-client/src/shell.rs#L300-L302)

## Stash shelf, column strips and glides

### The stash shelf

⌘M (`stash_key`) stashes the key window, or else the window under the pointer, through `Node::stash_window`. The shelf draws the layout's `stash` at the title bar's right end, latest on top, like a hand of cards. Each card peeks 5 px out from under the one above (at most 4 do). A card whose window is working or notified slides out a further 34 px (`PULL`) over 0.3 s, tracked per window by `sync_pulls` and `shelf_pulls`.

Hovering or scrolling opens the fan (`Shelf::fan`, 160 ms, eased, reversible midway). The cards then spread leftward across at most half the bar, and the picked card's window appears below the bar as `shelf_preview`. For text and terminal windows the preview is the real `TextElement` and `TermElement`, live and editable; pages are drawn as a miniature. The preview's size is the window's stashed size, at least 480 by 320 (or 40% of the height), scaled down to fit. The fan closes `GRACE = 250 ms` after the pointer leaves both the cards and the preview, but not while a button is held. When it closes, `restack` brings windows worked in during the fan (`touched`) forward with `restash_window`. Clicking a card calls `unstash`.

`peek_errors` serves a toast's Show All. It opens the fan on a stashed window, selects the first line of what the toast said, and warps the pointer there.

Sources: [shelf.rs:24-163](crates/apex-client/src/shelf.rs#L24-L163), [shelf.rs:165-326](crates/apex-client/src/shelf.rs#L165-L326), [shelf.rs:328-590](crates/apex-client/src/shelf.rs#L328-L590)

### Column strips

A column can be shown as a strip in two ways:

- **Stashed** (B3 on its box). `strip_element` draws three sheet edges at the right of the row; a notified window in the column tints the edges toward the accent. Hovering it (`strip_tick`, also driven from the 100 ms tick through `native_mouse`) opens `strip_slice`: a live `snapshot_column_at` miniature at the width the column would come back to, from `restore` as a millionth of the row (default a third, clamped to 240 px up to half the row), placed on the side with more room. Clicking the slice calls `bring_back_column`. The slice closes 150 ms after the pointer leaves, and stays while a button is held.
- **Minimized.** `minimized_element` draws a slim card with the column grip at the top and each window's handle dot where the window stands (`window_dot`). Clicking a dot calls `press_handle` for that window; clicking anywhere else is the column box (`press_col_box`).

`window_dot` is a thin wrapper over `dot_in(node, w, note)`, which builds a handle's `Dot` from any replica: unsaved, live (a live window or a page from a URL), working, and progress. The sidebar uses `dot_in` directly to draw the windows of parked sessions.

Sources: [strips.rs:1-24](crates/apex-client/src/strips.rs#L1-L24), [strips.rs:29-110](crates/apex-client/src/strips.rs#L29-L110), [strips.rs:113-170](crates/apex-client/src/strips.rs#L113-L170), [strips.rs:174-210](crates/apex-client/src/strips.rs#L174-L210), [strips.rs:213-219](crates/apex-client/src/strips.rs#L213-L219)

### Glides

The core moves windows instantly. `glided_layout` returns a copy of the layout in which any window or column whose rectangle changed since the last frame sits partway along a 160 ms ease-out cubic (`ease`) from the old rectangle to the new. A move begun before the last one finished restarts from the last target with an 80 ms `HURRY`. A window that appears among existing ones opens down from its top edge. Resizing the window, switching tabs (`row` holds both the row rectangle and the `TabId`) or turning off View ▸ Layout Animations makes everything snap into place.

Sources: [glide.rs:1-147](crates/apex-client/src/glide.rs#L1-L147), [shell.rs:74](crates/apex-client/src/shell.rs#L74)

## Notifications, the Dock and toasts

### Where notifications come from

A window's notification is not only raised by tools: the server keeps it following the window's status. It is raised when the status comes to ask for the user and lowered when it no longer does (the daemon re-derives the notifications from the statuses as they now are). The client only reads the result, so a status that asks for the user shows up in the Dock queue, the sidebar button's badge and the window's handle without the client doing anything of its own. See [Program Status Protocol (OSC 7501)](program-status.md) and [Proposals: How Others Change State](proposals.md).

Sources: [lib.rs:576](crates/apex-server/src/lib.rs#L576), [daemon.rs:1547-1554](crates/apex-server/src/daemon.rs#L1547-L1554)

### attention.rs

Each session's notifications are ordered by `Notification::at`, a sequence number in that session's metalog, so they cannot be compared across sessions. `attention::tick`, called at the end of every 100 ms tick, collects `(TabId, [(WindowId, Seq)])` from the parked sessions (`Pool::notifications`) and from the window's own node. `advance` then updates a global `Queue` of `Note = (TabId, WindowId, Seq)`. It drops notifications that have gone and appends new ones in the order first seen. It reports news only for tabs already in `Looked`, so a session seen for the first time brings no bounce for what it already had.

The Dock badge shows the queue length; `badge` skips the AppKit call if the label is unchanged. If news came while no app window is active, `bounce` calls `requestUserAttention:` once with the informational level. `next_notification` takes the oldest note. It is bound to **⌘J** in `shell::bindings` (⌘G is Find Next; some comments in `attention.rs` and `app.rs` still say ⌘G). It switches tabs if needed (deferring through `pending_note` if the tab is still attaching), then `take_note` dismisses the notification, shows its window and warps the pointer to it. With nothing queued it calls `beep()`. At launch, `dock_icon` sets the running icon from `mac/space-bunny-1024.png`. A forgotten-on-arrival rule applies when a session is shown for the first time (`forget_seen`): its existing notifications are marked as long seen, so they do not ping.

### The sidebar button's badge

Separately from the Dock queue, the window keeps `others_noted`, a map from `(TabId, WindowId, Seq)` to the `Instant` the window first saw each notification of *another* session. `tabs_tick` fills it from `Pool::notifications` (leaving out the shown tab) and drops entries that have gone. `other_badge` picks the entry with the latest `(Instant, Seq)`, finds that tab's parked node, and, if the window's status there asks for the user, takes the icon of its state. The result is `Option<(TabId, WindowId, Option<usize>)>`: `Some(Some(icon))` draws the icon, `Some(None)` draws the blue dot, and `None` draws nothing (see [Title bar](#the-title-bar)).

Sources: [attention.rs:1-160](crates/apex-client/src/attention.rs#L1-L160), [app.rs:2026-2056](crates/apex-client/src/app.rs#L2026-L2056), [app.rs:2085-2114](crates/apex-client/src/app.rs#L2085-L2114), [app.rs:596-598](crates/apex-client/src/app.rs#L596-L598), [main.rs:170](crates/apex-client/src/main.rs#L170), [shell.rs:276-278](crates/apex-client/src/shell.rs#L276-L278)

### Toasts

Command errors go to their directory's +Errors window, and tool reports, such as LSP diagnostics, go to diagnostic windows. Neither is opened over the user's work. `diagnostic_news` runs during sync. For each window flagged `diagnostic`, it compares the text with what this client last saw (`diag_seen`). It skips windows that are laid out and showing lines (`diagnostic_seen`: `frmax > 0`). It stashes the window if it was laid out. It then appends the new text to the window's `Toast` or makes a new one. Windows that already existed when the client first looked are primed silently (`diag_primed`); `forget_seen` and `park` reset that flag so a session shown anew is primed again.

`news(old, text, typing)` takes what was appended. If the text was rewritten, it takes the lines that were not there before, counting multiplicity. Either way it drops lines about the file being typed in (`path:` prefix), whose errors come and go with each key.

Toasts stack at the lower right, newest at the bottom, and show the last 6 lines. Each stays 8 s (`STAY`) unless hovered. Any click off the toasts dismisses all of them (`toasts_click`). B1 on a toast, or its Show All, calls `toast_show_all`, which opens the stash preview (`peek_errors`) or reveals a window already in a column. The words in a toast respond as they would in the errors window: B3 or ⌘-click calls `look_in_errors` with the position of the word in the window, so `file:line` plumbs; B2 or ⌥-click executes the word in the window's context.

Sources: [toasts.rs:1-110](crates/apex-client/src/toasts.rs#L1-L110), [toasts.rs:112-316](crates/apex-client/src/toasts.rs#L112-L316), [toasts.rs:318-362](crates/apex-client/src/toasts.rs#L318-L362), [app.rs:2155](crates/apex-client/src/app.rs#L2155)

## Process pills

`running_procs` returns the metalog's `procs` that are still running, excluding terminal shells (`ProcKind::Term`). `titlefit::choose` picks the richest step of a five-step `LADDER` of (`PathFit`, `ProcFit`) that leaves the top tag its room. The path gives way first: (Full, Full), (Short, Full), (Short, Stack), (Last, Stack), (Last, Count). The top row needs its whole text while its caret is in it, and otherwise at most `TEXT_MIN = 200` px. Path fits are `Full` (host and every crumb), `Short` (host, `…/` and the last two crumbs) and `Last` (`…/` and the last crumb); the cwd picker forces `Full`. Process fits are:

- `Full`: one pill per command name, with a count when several share a name (`groups` keeps the order each name first ran).
- `Stack`: pills stacked as cards, newest on top, up to 4 showing, `STEP = 7` px apart.
- `Count`: just a number.

The room is measured by `title_room_mark` where the bar is laid out, and a change redraws with the new fit. If anything is folded away (the path is not `Full`, or processes are grouped or stacked), hovering fans it out over the top row, so the bar never reflows under the pointer. The path fan is a card holding the whole path; the process fan puts every process on a pill of its own, wrapping within 240–720 px and kept inside the window. Fans are placed by where the parts were drawn the frame before (`Marks::drawn_procs`, `drawn_path`), because `clear` resets the marks each frame.

On a pill, × calls `kill_proc` (via the × itself on B1, or through `press_proc` for B1 or B2 on an `Atom::ProcKill`), which sends `ClientMsg::Kill` with the pid, or calls `server.kill` in process. This asks the server directly rather than running a command. B1 on the name (`proc_output`) goes to the process's output: its directory's errors window (created if missing) or the window whose buffer it replaces, else where it was run from. B3 (`proc_origin`, reached by `press_proc` on a right press) goes to the window it was run from. Both use `go_to_window`, a `Proposal::Goto` by window id that pushes the back stack. Hovering a pill shows `proc_card`: the full command in the mono face, the pid and directory, and when and where it was started. A stack or count only offers a ×, on a single-process pill. The sidebar's "Processes" rows offer the same actions.

These are processes the server started. The status pills drawn in a *window's* tag (a program's own report of what it is doing) are a different thing, drawn by `text_element.rs` and described on [Program Status Protocol (OSC 7501)](program-status.md); the sidebar repeats their text under the window's name.

Sources: [procs.rs:1-149](crates/apex-client/src/procs.rs#L1-L149), [titlefit.rs:24-61](crates/apex-client/src/titlefit.rs#L24-L61), [titlefit.rs:100-127](crates/apex-client/src/titlefit.rs#L100-L127), [titlefit.rs:171-255](crates/apex-client/src/titlefit.rs#L171-L255), [titlefit.rs:279-367](crates/apex-client/src/titlefit.rs#L279-L367), [titlefit.rs:373-430](crates/apex-client/src/titlefit.rs#L373-L430), [titlefit.rs:468-481](crates/apex-client/src/titlefit.rs#L468-L481), [sidebar.rs:368-424](crates/apex-client/src/sidebar.rs#L368-L424)

## Connection standing

`Acme::standing` classifies the window's relationship with its session:

| Standing | When | Word | Mark |
|---|---|---|---|
| `Leading` | In process, or connected and holding the layout lease | none | none |
| `Watching { leader }` | `fenced()`: the layout lease is held by another attachment or has been released | watching | eye, blue |
| `Stalled` | Remote, not connected, link still open | stalled | broken link, gold |
| `Offline { why }` | Link closed, or the tab's attach failed | offline | broken link, gold |
| `Coming` | Waiting on a tab that is coming up | connecting… | spinner |

Blue and gold are fixed values, not the palette's accent. They sit on the blue–yellow axis, which red-green colour blindness keeps apart, and were checked under a deuteranopia simulation (see [Themes, Fonts and Colour](themes-and-fonts.md)). A non-leading standing shows a chip (`standing_chip`) at the head of the title bar's middle, after the session's name when the sidebar is hidden; clicking it opens `standing_card_panel` with the host, the state, the ping time, the retry countdown and any unconfirmed edits. A banner under the title bar explains what the standing means and offers actions. Watching offers **Take over**, which is never done automatically, and reconnects so that the daemon lets the latest UI lead (⌘⇧R is bound to Reconnect). Typing while watching flashes the banner (`flash_watching`). A parked tab reports "watching" in the tab list through `Pool::fenced`.

`link_tick` runs every 100 ms:

- A gap over 5 s between ticks (`WAKE_GAP`) means the Mac slept. The tick sends a `Ping`, and if no pong arrives within 2 s it marks the link dead and retries at once.
- A stalled link is given up on 15 s after the daemon last answered (`STALL_GIVE_UP`), but only if nothing is unconfirmed.
- Offline retries follow `BACKOFF = [1, 2, 5, 10, 20, 30]` seconds, unless `hopeless(why)`: a protocol mismatch, a tab let go, or "not attached".

`unconfirmed` counts, per shard this window leads, entries in the log beyond `link.acked`. Reconnecting would drop those edits. While the link is still open, `ask_reconnect` therefore asks first (`confirm_drop`, with "Reconnect anyway" or "Wait"). After a drop, a `lost_note` names the affected windows for 20 s.

Sources: [standing.rs:19-110](crates/apex-client/src/standing.rs#L19-L110), [standing.rs:112-273](crates/apex-client/src/standing.rs#L112-L273), [standing.rs:275-546](crates/apex-client/src/standing.rs#L275-L546), [pool.rs:439](crates/apex-client/src/pool.rs#L439), [shell.rs:306](crates/apex-client/src/shell.rs#L306)

## Restarting a mismatched server

When an attach to a local URL fails with `Unsupported` (the daemon "speaks apex protocol N"), `Pool::landed` and launch both call `offer_restart`. At launch, `main.rs` records the error as `stale` while ensuring the daemon and offers the restart once the window is open. A static `OFFERED` flag limits the offer to once per launch. The offer is a warning prompt naming the daemon's protocol (parsed by `their_protocol`) and this build's `PROTOCOL`, with Restart Server or Not Now. Apex ▸ Restart Server… (`restart_server_asked`) asks the same question at any time, and counts windows with unsaved changes across this window and parked local sessions (`local_unsaved`).

`restart_server` calls `remote::stop_any` on the default socket and then `reattach_on(None)`. That reconnects this window if its session is local, calls `Pool::start` for every other local tab not shown, and reconnects other windows on that daemon. The first attach brings up a fresh server. Tabs are kept, and each attaches to a fresh session of its name. For a remote session, the standing card's and waiting page's **Restart daemon** (`restart_daemon_asked`) runs `providers::deploy` and then `apex stop` on the destination, off the UI thread, before calling `reattach_on(Some(dest))`. See [Remote Hosts and Providers](remote-hosts.md) and [The Attach Protocol](attach-protocol.md) for the build id and protocol check.

Sources: [restart.rs:1-172](crates/apex-client/src/restart.rs#L1-L172), [pool.rs:398-410](crates/apex-client/src/pool.rs#L398-L410), [main.rs:827-892](crates/apex-client/src/main.rs#L827-L892)

## The tick that drives the chrome

Several pieces of chrome change without user input: parked links, links coming up, window statuses, notifications of other sessions, toasts expiring. They are re-evaluated on a 100 ms loop started in `Acme::over` in `app.rs`.

```mermaid
flowchart TD
    T["100 ms tick (app.rs)"] --> SP["spinners: web_focus_tick, any_working, waiting"]
    T --> SW["switcher_commit if ctrl released unseen"]
    T --> ST["strip_tick (native pointer)"]
    T --> TB["tabs_tick: tab names, notified, words, statuses, badge changed?"]
    T --> LK["link_tick: wake probe, retries, flashes"]
    T --> TO["toasts present: redraw to expire"]
    T --> AT["attention::tick: queue, Dock badge, bounce"]
    PW["parked link wake"] --> TE["Pool::tend"]
    TE --> TB
```

`tabs_tick` compares three things with what was last drawn: a tuple of (id, URL, notified, word) for every tab; the statuses of every session's windows (`status_windows`, shown tab and parked ones) as (tab, window, text) together with the current minute whenever any exist; and the sidebar button's badge from `other_badge`. The minute makes the sidebar's ages ("now", "3m") redraw as they pass. It is the only thing that redraws the window when a parked session or a background attach changes a tab, a status or the badge. The same tick keeps spinners turning: it notifies while a window is working (`any_working`) or waiting on a tab (`acme.waiting.is_some()`).

Sources: [app.rs:1698-1755](crates/apex-client/src/app.rs#L1698-L1755), [app.rs:2008-2056](crates/apex-client/src/app.rs#L2008-L2056), [pool.rs:189-218](crates/apex-client/src/pool.rs#L189-L218)

## Tests

The chrome's unit tests are small and pure:

- `pool.rs`: which URL pairs map to the same tab.
- `attention.rs`: queue order, first-look suppression, and re-raised notifications.
- `standing.rs`: backoff growth, `hopeless`, and that only leading says nothing.
- `restart.rs`: protocol parsing.
- `toasts.rs`: the `news` diff rules.
- `sidebar.rs`: `age` and `shortened` for a status's age and cut, `split_name`, and `shown`.
- `titlefit.rs`: the ladder gives way path first and then the pills, processes group by name in first-run order, a short path keeps its last crumbs, and a fan is placed by the previous frame's marks.

The server side of notifications following statuses is tested elsewhere (`apex-tool/tests/tool.rs`, `apex-server/tests/server.rs`). The rendering and gpui wiring, including the badge, have no automated tests.

Sources: [pool.rs:662-689](crates/apex-client/src/pool.rs#L662-L689), [attention.rs:162-199](crates/apex-client/src/attention.rs#L162-L199), [standing.rs:647-671](crates/apex-client/src/standing.rs#L647-L671), [restart.rs:174-181](crates/apex-client/src/restart.rs#L174-L181), [toasts.rs:346-362](crates/apex-client/src/toasts.rs#L346-L362), [sidebar.rs:709-739](crates/apex-client/src/sidebar.rs#L709-L739), [titlefit.rs:483-537](crates/apex-client/src/titlefit.rs#L483-L537), [tool.rs:262-266](crates/apex-tool/tests/tool.rs#L262-L266)
