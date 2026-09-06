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
apex term new | term send TERM TEXT | term read TERM
apex plumb TEXT
```

`WIN` is a window id or a unique substring of a window's name. `APEX_SOCKET`
and `APEX_SESSION` set the defaults; `apex attach` starts a daemon if the
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
