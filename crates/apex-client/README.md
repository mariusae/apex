# apex-client

`apex-ui`, the gpui client: acme's interaction model over the core's
state, attached to a daemon (or, with `--local`, with the server
in-process).

```
apex-ui [files]                      attach to the local daemon; reopen last sessions
apex-ui --session S [files]          one session
apex-ui --attach SOCKET ...          another daemon
apex-ui --via CMD --session S        through CMD's stdin/stdout (ssh)
apex-ui --local [files]              server in-process
```

`mac/build-app.sh` builds `target/Apex.app` (release by default); the app
bundles the `apex` command and starts the daemon through it. Double-click
it, or `open target/Apex.app`.

In the window: the session name in the title bar (or ⌘K) opens the session
selector; type to filter, or type a new name and press return to create
it. ⌘N opens another window on the same session. Opt-click is B2,
cmd-click is B3.
