# apex-cli

`apex`, the command. Scripts and tools never see the wire; they see this.

```
apex [--socket P] [--session S] server               run the daemon (foreground)
apex ls                                              list sessions
apex new-session NAME
apex rename-session [FROM] TO
apex attach [host/]SESSION [--stdio] [FILE...]       a UI; --stdio bridges the socket to stdin/stdout
apex new FILE...                                     open files in the first column
apex win list | win del WIN
apex text read WIN [--addr ADDR]                     an Edit address, e.g. `3,5` or `/fn main/`
apex edit WIN PROGRAM                                the Edit language
apex sel WIN [Q0 Q1]
apex exec [WIN] COMMAND                              as if B2
apex events [--shard S]                              entries as JSON lines, forever
apex term new [CMD...] | term send TERM TEXT | term read TERM
apex plumb [--dry-run] [--edit] TEXT
apex plumb rule add FLAGS | rm ID | ls
apex B FILE[:LINE] ...
apex label TEXT
apex awd [LABEL]
apex env [KEY=VALUE ...]
apex set [KEY VALUE]
apex cat PATH
```

`WIN` is a window id or a unique substring of a window's name. `APEX_SOCKET`
and `APEX_SESSION` set the defaults (a shell in an apex terminal has
`apexsession` and `APEX_SOCKET` set, so `apex` there works on the session
it is in); `apex attach` starts a daemon if the
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
--stdio` starts the daemon if needed and copies bytes between its stdio
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
lives: the app subscribes to the file (`Watch`, `apex cat` is the
one-shot form) until Quick Look exits, the file's window goes, or the
app loses its lead.
[examples/profile](../../examples/profile) is one that has zsh, bash and fish
name their window after the directory on every `cd`, through their
environment; rc does that on its own in an apex terminal (its rcmain
checks `$TERM_PROGRAM`).

## Builds

A daemon's first word on every connection is its build id, a hash of the
sources it was built from. A client of another build stops there and
says so: the daemon keeps running (its sessions may hold work), and when
they can be let go, `apex stop` on its machine ends it; the next attach
starts the current one. The app's Reconnect (⌘R) is that next attach.

## Plumbing rules

B3 (and `apex plumb`) walks the session's rule table in priority order,
highest first, then by age; the first rule that matches and is taken
ends the walk, and with none left the text is looked for in the window
(acme's Look). A rule is one `apex plumb rule add` command:

    --text RE        the plumbed text (a verb's arguments) must match RE, whole;
                     its groups bind $0..$9
    --file RE        the window's name must match RE
    --kind K         file | dir | term | errors
    --isfile EXPR    EXPR, expanded, is a file (relative to the window's directory)
    --isdir EXPR     ... a directory
    --verb NAME      the command this rule answers; `plumb` (B3) unless given.
                     Any other verb is offered in the tools menu (B4, or
                     shift-click) of every window the rule applies to, and
                     runs there as B2 would; B2 on the word does the same.
    --edit EXPR      open EXPR (`name` or `name:line`) in the session
    --run CMD        run CMD on the host in the window's directory, the
                     selection on stdin, output to dir/+Errors
    --client-do V A  ask the UI that asked to do V with A (`open` a URL, say);
                     a UI that cannot refuses, and the walk goes on
    --tool NAME      ask the tool attached as NAME; it answers within a second
                     or is taken to refuse (NACK), and the walk goes on
    --priority N     default 0
    --mine           owned by this attachment (gone when it detaches) rather
                     than the session

Templates expand `$0`..`$9`, `$file`, `$dir`, `$win`, `$line`, `$sel`.
Rules from the CLI are the session's; a UI installs its own on attach
(URLs go to the platform's `open`) and a tool installs those naming it.
`apex plumb --dry-run TEXT` prints what each rule would do. `apex B` is
plan 9's: each argument goes to the rules that open in the session, else
is opened as a path, from the current directory. The session starts with
three rules at priority -100 that open `name` and `name:line` when they
exist, as B3 always did.
