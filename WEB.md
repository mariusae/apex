# apex — web windows, the I/O plane, and Preview

A design for three things that belong together: an I/O plane multiplexed
over the session socket, web windows rendered on the client, and `Preview`
as a live pipe through a converter. It resolves the "decide later" in
DESIGN.md §10. Nothing here is built yet; where this document and
DESIGN.md disagree, this one is the intent for web windows and file I/O,
and DESIGN.md's *as built* notes describe what runs today.

Decisions taken in discussion, restated so they are not relitigated:

- A web window always renders on the client. Only the URL (and a title)
  is session state. Cookies, history, zoom and scroll are the client's.
  Reattaching reloads the page; that is accepted.
- Network and file I/O for clients go through the server, on a plane of
  their own, separate from the log. Streams belong to the connection
  that opened them and end with it. The plane is HTTP-shaped and nothing
  else: file reading and watching are HTTP requests too, a watch being a
  response that streams `(version, path, contents)` frames.
- The host-file scheme a web view uses is `apexfile://`.
- `Preview` is a pipe through a converter, live from the buffer.
  External applications (Marked) are an ordinary rule anyone can add.
- Preview from a terminal or the CLI on a file that is not open opens it
  first. The preview window's buffer is a normal buffer; debounce and
  diffing keep the log small, and we measure before inventing a
  buffer-without-history.
- The preview follows dot: scroll sync is in scope, as a second pass.

Order of work: the I/O plane (porting today's `Watch` onto it), then web
windows with `wry`, then `apex md` and `apex tool preview`.

---

## 1. The I/O plane

*As built (stage 1):* `Io{stream, frame}` both ways, `IoFrame::{Request,
Response, Body, End, Reset}`, `GET file://` (with `Watch`: `FileFrame`
bodies, version 1 first), `PUT file://`; `ReadFile`/`Watch`/`Unwatch`/
`File` and the per-connection watch sets are gone; `apex io` drives it
from a shell; `Remote::{io_open, io_send, io_end, io_response,
io_collect, io_next_file, read_file, watch, unwatch}` for programs.
*Stage 2:* `CONNECT host:port` (a thread per tunnel connects and reads;
the daemon writes; the client's `End` half-closes, the far end's close
ends the stream; a refused connection is a 502) and `http(s)://` (any
method, the body gathered until `End`, fetched with `ureq` on a thread,
status, headers and body streamed back; statuses pass through, a
failure before the head is a 502, after it a `Reset`). `apex io
CONNECT HOST:PORT` is nc over the plane.

### 1.1 Why a second plane

`ReadFile`, `Watch` and `Unwatch` are already connection-scoped messages
rather than log entries, but they live in the same enum as everything
else and the daemon keeps their bookkeeping by hand: a `watched` set per
connection, `drop_watches` on gone and on fencing, `unwatch_unused` to
reference-count the file watcher. Web views need more of the same kind
of thing (fetches, tunnels), driven by programs on the client that are
outside the shared state. So: one socket, two planes. The log plane is
what DESIGN.md §6 describes. The I/O plane is a set of numbered streams
multiplexed beside it.

### 1.2 Streams

Two messages, one per direction:

```
ClientMsg::Io { stream: u32, frame: IoFrame }
ServerMsg::Io { stream: u32, frame: IoFrame }

IoFrame
  Request  { method, url, headers }      opens the stream (client → server)
  Response { status, headers }           the answer's head (server → client)
  Body     (bytes)                       either direction, any number
  End                                    the sender is done
  Reset    { reason }                    abort, either direction
```

A client picks stream ids (odd, increasing; the server never opens a
stream). A stream is open from `Request` until both sides have sent
`End` or either has sent `Reset`. Postcard-encoded like every other
frame: HTTP is the shape, not the encoding. There is no HTTP/1.1 text on
the wire and no HTTP library on the log side of the daemon.

HTTP is the right shape because it already has the three things needed:
a request that opens, a body that streams, and `CONNECT` for a raw byte
tunnel. Nothing else is added; in particular there is no second "raw
stream" kind.

Every stream belongs to the connection that opened it and dies with it.
A fenced UI keeps its streams (they are read-only and it is still
attached); they end when its connection does. That removes the
per-connection sets and the special handling on fencing.

### 1.3 Schemes the server answers

`file://` — files on the host, resolved as the server resolves names.

- `GET file:///path` reads the file: `Response{200}`, `Body` frames, `End`.
  Errors are `Response{404|403|500}` with the message as body.
- `GET file:///path` with header `Watch: 1` never ends on its own. The
  first body is the file now; then, on every change the host's watcher
  sees, another `(version, path, contents)` frame (a postcard struct per
  `Body`; version is a counter per stream). The client ends the watch by
  sending `End` or `Reset`. The daemon keeps one filesystem watch per
  path, reference-counted by open streams, sharing the watcher the
  server already runs for open files (DESIGN.md §9).
- `PUT file:///path` writes the body to the file (used later by tools on
  the client side; Put from the UI stays a proposal on the log plane).

`http://`, `https://` — fetched by the host with a streaming client
(`ureq` or `reqwest`), request body forwarded, response streamed back.
For a program on the client that wants a resource as the host sees it.

`CONNECT host:port` — a byte tunnel. The server opens the TCP connection;
from then on `Body` frames flow both ways until `End`. This is how a web
view's own traffic reaches the host's network (§2.3).

The daemon does not do TLS: for `https://` the host's HTTP client does;
for `CONNECT` the web view does, end to end through the tunnel.

### 1.4 What moves onto the plane

- `ReadFile` → `GET file://`.
- `Watch`/`Unwatch`/`File` → `GET file://` with `Watch: 1`, ended by the
  client. The remote Preview of today (a subscribed local copy under a
  mirrored path, for an external application) is one such stream.
- `Conn.watched`, `drop_watches`, `unwatch_unused` go away; a stream
  table per connection replaces them.

Tools keep the log plane for everything they do today. The lsp tool
reads buffers from entries, not files; win never touches files.

`apex events` and the CLI are unaffected. A CLI verb `apex io GET URL`
(streaming the body to stdout) is cheap and useful for testing the
plane headlessly; it is the first thing to build after the frames.

---

## 2. Web windows

### 2.1 State

*As built (stage 4):* no web shard. `Body::Web` carries nothing; the
URL is the window's name, the first word of its tag, exactly as a
terminal's directory is (the buffer-less naming already existed), so
`WebNavigate{window, url}` is a tag edit plus a `Visit` on the
navigation stack, and `OpenWeb{col, url}` makes the window. Titles are
not kept yet. `WinKind::Web` for rules (`-kind=web`). A `Goto` to a URL
no window shows opens a web window on it (the daemon and the client
both), which is also what Back does after a page moved on: a new window
on the old page rather than the same window going back.

A third body kind beside text and terminal:

```
Body::Web(WebId)

web shard (per window, pinned to the server like term)
  WebOp::Navigate { url, title }      the client hosting the view reports where it is
  WebOp::Title    { title }
```

Session state is the URL and the title; nothing else. The window's name
is the title (or the URL until there is one), so the tag reads as acme
would name it. The client that hosts the view proposes `Navigate` when
the page navigates (links, redirects, form posts), so a reattach
elsewhere lands on the current page. Navigations also push onto the
session's navigation stack (`LayoutOp::Visit`, DESIGN.md §6.1), so
`Back`, `Fwd`, ⌘[ and ⌘] work across web and text windows alike.

Cookies, local storage, history, zoom and scroll live in the client's
own data store, one per session URL, never in the log. Attaching reloads
the page.

Two ways to open one:

- `Newweb URL` in a tag (acme's word, a stub today) or `apex web open
  URL` from a shell: a window on a URL.
- `cmd | apex web [-name NAME]`: a window whose HTML comes from a
  buffer (§2.4).

`Del` closes it as any window. Zerox of a web window is a second view
on the same URL, not a shared page.

### 2.2 Rendering

*As built (stage 4):* `client/web.rs` keeps one `wry` view per web
window, placed by a `canvas` element at the body's rectangle every
frame, hidden while the tools menu, the finder or the picker is up and
when the layout does not draw the window, dropped with the window; a
name that changed under it (a Goto, another client) is loaded. The
view's traffic is its own until stage 5.

*Spike (stage 3), done:* a `wry` 0.56 child view built with
`build_as_child` on gpui's `Window` (which implements
`HasWindowHandle`) sits exactly at the rectangle gpui gives, in logical
coordinates with a top-left origin, moves with `set_bounds`, and hides
and shows with `set_visible`; the `mac-proxy` feature's
`with_proxy_config(ProxyConfig::Http)` sent the page's traffic to a
localhost `CONNECT` proxy on macOS 26; and a gpui element painted over
the same area is indeed hidden behind the native view. `wry` is the
dependency.

gpui has no web view; Zed's `gpui_web` is gpui compiled to WebAssembly,
not a browser. The view is a native `WKWebView` (WebKitGTK on Linux)
placed as a child view of the gpui window at the rectangle the layout
gives the window body. The `wry` crate does this on both platforms and
is taken as the dependency rather than bridging Objective-C ourselves.

A native child view draws above everything gpui paints. The tools menu,
the finder (⌘P) and the session picker are gpui overlays and would be
hidden behind a web window. To start, web views are hidden while any
gpui overlay is up and shown again when it closes; if that proves
annoying the overlays move into small windows of their own. Column and
window resizing move and clip the native view with the layout.

Keys: a web body takes the keyboard when the pointer is over it, as
text and terminals do. Mouse: B1 goes to the page; B2 and B3 on the
tag work as in any window; B2/B3 inside the page are the page's
(B3 = context menu is suppressed, since acme's B3 means plumb, and a
page's own selection can be plumbed with the B3 on the selected text
through the tag's `Look` as a later refinement).

### 2.3 Network through the server

*As built (stage 5):* `plane::IoPlane` is the link's I/O plane for
threads (streams they open get their frames from the link's reader
directly, through a sink registry); `plane::start_connect_proxy` is
the localhost `CONNECT` proxy, one per web-hosting client link, each
accepted connection a `CONNECT` stream on the plane; every view's data
store is pointed at it (`mac-proxy`), so its traffic leaves from the
host. With no link (an in-process server) the view is on its own.

`WKWebView` cannot intercept `http` or `https` with a scheme handler;
only custom schemes. It can, on macOS 14 and later, take a per-data-store
proxy configuration. So:

- The client runs a small HTTP `CONNECT` proxy on localhost, one per
  session connection, and points the view's data store at it.
- Each proxied connection is a `CONNECT host:port` stream on the I/O
  plane; the host opens the TCP connection. TLS is between the page and
  the origin, through the tunnel.

The page sees the host's network: an internal dashboard on the remote
box just works, which is the point of a remote session. Locally the
tunnel is a loop through the daemon and costs little.

On macOS before 14, and if WebKitGTK's proxy setting misbehaves, the
fallback is the view's own network, documented as such. It is not the
design.

### 2.4 `apexfile://`

*As built (stage 5):* an asynchronous custom protocol per view: the
path is fetched with `GET file://` on the plane (the disk, with no
link), answered with a content type from the extension, and watched
from then on (one watch stream per path per page, capped at 200,
ended with the page); a change after the first frame reloads the page.
WebKit dispatches a custom scheme only when the URL has a host, so the
client loads `apexfile:///path` as `apexfile://localhost/path` and maps
navigations back; the session never sees the `localhost` form.

A custom scheme handled by the client: `apexfile:///path` is
`GET file:///path` on the plane. With it:

- `Newweb apexfile:///home/me/doc.html` renders a host file; the handler
  adds `Watch: 1` and reloads on change.
- Relative resources of a page built from a buffer (§2.5) resolve
  through a `<base href="apexfile:///dir/">`, so images and stylesheets
  beside the source load from the host.

MIME types come from the extension; the response headers carry them.

### 2.5 HTML from a buffer

*As built (stage 6):* `Body::Html(buffer)`, a text buffer the client
shows as a page: `cmd | apex web [-name NAME]` (`Proposal::OpenHtml`,
the name `DIR/+web` unless given), edited, put and got as text. The
view is built with the HTML and, on every later version, patched in
place by an injected script (a small morphdom: nodes matched by
position and name, attributes and text updated), so scroll and state
survive; a `<base>` on the window's directory (`apexfile://`) is
added unless the HTML brings one. A link followed in such a page does
not navigate it: it opens a web window on the link (a `Goto`).
`WinKind::Web` too.

A web body may render a text buffer instead of a URL: `cmd | apex web`
makes a window whose body buffer holds the HTML and whose view renders
it. The HTML lives in the log, which is fine at the sizes commands and
converters produce; editing the buffer re-renders.

The client keeps the page alive across re-renders: rather than reloading,
it patches the document in place through injected script (a morphdom-
style diff of the new HTML against the DOM), so scroll position and any
state in the page survive. The base URL is `apexfile://` at the source's
directory when there is one (the preview tool sets it; §3.2).

---

## 3. Preview

*As built (stage 7):* `apex tool preview FILE` (crate
`apex-tool-preview`, a command named `preview`) opens the file if need
be (a `Goto`), finds or makes `FILE+Preview` as an HTML window
(`OpenHtml`) in the column beside the source, marks it live, and
renders the source buffer through the converter on every change once
it has settled for 250 ms, writing the page as a minimal diff
(`ReplaceRange` over the changed span); it ends with either window and
a second `Preview` warps to a live one. Converters are
`Preview.EXT` settings, `apex_core::preview` resolving the session's,
then any attachment's, then the defaults (`md`/`markdown`: `apex md`,
`html`/`htm`/`svg`: `cat`; an empty setting turns one off). The server
derives the rules for every setting it sees (session and attachments
alike), so the client derives none; `apex preview FILE` runs the tool
as a command of the session; `apex md` is pulldown-cmark with tables,
footnotes, strikethrough, task lists and a small stylesheet. External
applications are a rule (`-run='open -a Marked $file' -priority=10`);
the client's `-client=preview` action (a local app or Quick Look on a
watched copy) stays for rules that ask for it. Not yet: line markers
and following dot (§3.3).

### 3.1 A pipe, live from the buffer

`Preview` on a file window runs `apex tool preview` on the host, a
command named `preview` in the top row, `apex ps` and `Kill`. Live means
from the buffer, not the file: the log carries the source, so the tool
subscribes to its entries as the lsp tool does, waits for a few hundred
milliseconds of idle, pipes the current text through the converter and
writes the HTML into the preview window. Unsaved edits show; `Put`
changes nothing. Watching the file on disk would be the wrong signal.

The tool:

- opens, or reuses, a web-bodied window named after the source with a
  `+Preview` suffix (the `+Errors` convention), in the column beside it,
  rendering from a buffer (§2.5);
- injects `<base href="apexfile:///dir/of/the/source/">`, so relative
  images and stylesheets resolve through the plane;
- replaces the window's buffer with the new HTML as a minimal diff
  (`ReplaceRange` over the changed span), so the log grows with the edit
  rather than with the document;
- exits when either window closes; `Del` of the preview ends it.
  A second `Preview` on the same source warps to the existing preview
  rather than starting another.

`apex preview FILE` from a terminal, and `Preview` from the CLI, open the
file first if it is not open, then do the same.

### 3.2 Converters are settings; one ships with apex

A converter is a command reading the source on stdin and writing HTML on
stdout, named by extension in settings: `Preview.md`, `Preview.rst`,
`Preview.dot`. The default profile carries:

```
apex set Preview.md 'apex md'
apex set Preview.markdown 'apex md'
apex set Preview.html cat
apex set Preview.svg cat
```

`apex md` is a Markdown-to-HTML converter built into the CLI on the
pulldown-cmark crate: GitHub-flavoured tables, task lists, footnotes,
strikethrough, a small default stylesheet, and line markers (§3.3).
Requiring pandoc on every host for the common case would be a poor
default. Someone who wants more adds a setting:

```
apex set Preview.rst 'pandoc -f rst -t html5'
apex set Preview.dot 'dot -Tsvg'
```

Rules are derived from these settings, so the verb is offered only in
windows whose extension has a converter. Today the client derives them
from its own per-attachment settings (DESIGN.md §7, *as built*); the
server takes over deriving rules for session settings (from the
profile), the client keeps deriving them for its attachment's.

External applications are an ordinary rule at a higher priority, and
nothing else is special about them:

```
apex plumb rule add -verb=Preview -file='\.md$' -run 'open -a Marked $file'
```

`Preview.EXT` then means one thing, a converter, and the remote case
(an application on the client showing a host file) is the same rule
with the client-side action and a watch stream behind it, as today.

### 3.3 Following dot

Because the tool sees the source window's selection in the log, the
preview can follow dot. `apex md` emits `data-line="N"` on block
elements (pulldown-cmark gives byte offsets; the converter maps them to
lines). On a selection change in the source, the tool proposes nothing
on the log; the client that renders the preview watches the source
window's dot in its own mirror and scrolls the page to the marker
nearest the current line, through injected script. Converters that do
not emit markers get no sync and nothing breaks. This is a second pass,
after the rest works.

### 3.4 Log size

Every re-render is entries in the preview buffer. Debounce bounds the
rate; the diff bounds the size to what changed in the output. A normal
buffer, with undo, to start. If measurement shows a long editing session
on a large document bloating the log, the options are a shorter undo
bound for preview buffers or a buffer kind not kept in history; neither
is designed until it is needed.

---

## 4. Wire summary

Added to DESIGN.md §6.1 when built:

```
client → server
  Io{stream, frame}                       the I/O plane: Request opens; Body, End, Reset
  (ReadFile, Watch, Unwatch removed)

server → client
  Io{stream, frame}                       Response, Body, End, Reset
  (File removed)

proposals
  OpenWeb{col, url}                       a web window on a URL
  WebNavigate{window, url, title}         the hosting client reports the page

shards
  web (pinned to the server)              WebOp::Navigate{url, title} · Title{title}
```

## 5. Open questions

- Linux: WebKitGTK's proxy configuration and child-view placement under
  Wayland need trying before the client claims it.
- Whether a web window's page selection should feed the snarf buffer
  (⌘C in the page copies as a browser would; making it acme's snarf is
  a small bridge, probably wanted).
- Printing and downloads inside a page: out of scope; a download link
  is plumbed to the host instead, later.
