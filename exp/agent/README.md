# apex-agent (experimental)

One window that says what every agent on the machine is doing -- each
Claude Code and Codex, whichever terminal or editor it was started from
-- and a transcript window for any of them, B3'd open. It is fed by the
hooks those agents offer and written against the public tool API
(`apex-tool`) only, as `apex-acp` is; it is not part of the supported
surface.

## Running it

    cargo build -p apex-agent
    apex-agent install          # the hooks, for claude and codex both
    apex-agent install claude   # or one of them
    apex-agent                  # the pane, DIR/-agents
    apex-agent -thoughts        # transcripts show the agents' thinking
    apex-agent uninstall

`install` writes the hooks into `~/.claude/settings.json` and
`~/.codex/hooks.json`, naming this binary by its full path; everything
else in those files stays as it was, an install over an install
changes nothing, and `uninstall` takes ours out and no others. Agents
started after that are seen; one already running is not, until it is
started again. Codex has its hooks on by default in current versions;
an older one wants `codex_hooks = true` under `[features]` in
`~/.codex/config.toml`.

Run `apex-agent` from an apex terminal or B2 it in a window: it attaches
to the session it was started in and makes the window `DIR/-agents`.

## How it works

The hook is this same program, `apex-agent hook claude`, run by the
agent at every event with the event's JSON on its standard input. It
appends one line to `~/.apex/agents/SESSION.jsonl` and exits: what
happened, when, which tool and what the call was in words, the prompt
or the last message when the event carries one, and, once, the agent's
process and the apex session and window it was started in. The agent's input is
not kept -- a `Write`'s input is the file -- so a log stays small.

The pane reads those logs, and nothing else: no socket, no daemon.
Nothing need be running when an agent starts, nothing is lost when the
pane is not, a pane started late sees what came before it, and two
panes see the same. The logs' directory is watched (FSEvents on a Mac,
inotify on Linux, through the `notify` crate the server's own watcher
uses), and so is the directory of every transcript open, so a line is
read the moment it lands; directories rather than files, since a file
appended to is one thing and one renamed into place another. A slow
pass every five seconds looks anyway, so nothing a watch lets by is
missed for longer than that. A session that ends (`SessionEnd`) is
cleaned away, log and all; one killed outright sends no such thing, so
the agent's process, found once when the session starts, is asked
after on that same pass and a session whose process is gone goes the
same way.

## The pane

    – 2 agents

    ? claude  ~/src/apex  0b1c1425  2m
      build an experimental tool, apex-agent…
      ? Bash: Remove the build directory

    ▶ codex  ~/src/cmd  9e21ab77
      port the rc shell
      ▶ shell: cargo test

A block an agent. The margin is its state, so a glance down the column
finds the one that wants you, and the blocks come in that order:

- `?` a permission or a question: it cannot go on until you say
- `✗` the turn ended badly (a rate limit, a server error)
- `~` the turn is over and the next prompt is yours -- the mark apex-acp
  ends a turn with, and means the same here
- `▶` at work
- `⋯` heard of, and nothing said yet

The first line of a block says which agent, where (the home directory
as `~`), enough of its id to tell it apart, how long it has been quiet
when that is a minute or more, and how many subagents it has out. The
second is what it was asked, cut to one line. The third is what it is
doing about it -- the tool call going, in the agent's own words for it
(`Bash: Build and test`) or the thing itself (`Edit: src/main.rs`) --
or the call it is asking leave for, or its last word on the turn, or
why the turn failed. A block that changes is written in place; the
window's handle pulses while any agent works. The first line is
apex's own, `–`, and says how many.

B3 anywhere in a block opens the agent's transcript beside the pane, as
does `Open` with dot in it, or `Open ID` (or `Open claude`, when there
is one). B3 outside any block is handed back and does what it always
does.

`Goto`, the same way, goes to the agent itself: the window it was
started in, in whatever apex session that was -- apex puts
`apexsession` and `winid` in a command's environment, the agent passes
them on to its hooks, and the hook keeps them -- so the pane is a way
straight to any agent, wherever it is. One started outside apex has
nowhere to go to, and `+Errors` says so.

## The transcript

`AGENTDIR/-claude+0b1c1425`: named for the agent's own directory, so a
`path:line` in it is B3'd from where the agent worked, and for the
agent after a `-` as apex-acp's window is, with the id after a `+` so
two agents in one directory have windows of their own. It is read from
the agent's own record (Claude Code's `~/.claude/projects/.../SESSION.jsonl`,
Codex's rollout) as that grows, and reads as apex-acp's transcript
window does:

    ~

    what is in /etc/hosts?

    ✓ Read: /etc/hosts
        127.0.0.1 localhost
        ::1 localhost
    • It names localhost, and nothing else worth saying.

`~` where a prompt begins and what you sent under it; `•` a message of
the agent's; a line a tool call, `⋯` made, `▶` running, `?` asking, `✓`
done, `✗` failed, the glyph written over in place as the hooks and the
record say; under it what more there is to say -- the whole command
where the title was the words for it, an edit as its diff, a plan as
its steps -- and then the result, cut to a dozen lines. `–` is apex
itself: an API error, or that the agent is gone. `-thoughts` shows the
agent's thinking as `  ·` lines. The window follows its own output as a
win does, so scrolling back to read something holds it still, and its
handle pulses while the agent works.

Subagents' doings are not in the transcript; the pane counts them.

## Not done

Codex is read from its documented formats and not against a running
one. Nothing here talks back to an agent: a permission is answered
where it was asked. Sessions the hooks were installed after are not
seen until they are started again; the transcripts on disk could be
listed, as `Resume` does in apex-acp, but are not.
