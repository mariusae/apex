# apex-cli

`apex`, the command. Scripts and tools never see the wire; they see this.

```
apex help                                  the commands and the help topics
apex help <command> | <topic>              what one does; apex <command> -h is the short form
apex [-socket=PATH] [-session=NAME] [-ensure-server] <command> [arguments]

server, ls, stop, new-session, rename-session, attach
new, open, win, text, edit, sel, exec, events, term, ps, kill
plumb, B, env, set, cat, tool, label, awd, version
```

Flags are Go's: `-flag=value`, or `-flag` for a boolean, before the
arguments. The command's own documentation (`apex help new`, and the
topics `sessions`, `scripts`, `rules`, `windows`) is the reference; what
follows is the shape of things.

`WIN` is a window id or a unique substring of a window's name. `APEX_SOCKET`
and `APEX_SESSION` set the defaults (a shell in an apex terminal has
`apexsession` and `APEX_SOCKET` set, so `apex` there works on the session
it is in, and `EDITOR` set to `apex-editor`, a link to the binary that
is plan9port's editinacme, `apex editor`: the file opens in the session
and the command returns when its window is deleted); `apex attach`
starts a daemon if the
socket does not answer.

Every subcommand attaches as a tool: it gets the session snapshot, reads
from its own replica, and proposes to the leader — the UI if one is
attached, else the daemon. So

```
apex new notes.txt
apex edit notes.txt ',x/TODO/ c/DONE/'
apex exec notes.txt Put
```

works with no UI running, and a UI attaching later finds the result.

Sessions are URLs: `local:///name`, `ssh://user@host/name`,
`sprite://box/name`; a daemon's first session is `default`. `apex attach
URL`, or `apex attach dest/session`, reaches `dest` through a provider (`user@host`
over ssh, or `provider:name` through an `apex-remote-<provider>` script on the
PATH, see `providers/README.md`), installs (or updates) our `apex` for the
destination's OS and architecture in `~/.apex/bin` there, then launches
`apex-ui --remote dest --session session`; on the destination `apex attach
-stdio` starts the daemon if needed and copies bytes between its stdio
and the daemon's socket. The Mac app carries a `linux-amd64` build.

`apex label TEXT` and `apex awd [LABEL]` are plan9port's `label` and
`awd`: they write `ESC ] ; text BEL` to the terminal, and apex names the
terminal's window after it, `awd` as `pwd/-LABEL` (the host by default).
In rc, `fn cd { builtin cd $1 && apex awd }` keeps the window's name (and
so where relative names resolve) on the shell's directory; shells that
report their directory with OSC 7 (`ESC ] 7 ; file://host/path BEL`) or
set an xterm title get the same treatment.

## Profile and attach

Two scripts, like a shell's profile and rc. When a session is made, one
`rc` on its host sources `~/.apex/profile` there, then the creator's
`~/.apex/profile` (shipped in the request; skipped when it is the same
file). It runs like any command, named `profile` in the top row with
output in `+Errors`, with `apexsession`, `APEX_SOCKET` and `apexclient`
(the creator's host name) set, so `apex` in it configures the session
being made: `apex new`, `apex exec Newcol`, and so on. Exports in it die
with it; `apex env KEY=VALUE` sets what terminals and commands made from
then on get, and `apex env` alone prints the environment.

Every time a client attaches, its `~/.apex/attach` runs on the host the
same way, named `attach`, with `apexattachment` naming the attaching
client. `apex set KEY VALUE` there records a setting of that client's,
gone when it detaches; the same command from the profile, or from a
terminal, records the session's. A client reads its own settings first,
then the session's, and `apex set` alone lists them all. `Preview`
is offered in the tools menu of every file whose extension a `Preview.EXT`
setting names an app for (`apex set Preview.md Marked`); `Preview` alone is the
fallback app when a rule of your own asks to preview something else, and
without it the platform previews (Quick Look on macOS). A remote file is
fetched into a local copy and the copy is kept current while the preview
lives: the app watches the file on the session's I/O plane (`apex io
-watch GET file://PATH` from a shell; `apex cat` is the one-shot form)
until Quick Look exits, the file's window goes, or the app loses its
lead.
[examples/profile](../../examples/profile) is one that has zsh, bash and fish
name their window after the directory on every `cd`, through their
environment; rc does that on its own in an apex terminal (its rcmain
checks `$TERM_PROGRAM`).

## Builds

A daemon's first word on every connection is its protocol version (and
its build id, a hash of the sources it was built from, to say which
binary it is). A client of another protocol version stops there and
says so: the daemon keeps running (its sessions may hold work), and when
they can be let go, `apex stop` on its machine ends it; the next attach
starts the current one. The app's Reconnect (⌘⇧R) is that next attach.
The version is `PROTOCOL` in `apex-server/src/proto.rs`, bumped by hand
with every change to what goes over the wire; builds that differ
elsewhere talk to each other.

## Plumbing rules

B3 (and `apex plumb`) walks the session's rule table in priority order,
highest first, then by age; the first rule that matches and is taken
ends the walk, and with none left the text is looked for in the window
(acme's Look). A rule is one `apex plumb rule add` command:

    -text=RE         the plumbed text (a verb's arguments) must match RE, whole;
                     its groups bind $0..$9
    -file=RE         the window's name must match RE
    -kind=K          file | dir | term | errors
    -isfile=EXPR     EXPR, expanded, is a file (relative to the window's directory)
    -isdir=EXPR      ... a directory
    -verb=NAME       the command this rule answers; `plumb` (B3) unless given.
                     Any other verb is offered in the tools menu (B4, or
                     shift-click) of every window the rule applies to, and
                     runs there as B2 would; B2 on the word does the same.
    -edit=EXPR       open EXPR (`name` or `name:line`) in the session
    -run=CMD         run CMD on the host in the window's directory, the
                     selection on stdin, output to dir/+Errors
    -client=V -args=A
                     ask the UI that asked to do V with A (`open` a URL, say);
                     a UI that cannot refuses, and the walk goes on
    -tool=NAME       ask the tool attached as NAME; it answers within a second
                     or is taken to refuse (NACK), and the walk goes on
    -priority=N      default 0
    -mine            owned by this attachment (gone when it detaches) rather
                     than the session

Templates expand `$0`..`$9`, `$file`, `$dir`, `$win`, `$line`, `$sel`.
Rules from the CLI are the session's; a UI installs its own on attach
(URLs go to the platform's `open`) and a tool installs those naming it.
`apex plumb -dry-run TEXT` prints what each rule would do. `apex B` is
plan 9's: each argument goes to the rules that open in the session, else
is opened as a path, from the current directory. The session starts with
three rules at priority -100 that open `name` and `name:line` when they
exist, as B3 always did.

## Tools: win and lsp

`apex tool win [CMD...]` is acme's win, a shell in a text window: the
editable transcript, with the shell's output at the output point and
your typing sent line by line, ^C/DEL to interrupt, B2 on any text typing
it to the shell (an old command line again), Send, and Interrupt
and EOF in the tools menu, and `Snarfout` (in terminals too): the last
command as `$ cmd`, the prompt normalised, and its output, found between
the last two prompts, into the snarf buffer. `Win` in a tag runs it in the window's
directory with `$acmeshell` (rc); `Newterm` is the real terminal running
the user's shell. `Kill Win` ends it.

### Language servers

`apex tool lsp` is a tool like any other, started from the host's profile
(`apex tool lsp &`) or a terminal; it attaches as `lsp`, is `lsp` in the
top row and `apex ps` (`Kill lsp` ends it), and nothing in the daemon
knows it. It runs one language server per workspace root for the
files open in the session (`lsp.LANG` names the command: `apex set
lsp.go gopls`; gopls, rust-analyzer, pyright, typescript-language-server
and clangd are the defaults), opens and closes documents as buffers come
and go, and feeds every edit incrementally, read off the entry stream.
Diagnostics go to `root/+lsp`, one plumbable `file:line:col: message`
per line, replaced as they change. Its rules, gone when it is: B3 on an
identifier in a source file goes to the definition (no definition, and
the walk goes on to the path rules and Look), and the tools menu of a
source window offers `Def Refs Type Hov Sig Fmt Rn`: definition and type
definition open and select, references, hover and signatures go to
`+Errors`, `Fmt` replaces the text with the server's formatting, `Rn
name` renames through the session for open buffers and on disk for the
rest. `APEX_LSP_DEBUG=1` traces the JSON-RPC.
