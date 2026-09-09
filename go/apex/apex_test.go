package apex

import (
	"context"
	"os"
	"strings"
	"testing"
	"time"
)

// An integration test against a running daemon: set APEX_SOCKET (and
// apexsession) as apex does for the commands it runs, and have apex on
// the path or in APEX_BIN.
func TestAgainstASession(t *testing.T) {
	if os.Getenv("APEX_SOCKET") == "" {
		t.Skip("APEX_SOCKET not set: no session to test against")
	}
	tool, err := Attach("gotest", &Options{Binary: os.Getenv("APEX_BIN")})
	if err != nil {
		t.Fatal(err)
	}
	defer tool.Close()
	w, err := tool.New("/tmp/gotest-window")
	if err != nil {
		t.Fatal(err)
	}
	if err := w.Append("one\n"); err != nil {
		t.Fatal(err)
	}
	text, err := w.Read()
	if err != nil || text != "one\n" {
		t.Fatalf("read: %q, %v", text, err)
	}
	got := make(chan string, 1)
	if _, err := tool.Offer(Rule{Verb: "Shout", Window: w}, func(p Plumb) bool {
		got <- p.Text
		return true
	}); err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	go tool.Serve(ctx)
	if err := w.Exec("Shout loud"); err != nil {
		t.Fatal(err)
	}
	select {
	case s := <-got:
		if strings.TrimSpace(s) != "loud" {
			t.Fatalf("got %q", s)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("the verb never arrived")
	}
	if err := w.Delete(); err != nil {
		t.Fatal(err)
	}
}

func TestQueueEndsWhenDone(t *testing.T) {
	q := newQueue()
	done := make(chan struct{})
	go func() {
		time.Sleep(10 * time.Millisecond)
		close(done)
	}()
	if _, ok := q.pop(done); ok {
		t.Fatal("an item from an empty, finished queue")
	}
}
