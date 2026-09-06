# apex — design

`apex` is a modernised acme: text-first, mouse-oriented, plumbing everywhere,
fast and simple. It keeps acme's interaction model and adds what acme lacks
for today's work: a client–server split so the UI can detach and re-attach
locally or over ssh, embedded terminals for running coding agents, and an
extensibility API that does not depend on 9P or a mounted filesystem.

This document is the source of truth for the architecture. Sections marked
*v1* are in scope for the first milestone; everything else is design intent.
The current gpui prototype (`acme-gpui`) is the seed of the client.

---

## 1. Goals and non-goals

Goals:

- **The acme experience.** Three-button mouse, tags, columns, B2 executes,
  B3 looks, chords, everything is text. Colours, fonts and geometry follow
  plan9port. Speed is a feature: no perceptible latency for local editing.
- **All state on the server.** The UI is a renderer plus an input device. A
  client can detach at any time and re-attach with full fidelity; the
  server keeps scrollback and history and serves them on demand.
- **Remotable.** `apex attach host` over ssh is the remote story. Files stay
  where the server runs; nothing is synchronised.
- **Agents.** Terminal windows are first-class. An agent runs in one and
  drives the editor through the same CLI a shell script would use.
- **Extensibility at least as good as acme's**, with few concepts, exposed
  through the `apex` command (tmux-style) and a typed protocol for programs.
- **Modern extensions fit:** LSP, formatters, file watchers, agents.

Non-goals (for now): multiple simultaneous UI clients editing the same
session ("single player mode": one UI attachment per session at a time), a
plugin language, a configuration language, web windows, persistence of the
server's state across restarts, Windows.

### Terminology

- **Session**: what tmux calls a session — a whole workspace (its shards,
  layout, windows, terminals) living on one server. A server hosts many.
- **Attachment**: the fenced identity a client gets when it attaches to a
  session. Leases are granted to attachments. A client may hold several
  attachments at once, to sessions on local and remote servers.
- **Connection**: a socket. Connections drop and resume without affecting
  attachments or sessions.

### Principles

- **Nothing is ever lost.** Whenever the system must set something aside —
  unflushed edits from a fenced attachment, a buffer overwritten by `Get`,
  a disk change under a dirty buffer — every version is kept and shown,
  with provenance, in a `+Recovered` buffer or in the tag. Silent discard
  is a bug.
- **State on the server, rendering on the client.** The client is a
  renderer and input device with a replica; if it lacks data for an
  operation it hands the operation to the server (§5) rather than blocking.

---

## 2. Overview

```
                 ┌──────────────────────────── server (one per host) ───────────────────────────┐
                 │  logs + snapshots      state machine (core crate)      ptys / files / procs  │
                 │  fencing / attachments plumber, Edit language          file watcher          │
                 └───────────┬───────────────────────────┬────────────────────────┬─────────────┘
       attach protocol       │                           │ control protocol       │
   (unix socket / ssh stdio) │                           │ (same frames)          │
                 ┌───────────┴──────────┐     ┌──────────┴──────────┐   ┌─────────┴─────────┐
                 │  UI client (gpui)    │     │  apex CLI / scripts │   │ tools: lsp, agent │
                 │  state machine       │     │  win, Watch, ...    │   │ formatter, ...    │
                 │  (core crate, leader │     └─────────────────────┘   └───────────────────┘
                 │   for its shards)    │
                 └──────────────────────┘
```

Three kinds of process:

- **Server.** A daemon, one per host, hosting many sessions. Owns the logs
  (in memory), sequences or stores every change, runs ptys and external
  programs, watches files, hosts the plumber. Runs where the files are.
- **UI client.** Attaches to one or more sessions (one OS window each).
  Mirrors each session's state, renders it, and while attached *leads* the
  shards a user edits (buffers, windows, layout) so editing never waits on
  a round trip.
- **Tools.** Anything else: the `apex` CLI, shell scripts, `apex lsp`, an
  agent harness. Tools read state and *propose* changes through the server.

The **core crate** (`apex-core`) is shared by server and client: buffers,
selections, undo, plumbing rules, built-in commands, and the state machine
that applies log entries. The Edit language is its own crate (`apex-edit`,
§8a) that core depends on. Neither has UI, I/O or a dependency on gpui, so
both nodes run the identical code and hold the identical state, and the UI
is a separate layer that only reads state and emits entries.

---

## 3. Model

### 3.1 Nouns

Four nouns, mirroring acme's files:

| noun | acme equivalent | contents |
|---|---|---|
| **Buffer** | `File` | id, name, rope, version, undo history, dirty/stale flags, disk hash |
| **Window** | `Window`/`Text` | id, tag view, body view, kind (`text` \| `term` \| `web`), column slot |
| **Event** | `event` file | the log itself: edits, selections, layout changes, executions |
| **Command** | writing to `ctl`, B2 | "execute this string in the context of that window" |

A **view** is `(buffer id, q0, q1, origin)`: acme's `Text`. A window has a
tag view and, for text windows, a body view. Two windows may view one buffer
(Zerox) with independent selections and origins; an edit in either shows in
both, undo is per buffer, and only the last window on a dirty buffer warns
on Del. Every tag is its own buffer. `+Errors` is a buffer.

Terminal and web windows have no body buffer; their body state is a
server-owned emulator grid or page.

### 3.2 Shards

State is partitioned into **shards**, each an independent replicated state
machine with its own log and its own lease. A shard is either *leasable*
(its lease moves between attachments) or *pinned* (its leader is fixed and the
lease never moves):

| shard | one per | entries | default leader |
|---|---|---|---|
| `buffer` | buffer | `Create`, `Edit{version, q0, nd, text, group}`, `Undo/Redo{version}`, `Clean{version}`, `Stale{hash}`, `Rename`, `ViewAdd/Del{view}`, `Select{view, q0, q1}`, `Origin{view, off}` | server |
| `window` | window | `Create{tag, body}`, `Font`, `Exec{text, handler, at}`, `Status{exec, Done|Failed|Unknown}`, `Delete` | server |
| `layout` | session | `Init{top}`, `ColNew/Del/Resize`, `WinPlace{window, col, at}`, `WinRemove`, `WinResize`, `Snarf`, `Exec` from column tags and the top row | server |
| `term` | terminal | `Rows{seq, rows: [cells]}`, `Cursor`, `Mode`, `Resize`, `Exit` | server, **pinned** |
| `metalog` | session | `ShardNew/Del`, `Attach/Detach`, `LeaseRequest/Release/Grant/Reclaim{shard, attachment, epoch, seq}`, `PlumbRuleInstall/Remove{attachment, priority, predicate, action}` | server, **pinned** |
| `registry` | server | `SessionNew/Del/Rename` | server, **pinned** |

The **metalog** is the session's authority on everything *about* shards
rather than in them: which shards exist, which attachment holds which lease
at which epoch, every transfer or reclaim, and the plumbing rule table
(§8), so a reader can tell what happened and why. Fencing is decided there
and enforced by the log store against its latest state. Layout
cross-references windows and buffers but, being leasable, cannot be the
source of truth for what exists. The server-wide **registry** lists
sessions.

**Views (selections and origins) live in the buffer shard**, not the
window shard: a buffer edit adjusts every view on the buffer, so `Select`
and `Edit` must share one sequence or replicas could interleave them
differently and disagree. Column tags and the top row have views too.

Cross-shard operations are several entries in several logs with no
atomicity between them. acme has none either. Zerox is a `layout.WinNew`
plus a `window.*` entry naming the buffer and version; the reader tolerates
seeing them in either order.

### 3.3 The state machine

```
state(shard) = apply*(snapshot(shard, N), entries N+1..)
```

Rules that keep both nodes identical:

1. `apply` is deterministic and pure: no clock, no randomness, no I/O.
2. Anything with an effect is an **exec entry** (§5) naming its handler.
   `apply` records it as state; the node matching the handler performs the
   effect and its outcome comes back as further entries referencing the
   exec's sequence. **Effects fire on live observation only, once**: a
   performer acts on an exec it sees arrive at the head of a live stream,
   never while replaying a tail on attach or restart, and appends
   `Done{exec_seq}` or `Failed{exec_seq, reason}` (or `Unknown` after a
   crash between the two — at-most-once, the right default for a shell
   command; `Put` is idempotent anyway).
3. An edit to a buffer adjusts every view on that buffer (selection and
   origin), in one place in the core crate, exactly as acme's `textinsert`
   walks `file->text[]`.
4. Snapshots are a pure function of the log prefix. Either node may take one.
5. Divergence is detectable: nodes hash shard state at attach and
   periodically; a mismatch is a bug in `apply`, surfaced immediately.
   Tests feed random entry sequences to two instances and compare.

Terminal grids are the one thing not reconstructible from their raw input
(the emulator has no serialisable state), so terminal entries are grid rows,
not pty bytes. The raw pty byte stream is still logged to disk for
`apex term log`, search and agent transcripts, but correctness never depends
on replaying it.

---

## 4. Leadership: attachments, leases, fencing

### 4.1 Attachments

An **attachment** is explicit and server-issued (`apex attach` creates one,
recorded in the session's metalog), with a monotonically increasing id.
Leases are granted to attachments, never to connections. A dropped
connection does not end an attachment: the client reconnects, presents its
attachment id, and both sides resume from the last acknowledged sequence,
keeping their tails. Nothing is lost on a network blip.

Resuming is decided per shard, and the two directions differ:

- Shards the attachment **leads** (buffers, windows, layout): the client is
  ahead of the server, not behind. It flushes its unflushed tail; there is
  nothing to replay.
- Shards the attachment **follows** (terminals, tool-led buffers): the
  server chooses, by size, between replaying the tail since the last
  acknowledged sequence and sending a fresh snapshot. If the tail is larger
  than the snapshot (or over a fixed threshold), it re-snapshots — an hour
  of agent output is a huge tail and a small grid. Time away is only a
  hint; bytes decide.

Proposals addressed to a leader whose attachment is disconnected are held
briefly, then rejected with "leader unreachable" so tools retry rather than
hang. Once the attachment is fenced, proposals go to the new leader.

*As built:* a client that has lost its leases (another UI attached and
took them) sees it in the metalog it follows, and shows it: the square
at the top-left of the top row fills red, the title bar reads
"fenced", and the window title says another client leads.

While an attachment holds entries the server has not acknowledged, the
affected buffers are **unsynced** — a state distinct from dirty (§9), shown
in the tag box in its own colour. It clears when the attachment resumes
and flushes; if the attachment is fenced instead, §4.4 applies.

### 4.2 Leases

Every shard has exactly one **leader** at any time: the holder of its lease.
Only the leader appends to the shard's log; everyone else **proposes** to
the leader (§5). The lease carries a **fence epoch**, bumped on every
transfer and every reclaim.

- On attach, the UI attachment takes the leases for `layout`, every
  `window`, and every `buffer` it shows, in one bulk grant. It takes further buffer
  leases lazily as it opens them. Editing, selecting and rearranging then
  happen locally with no round trip; entries stream to the server
  asynchronously, batched per frame.
- `term` shards are **pinned** to the server: the lease never moves. The
  client is a follower that renders rows and proposes keystrokes. Terminals
  therefore never suffer lease loss.
- A tool may take a `buffer` lease for exclusive multi-step work (an agent
  rewriting a file). While it holds it, the UI's edits to that buffer become
  proposals routed through the tool, so human and agent edits are serialised
  without interleaving. Tool leases have a TTL so a crashed tool cannot lock
  a file forever. This is opt-in; the default for tools is to propose.

### 4.3 Fencing

Every append and every proposal — terminal keystrokes included — carries
`(attachment id, fence epoch)` for the shard. The server's log store, which
every attachment must go through, refuses anything stale. A client that still
believes it leads cannot append, cannot propose and cannot type into a
terminal.

### 4.4 Transfer and reclaim

Both are driven through the metalog, and a new UI attachment always
tries the first before the second:

- **Transfer** (cooperative): the server appends `LeaseRequest`; the holder,
  if connected, flushes and appends `LeaseRelease{seq}`; the server appends
  `LeaseGrant{new holder, epoch+1, seq}`. Lossless.
- **Reclaim**: the holder does not answer within a short deadline (it is
  disconnected or wedged). The server appends `LeaseReclaim{epoch+1, last
  flushed seq}` and grants from there; the old attachment is dead from that
  instant, whether or not it ever returns. The deadline is the one timeout
  that is not garbage collection, and it is safe because a reclaim is only
  ever lossy, never incorrect. Other timeouts only collect attachments
  nobody will resume and expire tool leases.
- **Recovery**: a fenced client re-attaches only as a *new* attachment,
  takes fresh snapshots, and re-submits its unflushed tail as
  base-versioned proposals. Where versions still match they apply and
  nothing is lost; where they don't, the text goes into `+Recovered` with
  provenance (attachment, time, base version) so work is un-sequenced
  rather than lost.

The UI flushes every frame, so the unflushed tail is milliseconds of typing.
Server acks give an honest "unsynced" signal; the tag's layout box can show
it.

Every entry records the attachment and epoch that wrote it. `apex log`
shows provenance: "this edit came from the agent's attachment".

---

## 5. Proposals and commands

Tools write through one verb:

```
propose(shard, base_version, ops) -> Accepted{seq} | Rejected{current_version}
```

The server routes a proposal to the shard's current leader (itself
included); the caller never learns who that was. A proposal is applied
atomically and in order within its shard, so a formatter's hundred edits
travel as one proposal and cannot interleave with typing. There are no
cross-shard proposals.

Commands are what B2 does, and they are **log entries**, exactly as acme
writes every B2 and B3 to its `event` file:

```
Exec{ text, window, at_seq, handler }      handler ∈ { leader, server, tool(name) }
```

The leader appends the exec like any entry (provenance, and `apex events`
shows what was executed). The node matching `handler` performs it and
appends the outcome. Resolution of the handler is recorded in the entry so
it replays as data:

| command | handler | outcome |
|---|---|---|
| `Cut Paste Snarf Undo Redo Look Edit Font Zerox`, layout commands | leader | concrete entries, no round trip |
| `Put Get`, `New` on a path, `Newterm Newweb`, `\|cmd <cmd >cmd`, unknown words (shell) | server | proposals the leader applies; terminal rows |
| words a tool registered for (`Definition`, `Format`, ...) | tool(name) | proposals |
| B3 text | server (plumber) | a further exec, or a window |

This is acme's model where a tool may consume an event and act instead of
acme, made explicit. Locally the split is invisible; remotely it is ssh's:
editing is instant, running things costs one round trip.

The `|cmd`, `<cmd`, `>cmd` forms are commands whose input is the selection.
The input text is not stored in the entry: the exec sits at a sequence and
any replica reconstructs the text from the state there. What the entry does
carry is the buffer id, its version and the range, because the exec lives in
the window log and the text in a buffer log; cross-shard ordering must never
be guessed. The output, being an effect, returns as a proposal against that
version.

**Handlers also give the client a fallback.** A command the client can
satisfy from mirrored state runs locally; one it cannot — `Look` past the
mirrored part of a huge buffer, `Edit` over paged text — it appends with
`handler: server`. The server evaluates it and proposes the resulting
selection and origin; the client's ordinary "not in view, page it" path
fetches the text. Full mirroring (§6.1) is therefore an optimisation, not an
invariant.

**Proposals may be intents; entries are always concrete.** A proposal is
either concrete ops against a base version (a formatter that diffed the
version it read) or an *intent* the leader evaluates against its exact
current state and lowers into concrete entries: `SetContent{text}` (the
result of `Get` or a file-watch reload), an Edit program (`apex edit`), a
selection by address. Intents have no base-version race, because the leader
runs them on the live text.

The lease never moves for a server-side action. `Get` on a client-led buffer
is: leader appends `Exec{Get, handler: server}` → server sees it live, reads
the file → server `Propose{SetContent}` → leader diffs, appends the edits
and `Clean{version}` → server appends `Done`.
`Put` is the mirror: server applies its replica to `at_seq` (the entries are
ahead of the exec on the same stream), writes the file, records the hash so
the watcher ignores the event, and proposes `Clean{version}`. Handing the
lease to the server instead would cost two transfers, stall typing while the
server held it, and open a fencing window on a buffer being edited.

`Clean{v}` means *clean as of version v*; dirty is derived as "current
version ≠ clean version", so typing during a slow `Put` leaves the buffer
correctly dirty afterwards. Between the exec and its `Done` the UI knows a
`Put` is in flight and shows it; a `Failed{exec, reason}` is shown in place
rather than as an `+Errors` line that scrolls away.

*As built (`apex-server/src/proposal.rs`).* The server never writes a
shard it does not lead. Every effect it has on buffers, windows or the
layout is a value of one `Proposal` enum — `OpenWindow`, `NewWindow`,
`TermWindow`, `SetContent`, `Clean`, `Rename`, `ReplaceRange{version}`,
`Errors`, `Status`, `Look` — which the leader lowers into entries with
`proposal::apply`. In-process the client applies proposals the moment the
server returns them; over the socket they travel as `Propose` messages.
Only `ReplaceRange` and `Clean` carry a base version; the rest are intents
lowered against the leader's live state, as this section says. The
handler predicate is not a general predicate yet: `Handler::Server` is
resolved by the leader at exec time from the command table.

---

## 6. Protocols

One schema (protobuf), one framing (length-prefixed messages), two
"services" on it. Transport is a Unix socket locally. Remotely,
`apex attach host` runs `ssh host apex attach --stdio` and speaks the same
frames over the pipe — no socket forwarding, no daemon beyond sshd. gRPC is
not the transport (HTTP/2 over stdio is awkward and the attach path must be
the fastest thing in the system); it can be a later gateway.

### 6.1 Attach protocol (UI ⇄ server) — *v1*

Tuned for one thing: the client mirrors only what it can see.

```
client → server
  Hello{session, attachment?: id}            attach to a session, or resume an attachment
  Take{shards}                               bulk lease request (on attach)
  Append{shard, epoch, entries}              leader appends (incl. Exec), batched per frame
  Propose{shard, epoch, base, ops}           for shards it doesn't lead (term input)
  Page{term, rows: range}                    demand-page scrollback
  Ack{shard, seq}

server → client
  Welcome{attachment, fenced?: reason}
  Grant{shard, epoch, seq} | Deny
  Snapshot{shard, seq, state}                full buffer / window / layout / term grid
  Entries{shard, from, entries}              tail, or server-authored rows
  Propose{shard, base, ops}                  server asking the leader to apply
  Ack{shard, seq}
  Hash{shard, seq, hash}                     divergence check
```

- Buffers a window shows are mirrored in full by default (acme loads whole
  files; they are bounded by what a person opens), so `Look`, `Edit` and
  B2 built-ins run locally. Where a buffer is too large to mirror, the
  client pages it and hands those commands to the server (§5).
- Terminal scrollback and `+Errors`-like unbounded output are demand-paged
  by row range. Attach sends the visible grid (a 200×50 grid is ~50 KB).
- Frames coalesce per display refresh; nothing is sent per keystroke.
- Attach and re-attach are the same code path: snapshot plus tail per shard,
  lazily per shard as windows become visible.

*As built (`apex-server/src/{proto,daemon,remote}.rs`).* Framing is
postcard with a `u32` little-endian length prefix — the entries already
derive serde, so one schema serves the log, snapshots and the wire;
protobuf stays an option for a gateway. What differs from the sketch:

- **No `Take`/`Grant` round trip.** `Hello{session, name}` makes a new
  attachment, grants it every leasable shard, and answers `Welcome
  {attachment, snapshot}` with the *whole* state (§12 on its size). A
  client that attaches while another holds the leases reclaims them at
  once — single-player mode collapses transfer-then-reclaim to reclaim.
  The fenced client stays connected and sees its appends refused. Resuming
  an attachment by id, `Page`, and `Hash` are not implemented yet.
- **Mirror logs.** The client runs its `Node` over `Log::mirror`, a log
  store built from the snapshot that assigns the same sequence numbers
  the server will store. The client leads with no round trip at all;
  `flush` ships `Append{shard, entries}` for every led shard, the server
  answers `Ack{shard, seq}`. Leases are derived from the log store on both
  sides (`Log::held_by`); the metalog is streamed to the client like any
  followed shard.
- **Shard creation is a fire-and-forget request.** `CreateShard{shard}`
  (sent by the mirror's hook) is processed by the server before the
  `Append` that follows it on the same connection, and the grant it makes
  (creator, epoch 1) is the one the mirror assumed. `DeleteShard` likewise.
- **Terminal input** is not a `Propose` but its own messages (`TermKey`,
  `TermPaste`, `TermResize`, `TermScroll`); the server's log store still
  fences them by connection. `OpenFile` and `Plumb` ask the server to look
  at the file system and answer with a proposal.
- The daemon keeps a follower replica of the whole session (the "view")
  for execs and snapshots, and forwards the metalog before any other
  shard so a client learns of a shard before its entries.

### 6.2 Control protocol (tools ⇄ server) — *v1*

Same frames, request/response plus subscription:

```
Read{buffer, range|addr}                     text at the latest flushed version
Info{window|buffer|layout}
Propose{...}                                 §5
Exec{window, text}
Subscribe{shards?, since}                    stream of entries (the acme event file, generalised)
Lease{take|release, buffer, ttl}             opt-in exclusive editing
Term{new|send|read|log}
Plumb{text, context}
```

Reads answer from the server's replica, which lags the leader by
microseconds locally or by link latency remotely; every answer carries the
version it is valid at.

### 6.3 Schema sketch

```proto
message Entry   { uint64 seq = 1; uint64 attachment = 2; uint32 epoch = 3; oneof op { ... } }
message Edit    { uint64 version = 1; uint64 start = 2; uint64 end = 3; string text = 4; }
message Select  { Part part = 1; uint64 q0 = 2; uint64 q1 = 3; }
message Rows    { uint32 first = 1; repeated Row rows = 2; }        // Row = repeated Cell
message Cell    { uint32 ch = 1; uint32 fg = 2; uint32 bg = 3; uint32 flags = 4; }
message Propose { Shard shard = 1; uint64 base = 2; repeated Op ops = 3; }
message Exec    { uint64 window = 1; string text = 2; uint64 at_seq = 3; Handler handler = 4; }
message Done    { uint64 exec_seq = 1; }   // also Failed{reason}, Unknown
```

---

## 7. The `apex` command — *v1 subset marked ●*

The CLI is the stable public API. Scripts never see the wire.

```
● apex server [--socket P]           start the daemon (also auto-started by attach)
● apex ls [host]                     list sessions on a server
● apex new-session [host/]name       create a session
● apex attach [host/]session [--stdio]  attach a UI to a session (transfer, else fence, other UI attachments)
● apex detach                        drop the current attachment (leases return to the server)
● apex new <path>...                 open files (windows in the current column)
● apex win list|del|move|resize      layout
● apex text read <buf> [--addr A]    read a buffer or address range
● apex edit <buf> '<Edit program>'   run the Edit language (a proposal)
● apex sel <win> [q0 q1]             read/set a selection
● apex exec <win> '<command>'        as if B2
● apex events [--since N] [--shard S] stream entries as JSON lines
● apex term new|send|read|log        terminals
  apex plumb <text>
  apex lease take|release <buf>
  apex lsp ...                       LSP bridge (§8)
  apex log [<shard>]                 provenance
```

Configuration is a script of `apex` commands run at server start
(`~/.config/apex/init`): plumb rules, tools to launch, defaults. Client-side
settings (fonts, scale) are `apex` commands the client interprets. There is
no configuration language.

*As built (`apex-cli`).* `apex` is the command; the gpui client is
`apex-ui`, which `apex attach` launches. Every subcommand attaches to the
session as a **tool**: it receives the same snapshot a UI would, reads
from its own replica, and proposes to the leader — there is no separate
control protocol, only `Propose{id, proposal}` / `Applied{id, result}` on
the attach protocol, plus `NewSession` and `ListSessions`. A session with
no UI attached is led by the daemon itself, so scripts work headless and
a UI that attaches later takes over what they did. `apex attach` starts
the daemon if the socket does not answer. Implemented: `server ls
new-session attach [--stdio] new win text edit sel exec events term
plumb`; `WIN` is an id or a unique substring of a window's name. Not yet:
`detach lease lsp log`, the init script.

Every session is a URL: `local:///name` on this machine's daemon
(`local` is the pseudo-provider that takes no argument),
`ssh://user@host/name`, `sprite://box/name` — scheme is the provider,
authority its argument, path the session. A daemon's first session is
`default`. The selector lists this window's session, the recent ones
(`~/Library/Application Support/apex/recent-sessions`, latest first),
this machine's, and for a remote window the destination's; what is typed
filters them, and typed as a name or URL and not listed, becomes
"Create …" — a bare name is `local:///name`, so making a session never
lands on the current window's provider by accident. "Rename this
session…" renames it on its daemon, attachments staying attached
(`RenameSession`; sessions have ids inside the daemon, names are labels).

`apex attach dest/session` (or a URL) goes through `providers.rs`. A *provider* is an executable `apex-remote-<provider>` on the
PATH, called as `apex-remote-<provider> DESTINATION COMMAND`: it runs COMMAND, a
single shell command line, on the destination with stdio connected —
ssh's own convention, and `ssh` is the built-in provider, so `user@host`
needs nothing. Other destinations are `provider:name` (`sprite:mybox`)
and need only that script (`providers/apex-remote-sprite` is one: `sprite exec
-s "$1" -- sh -c "$2"`). This keeps the rule that there is no
configuration file: the provider script is the whole configuration. The
destination needs nothing but the provider's access. We ask `uname -sm`,
pick the `apex` we carry for that OS and architecture (the Mac app
bundles `linux-amd64`, cross-compiled statically against musl with Zig
as the linker, and its own `darwin-arm64`), compare its sha256 with
`~/.apex/bin/apex` there, upload it over ssh's stdin if it differs, and
run `~/.apex/bin/apex --session S attach --stdio`, which starts the
daemon on the host if it must and copies bytes between its stdio and
the daemon's socket. The bridge knows nothing of frames; the UI speaks
them to the child's stdin/stdout exactly as to a socket. A remote
window's selector lists the host's sessions (`apex --ensure-server ls`
there); "Local sessions" leads back. Remembered sessions are
`host/session`; one that cannot be reached at launch falls back to the
local `local` with the error in `+Errors`. `APEX_SSH` names the ssh
program, which the tests point at a script running the commands under a
scratch HOME.

---

## 8. Extensibility

acme's tools port directly because the event model is the same, generalised:

- `win` → `apex term new`.
- `Watch` → twenty lines over `apex events`.
- acme-lsp → `apex lsp`: subscribes to versioned buffer edits, feeds language
  servers incrementally (the log *is* an incremental sync protocol), writes
  diagnostics into a buffer, and installs plumb rules so B3 on an identifier
  goes to its definition. Formatting is one proposal.
- Agents run in a terminal window and **edit files directly**; the server
  detects the edits through the file watcher (§9): a clean buffer takes
  them as proposals, a dirty one is flagged stale with `Get` in the tag,
  and by the nothing-is-lost principle a person's unsaved edit and the
  agent's write both stay visible. The CLI is a second door for agents
  that want to read selections, open windows or write to `+Errors`; a
  harness that needs exclusivity may take a buffer lease.
- **Plumbing** is a rule table, as in acme, with two additions. Tools
  install and remove rules at runtime, with a priority, as **metalog
  entries**: the table is replicated state, the client can display it,
  `apex log` shows who installed what, and a tool's rules are tied to its
  attachment — when the tool detaches or is garbage-collected, a metalog
  entry removes them.
  A rule may target a tool, which may **NACK**, after which the server
  falls through to the next matching rule. Predicates are richer than
  acme's: file type, syntactic context ("in a comment"), selection shape.
  So "LSP handles identifiers in source files; paths in comments get the
  default" is two rules, and only the tool a rule names is consulted. The
  exec entry records the chain (rule matched, tool NACKed, next rule
  fired), and `apex plumb --dry-run` shows it. Bidding — every plumber
  voting on every B3 — is deliberately not the model: it makes each click
  wait for the slowest plumber and its outcome hard to predict; a rule
  that fans out to several tools in priority order emulates it if ever
  needed.

Typed client libraries (Rust, Go) are generated from the schema for programs
that want more than the CLI.

---

## 8a. The Edit language — `apex-edit` — *v1*

The Edit language is an **independent crate**, a careful port of plan9port's
acme implementation rather than a reimplementation from the manual:

| source (plan9port `src/cmd/acme`, `src/libregexp`) | role |
|---|---|
| `edit.c` | parser: addresses, commands, text and regexp arguments, `{ }` blocks |
| `ecmd.c` | command semantics: `a c i d s m t x y g v p = u B D < \| >`, address evaluation |
| `elog.c` | the change log: a command's edits are collected against the original text and applied atomically at the end, with acme's ordering and overlap rules |
| `libregexp` | plan 9 regular expressions: **leftmost-longest** matching, `\n` in classes, no backreferences; Rust's `regex` crate is leftmost-first and is not a substitute |

Scope is acme's table exactly: sam's language minus `k n q ! cd`, plus acme's
`B` and `D`. Everything with an effect (`u e r w b B D < | >`) is not
performed by the crate; it is returned as an intent for the caller (core,
then the handler) to carry out, so the crate stays pure. Where acme and sam
disagree (dot after a change, changes out of sequence, `$` at the end of an
unterminated text, the bare newline command) the crate follows acme; the
crate's README lists them. `X`/`Y` are not supported (no file menu).

Interface: the crate takes a text (a trait over char-indexed, read-only
text, satisfied by a rope or a `&str`), a dot, and a program, and returns the
resulting change log (`Vec<Change{start, end, text}>` in original
coordinates, already validated non-overlapping and ordered), the new dot,
any output (`p`, `=`), and any intents. Core lowers the change log into
concrete `Edit` entries (§5).

**Acceptance tests** are the definition of "carefully ported":

1. *Golden tests* written from `sam(1)` and acme's documented semantics,
   one file per feature: each address form and combination, default
   addresses per command, `s` with `&` `\1`..`\9` and `g`, `x`/`y`/`g`/`v`
   nesting, empty matches (`x/^/`, `x/$/`), `{ }` blocks, `m`/`t` including
   moves into the moved range, `u` counts, error cases (overlapping changes,
   changes out of order, bad addresses) with acme's exact messages.
2. *Differential tests against plan9port.* `sam -d` runs the same language
   from stdin on a file. A test harness builds plan9port's `sam` from source
   (clang only, no dependencies) into the test directory, feeds it each
   program and input, and compares the resulting file and `p`/`=` output
   with the crate's. Skipped when plan9port cannot be built; run in CI.
   Programs come from the golden corpus and from a generator (random
   addresses and commands over random small texts), so the comparison covers
   cases nobody thought to write down.
3. *Regexp differential tests.* `,x/re/ =#` in `sam -d` prints the char
   address of every match, which makes sam itself the regexp oracle:
   generated patterns and texts, compared match by match, including the
   leftmost-longest cases and empty-match stepping.
4. *Performance:* `x/re/ c/.../` over multi-megabyte texts, thousands of
   matches, and deep nesting, with criterion, so regressions are visible
   before the UI exists.

## 9. Files

The server runs where the files are; that is the whole remote story.

- Three derived flags per buffer, each with its own signal: **dirty** =
  version differs from the version at last load or `Put` (acme's `Put`
  appears in the tag); **stale** = the disk changed underneath (`Get` in
  the tag); **unsynced** = the leader holds entries the server has not
  acknowledged (§4.1, the tag box).
- **Watching.** The server watches the parent directories of open files
  (editors and `git checkout` replace files by rename, which breaks per-file
  watches) with FSEvents/inotify via the `notify` crate, debounced. On an
  event it hashes the file; an unchanged hash (`touch`, the server's own Put)
  is ignored.
- **Clean buffer, disk changed:** the server diffs the new content against
  its replica at version V and proposes the edits with base V. Diffing rather
  than replacing lets every view's selection and origin adjust through the
  normal edit path. If the proposal is rejected because the user typed
  meanwhile, the buffer is now dirty and falls into the next case. The
  clean-buffer case works by version alone; no lease is taken.
- **Dirty buffer, disk changed:** the buffer is marked `stale` with the disk
  hash and `Get` appears in the tag — the word is the signal and the
  affordance, as with `Put`. `Put` refuses once with acme's "modified since
  last read" message and overwrites on the second `Put`. `Get` reloads. A
  later `Merge` may three-way merge base, buffer and disk.
- Directory windows refresh from the same watches. Deleted or renamed files
  are flagged the same way instead of failing at the next `Put`.

*As built (`apex-server/src/watch.rs`).* Parent directories are watched
with `notify`; the server hashes the file on every event and ignores it
if the hash equals what it last wrote there or the buffer's recorded disk
hash. A clean buffer gets `SetContent{version}` — *base-versioned*, which
matters: the daemon's replica lags the UI by a round trip, so it can
judge a buffer clean that the user has just typed into. The leader
lowers a `SetContent` whose version has moved on into `Stale` instead,
so typing is never overwritten by a reload; `Get` sends an unversioned
`SetContent`. A dirty buffer gets `Stale{hash}`; `Put` on a stale buffer
refuses once with "modified since last read" and writes on the second.
Diffing rather than replacing, directory refresh, and delete/rename
flags are not done.

---

## 10. Terminals and web windows

**Terminals** use alacritty_terminal on the server: pty, reader thread and
grid. The `term` shard is pinned to the server. The client renders rows from
its mirror and proposes keystrokes. Attach sends the visible grid; scrollback
is paged. Resizing is a proposal (`Resize{cols, rows}`) the server applies to
the pty. Selection inside a terminal (v2) is a `window` shard entry like any
other selection, computed over the mirrored rows, so B2/B3 on terminal text
work exactly as in text windows.

**Web windows** are out of v1. The prototype's headless-Chrome screencast
is a server-side renderer and fits the model, but it is bandwidth-heavy
over ssh, and the alternative (URL as server state, client-side rendering)
breaks "all state on the server" for page state. Decide later.

---

## 11. Client

The gpui prototype becomes the client. It holds one attachment per OS
window, each to a session on a local or remote server. The window title is
`host/session`; clicking it opens the **session selector**, listing sessions
on the servers the client knows, with "new local session" and "new remote
session" at the bottom. Switching is opening another window or re-pointing
this one (detach, attach).

- Holds a full replica of the shards it leads and a paged replica of
  terminals. Rendering reads only local state; the element code
  (`text_element`, `term_element`, `web_element`) already paints from its own
  records and needs no change.
- Input routing: B1/keys on a led buffer → local apply + append. B2 → §5
  split. B3 → plumb command to the server unless it is a plain `Look`, which
  runs locally. Keys over a terminal → proposal. Wheel/scroll → local
  `window.Origin` entry (leased) or a page request for terminals.
- Scroll origin is a `window` entry so re-attach lands where you were; it is
  cheap because it is coalesced per frame like everything else.
- The system clipboard is client-side, so Snarf/Paste work remotely with no
  protocol support.
- Fonts, geometry and colours are client-side. Colours follow plan9port
  exactly (`README.md`, Colours).
- Over a WAN, mosh-style predictive echo is unnecessary for editing (the
  client leads) and only relevant for terminals; it can come later.

*Tiling, as built (`apex-core/src/tiling.rs`).* Window and column
placement is plan9port acme's, ported function for function from
`cols.c`, `rows.c` and the geometry of `wind.c`: `coladd` (a new window
steals the lower half of the last one, or lands at the pointer), `colclose`
(the next window extends up, else the previous extends down), `colgrow`
(button 1 a few lines from the neighbours, 2 as big as can be, 3 the
whole column, leaving the others obscured until the column is "safe"
again), `colresize`, `colsort`, `coldragwin` (click, shuffle, move to
another column, flick right, resize against the window above), `rowadd`
(40% of the last column), `rowresize`, `rowclose`, `rowdragcol`, and
`makenewwindow`'s choice of where `New` and plumbed files go (the biggest
empty space, else the biggest window; `activecol` first). The layout shard
therefore stores pixel rectangles, like acme's `Dump`: each column's and
window's rectangle, the body rectangle, tag lines, lines shown, and
acme's `maxlines`. Every operation is computed by the leader against its
copy and appended as one `Arrange` entry, so replicas take geometry, not
recompute it; the client reacts to an OS window resize with `rowresize`.
What acme reads off its frames the tiling asks through an `Info` trait: tag
line counts and body lines measured in the client's last frame, or
one-line tags and full bodies for a headless leader. Mouse warps are
acme's too: into a new window's body near its box, onto `Del` of the
window that took a closed one's place (or back to where the mouse was
before the closed window was made), to the layout box after a grow or
drag, and to the selection when an already open file is plumbed. macOS
gives no mouse event for a warp, so the client remembers where it put the
pointer for keyboard routing until the mouse really moves.

*Fidelity audit against plan9port acme (2026-09-05).* Read side by side
with `acme.c`, `text.c`, `exec.c`, `look.c`, `wind.c`, `cols.c`, `rows.c`
and `scrl.c`. Matching now: the three buttons and their chords, including
B1 while B2 is down passing the last selection as the command's argument;
double-click selection of bracketed and quoted spans and lines
(`textdoubleclick`/`textclickmatch`); `Del`/`Delete`/`Delcol`/`Get` through
`winclean` (scratch windows such as `+Errors` and directories go at once;
a dirty window warns "name modified" once and goes the second time; a
small unnamed window never whines); the tag's words `Undo Redo Put Get`
coming and going as acme's `winsettag1`; `Newcol` making an empty window;
the last column being deletable, `New` making a column when there is none;
`Look` searching the last-selected text (`seltext`), wrapping, and moving
the mouse to the match; `Tab`, `Indent on|off` (autoindent), `ID`; Up and
Down in a tag shrinking it to one line and expanding it; Up/Down scrolling
a third of the window and PageUp/PageDown two thirds; Home and End; the
erase keys; and the pointers: plan9port's big arrow always, the box while
a layout box is held. Also matching: the scrollbar scrolling
continuously while a button is held, the pointer kept on the bar
(`textscroll`: 200 ms, then every 80 ms, by the pointer's height in the
bar); `^F`/Insert filename completion (`textcomplete`, with the file
system on the server: a unique match completes with `/` or a space, a
common extension is inserted, otherwise the candidates are listed in
`+Errors`); B1 dragging past the top or bottom scrolling by the distance
(`framescroll`); the mouse pulled up as a tag closes under it and pushed
down as one expands over it (`winresize`); `Kill name` ending every
running command whose first word is `name` (the server tracks the
commands it starts); the top row naming each running command at its
front while it runs, "`name: exit status`" in `+Errors` when one ends
badly (`waitthread`, `runproc`'s naming: the first word without its
directory, `|sort` for a pipe); `Zerox` refusing directories; newline ignored in
column and top tags; `+Errors` named `dir/+Errors` after the directory of
the window whose command produced the output, made in the last column
(`errorwin1`), with no `Undo`/`Put` words in its tag. Decided against
acme, by design: `Exit` detaches this window and the session lives on
(acme refuses while windows are dirty; nothing is lost here); there is no
`Dump`/`Load`, sessions are the only persistence; `Send` on a terminal
types the snarf buffer plus a newline into the shell and on a text window
appends it to the body; `Local`, `Incl` and `Abort` are not commands.

^H, ^U and ^W erase exactly as acme's `textbswidth`: ^W skips
non-alphanumerics, then erases the alphanumeric run, never past the start
of the line or the window's origin; with a selection, the selection is
cut first and then the width is erased, as acme does.

*As built (`apex-client/src/shell.rs`, `mac/`).* `mac/build-app.sh`
makes `Apex.app`: `apex-ui` as the executable, the `apex` command beside
it, and an icon of Glenda with big eyes (`mac/glenda.svg`, rasterised by
headless Chrome). Launched with no arguments the app makes sure a daemon
answers on the default socket — starting one through the bundled `apex`
with the home directory as its working directory — and opens one window
per session it had open last time (`~/Library/Application
Support/apex/last-sessions`), else the first existing session, else a new
`local`. The title bar is the app's own: the session name is a button
that drops down the selector (also ⌘K), a list filtered by what you type,
where a name that matches nothing becomes "Create session". Choosing
re-points this window; ⌘N opens another window on the same session. The
menu bar carries Quit, Hide, New Window, Sessions, Close Window and the
Edit menu, whose items act on the text under the pointer, as acme's keys
do. Not yet: remote sessions in the selector, an About window.

---

## 12. Logs in memory; compaction

Logs live in the server's memory; the server is a long-running daemon. No
persistence in v1: terminals and tools could not be restored anyway, so we
admit it rather than pretend. Snapshots are taken periodically (and on
detach) by the node holding the state, and compaction truncates a log
below its snapshot, so logs stay bounded. A server restart is a fresh
world, as acme's is; `Dump`/`Load` of layouts and buffer contents can come
later.

*Measured:* the attach snapshot carries each buffer's undo and redo
history, so a session that has absorbed 100k keystrokes ships a 770 KB
snapshot for 115 KB of text. Undo should be bounded (acme keeps it all,
but acme never ships it) or left out of the attach snapshot and paged on
demand; either brings attach down to the cost of the text.

---

## 13. Security

Local access is the Unix socket's file permissions. Remote access is ssh;
the server never listens on the network. Attachments are unforgeable
server-issued ids; fencing prevents a stale attachment from writing. Tools run with
the server's credentials, as acme's do.

---

## 14. Milestones

1. **`apex-edit`** — the Edit language and libregexp port with the
   acceptance suite of §8a, including the `sam -d` differential harness.
   *Done.*
2. **`apex-core`** — buffers, views, undo, shards, entries, `apply`,
   snapshots, hashing, built-in commands, lowering of Edit change logs.
   Property tests: two instances, random entries, equal hashes. Criterion
   benches for apply, view adjustment, snapshot/hash, and replay. No UI: a
   headless driver replays recorded logs for profiling. *Done.*
3. **Server + attach** — registry and metalogs, sessions, attachments,
   leases with transfer-then-reclaim, fencing, the attach protocol over a
   Unix socket, detach/re-attach, terminal rows paged, `+Recovered`.
   *Done except:* transfer (reclaim only), resume by attachment id, row
   paging, `+Recovered`.
4. **Client** — the gpui prototype re-layered as a pure renderer and input
   device over core state, first in-process, then over the socket. Zerox
   becomes two views on one buffer. Multiple sessions per client and the
   session selector. *Done except* the session selector.
5. **CLI + control protocol** — `apex` subcommands, `events`, proposals,
   `win`/`Watch` ports, plumbing rules as commands. *Done except* `Watch`,
   plumbing rules, `detach`/`lease`/`log`, the init script.
6. **Files** — watcher, stale/dirty flow, `Get` in the tag. *Done except*
   directory refresh and diff-based reload.
7. **Remote** — `apex attach host/session` over ssh stdio; hash checks.
   *Done except* hash checks; exercised with the bridge run locally, not
   yet over a real ssh session.
8. **Tools** — `apex lsp`, agent harness, tool leases.

---

## 15. Measurements

Release builds on an M-series laptop. Core (`apex-core/benches`): a
keystroke applies in ~260 ns, an edit adjusting 100 views in 0.7 µs, a
1 MB buffer hashes in 0.9 ms and snapshots round-trip in 0.3 ms, 20k
entries replay in 2.2 ms. Edit (`apex-edit/benches`): `x/fox/ c/cat/` over
1M runes in 3.2 ms.

Socket (`apex-bench`, daemon on a thread of the same process, one round
trip = client → kernel → daemon → kernel → client):

| what | p50 | p99 |
|---|---|---|
| ping round trip | 12–20 µs | 18–42 µs |
| one keystroke: local apply, flush, server ack | 14 µs | 20 µs |
| 100 000 keystrokes in one flush | 1.8 M entries/s | |
| attach: connect, snapshot, decode; 120 KB text, 770 KB snapshot | 4.2 ms | |
| attach; 1.2 M runes, 1.9 MB snapshot | 6.2 ms | |
| attach; 11.5 M runes, 11.9 MB snapshot | 32 ms | |
| terminal key → new rows on the client | 180 µs | 260–540 µs |

Through `apex attach --stdio` run locally (the ssh path minus the
network: two more processes and two pipes per direction): ping 26 µs,
keystroke → ack 21 µs, throughput unchanged, attach +4.5 ms flat (the
snapshot crosses two more pipes), terminal key → rows 195 µs.

The transport is nowhere near the budget of a frame; the attach cost is
the snapshot's undo history (§12), and terminal latency is the pty and
the parser, not the socket.

---

## 16. Open questions and exploration

### Exploration: tool-defined shards

Not for v1, but worth keeping in view. Today the shard kinds are fixed
(buffer, window, layout, term, metalog, registry) and tools reach the
system through proposals, execs and metalog entries. The generalisation is
to let a tool **register its own shard kind** with its own entry schema:
the shard is created through the metalog like any other, leased to the
tool (pinned to it while it lives), replicated to clients that subscribe,
snapshotted, hashed and paged by the same machinery. A plumber would then
be a shard whose entries are rules rather than a set of metalog entry
types; an LSP tool would keep diagnostics in a shard the client renders
in the gutter; an agent harness would keep its transcript in one. The
appeal is that "extension" would mean "another shard", with fencing,
provenance and re-attach for free. The cost is a schema-registration
story, clients that must render entries they do not understand, and a
harder question of what a client-led shard kind would even mean. Do it
only if a second or third concrete need for it appears beyond plumbing.

### Open questions

- Granularity of `layout` leasing when a tool wants to open windows while the
  UI leads layout: proposals suffice, but a burst of `apex new` from a script
  is many round trips; batch in the CLI.
- Whether `window` selections for terminals should be cell-based or
  offset-based over the paged rows.
- Web windows (deferred): server-side screencast vs client-side Chrome
  with server-side URL state.
- Persistence: whether to snapshot buffers and layouts to disk later, and
  how that interacts with the nothing-is-lost principle on a server crash.
- Multiple UI attachments to one session with independent layouts
  (multi-player) — explicitly out of scope; the per-shard design leaves
  the door open.
- Naming: keep acme's command vocabulary verbatim (`Newcol`, `Delcol`,
  `Zerox`, `Putall`) so muscle memory and existing scripts carry over.
