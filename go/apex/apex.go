// Package apex lets a program take part in an apex session as a tool:
// make and write windows, offer verbs in their tools menus, answer
// those verbs when they are used, and follow what others type.
//
// A tool attaches by name and works through the apex command, which
// must be on the path (or named in Options): the package starts
// `apex tool bridge NAME` and talks JSON to it, so nothing here depends
// on the wire protocol or the state model behind it. Text offsets count
// characters, not bytes, as apex does throughout.
//
// The shape of a tool:
//
//	t, err := apex.Attach("shout", nil)
//	...
//	defer t.Close()
//	w, err := t.New("/tmp/shout")
//	t.Offer(apex.Rule{Verb: "Shout", Window: w}, func(p apex.Plumb) bool {
//		w.Append(strings.ToUpper(p.Text) + "\n")
//		return true
//	})
//	t.Serve(context.Background())
//
// Serve runs the handlers until the session ends or the context is
// done. A handler answers within a second, or the verb is taken to be
// refused and the next rule is tried.
package apex

import (
	"bufio"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"os/exec"
	"sync"
)

// Options for Attach; the zero value takes everything from the
// environment apex sets for commands it runs (APEX_SOCKET,
// apexsession) and finds `apex` on the path.
type Options struct {
	// The daemon's socket; empty means apex's default.
	Socket string
	// The session; empty means the one this program was started in,
	// else apex's default.
	Session string
	// The apex binary; empty means "apex" on the path.
	Binary string
}

// A Tool is an attachment to a session: the program's presence there.
type Tool struct {
	name       string
	cmd        *exec.Cmd
	in         io.WriteCloser
	writeMu    sync.Mutex
	nextID     int64
	pendingMu  sync.Mutex
	pending    map[int64]chan map[string]json.RawMessage
	events     *queue
	handlersMu sync.Mutex
	handlers   map[RuleID]func(Plumb) bool
	watchers   map[int]func(Edit)
	onRename   func(*Window, string)
	onDelete   func(*Window)
	session    string
	attachment int
	done       chan struct{}
	err        error
}

// Attach joins the session as the tool called name. The name is what
// rules and the session's process list know the tool by.
func Attach(name string, opts *Options) (*Tool, error) {
	if opts == nil {
		opts = &Options{}
	}
	bin := opts.Binary
	if bin == "" {
		bin = "apex"
	}
	args := []string{}
	if opts.Socket != "" {
		args = append(args, "-socket="+opts.Socket)
	}
	if opts.Session != "" {
		args = append(args, "-session="+opts.Session)
	}
	args = append(args, "tool", "bridge", name)
	cmd := exec.Command(bin, args...)
	cmd.Stderr = os.Stderr
	in, err := cmd.StdinPipe()
	if err != nil {
		return nil, err
	}
	out, err := cmd.StdoutPipe()
	if err != nil {
		return nil, err
	}
	if err := cmd.Start(); err != nil {
		return nil, fmt.Errorf("apex: %w", err)
	}
	t := &Tool{
		name:     name,
		cmd:      cmd,
		in:       in,
		pending:  map[int64]chan map[string]json.RawMessage{},
		events:   newQueue(),
		handlers: map[RuleID]func(Plumb) bool{},
		watchers: map[int]func(Edit){},
		done:     make(chan struct{}),
	}
	go t.read(out)
	// the bridge says hello first, or not at all
	ev, ok := t.events.pop(t.done)
	if !ok || ev["event"] == nil {
		_ = cmd.Wait()
		return nil, errors.New("apex: the bridge did not attach (is the daemon running, and apex on the path?)")
	}
	var hello struct {
		Session    string `json:"session"`
		Attachment int    `json:"attachment"`
	}
	_ = unmarshalAll(ev, &hello)
	t.session, t.attachment = hello.Session, hello.Attachment
	return t, nil
}

// Session is the name of the session the tool is attached to.
func (t *Tool) Session() string { return t.session }

// Close detaches: the bridge exits, and with it every rule the tool
// installed and its live marks on windows.
func (t *Tool) Close() error {
	t.writeMu.Lock()
	err := t.in.Close()
	t.writeMu.Unlock()
	_ = t.cmd.Wait()
	return err
}

// read is the goroutine that takes the bridge's lines apart: answers to
// whoever waits for them, events onto the queue for Serve.
func (t *Tool) read(out io.Reader) {
	sc := bufio.NewScanner(out)
	sc.Buffer(make([]byte, 1<<20), 64<<20)
	for sc.Scan() {
		var m map[string]json.RawMessage
		if err := json.Unmarshal(sc.Bytes(), &m); err != nil {
			continue
		}
		if raw, ok := m["id"]; ok {
			var id int64
			if json.Unmarshal(raw, &id) == nil {
				t.pendingMu.Lock()
				ch := t.pending[id]
				delete(t.pending, id)
				t.pendingMu.Unlock()
				if ch != nil {
					ch <- m
				}
			}
			continue
		}
		t.events.push(m)
	}
	t.err = sc.Err()
	close(t.done)
	// nobody waits on a closed bridge
	t.pendingMu.Lock()
	for id, ch := range t.pending {
		close(ch)
		delete(t.pending, id)
	}
	t.pendingMu.Unlock()
}

// call sends one command and waits for its answer.
func (t *Tool) call(cmd string, args map[string]any, result any) error {
	t.pendingMu.Lock()
	t.nextID++
	id := t.nextID
	ch := make(chan map[string]json.RawMessage, 1)
	t.pending[id] = ch
	t.pendingMu.Unlock()
	msg := map[string]any{"id": id, "cmd": cmd}
	for k, v := range args {
		msg[k] = v
	}
	line, err := json.Marshal(msg)
	if err != nil {
		return err
	}
	t.writeMu.Lock()
	_, err = t.in.Write(append(line, '\n'))
	t.writeMu.Unlock()
	if err != nil {
		return fmt.Errorf("apex: %w", err)
	}
	reply, ok := <-ch
	if !ok {
		return errors.New("apex: the session is gone")
	}
	var okv bool
	_ = json.Unmarshal(reply["ok"], &okv)
	if !okv {
		var e string
		_ = json.Unmarshal(reply["error"], &e)
		return fmt.Errorf("apex: %s: %s", cmd, e)
	}
	if result != nil {
		return unmarshalAll(reply, result)
	}
	return nil
}

func unmarshalAll(m map[string]json.RawMessage, into any) error {
	b, err := json.Marshal(m)
	if err != nil {
		return err
	}
	return json.Unmarshal(b, into)
}

// ---- windows -------------------------------------------------------------

// A Window is a text window in the session, known by its id (ids are
// never reused).
type Window struct {
	ID int
	t  *Tool
}

// WindowInfo describes a window as Windows lists them.
type WindowInfo struct {
	ID   int    `json:"id"`
	Name string `json:"name"`
	// file, dir, term, errors or web.
	Kind string `json:"kind"`
	// A process is behind it (a shell's, a tool's).
	Live bool `json:"live"`
}

// Windows lists the session's windows.
func (t *Tool) Windows() ([]WindowInfo, error) {
	var r struct {
		Windows []WindowInfo `json:"windows"`
	}
	if err := t.call("windows", nil, &r); err != nil {
		return nil, err
	}
	return r.Windows, nil
}

// Window is the window with this id, for a tool that learned the id
// elsewhere (a plumb, Windows).
func (t *Tool) Window(id int) *Window { return &Window{ID: id, t: t} }

// New makes a new, empty window with this name in the last column.
func (t *Tool) New(name string) (*Window, error) {
	var r struct {
		Window int `json:"window"`
	}
	if err := t.call("new", map[string]any{"name": name}, &r); err != nil {
		return nil, err
	}
	return &Window{ID: r.Window, t: t}, nil
}

// Open shows the file (or directory) of this name, opening it if it is
// not open, at line (1-based) when line is not 0.
func (t *Tool) Open(name string, line int) (*Window, error) {
	args := map[string]any{"name": name}
	if line > 0 {
		args["line"] = line
	}
	var r struct {
		Window int `json:"window"`
	}
	if err := t.call("open", args, &r); err != nil {
		return nil, err
	}
	return &Window{ID: r.Window, t: t}, nil
}

// Exec runs text as a command in the session, as B2 on it in the top
// row would: a builtin (Newcol, Exit), or a shell command on the host.
func (t *Tool) Exec(text string) error {
	return t.call("exec", map[string]any{"text": text}, nil)
}

// Errors appends text to the +Errors window of dir (the session's when
// dir is empty), where tools say things.
func (t *Tool) Errors(dir, text string) error {
	args := map[string]any{"text": text}
	if dir != "" {
		args["dir"] = dir
	}
	return t.call("errors", args, nil)
}

// Switch shows another session, at the window with this id there when
// window is not 0. The session is named by its id, a unique prefix of
// it, or its label. The UI showing this session switches to it.
func (t *Tool) Switch(session string, window int) error {
	args := map[string]any{"session": session}
	if window != 0 {
		args["window"] = window
	}
	return t.call("switch", args, nil)
}

// Set records a setting of the tool's own (gone when it detaches).
func (t *Tool) Set(key, value string) error {
	return t.call("set", map[string]any{"key": key, "value": value}, nil)
}

// Setting reads a setting: the tool's own, else the session's; the
// bool says whether one is set.
func (t *Tool) Setting(key string) (string, bool, error) {
	var r struct {
		Value *string `json:"value"`
	}
	if err := t.call("setting", map[string]any{"key": key}, &r); err != nil {
		return "", false, err
	}
	if r.Value == nil {
		return "", false, nil
	}
	return *r.Value, true, nil
}

// End, as an offset to Replace, means the end of the text.
const End = -1

// Read returns the window's whole text.
func (w *Window) Read() (string, error) {
	var r struct {
		Text string `json:"text"`
	}
	if err := w.t.call("read", map[string]any{"window": w.ID}, &r); err != nil {
		return "", err
	}
	return r.Text, nil
}

// Selection returns the window's selection (dot) as character offsets.
func (w *Window) Selection() (q0, q1 int, err error) {
	var r struct {
		Q0 int `json:"q0"`
		Q1 int `json:"q1"`
	}
	if err := w.t.call("selection", map[string]any{"window": w.ID}, &r); err != nil {
		return 0, 0, err
	}
	return r.Q0, r.Q1, nil
}

// Replace replaces the characters [q0, q1) with text; End for either
// means the end of the text, so Replace(End, End, s) appends.
func (w *Window) Replace(q0, q1 int, text string) error {
	return w.t.call("write", map[string]any{"window": w.ID, "q0": q0, "q1": q1, "text": text}, nil)
}

// Append adds text at the end.
func (w *Window) Append(text string) error { return w.Replace(End, End, text) }

// Select sets the window's selection.
func (w *Window) Select(q0, q1 int) error {
	return w.t.call("select", map[string]any{"window": w.ID, "q0": q0, "q1": q1}, nil)
}

// Rename gives the window (its buffer) a new name.
func (w *Window) Rename(name string) error {
	return w.t.call("rename", map[string]any{"window": w.ID, "name": name}, nil)
}

// SetLive marks the window as having this tool behind it: its handle
// shows so, and Del does not ask about unsaved text. The mark goes when
// the tool detaches.
func (w *Window) SetLive(on bool) error {
	return w.t.call("live", map[string]any{"window": w.ID, "on": on}, nil)
}

// Delete closes the window (Del there).
func (w *Window) Delete() error {
	return w.t.call("delete", map[string]any{"window": w.ID}, nil)
}

// Exec runs text as a command in the window, as B2 on it there would.
func (w *Window) Exec(text string) error {
	return w.t.call("exec", map[string]any{"window": w.ID, "text": text}, nil)
}

// An Edit is a change someone else made to a watched window's body.
type Edit struct {
	Window *Window
	// Q0 is where it happened, Nd how many characters were deleted
	// there, Text what was inserted, in the text as the tool last saw it.
	Q0, Nd int
	Text   string
}

// Watch reports edits by others to the window's body, to fn from
// Serve, in order.
func (w *Window) Watch(fn func(Edit)) error {
	w.t.handlersMu.Lock()
	w.t.watchers[w.ID] = fn
	w.t.handlersMu.Unlock()
	return w.t.call("watch", map[string]any{"window": w.ID}, nil)
}

// Unwatch stops the reports.
func (w *Window) Unwatch() error {
	w.t.handlersMu.Lock()
	delete(w.t.watchers, w.ID)
	w.t.handlersMu.Unlock()
	return w.t.call("unwatch", map[string]any{"window": w.ID}, nil)
}

// OnRename is told, from Serve, when a window the tool made, opened or
// watches is renamed (a shell's cd, a Put under a new name).
func (t *Tool) OnRename(fn func(w *Window, name string)) { t.onRename = fn }

// OnDelete is told, from Serve, when such a window is deleted.
func (t *Tool) OnDelete(fn func(w *Window)) { t.onDelete = fn }

// ---- rules and verbs -------------------------------------------------------

// A Rule says where a verb of the tool's is offered. The zero value
// offers it everywhere; each field set narrows that.
type Rule struct {
	// The verb: a word offered in the tools menu of matching windows and
	// run by B2 there. Empty means plumb: B3 (Look) on text the rule
	// matches goes to the tool.
	Verb string
	// A regexp the plumbed text (or the verb's arguments) must match
	// whole; its groups arrive in Plumb.Groups.
	Text string
	// A regexp the window's name must match.
	File string
	// file, dir, term, errors or web.
	Kind string
	// This one window only, whatever its name.
	Window *Window
	// Higher goes first among rules that match; 0 is usual.
	Priority int
}

// A RuleID names an installed rule.
type RuleID int

// A Plumb is a use of one of the tool's rules: the verb run, or text
// plumbed, in a window.
type Plumb struct {
	Rule RuleID
	Verb string
	// The verb's arguments, or the text plumbed.
	Text string
	// The window's directory, where relative names resolve.
	Dir string
	// The window it happened in; nil from the top row.
	Window *Window
	// The Text regexp's groups, $0 first.
	Groups []string
	// At is where it happened in the window's body: for a verb, the
	// window's dot (the selection, or the insertion point); for B3, the
	// pointer. Sel is what B3 took, the text swept or expanded; nil for
	// a verb. Range picks the one to act on.
	At, Sel *Span
}

// Range is the text a handler should act on: what B3 took when there
// is such a thing, else the window's dot; ok is false when there is
// neither, or it is empty.
func (p Plumb) Range() (q0, q1 int, ok bool) {
	for _, s := range []*Span{p.Sel, p.At} {
		if s != nil && s.Q1 > s.Q0 {
			return s.Q0, s.Q1, true
		}
	}
	return 0, 0, false
}

// A Span is a range of characters in a window's body.
type Span struct {
	Q0 int `json:"q0"`
	Q1 int `json:"q1"`
}

// Offer installs a rule answered by handle, which runs from Serve and
// returns whether the tool took it (false lets the next rule try). It
// must return within a second.
func (t *Tool) Offer(r Rule, handle func(Plumb) bool) (RuleID, error) {
	args := map[string]any{}
	if r.Verb != "" {
		args["verb"] = r.Verb
	}
	if r.Text != "" {
		args["text"] = r.Text
	}
	if r.File != "" {
		args["file"] = r.File
	}
	if r.Kind != "" {
		args["kind"] = r.Kind
	}
	if r.Window != nil {
		args["window"] = r.Window.ID
	}
	if r.Priority != 0 {
		args["priority"] = r.Priority
	}
	var res struct {
		Rule int `json:"rule"`
	}
	if err := t.call("rule", args, &res); err != nil {
		return 0, err
	}
	id := RuleID(res.Rule)
	t.handlersMu.Lock()
	t.handlers[id] = handle
	t.handlersMu.Unlock()
	return id, nil
}

// Withdraw removes a rule.
func (t *Tool) Withdraw(id RuleID) error {
	t.handlersMu.Lock()
	delete(t.handlers, id)
	t.handlersMu.Unlock()
	return t.call("unrule", map[string]any{"rule": int(id)}, nil)
}

// Serve runs the tool's handlers as things happen, until the context
// is done, the session ends, or the bridge is gone. It returns nil
// when the session ended cleanly.
func (t *Tool) Serve(ctx context.Context) error {
	for {
		var ev map[string]json.RawMessage
		var ok bool
		popped := make(chan struct{})
		go func() {
			ev, ok = t.events.pop(t.done)
			close(popped)
		}()
		select {
		case <-ctx.Done():
			return ctx.Err()
		case <-popped:
		}
		if !ok {
			return t.err
		}
		var kind string
		_ = json.Unmarshal(ev["event"], &kind)
		switch kind {
		case "bye":
			return nil
		case "plumb":
			var p struct {
				Plumb  int      `json:"plumb"`
				Rule   int      `json:"rule"`
				Verb   string   `json:"verb"`
				Text   string   `json:"text"`
				Dir    string   `json:"dir"`
				Window *int     `json:"window"`
				Groups []string `json:"groups"`
				At     *Span    `json:"at"`
				Sel    *Span    `json:"sel"`
			}
			_ = unmarshalAll(ev, &p)
			t.handlersMu.Lock()
			h := t.handlers[RuleID(p.Rule)]
			t.handlersMu.Unlock()
			taken := false
			if h != nil {
				pl := Plumb{Rule: RuleID(p.Rule), Verb: p.Verb, Text: p.Text, Dir: p.Dir, Groups: p.Groups, At: p.At, Sel: p.Sel}
				if p.Window != nil {
					pl.Window = t.Window(*p.Window)
				}
				taken = h(pl)
			}
			_ = t.call("ack", map[string]any{"plumb": p.Plumb, "ok": taken}, nil)
		case "edit":
			var e struct {
				Window int    `json:"window"`
				Q0     int    `json:"q0"`
				Nd     int    `json:"nd"`
				Text   string `json:"text"`
			}
			_ = unmarshalAll(ev, &e)
			t.handlersMu.Lock()
			fn := t.watchers[e.Window]
			t.handlersMu.Unlock()
			if fn != nil {
				fn(Edit{Window: t.Window(e.Window), Q0: e.Q0, Nd: e.Nd, Text: e.Text})
			}
		case "renamed":
			var r struct {
				Window int    `json:"window"`
				Name   string `json:"name"`
			}
			_ = unmarshalAll(ev, &r)
			if t.onRename != nil {
				t.onRename(t.Window(r.Window), r.Name)
			}
		case "deleted":
			var d struct {
				Window int `json:"window"`
			}
			_ = unmarshalAll(ev, &d)
			t.handlersMu.Lock()
			delete(t.watchers, d.Window)
			t.handlersMu.Unlock()
			if t.onDelete != nil {
				t.onDelete(t.Window(d.Window))
			}
		}
	}
}

// ---- an unbounded queue of events ---------------------------------------

type queue struct {
	mu    sync.Mutex
	cond  *sync.Cond
	items []map[string]json.RawMessage
}

func newQueue() *queue {
	q := &queue{}
	q.cond = sync.NewCond(&q.mu)
	return q
}

func (q *queue) push(m map[string]json.RawMessage) {
	q.mu.Lock()
	q.items = append(q.items, m)
	q.mu.Unlock()
	q.cond.Broadcast()
}

// pop waits for an item, or for done to close with nothing queued.
func (q *queue) pop(done <-chan struct{}) (map[string]json.RawMessage, bool) {
	// done closing wakes the waiter through a broadcast from a helper
	stop := make(chan struct{})
	go func() {
		select {
		case <-done:
			q.cond.Broadcast()
		case <-stop:
		}
	}()
	defer close(stop)
	q.mu.Lock()
	defer q.mu.Unlock()
	for len(q.items) == 0 {
		select {
		case <-done:
			return nil, false
		default:
		}
		q.cond.Wait()
	}
	m := q.items[0]
	q.items = q.items[1:]
	return m, true
}
