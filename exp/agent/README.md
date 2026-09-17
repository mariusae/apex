# apex-agent (experimental)

Serves the terminals of an apex session that are running agents --
Claude Code and Codex -- so that each of them has its transcript, its
last answer as a page and its changes as a diff on its own window, and
says with a notification when one wants you. It is fed by the hooks
those agents offer and written against the public tool API
(`apex-tool`) only, as `apex-acp` is; it is not part of the supported
surface.

## Running it

    cargo build -p apex-agent
    apex-agent install          # the hooks, for claude and codex both
    apex-agent install claude   # or one of them
    apex-agent                  # this session's agents; no window of its own
    apex-agent -a               # and the overview window, DIR/-agents
    apex-agent -all             # every agent on the machine, not only this session's
    apex-agent -quiet           # no notes in +Errors
    apex-agent -thoughts        # transcripts show the agents' thinking
    apex-agent uninstall

    apex-agent ls               # every agent, as text
    apex-agent wait ID          # until the agent's turn ends; exits with its state
    apex-agent events [-all]    # the events as they come, a line each

`install` writes the hooks into `~/.claude/settings.json` and
`~/.codex/hooks.json`, naming this binary by its full path; everything
else in those files stays as it was, an install over an install
changes nothing, and `uninstall` takes ours out and no others. Agents
started after that are seen; one already running is not, until it is
started again. Codex has its hooks on by default in current versions;
an older one wants `codex_hooks = true` under `[features]` in
`~/.codex/config.toml`.

Run `apex-agent` from an apex terminal or B2 it in a window: it attaches
to the session it was started in, and from then on the agents in that
session's terminals have its verbs on their own windows. It has no
window of its own unless `-a` asks for the overview one, `DIR/-agents`.

## How it works

The hook is this same program, `apex-agent hook claude`, run by the
agent at every event with the event's JSON on its standard input. It
appends one line to `~/.apex/agents/SESSION.jsonl` and exits: what
happened, when, which tool and what the call was in words, the prompt
or the last message when the event carries one (those two kept whole,
within reason, since the page shows them), and, once, the agent's
process, the apex session and window it was started in, and where its
repository stood. The agent's input is not kept -- a `Write`'s input is
the file -- so a log stays small.

apex-agent reads those logs, and nothing else: no socket, no daemon.
Nothing need be running when an agent starts, nothing is lost when it
is not, one started late sees what came before it, and two see the
same. The logs' directory is watched (FSEvents on a Mac,
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

What apex-agent says back to an agent goes the way apex already has. A
decision on a permission is an event in the agent's log, which the hook
that asked is waiting to read. A prompt is typed into the agent's
terminal by `apex term send`. An agent is started by `Newterm`. Nothing
is added to the agents, and nothing to apex.

## Notifications

An agent that wants you has its terminal notified: the turn is over
and the next prompt is yours, it is asking a permission, or the turn
failed. The terminal's handle shows it, a circle in a frame of the
notification colour; the session's square at the top left fills with
that colour, and so does its tab, so a session with an agent waiting
shows from any other. A click on the square goes to the terminal of the
agent that has waited longest and dismisses its notification; the next
click takes the next, one agent a click. Clicking or typing in the
terminal dismisses it too, and one raised while you are already in the
terminal, with apex in front, is dismissed at once.

A notification is lowered as soon as the agent leaves that state --
back at work, or gone -- and one you have dismissed is not raised again
until the agent has been back to work and come to want something
afresh. The notifications are apex-agent's, so it dying leaves none
waiting. An agent with no terminal in this session -- one of those
`-all` adds -- has none.

## The overview window (`-a`)

    – 2 agents

    ? claude  ~/src/apex  0b1c1425  2m
      build an experimental tool, apex-agent…
      ? Bash: Remove the build directory  Allow Deny Ask

    ▶ codex  ~/src/cmd  9e21ab77  1 subagent
      port the rc shell
      ▶ shell: cargo test
        ▶ Explore: Grep: fn main in src

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
why the turn failed. Under that, a line a subagent, with what each is
doing. A block that changes is written in place. The window's handle
pulses while any agent works, and the window is clean while none does:
what it says is whole and nothing is going on behind it. The first line
is apex's own, `–`, and says how many; with no agents it is a guide,
and `Start claude` there is a verb to B2.

The pane shows the agents started in the apex session it was launched
from, wherever their directories are, and a last line counts the rest
(`– 2 in other sessions: -all shows all`); `-all` shows every agent on
the machine. Outside an apex session there is none to narrow to, and
the pane's name is its filter instead: `DIR/-agents` shows the agents
under `DIR`.

Every verb below takes the agent dot is in, or the only one there is,
or the one named after it by its id (`Open 0b1c`), its kind (`Goto
codex`, when there is one) or its directory. Outside any block with
several to choose from, `+Errors` says so.

## Answering

`Allow`, `Deny` and `Ask` are verbs, in the agent's own window while it
asks and, in the pane, written into its block to be B2'd where they
stand. The answer is written into the agent's log as a `Decision`, the
record of it, and the hook that asked -- which has been waiting, up to a
minute and a half, since it saw apex-agent was there -- reads it and
tells the agent. `Ask` hands the question to the agent's own prompt, as
does saying nothing in time. With no apex-agent running a hook does not
wait at all, so an agent is never held up by one that is not there.
Until the answer is in, the agent's terminal shows the hook running and
no prompt: the notification is what says where the question is.

When an agent comes to ask, or its turn fails, a line in the session's
`+Errors` says which and what (`claude 0b1c1425 asks: Bash: rm -rf
target`); B3 on the id there opens the transcript. `-quiet` leaves
`+Errors` alone.

## Reaching the agent

`Goto` goes to the agent itself: the window it was started in, in
whatever apex session that was -- apex puts `apexsession` and `winid`
in a command's environment, the agent passes them on to its hooks, and
the hook keeps them -- so the pane is a way straight to any agent,
wherever it is; a notification is the same way to one of this session's.
An agent started outside apex has nowhere to go to, and `+Errors` says
so.

`Send TEXT` types the text into that window, Enter after it, by `apex
term send`, which reaches a terminal in any session: `Send 0b1c now run
the tests`, or `Send now run the tests` with dot in the block. In a
transcript window `Send` in the tag sends what is typed after the end
of the transcript, as apex-acp's window does, and takes it away again,
since it comes back as the agent's own record of it. `apex` must be on
PATH, which in an apex terminal it is.

`SendToClaude` (or `SendToCodex`) is offered in every text window --
a file's, a directory's, `+Errors` -- for each agent that can be sent
to, and sends that window's selection with where it came from, fenced:

    /path/to/file:123:
    ```
    the selected text
    ```

so the agent reads the place as well as the text, and B3 on the first
line goes there. With two agents of a kind the verb carries as much of
each id as tells them apart, `SendToClaude0b1c`; the verbs come and go
with the agents.

`Start [claude|codex] [DIR]` makes a terminal beside the pane running
the agent, in the pane's directory or the one named, by `Newterm`; the
hooks pick it up from there. `Resume ID` does the same with a past
session, in the directory it was had in, so the agent replays it there.

## In the agent's own window

An agent running in a terminal of apex-agent's own session is known by
the session and window its hooks recorded, and its verbs are offered on
that window: `Transcript`, `Preview` and `Changes` in the terminal's
tools menu for as long as the agent runs, and `Allow`, `Deny` and `Ask`
for as long as it asks -- so the agent's window is the place to answer
it from, as apex-acp's is, with nothing added to the agent. The verbs go
when the agent does. A rule may name a window
(`Rule::verb("Preview").window(w)`), which is all this is. Without `-a`
this and the notifications are the whole of apex-agent's face.

## The transcript

`AGENTDIR/-claude+0b1c1425`: named for the agent's own directory, so a
`path:line` in it is B3'd from where the agent worked, and for the
agent after a `-` as apex-acp's window is, with the id after a `+` so
two agents in one directory have windows of their own. B3 anywhere in a
block opens it, as does `Open`; so does B3 on a session's id wherever
it is written -- a terminal, a commit message, `+Errors` -- a hex word
that names no session of ours being handed back to what B3 always
does. It is read from the agent's own record (Claude Code's
`~/.claude/projects/.../SESSION.jsonl`, Codex's rollout) as that grows,
and reads as apex-acp's transcript window does:

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
handle pulses while the agent works. What is typed after the end is a
draft, and stays yours while the transcript goes on above it.

Subagents' doings are not in the transcript; the pane, with `-a`, shows
them.

## The page

`Preview` opens a page with the last exchange the agent finished, rendered as apex-acp's `Preview` renders its own: what
was asked, quoted, and then the answer -- the agent's last word on the
turn, not the running commentary on the way to it, which the
transcript has. It is written afresh as each turn ends, so it always
shows the latest answer whole, and holds still while the next one is
being made. `Preview` again closes it. The page is
`AGENTDIR/-claude+ID+Preview`, and goes through the converter the
session names for markdown (`Preview.md`, `apex md` unless a setting
says otherwise), so it looks like every other preview.

## The changes

`Changes` opens `ROOT/-claude+ID+diff`, at the root of the agent's
repository, with what the repository says has changed since the session
began: the status, a line a file, and then the diff, each hunk's header
followed by the `path:line` it lands at, which B3 opens. The hook
recorded where the repository stood when the session started, so
commits the agent made since are in it, not only what is uncommitted;
with nothing recorded, it is since the last commit. Git, Sapling and
Mercurial are known by the directory at their root and asked in their
own words (`git diff --no-prefix`, `sl diff --noprefix`, `hg diff
--noprefix`). `Changes` again writes it afresh, in the diff window too.

## The history

`History` is a verb of the pane's (`-a`). It opens
`DIR/-agents+history` with the sessions that directory has had, newest first, as apex-acp's `Resume` lists them:

    – sessions in /Users/me/src/apex, newest first; B3 an id for its transcript, Resume ID to take it up

      0b1c1425-a679-4e30-a289-4d0d2f2d47c4  3:45PM      claude  build an experimental tool, apex-agent…
      9e21ab77-0049-41a2-b614-f7ad8a71fb56  Tue11:14AM  codex   port the rc shell

The time is at the resolution that tells it apart. B3 on an id opens the
transcript, read from the record on disk, with a word at the top saying
it is a past session; `Resume ID` takes it up again in a new terminal.
`History` again closes the listing.

## As text

The same logs are there for scripts. `apex-agent ls` prints every agent
as the pane would.
`apex-agent wait ID` (an id, a prefix, or a kind when there is one)
returns when the agent is no longer working, with its state as the exit
status: 0 its turn is over, 1 it is asking something, 2 the turn
failed, 3 it is gone, 4 no such agent -- so `apex-agent wait 0b1c &&
apex-agent events` chains. `apex-agent events` prints each event as it
lands, tab-separated: when, the session, the agent, what happened, and
the words for it; `-all` starts from the beginning of every log.

## Not done

Codex is read from its documented formats and not against a running
one, its permission decision included. Sapling and Mercurial are asked
in the words their documentation gives and not against a running one
either. Sessions the hooks were installed after are not seen until they
are started again.
