# Providers

apex reaches other machines through *providers*. A provider is an
executable named `apex-remote-<provider>` on the PATH, called as

```
apex-remote-<provider> DESTINATION COMMAND
```

It must run COMMAND, a single shell command line, on DESTINATION with
stdin and stdout connected, and exit with its status. That is `ssh`'s own
calling convention, and `ssh` is the built-in provider: `user@host` needs
no script. Any other destination is written `provider:name`, e.g.
`sprite:mybox`, and needs an `apex-remote-<provider>` script; `apex-remote-sprite` here is
an example. The command line is one argument (it uses `&&`, redirections
and the destination's `$HOME`), so a tool that takes argv wraps it in
`sh -c "$2"`.

That is the whole configuration: there is no configuration file. With a
provider in place, apex installs its own command on the destination and
starts a daemon there when you attach, from the command line

```
apex attach sprite:mybox/local
```

or from the session selector's "Remote…" entry.

`APEX_PROVIDER_<NAME>` (or `APEX_SSH`) names the program to use instead of
the one on the PATH; the tests use that.
