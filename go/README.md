# apex for Go

A Go package for writing apex tools: programs that take part in a
session, make and write windows, offer verbs in their tools menus and
answer them, and follow what others type.

```go
import "github.com/mariusae/apex/go/apex"

t, err := apex.Attach("shout", nil)
if err != nil { log.Fatal(err) }
defer t.Close()

w, _ := t.New("/tmp/shout")
t.Offer(apex.Rule{Verb: "Shout", Window: w}, func(p apex.Plumb) bool {
    w.Append(strings.ToUpper(p.Text) + "\n")
    return true
})
t.Serve(context.Background())
```

The package needs no knowledge of apex's wire protocol: it starts
`apex tool bridge NAME`, which attaches as the tool and speaks JSON on
its standard input and output, and talks to that. `apex` must be on the
path (or named in `Options`), and the tool finds its session the way
any command does, through `APEX_SOCKET` and `apexsession`, which apex
sets for everything it runs. So a tool started from a terminal in a
session, or from the profile with `&`, is in that session.

What a tool can do:

- `t.Windows()`, `t.New(name)`, `t.Open(name, line)`: the session's
  windows, a new one, a file shown.
- `w.Read()`, `w.Replace(q0, q1, text)`, `w.Append(text)`,
  `w.Select(q0, q1)`, `w.Selection()`, `w.Rename(name)`,
  `w.SetLive(on)`, `w.Delete()`, `w.Exec(text)`: a window's text and
  state. Offsets count characters, not bytes.
- `t.Offer(rule, handler)`: a verb in the tools menu of the windows the
  rule matches (or B3 on text it matches, with `Verb` empty), answered
  by the handler, which returns whether it took it. A verb acts on the
  window's dot, `p.At`; B3 also brings what it took, `p.Sel`;
  `p.Range()` picks the one to act on. `t.Withdraw(id)`.
- `w.Watch(fn)`: edits by others to the window's body, as win follows
  typing; `t.OnRename`, `t.OnDelete`: the windows the tool made or
  watches.
- `t.Errors(dir, text)`: a note in +Errors. `t.Exec(text)`: a command
  as B2 would run it. `t.Set`, `t.Setting`: settings of the tool's own.
- `t.Serve(ctx)`: runs the handlers until the session ends.

`examples/upper` is a whole tool: `Upper` in every file window,
upper-casing the selection. Run it from a terminal in a session.

A tool in Rust uses the `apex-tool` crate in-process instead; this
package and the bridge are built on the same surface.

The integration test runs against a session: set `APEX_SOCKET` (and
`APEX_BIN` if apex is not on the path) and `go test ./...`.
