# apex-acp (experimental)

A spike: an [Agent Client Protocol](https://agentclientprotocol.com) client
that lives in an apex window, written against the public tool API
(`apex-tool`) only. It is not part of the supported surface; it is here to
play with the idea of a native, responsive interface to coding agents.

## Running it

An ACP agent is needed. The default is the SDK's
`npx -y @agentclientprotocol/claude-agent-acp@latest` (needs node);
`codex-acp` and others work the same way.

    cargo build -p apex-acp
    apex-acp                          # in a window's directory, from apex
    apex-acp -agent "codex-acp"       # another agent
    APEX_ACP_AGENT="..." apex-acp     # the same, from the environment
    apex-acp -cwd DIR -thoughts       # elsewhere; show the agent's thinking

Run it from an apex terminal or B2 it in a window: it attaches to the
session it was started in (`APEX_SOCKET`, `apexsession`) and makes the
window `DIR/+agent`.

## Using it

- Type at the end of the window. Enter on the last line sends the text
  after the last output as the prompt (as win does); `Send` in the tag
  sends a block that has newlines in it.
- The agent's reply streams in. Tool calls are lines:
  `⋯ title` pending, `▶` running, `✓` done, `✗` failed, ticked off in
  place; their file locations follow as `path:line`, which B3 opens.
  A plan is a block that is replaced as it changes.
- A permission request is a line `? title: Allow Always Deny Never`;
  B2 one of the words (they are verbs on the window, so the tag works too).
- `Cancel` interrupts the turn. Something typed while the agent is busy
  goes as the next prompt when the turn ends, if it ends in a newline.
- The agent reads files through apex when a window has them (unsaved
  edits included) and writes them to disk; apex reloads clean windows.
- Deleting the window ends the agent.

## Trying it without an agent

`fake_agent.py` here speaks enough ACP over stdio to show every path:
a streamed reply, a plan, a tool call that reads a file through apex,
a permission request, cancellation (prompt `slow`, then `Cancel`).

    apex-acp -agent "python3 exp/acp/fake_agent.py" -thoughts

## Not done

Terminals (agents run commands themselves and report), images, session
modes and slash commands, resuming a session, several agents in one
window, styling of any kind.
