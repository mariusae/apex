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
| `window` | window | `Create{tag, body}`, `Font`, `Exec{text, handler, at}`, `Status{exec, Done|Failed|Unknown}`, `Delete`; *as built also* `Tab`, `Indent`, `TagExpand`, `Live{by}` | server |
| `layout` | session | `Init{top}`, `ColNew/Del/Resize`, `WinPlace{window, col, at}`, `WinRemove`, `WinResize`, `Snarf`, `Exec` from column tags and the top row; *as built also* `Arrange{r, cols}`, `Status`, `Visit{from, to}`, `NavPop{back, at}` (the navigation stacks, `Layout{nav_back, nav_forward}` of `Loc{name, pos}`) | server |
| `term` | terminal | `Rows{seq, rows: [cells]}`, `Cursor`, `Mode`, `Resize`, `Exit` | server, **pinned** |
| `metalog` | session | `ShardNew/Del`, `Attach/Detach`, `LeaseRequest/Release/Grant/Reclaim{shard, attachment, epoch, seq}`, `PlumbRuleInstall/Remove{attachment, priority, predicate, action}`; *as built also* `Set/Unset{owner, key, value}` (settings, the session's or an attachment's) | server, **pinned** |
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
`apex attach host` runs `ssh host apex attach -stdio` and speaks the same
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

*As built, the wire (`proto.rs`, `proposal.rs`; postcard frames, u32 LE
length):*

```
client → server
  Hello{session, name, kind, attach?}     attach; a UI ships its ~/.apex/attach
  NewSession{name}                        make a session labelled name (idempotent); its host's profile runs
                                          (Hello/EndSession/RenameSession name a session by its id, a
                                          unique prefix of it, or its label; Sessions lists {id, label};
                                          Ended{id, label})
  ListSessions · RenameSession{from, to} · Stop · Ping{t}
  Append{shard, entries}                  entries this client sequenced as leader
  CreateShard{shard} · DeleteShard{shard}
  TermKey{term, key} · TermPaste · TermResize · TermScroll{term, delta}
  TermText{term, p0, p1}                  snarf a terminal range (answer: a Snarf proposal)
  TermRead{term, from, to}                a terminal's lines, scrollback included → TermLines
  OpenFile{col, ctx, name}                the server reads the file and proposes OpenWindow
  Plumb{ctx, text, dir?, edit_only, dry, at?, sel?, alt?, reverse, verb?}
                                          B3 / apex plumb / B; at: the pointer, sel: what was taken,
                                          reverse: shift-B3; verb: a rule's verb at the pointer
                                          instead of plumb (cmd-B3 is Def)
  PlumbAck{id, ok}                        a tool's answer to a Plumb it was handed (the Plumb names
                                          the rule that matched, so a tool with several can tell)
  RuleAdd{rule, priority, mine} · RuleRm{id}
  Complete{view, ctx, at, prefix}         ^F
  Propose{id, proposal} · Applied{id, result}
  Env{set} · EnvImport{vars} · Set{key, value, attachment?} · Ps · Kill{targets}
  EndSession{name, force}                 the session ended: killed, everyone cut off (Ended)
  Named{name, group, pid, cmd}            what a program is called: the entry of its process
                                          group (the shell the server started) takes the name in
                                          the top row, ps and Kill; a group the server did not
                                          start is adopted under pid until this connection goes
  Io{stream, frame}                       the I/O plane (WEB.md §1): Request opens a stream
                                          the client numbers; Body, End, Reset

server → client
  Build{protocol, id}                     first frame, frozen: refuse another protocol version
  Welcome{attachment, snapshot} · Entries{shard, entries} · Ack{shard, seq} · ShardReady{shard}
  Propose{id, proposal} · Applied{id, result}
  Sessions{names} · Error{text} · Pong{t} · Env{vars} · RuleAdded{id}
  PlumbTrace{lines}                       a dry run's report
  Plumb{id, ctx, verb, text, dir, groups, at?, sel?}
                                          a rule named this tool; answer PlumbAck within a second
  Io{stream, frame}                       Response{status, headers}, Body, End, Reset
  Ended{session}                          the session this connection was on is gone
  Ps{procs} · TermLines{term, text}

proposals (tools and the server → the leader; applied by whoever leads)
  OpenWindow{col, from?, name, text, hash, select_line?} · NewWindow{col, name} · TermWindow{col, name, term}
  SetContent{buffer, version?, text, hash} · Clean · Rename{buffer, window, name} · Stale
  Switch{session, window?}                 show another session (by id, prefix or label), at a
                                           window there: the leading UI switches to it
  ReplaceRange{select, dir?, buffer, version, q0, q1, text}
                                                       select: pipe output, left selected as acme's |
                                                       does; else a tool's write, dot left alone as a
                                                       write to acme's data file leaves it
  Insert{buffer, version, at, text}                    at a point, the selection left alone (win)
  Errors{dir?, text} · Complete{view, at, text} · Snarf{text} · TermName{window, name}
  CommandStart{name} · CommandExit{name} · Status{ctx, exec, status}
                                          (the top row edited without moving seltext: a command
                                          ending never steals where keys and looks go)
  Look{ctx, text, reverse} · ClientDo{verb, args}     the last resort of a plumb; a UI's own verbs
  Exec{ctx, text} · Edit{window, program} · Select{view, q0, q1}
  Live{window, by?}                                    a process behind a window
  Goto{loc} · Nav{back}                                a jump; Back and Fwd along the stack
  OpenWeb{col, url} · WebNavigate{window, url}         a web window; its page moved (WEB.md §2)
```

*As built, versions:* the daemon's first frame on every connection is
`ServerMsg::Build{protocol, id}`: `PROTOCOL`, a number in `proto.rs`
bumped by hand with every change to anything on the wire (messages,
proposals, entries, ops, state, since postcard is not self-describing),
and the build id, a hash of the workspace sources computed at build time
(the same on every target), which only says which binary it is. The
variant stays first and unchanged. A client of another protocol version
stops at that frame with an `Unsupported` error that says what to do:
when the daemon's sessions can be let go, `apex stop` on its machine,
then attach again (Reconnect, ⌘⇧R). Builds of the same protocol talk
to each other, so a rebuild for a bug fix in the client needs no
restart of the daemon. The daemon is
never restarted behind the user's back: its sessions may hold work. When
the local daemon refuses at startup, the app opens an in-process window
showing the error, pointed at the session so Reconnect tries it again.

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
new-session attach [-stdio] new open win text edit sel exec events term
plumb B env set cat lsp label awd version`; flags are Go's (`-flag=value`)
and `apex help` documents everything, `apex help <topic>` included;
`apex ps` and `apex kill` see and end what the server runs (the
`Running` list behind the top row's names: pid, name, origin, start
time, directory, command line). `apex end-session [-f] [NAME]` ends a
session: refused while a window there is unsaved unless forced; else
its commands and terminals are killed, everything attached gets
`Ended` and is cut off (a window on it goes offline saying so, a
parked one is dropped), and the daemon goes on for the others; the
picker's `end` on a session row does the same, on the host through
`apex end-session` there, and so does `End` in the top row (`End -f`
forces), which then closes the window. A program says what it is
called with
`Named`, so `apex tool lsp` is `lsp` in the top row, not `apex`: the
server renames the entry of the announcer's process group (the exit
reports the new name too), or adopts an announcer it did not start for
as long as its connection lasts, ending it by pid. What a script
(`profile`, `attach`) leaves in the background, `apex tool lsp &`, is
adopted the same way, its own entry; and a command is over when it
exits, not when its pipes close, so the script is gone from `ps` and the
top row while the tool it started lives on.
`apex tool win` and `apex tool lsp` announce themselves once, after
attaching. `WIN` is an id or a unique substring of a window's name. Not yet:
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

Commands run as plan9port acme runs them: `rc -c command`, with `$winid`
set to the window's id (or the last selected window's), and `$%` and
`$samfile` naming the window's file; `$acmeshell` names another shell.
The `rc` is [mariusae/rustrc](https://github.com/mariusae/rustrc), a
drop-in port of plan9port's; the app carries it beside `apex` for this
machine and for each remote target, `deploy` puts both in `~/.apex/bin`
on a destination, and the daemon puts that directory and its own on the
session's PATH. Without an `rc`, commands fall back to `sh`.

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
run `~/.apex/bin/apex -session=S attach -stdio`, which starts the
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

*As built, session identity:* every session has an identity, a UUID
minted when it is made and recorded as the second entry of its metalog
(`MetaOp::Identity{id}`, so every replica knows it from its snapshot),
and a label for people (`MetaOp::Label{label}`, appended again on a
rename, so attached clients learn the new label from the log). The
daemon keys sessions by id and resolves whatever names one — the id,
a unique prefix of four characters or more, or the label. A label starts
with a letter, then lowercase letters, digits and `-`
(`providers::valid_label`, checked by the daemon, the CLI and the
picker alike). Commands
and terminals get `apexsession` = the id and `apexsessionlabel` = the
label; `apex ls` prints `label<TAB>id`; a window is nameable anywhere
as `id.N`: a bare id or name is the session at hand's, and the CLI
takes a `SESSION.N` argument as choosing that session for the command.
The socket names the daemon, not a session, and stays as it is.
*Places across sessions:* a `Loc` carries the session it is in (by
identity; `None` is the session at hand) and names a window by name or
by id as digits, so `<session>.<win>` is a place anywhere. A `Goto` or
`Nav` (Back, Fwd) whose place is in another session is not landed by
the node but handed to the UI as a switch (`Node::take_switches`);
the UI switches to that session (parking this one, taking or attaching
the other by its identity) and lands there once it is up. `current_loc`
records the session, so the navigation stack spans sessions and Back
returns across them. `Proposal::Switch{session, window?}` is the same
for a tool that only wants the session shown (`apex switch`, the tool
API's `switch`, the bridge's `switch`, Go's `Switch`); `apex B
session.N[:line]` makes the `Goto`. All of this is within one daemon:
a session id is meaningful anywhere but is acted on where its daemon
is. *Intended, not built:* places across daemons go through the client
UI, the one party that knows several — as a URL scheme the app
registers (`apex://<session>/<win>`), so a tool on any host emits a
URL the plumber already carries and the app resolves the session id
against its known hosts and switches, by the same path as `Switch`.

*As built, profile and attach:* two scripts, named by what they
configure. The host's `~/.apex/profile` is the session's setup on the
daemon's machine; the client's `~/.apex/attach` is its own per
attachment, the UI's tweaks. On one machine both live in the same
`~/.apex` and each runs once in its role. (A third, the creator's
`~/.apex/session` shipped in `NewSession` to provision hosts with no
profile of their own, was built and then dropped, for now: a host
configures itself.) A new session runs one `rc` on its host that
sources the host's profile, as a
command named `profile` with output in `+Errors`, and `apexsession`,
`APEX_SOCKET` set, and `EDITOR` to `apex-editor`, a link
beside the binary the CLI knows by name as `apex editor` (editinacme:
the file plumbed to edit, exit when its window goes; one word, since
zsh and rc do not split `$EDITOR` into words). The profile's environment at its
end is the session's: the server prefixes the script with an exit hook
(rc's `fn sigexit`, sh's `trap ... EXIT`) that runs `apex env -import`,
which sends the shell's whole environment (`ClientMsg::EnvImport`);
against the environment the profile was given, variables set or changed
are set in the session's and dropped ones unset, the shell's own
bookkeeping (`pid`, `status`, `path`, `PWD`...) ignored. rc exports
lists (`\x01`-joined) and functions (`fn#name`) as variables, so a
profile's `fn g {...}` works in every terminal and B2 command. The rc is
run in its own process, not embedded: rust-rc forks for pipelines and
`&` with the interpreter running on in the child, no fit for the
threaded daemon. `apex env KEY=VALUE` (`ClientMsg::Env`) still sets
the session environment from anywhere at any time. A client's
`~/.apex/attach` runs the same way on every attach (`Hello` carries it),
with `apexattachment` set, so `apex set` there records the attachment's
own settings (`MetaOp::Set{owner}`, dropped on detach); `apex set` from
the profile records the session's. Clients resolve a key through their
own settings, then the session's. `Preview` is a live pipe through a
converter (WEB.md §3, built): `Preview.EXT` names a command that reads
the file on stdin and writes HTML (`apex md` for Markdown, `cat` for
HTML and SVG unless set), the server keeps one rule per such extension
running `apex tool preview $file`, and the tool renders the buffer into
an HTML window `FILE+Preview` beside it as it changes. `Preview` alone
names the app a rule's `-client=preview` falls back to, else Quick
Look. The client drives file I/O on the I/O
plane (WEB.md §1, built): `GET file://path` is one-shot, with a `Watch`
header the stream stays open and every change brings a `Body` holding a
`FileFrame{version, path, bytes}` (the daemon adds the path to the
watcher and fans changes out to every watch stream) until the client
ends the stream or its connection goes; `PUT file://path` writes. A
stream belongs to its connection, so fencing changes nothing. A remote
preview is a watch stream feeding a local copy under a path mirroring
the host's, ended with the previewer's process, the file's window, or
the lead. `CONNECT host:port` tunnels bytes to the host's network and
`http(s)://` is fetched by the host (stage 2, built). `apex io` is the
plane from a shell; `apex cat` is `GET`.

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

*As built (plumbing):* the rule is `PlumbRule{verb, text, file, kind,
isfile, isdir, action, to}` in the metalog (`PlumbRuleInstall{id,
attachment, priority}`), owned by the session (`SERVER`) or by an
attachment, whose rules go with it. A **verb** is the command a rule
answers: `plumb` is B3; any other verb is offered in the window's
**tools menu** on B4 (shift-click without a fourth button): libdraw's
`menuhit` as mariusae/plan9port's acme pops it up (`client/menu.rs`, the
numbers and colours menuhit's) — the verbs the rules offer this window,
opened so that the last one chosen sits under the pointer, which is
warped onto it, so a click alone repeats it; tracked while the button is
held, the item under the pointer highlighted in negative, none outside;
released on an item it runs as B2 would, released outside nothing runs;
more than 25 items scroll. Verbs are not written into tags. B2 on a
verb's word walks the rules with it, ahead of the shell. A `Plumb`
carries where it came from: `at`, the pointer or dot as a `Span`, and
`sel`, what was swept or the selection the pointer was in; a click
sends `at` alone. The expansion is acme's, one for one (`look.c`'s
`expand`/`expandfile`, `apex_core::expand`), and runs on the server,
where the files are, at the start of the walk: from the pointer, the
text runs over `isfilec` characters to the first colon on the right (a
scheme's `http:`/`https:` colon excepted), back over file, address and
regexp characters, then over address characters after the colon; when
the name before the colon is a window's name or a file relative to the
window's directory (an empty name, `:12`, is the window's own), that is
what B3 took, with the address text up to white space (`123:1` gives
line 123, the rest ignored as acme's `address` stops there; `#12` a
character); a `http://` or `https://` run is taken whole; otherwise the
word (acme's `isalnum`). A sweep or selection is taken as it is, still
read for a name and address. The rules see that text; when none takes
it, a file opens at its address (acme's `look3` after plumbing) and
anything else is looked for. `alt` is no longer sent. Left out:
`<name>` as an include file. Actions: `Edit` (open in the session), `Run` (a host command,
the selection on stdin), `Client{verb,args}` (a `ClientDo` proposal to
the UI, which may refuse; a headless leader always does), `Tool(name)`
(`ServerMsg::Plumb` to the attachment of that name, `PlumbAck` within a
second or taken as NACK). *As built, the tool API:* `apex-tool` is the
Rust API for tools, a curated surface on `Remote` with nothing of the
wire or the replicated state showing through: `Tool::attach(name)`,
`next_event` (`Plumb`, `Edit`, `Renamed`, `Deleted`; `None` when the
session is over), `answer(plumb, taken)`, `offer(Rule)`/`withdraw`,
`new_window open read replace append select selection rename set_live
delete exec exec_in errors watch unwatch set setting`. Depending on
`apex-server` directly is the internals, not the API. The bundled
tools (win, lsp, preview) stay on the internals on purpose: they ship
with the daemon and are rebuilt with it, so the lockstep costs them
nothing, and win in particular needs what the crate hides. *Tools in other languages:* `apex tool bridge NAME` attaches as
the tool NAME and speaks JSON, one object a line, on stdin and stdout
(`apex-tool-bridge`, a client of `apex-tool`, its commands and events
the crate's methods and events one for one): commands in
(`windows new open read write select rename live delete exec errors
rule unrule ack watch unwatch set setting`), each answered in order by
id; events out (`hello`, `plumb` with the rule that matched, `edit`
for a watched window's body by others, `renamed`, `deleted`, `bye`).
The bridge is one `Remote` with the win-style `before` hook for edits;
nothing of the wire protocol or the replicated state shows through.
The Go package under `go/` (`github.com/mariusae/apex/go/apex`) is a
client of it: `Attach`, `Window` methods, `Offer(rule, handler)`,
`Watch`, `Serve`. The daemon drives a walk across those answers
(`Server::plumb_start`/`plumb_next`); an in-process client walks
synchronously. The trace of a walk is what `apex plumb --dry-run` prints;
recording it in the exec entry is still to do. Clients say what they can
do by installing rules on attach (macOS: URLs to `open`), so the server
holds no platform knowledge. `apex B` is plan 9's `B`, and rc's rcmain
defines `B` in apex terminals. The one-line rule form was chosen over
plan 9's rules file: each rule prints as the flags that made it.

*As built (`apex tool lsp`, crate `apex-tool-lsp`):* unprivileged, as intended: a
tool attachment named `lsp` that keeps a replica, reads `Entries` before
applying them to turn `BufferOp::Edit` into incremental `didChange`
(positions computed against the text as it still is), proposes
`Select`/`ReplaceRange`/`Errors`/`NewWindow`+`SetContent` for what
servers answer, and installs rules owned by its attachment: B3 on an
identifier at priority 10, the verbs `Def Refs Type Hov Sig Fmt Rn` in
the tools menu. Servers from settings (`lsp.LANG`), one per workspace
root found by markers; diagnostics in `root/+lsp`. Not yet: completion,
format on Put (needs rules ahead of builtins), semantic predicates for
rules ("in a comment"). `apex tool win` (crate `apex-tool-win`) is acme's win
ported from win.c: the shell on a cooked, echoing pty, the output point
and typing run kept from the buffer's entry stream (our own inserts told
apart by a queue of what we proposed), echo cancelled, ^C/DEL/^D as win
reads them, raw mode when echo is off; `Win` in the top tag runs it as a
command named Win. Unbound control keys are inserted as acme inserts
them, which is what lets win see ^D. The rules that make the window
win's are the window's own (`-win=ID`, an id never reused), so the
shell renaming it (`awd` on cd) changes nothing and two wins never
overlap; Home and End are acme's
`Khome`/`Kend` with `iq1`, the last insertion point (where typing or
erasing last left the cursor, moved along by output before it): Home
brings its line to the top when it has scrolled off above (a win's
output ran past the prompt typed at), else shows the top; End brings
it back when it is below, else shows the end; neither moves the
selection. The client keeps, per buffer, where the last `Insert` or
`ReplaceRange` proposal it applied as the lead ended, which is how
win's output arrives. The window follows that
output by acme's rule (`xfidwrite`'s `shouldscroll`): when the point
the text went in at was on screen, the end of it is shown, three
quarters of the window down as for a window with an event reader;
scrolled away from it, the window stays where it is. Win's `Select`
after its output moves dot only; a `Select` proposal names no window,
so it neither scrolls to nor focuses it.
B2 on any text in its window (win's
'x'/'X' events, `sende`) types it to the shell, appended after the typing
with a newline, dot after it: win holds a rule with the special verb
`exec`, which takes every B2 command in the windows it applies to that
no builtin and no other verb took, the whole line as its text, and is no
word in the tools menu.

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
Directory windows are watched too (the directory itself, for its
entries): a change under it lists the directory anew when the window
is clean, and flags it stale when edited, the same flow. A stale
window has its own handle colour, gold (`STALE`), a caution in one of
acme's yellows, chosen by simulating deuteranopia to stand clear of
every other handle colour and the tag's border (a dark cyan fell on
the live raspberry, an orange on the scrollbar's olive, a medium blue
on the border's purple-blue): dirty, and the disk has moved
on underneath, so `Get` would drop the edits. Diffing rather than
replacing and delete/rename flags are not done.

---

## 10. Terminals and web windows

**Terminals** use alacritty_terminal on the server: pty, reader thread and
grid. The `term` shard is pinned to the server. The client renders rows from
its mirror and proposes keystrokes. Attach sends the visible grid; scrollback
is paged. Resizing is a proposal (`Resize{cols, rows}`) the server applies to
the pty. Selection inside a terminal (v2) is a `window` shard entry like any
other selection, computed over the mirrored rows, so B2/B3 on terminal text
work exactly as in text windows.

*As built:* terminal selection is client-side, not a shard entry. The
term shard carries `View{top}`, the history line in the viewport's first
row, and a selection is two `(column, history line)` positions, so it
stays on its text as the terminal scrolls. B1 drag selects (acme's
yellow); cmd-c / Edit ▸ Copy, `Snarf` in the window's tag, or the B1+B2
chord send `TermText` to the server, which has the scrollback, and its
answer is a `Snarf` proposal (so Paste and Send have the text); the
clipboard follows the snarf buffer. Wrapped lines join, as in alacritty.
cmd-a selects the viewport. B2 and B3 sweep too (acme's `textselect23`,
in but2col/but3col): what was swept is executed or plumbed, a plain click
takes the word under it, and the B1 selection is left alone. Keys and
pastes first scroll the terminal
back to the bottom, republishing the rows at once. History lines are
numbered from the oldest line kept, so a selection drifts once the
scrollback limit truncates; a selection does not outlive that.

*As built, labels and environment:* the pty loop is alacritty's with
`win`'s label scan in front of the parser. A terminal's name follows one
rule, `{osc7 path}/-{title}` (`term::compose_name`): OSC 7
(`file://host/path`) is the path, and once it has reported, nothing else
ever is; the title is an xterm title (OSC 0/2) or plan9port's
`ESC ] ; text BEL` label (`apex label`); with a title but no directory
reported the name is `-title`. `apex awd [LABEL]` reports the directory
and titles the window `LABEL` (the host), so it reads `pwd/-host` as
plan9port's awd names it. The reported directory is the terminal's,
where B2/B3 resolve relative names. The win tool names its window by the
same rule. A new terminal is `dir/-host` (win's naming); `Newterm cmd args` runs
that through the login shell instead of a shell, named `dir/-cmd`, as
`win cmd` does. The
shell is a truecolor `xterm-256color` with `TERM_PROGRAM=apex`,
`apexsession` (the session's id), `apexsessionlabel` and `APEX_SOCKET`
set, so `apex` inside it addresses the
session it runs in; commands run from tags get the same two. The shell
is the `Newterm.shell` setting (`apex set Newterm.shell zsh` in the
profile), else the daemon's `$SHELL`. Keys go xterm-style with option
as meta: ESC before the key itself (opt-b is `ESC b`, not `∫`),
opt-left/right as `ESC b`/`ESC f` (Terminal.app's defaults, what zsh
and bash bind for words), opt-backspace `ESC DEL`, and other modified
keys in xterm's `CSI 1;m` form (opt-up is `ESC [1;3A`). OSC 8
hyperlinks travel in the term shard (`Cell::link` into
`TermOp::Links`), draw underlined, and B3 on one plumbs the link
rather than its text. A terminal's tag has `Send`, win's: the text
swept with B1, else the snarf buffer, typed into the shell with a
newline (the client sends a visible selection itself; the server sends
the snarf buffer). A terminal scrolled back stays on what it shows
while output goes on below, as win does, until a key or paste brings
it back to the live screen. The wheel over a terminal goes to the program
when it asked for the mouse (DECSET 1000/1002/1003: wheel buttons 64
and 65 at the cell, SGR or X10 form) or runs on the alternate screen
with alternate scroll (DECSET 1007, xterm's default: up and down
arrows); otherwise it scrolls the scrollback, as the scrollbar always
does.

**Web windows** are out of v1. The prototype's headless-Chrome screencast
is a server-side renderer and fits the model, but it is bandwidth-heavy
over ssh, and the alternative (URL as server state, client-side rendering)
breaks "all state on the server" for page state. *Decided:* the URL is
the state, rendering is the client's, and network and file I/O go
through the server on an I/O plane beside the log; see WEB.md, which
also makes Preview a live pipe through a converter. *As built:*
`Body::Web`, the URL as the window's name (its tag's first word, as a
terminal's directory is), `Newweb URL` and `apex web open URL`
(`Proposal::OpenWeb`), the client's `wry` view over the body, its
navigations `WebNavigate` proposals that rename the window and push
the place left onto the navigation stack; a `Goto` to a URL nobody
shows opens a web window on it. The view's traffic goes through the
host: a localhost `CONNECT` proxy per link whose tunnels are streams
on the I/O plane; `apexfile:///path` is a host file fetched on the
plane and watched, the page reloading when it changes (WEB.md §2.3,
§2.4). `Body::Html(buffer)` is a text buffer shown as a page
(`cmd | apex web`, `Proposal::OpenHtml`), patched in place as it
changes (WEB.md §2.5).

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
- *As built:* `+Errors` behaves as acme's `flushwarnings`: new text is
  appended, selected (`textshow(q0, end, 1)`), and when its start is off
  screen the origin is set so that it sits a quarter of the window down
  (`Node::errors` records the position; the client's element applies
  textshow's rule).
- *As built:* navigation is session state, so a session re-attached
  elsewhere keeps its headspace: `Layout{nav_back, nav_forward}` of
  `Loc{name, pos}` (`Pos::Keep|Chars|Line|LineCol`), moved by
  `LayoutOp::Visit{from, to}` and `NavPop{back, at}`. One proposal
  makes a jump, `Goto{loc}`: the leader records the origin (its
  `seltext` and dot), lands (select, reveal, warp) when the window is
  open, else leaves the place for whoever opens files (the app asks the
  server and lands when the window arrives; a headless daemon opens it
  itself). The lsp tool lands through `Goto`; plumbs to `file:line` and
  `apex B` are `Goto`s; ⌘P picks are too. `Back` and `Fwd` are verbs the
  lsp tool offers everywhere and answers with `Nav{back}`, which pops;
  ⌘[ and ⌘] are a UI hack that executes those words in the window under
  the pointer, on purpose: the keys are not the functionality.
- *As built:* ⌘P (File ▸ Go to…) is Zed's file finder in acme's colours
  (`client/finder.rs`): every open window of the session (files,
  directories, terminals, win, +Errors, marked ● or ▶) and the files
  closed lately (○, "closed": the last fifty per session, each once,
  kept in `recent-files` on this machine, noticed as windows go), ranked
  as Zed ranks: nothing typed, the open windows alone, in layout order
  (the closed files are there to be found by name, not scrolled
  through); a query, a fuzzy score in the spirit of Zed's
  matcher (file name best, then after `/` and at word starts, runs of
  consecutive matches, a penalty for the wrong case and for long paths),
  open before closed at equal scores. Enter or a click shows the window
  or opens the file, and warps the pointer to it as acme's warps do.
- *As built, tabs:* the title bar carries a tab per connected session
  — this window's, and the parked ones — in the order first shown
  (`Pool::order`), the current one selected (it toggles the picker);
  another tab switches to that session, its × lets a parked one go,
  the current tab's × lets this session go and shows the one parked
  last, and `+` opens the picker for a session not here yet. The
  selected tab is styled as a browser's: the colour of the row below
  it, rounded at the top, its bottom corners draping out into the strip
  (a square of its colour with the strip's colour rounded away), and
  no line under the strip, so it flows into the window.
  ⌘1…⌘9 go
  to the Nth tab. The tab order is kept in `open-sessions` beside the
  other state files, and at launch the tabs of last time are attached
  again in the background and parked, by identity: one that is gone
  (an ended session, a new daemon) is forgotten rather than made anew. Sessions shown
  by other windows are theirs, not tabs here.
- *As built, the picker:* while it (or the finder, ⌘P) is up it has
  the keyboard. Its field is a one-line editor (`field.rs`) with a
  cursor and a selection and the keys a Mac field answers: arrows with
  shift, option and command, delete back and forward, ^A ^E ^B ^F ^D
  ^H ^U ^W ^K, and the Edit menu's ⌘A ⌘X ⌘C ⌘V ⌘Z on it; the window
  keys do nothing below it. As hosts answer, the cursor keeps the row
  it is on wherever that row moves to (`Selector::keeping`); untouched,
  it lands on this window's session once that is listed. ⌘⇧K switches
  to the session parked most recently, and ⌘, opens (or makes) the
  host's `~/.apex/profile`. ⌃Tab, held, is the session switcher
  (`switcher.rs`), an application switcher's: a list of the connected
  sessions, the ones the title bar has tabs for, the label first and
  the host after it dimmed (none for a local session), most recently
  shown first (this one, then the parked ones by when they were
  parked), the mark one step on per ⌃Tab (⌃⇧Tab back) and the
  session under it switched to when control is let go; ⎋ leaves
  things as they are. (⌘Tab is the system's; no app can take it.)
  ⌘K remembers hosts, not sessions
(`known-hosts` beside the other state files, seeded from the recent
sessions; `local` always first). It is a section per host, the host's
name with its provider in parentheses dimmed and a × to forget it,
holding the host's sessions and "+ new session", which asks for a
name and creates it there. A section opens with the sessions the host
had when last asked (`known-sessions` beside the other state files,
plus what the recent sessions say: they are mostly the same), and each
host is asked again in the background (the section says "asking…"
meanwhile, then shows the answer, or keeps what it had and says why
the host could not be reached), so a host that is down holds nothing
up; an answer for an earlier opening is dropped. "+ new host…" opens a form:
the providers as pills (`ssh` and every `apex-remote-NAME` on the
PATH; ←→ or a letter picks one) and the host; enter remembers it and
asks for its sessions; a host without apex yet gets ours first, as
attaching would, and is asked again. "+ new session" is offered on
every host, reachable or not (attaching tries again and says what is
wrong). The search narrows sessions and hosts, and a URL typed in
still creates directly.
- *As built, parked sessions:* switching a window to another session
(⌘K) or closing it (⌘⇧W, the red button, `Exit`) does not detach: the
session is parked, still attached and still leading, in an app-wide
pool (`client/pool.rs`, a gpui global), where a task tends every
parked link: entries applied, tools' proposals answered, tags
refreshed, gotos opened, the rules' asks answered as far as an unseen
session can (`open` yes; the rest declined). A window asked for a
parked session, by ⌘K or as a new window, takes it back at once with
its state as it was left (web views are made again). A link's wake goes
through a target that moves between the window and the pool. Eight stay
parked, the least recently parked let go beyond that; a parked link
that ends is dropped; quitting closes them all, and nothing is parked
across a launch (a remembered window attaches afresh). Switching to a
session elsewhere attaches in the background like a launch does.
- *As built:* Enter repeats the leading whitespace of the line it ends,
up to dot, in every body: acme's `-a`, always on. The title bar shows,
left of the connection mark, the heartbeat's round trip and the log's
(an entry flushed to its `Ack`), as `1ms/2ms`.
- *As built:* full screen (⌘⌃F, View ▸ Enter Full Screen) hides the
  title bar, acme's area is the whole screen, and a window's full-screen
  state is remembered with its frame. B3 is acme's look and shift-B3
  looks backwards (the mariusae/plan9port acme's `look3(..., reverse)`:
  `search` with reverse, the last occurrence ending before dot,
  wrapping), always: no rule changes what B3 does with an identifier.
  The lsp's `Def` is ⌘-B3 (`Plumb{verb: Def}` at the pointer, walked as
  a verb: the lsp's rule takes it in a source window, elsewhere "no rule
  takes it here") and `Back` is ⇧⌘-B3, exactly what ⌘[ issues. On a
  laptop, where ⌘-click is B3, ⌃⌘-click is ⌘-B3 and ⇧⌃⌘-click is ⇧⌘-B3.
  The picker's recent sessions carry a × that forgets them.
- *As built:* a window is **live** when a process is behind it: a
  terminal whose program runs, or a text window a tool keeps so
  (`WindowOp::Live{by}`, the win tool's attachment; the state ends with
  the program, or with that attachment, so a crashed tool leaves nothing
  stuck). A third state beside clean and dirty: the handle is a raspberry
  (`0xB24073`, dark magenta with a quarter of yellow: unlike the dirty
  blue, the fenced red, the unsynced green and the scrollbar's dark
  yellow beside it), `Del` does not ask (the text is a transcript, not a file), and
  `apex win list` marks it `>`.
  The app offers `Snarfout` in terminals and win windows: the last
  command as `$ cmd` (the prompt normalised) and its output, the lines
  between the last two prompts
  (`transcript::last_output`: the last line is the prompt, earlier
  prompts begin like it and carry its marker), read from the host for a
  terminal (`TermRead`), into the snarf buffer and clipboard.
- *As built:* the app remembers every window with its session URL and
  its frame (`last-sessions`, one line each), saved on every move or
  resize and on Quit, and opens them all again at launch, each on its
  session and where it was. The name in a tag is editable, as acme's is:
  `winsettag` leaves a typed first word alone, a click in the tag or a
  command from the window commits it (`wincommit`: the buffer takes the
  name), and `Put` resolves a relative name where the window is and makes
  it absolute. `New` in a tag is an empty window; `apex new [LABEL]` is
  the same with stdin in it (acme's `cmd | New`); `apex open` opens files.
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
erase keys; over web and preview bodies the page's own pointer: WebKit
never sets a cursor inside this window (hover reaches the page, its
cursor request does not reach the screen), so a script in every page
reports the CSS cursor under the pointer over wry's IPC (`auto` read
as WebKit reads it: a hand within a link, a beam in a field), the app
maps it to a system cursor and sets it itself, and while the pointer
is over a page gpui's cursor rect asks for a style behind which
`cursor.rs` puts a cursor whose `set` does nothing, so gpui cannot
undo it (`APEX_CURSOR_DEBUG=1` logs every cursor set and by whom);
double-click expansion with acme's
`isalnum` (`_` and any
rune above Latin-1 are word characters, ASCII punctuation is not); the
wheel's fractions of a line carried over so a trackpad's small deltas
add up; and the pointers: plan9port's big arrow always, the box while
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

*As built:* the daemon compacts every shard's log after each round of
shipping, below what every connection has been sent and both its
replicas have applied, and a mirror compacts each shard as it applies
entries (a led shard once the server has acked them too), so a log
holds what is outstanding, not the session's history. A terminal
publishes only what changed since it last did (the rows that differ,
the top, the links, the cursor), nothing on a wakeup that changed
nothing, and the file watcher forwards writes, creations, removals and
renames, never opens or reads (on Linux every open of a watched file is
an event, the server's own reads included, which fed a loop). A daemon
and a win tool that had grown to gigabytes on a spewing terminal and a
watched file were what showed the need.

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
