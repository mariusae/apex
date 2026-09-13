# apex-acp (experimental)

A spike: an [Agent Client Protocol](https://agentclientprotocol.com) client
that lives in an apex window, written against the public tool API
(`apex-tool`) only. It is not part of the supported surface; it is here to
play with the idea of a native, responsive interface to coding agents.

## Running it

    cargo build -p apex-acp
    apex-acp                    # Anthropic's Claude adapter, the default
    apex-acp codex              # the Codex adapter
    apex-acp "CMD ARGS"         # any program that speaks ACP on stdio
    apex-acp -cwd DIR -thoughts # elsewhere; show the agent's thinking
    apex-acp -transcript        # open the transcript window too
    apex-acp -resume            # take up the newest session here

The agent is the argument, there being one thing to say;
`$APEX_ACP_AGENT` says the same. Run it from an apex terminal
or B2 it in a window: it attaches to the session it was started in
(`APEX_SOCKET`, `apexsession`) and makes the window `DIR/-claude`, named
after the agent: a program's window is named for it after a `-`, as
win's is, and the `+run` and `+transcript` beside it are its auxiliary
output, as acme's `+Errors` is. Nothing there is a file, so the tag
does not offer to `Put` it.

The Claude and Codex adapters are npm packages. `apex-acp` runs
`claude-agent-acp` when it is installed, else fetches it with npx; when
npx is not on `PATH` it will use a node another program brought along
(`$APEX_ACP_NODE_BIN`, else the one Zed keeps for its own agents). The
first run fetches the package, which takes a moment. Credentials are the
Claude Code CLI's own, so a logged-in `claude` needs nothing more; if the
agent does want a login, the window says so and `Login` does it.

## Using it

- Type at the end of the window; `Send` in the tag sends what follows
  the last output as the prompt. Enter is Enter, so a prompt is as many
  lines as you like, and `Send TEXT` sends the words after it instead.
  A `Send` while the agent is working queues: it goes when the turn ends.
- The window says three things and marks them, so a glance down the
  left margin tells them apart:

        – @agentclientprotocol/claude-agent-acp 0.76.0
        – ready [Auto]

        ~

        what is in /etc/hosts?

        • I'll take a look.
        • It names localhost, and nothing else worth saying.

        ~

  `–` is apex itself — what the agent is, what it is ready for, what
  was asked that could not be done. `•` is the agent, one to a message:
  the calls it made are taken away again as they finish, so these are
  the lines the work leaves behind, one a step of it. What you sent
  wants no mark of its own: it is what stands between the `~` ending
  the last turn and the bullets answering it.
- `~` ends a turn, and so begins the next thing said: what stands under
  it is yours. It is set off on both sides, and at the end of the
  window it is the mark of where to type, the cursor left on the line
  below it — put down when the window settles, taken away by anything
  written after it. A prompt sent under one leaves it where it is, and
  it stays as the record of where that exchange began; one sent during
  a turn, queued behind it, is given a `~` of its own, so a prompt is
  never without one.
- The cursor rides the output: it sits where a prompt is typed from the
  moment the session is ready, and the window follows what arrives, so
  nothing has to be clicked or scrolled to answer. What is half-typed
  keeps the cursor in it while the agent writes above, and a window
  scrolled back to read something holds still.
- This window holds the conversation and nothing else. What the agent
  is doing while it does it is one line at the end — its own words for
  the call, with `⋯` pending, `▶` running, `✓` done, `✗` failed —
  written over as the work moves on, and gone as soon as there is
  anything else to say. That line is one line: an agent's name for a
  call is often the command itself, heredoc and all, and what runs on
  is cut with a `…`. The window does not grow by a line a tool call,
  and never by a script, a file read or a diff.
- `Transcript` opens a window beside it, `DIR/-claude+transcript`, with
  the whole of what the session has said: a line a tool call, ticked
  off in place, the files they work on following as `path:line`, which
  B3 opens, and an edit as its diff. It follows its own output as this
  window does, cursor and all. `Transcript` again closes it, and
  so does Del. What it says is kept either way, so it opens on the
  whole of the conversation and not on the rest of it; `-transcript`
  has one open from the start. It follows its own output as a win does,
  so scrolling back to read something holds it still.
- A permission request is a line `? title: Allow Always Deny Never`;
  B2 one of the words (they are verbs on the window, so the tag works
  too). The line then says what was decided, and stays, in both
  windows: a decision is worth keeping where the question was asked.
- `Mode` says what modes the agent has and which it is in, `Mode NAME`
  asks for one (`Mode plan`, `Mode accept`, `Mode bypass`).
  `Commands` lists the agent's slash commands; a prompt that begins with
  `/` goes to the agent as it is.
- `Resume` lists the sessions the agent has had in this directory,
  newest first, a line each: the id the agent knows it by, when it was
  last worked in, and what it is about.

        sessions:
          99419ee2-0049-41a2-b614-f7ad8a71fb56  3:45PM      Implement session resumption
          f163be4d-1187-4fca-a534-4c61e2b9d70a  Tue11:14AM  Apex design summary
          11a241b8-fe78-448b-8a93-348034064c1a  3Aug26      Acme editor UI prototype

  The time is at the resolution that tells it apart: the clock today,
  the weekday within the week, the date beyond it. B3 an id to take
  that session up — a click takes the part of the id it landed on,
  which is enough when it tells the sessions apart, and a sweep takes
  the whole. `Resume ID` does the same in words, and `-resume` starts
  on the newest without asking. A hex word that names no session here
  is handed back, so B3 on one goes on to do what it always does. The
  listing is written on its own when a session opens, so taking one up
  again is the first thing offered.
- The agent replays the whole of a session it is given and the window
  is rebuilt from what comes: what stood in it outlived the program but
  not the conversation, and the agent's record is the one still true.
  The tool calls collapse into the action line as they replay, so a
  resumed window reads as the conversation and not as the machinery,
  and the transcript takes the whole reconstruction whether or not a
  window was showing one at the time. It is the same
  window throughout: a session is taken up in `DIR/-claude`, not beside
  it, and the line under the agent's name says which one it is.
- The window's handle pulses while the agent works, and rests while it
  waits for you to answer a permission. `Preview` opens a page beside
  the window with the last exchange the agent finished, rendered as
  markdown, and closes it again: what was asked, quoted, and then the
  answer to it — the agent's last word on the turn, not the running
  commentary it made on the way, which the window itself has. It holds
  still until the next answer is whole: what is half-said is in the
  transcript.
- `Interrupt` ends the turn the agent is in, as Esc does in the Claude
  CLI, and anything queued behind it goes too: it is an end to the work
  and not to one turn of it. The window says how many went.
- The tools menu (B4) holds the five verbs you reach for with nothing
  to click and nothing to say after them: `Send`, `Interrupt`,
  `Preview`, `Transcript`, `Stop`. The rest are unlisted — they work
  the same wherever B2 takes them, but would only crowd a menu, being
  either written into the window to be clicked where they stand
  (`Allow Always Deny Never`) or meaningless without an argument
  (`Mode plan`, `Resume ID`, `Login METHOD`, and `Commands`).
- Commands the agent runs go into one window beside this one,
  `DIR/-claude+run`, as acme's `+Errors` holds what B2 runs. Output
  arrives live, each command headed by itself and followed by how it
  ended, so a build is watched as it runs and a compiler's `path:line`
  is B3'd the moment it is printed; the window follows its output as a
  win does, so scrolling back to read an error holds it still. Its
  handle pulses while anything is running. `Stop` here ends every
  command going. Del on the run window clears it; what is still
  running goes on, into a fresh one.
- The agent reads files through apex when a window has them (unsaved
  edits included). It writes them back through the window too, so the
  change is an edit you can see and undo, and then a `Put`, so what the
  agent builds and tests next is on disk. With no window open, it writes
  straight to disk, where the watcher brings it into clean windows.
- Deleting the window ends the agent, and the commands it was running.

## Trying it without an agent

`fake_agent.py` here speaks enough ACP over stdio to show every path:
a streamed reply, a plan, modes, slash commands, a tool call that reads a
file through apex, a permission request, a diff, a write, a command run
in a terminal, three past sessions to `Resume` (uuids, at times that
show every resolution) and a conversation to replay when one is taken
up, and cancellation (prompt `slow`, then
`Interrupt`, which is also how to watch the handle pulse). Its command
ticks for three seconds, long enough to watch the output arrive and to
`Stop` it; `$FAKE_CMD` is a shell script of your own instead — give it
one of several lines to watch the window keep to the first of them
while the transcript takes the whole.
`$FAKE_NO_SESSIONS` makes it a directory nothing has happened in, and
`$FAKE_NO_RESUME` an agent that cannot take a session up at all.

    apex-acp "python3 exp/acp/fake_agent.py" -thoughts

## Not done

Images, several agents in one window, session config options, styling of
any kind. `session/fork`, `session/delete` and `session/close` are in
the protocol and unused here.

A terminal here is a child process with its output captured, which is
what the protocol asks for, but not a pty: nothing typed reaches a
command, so one that wants an answer will wait for an end of file it
never gets. A real pty would mean a `term` window, which the tool API
does not offer.
