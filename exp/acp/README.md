# apex-acp (experimental)

A spike: an [Agent Client Protocol](https://agentclientprotocol.com) client
that lives in an apex window, written against the public tool API
(`apex-tool`) only. It is not part of the supported surface; it is here to
play with the idea of a native, responsive interface to coding agents.

## Running it

    cargo build -p apex-acp
    apex-acp                    # Anthropic's Claude adapter, the default
    apex-acp -agent codex       # the Codex adapter
    apex-acp -agent "CMD ARGS"  # any program that speaks ACP on stdio
    apex-acp -cwd DIR -thoughts # elsewhere; show the agent's thinking

`$APEX_ACP_AGENT` says the same as `-agent`. Run it from an apex terminal
or B2 it in a window: it attaches to the session it was started in
(`APEX_SOCKET`, `apexsession`) and makes the window `DIR/+claude`, named
after the agent.

The Claude and Codex adapters are npm packages. `apex-acp` runs
`claude-agent-acp` when it is installed, else fetches it with npx; when
npx is not on `PATH` it will use a node another program brought along
(`$APEX_ACP_NODE_BIN`, else the one Zed keeps for its own agents). The
first run fetches the package, which takes a moment. Credentials are the
Claude Code CLI's own, so a logged-in `claude` needs nothing more; if the
agent does want a login, the window says so and `Login` does it.

## Using it

- Type at the end of the window. Enter on the last line sends the text
  after the last output as the prompt (as win does); `Send` in the tag
  sends a block that has newlines in it.
- The agent's reply streams in. Tool calls are lines:
  `⋯ title` pending, `▶` running, `✓` done, `✗` failed, ticked off in
  place; the files they work on follow as `path:line`, which B3 opens,
  and an edit as its diff.
- A permission request is a line `? title: Allow Always Deny Never`;
  B2 one of the words (they are verbs on the window, so the tag works
  too). The line then says what was decided.
- `Mode` says what modes the agent has and which it is in, `Mode NAME`
  asks for one (`Mode plan`, `Mode accept`, `Mode bypass`).
  `Commands` lists the agent's slash commands; a prompt that begins with
  `/` goes to the agent as it is.
- `Cancel` interrupts the turn. Something typed while the agent is busy
  goes as the next prompt when the turn ends, if it ends in a newline.
- The agent reads files through apex when a window has them (unsaved
  edits included). It writes them back through the window too, so the
  change is an edit you can see and undo, and then a `Put`, so what the
  agent builds and tests next is on disk. With no window open, it writes
  straight to disk, where the watcher brings it into clean windows.
- Deleting the window ends the agent.

## Trying it without an agent

`fake_agent.py` here speaks enough ACP over stdio to show every path:
a streamed reply, a plan, modes, slash commands, a tool call that reads a
file through apex, a permission request, a diff, a write, and
cancellation (prompt `slow`, then `Cancel`).

    apex-acp -agent "python3 exp/acp/fake_agent.py" -thoughts

## Not done

The terminal capability (agents run commands themselves and report the
output, which is what the transcript shows), images, resuming a session,
several agents in one window, session config options, styling of any
kind.
