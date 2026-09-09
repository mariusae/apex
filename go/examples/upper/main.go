// Upper is an apex tool: it offers Upper in every file window, and
// replaces the selection there with its upper-case form. Run it in a
// session (from a terminal there, or the profile with &): apex tool
// windows then show Upper in their tools menu.
package main

import (
	"context"
	"log"
	"strings"

	"github.com/mariusae/apex/go/apex"
)

func main() {
	t, err := apex.Attach("upper", nil)
	if err != nil {
		log.Fatal(err)
	}
	defer t.Close()
	_, err = t.Offer(apex.Rule{Verb: "Upper", Kind: "file"}, func(p apex.Plumb) bool {
		// the selection: from the menu that is the window's dot
		q0, q1, ok := p.Range()
		if p.Window == nil || !ok {
			return false
		}
		text, err := p.Window.Read()
		if err != nil {
			return false
		}
		r := []rune(text)
		if q1 > len(r) {
			return false
		}
		up := strings.ToUpper(string(r[q0:q1]))
		return p.Window.Replace(q0, q1, up) == nil
	})
	if err != nil {
		log.Fatal(err)
	}
	if err := t.Serve(context.Background()); err != nil {
		log.Fatal(err)
	}
}
