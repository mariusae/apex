#!/usr/bin/env python3
"""A fake ACP agent: streams a reply, makes a tool call, asks permission,
reads a file through the client, runs a command in its terminal, updates
a plan, and lists and replays the sessions it pretends to have had.
Logs to stderr."""
import json, sys, os, datetime

def send(msg):
    sys.stdout.write(json.dumps(msg) + "\n"); sys.stdout.flush()

def notify(sid, update):
    send({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": sid, "update": update}})

next_id = [100]
def ask(method, params):
    """Send a request to the client and wait for its response."""
    rid = next_id[0]; next_id[0] += 1
    send({"jsonrpc": "2.0", "id": rid, "method": method, "params": params})
    while True:
        line = sys.stdin.readline()
        if not line: sys.exit(0)
        m = json.loads(line)
        if m.get("id") == rid and "method" not in m:
            return m
        handle(m)  # something else arrived first

cancelled = [False]
authed = [False]
terminals = [False]
def handle(m):
    method = m.get("method")
    if method == "initialize":
        terminals[0] = bool(m.get("params", {}).get("clientCapabilities", {}).get("terminal"))
        auth = [{"id": "fake-login", "name": "Log in to Fake"}] if os.environ.get("FAKE_NEEDS_AUTH") else []
        # $FAKE_NO_RESUME: an agent that cannot take a session up again
        caps = {} if os.environ.get("FAKE_NO_RESUME") else {"loadSession": True, "sessionCapabilities": {"list": {}, "resume": {}}}
        send({"jsonrpc": "2.0", "id": m["id"], "result": {"protocolVersion": 1, "agentCapabilities": caps, "authMethods": auth, "agentInfo": {"name": "fake-agent", "version": "0.1"}}})
    elif method == "authenticate":
        authed[0] = True
        send({"jsonrpc": "2.0", "id": m["id"], "result": {}})
    elif method == "session/new":
        if os.environ.get("FAKE_NEEDS_AUTH") and not authed[0]:
            send({"jsonrpc": "2.0", "id": m["id"], "error": {"code": -32000, "message": "Authentication required"}})
            return
        send({"jsonrpc": "2.0", "id": m["id"], "result": {"sessionId": "s1", "modes": {"currentModeId": "default", "availableModes": [{"id": "default", "name": "Always ask"}, {"id": "acceptEdits", "name": "Accept edits"}, {"id": "plan", "name": "Plan mode"}]}}})
        notify("s1", {"sessionUpdate": "available_commands_update", "availableCommands": [{"name": "review", "description": "review the diff"}, {"name": "compact", "description": "compact the context"}]})
    elif method == "session/list":
        cwd = m.get("params", {}).get("cwd") or os.getcwd()
        if os.environ.get("FAKE_NO_SESSIONS"):  # a directory nothing has happened in
            send({"jsonrpc": "2.0", "id": m["id"], "result": {"sessions": []}})
            return
        # uuids, as the real adapters give, so B3 on one can be tried;
        # the times are relative so every resolution of them shows
        now = datetime.datetime.now(datetime.timezone.utc)
        def ago(**kw):
            return (now - datetime.timedelta(**kw)).isoformat().replace("+00:00", "Z")
        send({"jsonrpc": "2.0", "id": m["id"], "result": {"sessions": [
            {"sessionId": "9e4f1a20-0049-41a2-b614-f7ad8a71fb56", "cwd": cwd, "title": "What is in /etc/hosts", "updatedAt": ago(hours=3)},
            {"sessionId": "0d1ca0bf-b79e-4650-850f-f010b697e8a0", "cwd": cwd, "title": "A conversation from this week", "updatedAt": ago(days=3)},
            {"sessionId": "bed8cc1b-222c-4fb4-9f92-9eff45fb6068", "cwd": cwd, "title": "An older conversation", "updatedAt": ago(days=40)},
        ]}})
    elif method == "session/load":
        # a replay: a past conversation said again as ordinary updates,
        # the user's half included, and then the modes
        sid = m["params"]["sessionId"]
        notify(sid, {"sessionUpdate": "user_message_chunk", "content": {"type": "text", "text": "what is in /etc/hosts?\n"}})
        notify(sid, {"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "Let me look."}})
        notify(sid, {"sessionUpdate": "tool_call", "toolCallId": "r1", "title": "Read /etc/hosts", "kind": "read", "status": "completed", "locations": [{"path": "/etc/hosts", "line": 1}]})
        notify(sid, {"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "\nIt names localhost, and nothing else worth saying."}})
        notify(sid, {"sessionUpdate": "user_message_chunk", "content": {"type": "text", "text": "thanks\n"}})
        notify(sid, {"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "Any time."}})
        send({"jsonrpc": "2.0", "id": m["id"], "result": {"modes": {"currentModeId": "plan", "availableModes": [{"id": "default", "name": "Always ask"}, {"id": "acceptEdits", "name": "Accept edits"}, {"id": "plan", "name": "Plan mode"}]}}})
        notify(sid, {"sessionUpdate": "available_commands_update", "availableCommands": [{"name": "review", "description": "review the diff"}, {"name": "compact", "description": "compact the context"}]})
    elif method == "session/set_mode":
        send({"jsonrpc": "2.0", "id": m["id"], "result": {}})
        notify(m["params"]["sessionId"], {"sessionUpdate": "current_mode_update", "currentModeId": m["params"]["modeId"]})
    elif method == "session/cancel":
        cancelled[0] = True
    elif method == "session/prompt":
        p = m["params"]; sid = p["sessionId"]
        text = "".join(b.get("text", "") for b in p["prompt"])
        sys.stderr.write(f"fake: prompt {text!r}\n")
        if text.strip() == "slow":
            # hang until cancelled
            while not cancelled[0]:
                line = sys.stdin.readline()
                if not line: sys.exit(0)
                handle(json.loads(line))
            cancelled[0] = False
            send({"jsonrpc": "2.0", "id": m["id"], "result": {"stopReason": "cancelled"}})
            return
        notify(sid, {"sessionUpdate": "agent_thought_chunk", "content": {"type": "text", "text": "thinking about it\n"}})
        notify(sid, {"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "You said: "}})
        notify(sid, {"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": text + "\nLet me look."}})
        notify(sid, {"sessionUpdate": "plan", "entries": [{"content": "read the file", "priority": "medium", "status": "in_progress"}, {"content": "answer", "priority": "medium", "status": "pending"}]})
        path = p.get("_path") or os.environ.get("FAKE_PATH", "/etc/hosts")
        notify(sid, {"sessionUpdate": "tool_call", "toolCallId": "t1", "title": "Read " + path, "kind": "read", "status": "in_progress", "locations": [{"path": path, "line": 2}]})
        r = ask("fs/read_text_file", {"sessionId": sid, "path": path, "line": 1, "limit": 2})
        got = r.get("result", {}).get("content", "?")
        notify(sid, {"sessionUpdate": "tool_call_update", "toolCallId": "t1", "status": "completed", "content": [{"type": "content", "content": {"type": "text", "text": got}}]})
        r = ask("session/request_permission", {"sessionId": sid, "toolCall": {"toolCallId": "t2", "title": "Edit " + path}, "options": [{"optionId": "once", "name": "Allow once", "kind": "allow_once"}, {"optionId": "no", "name": "Reject", "kind": "reject_once"}]})
        outcome = r.get("result", {}).get("outcome", {})
        sys.stderr.write(f"fake: permission {outcome}\n")
        notify(sid, {"sessionUpdate": "tool_call", "toolCallId": "t2", "title": "Edit " + path, "kind": "edit", "status": "completed" if outcome.get("outcome") == "selected" and outcome.get("optionId") == "once" else "failed"})
        notify(sid, {"sessionUpdate": "tool_call_update", "toolCallId": "t2", "status": "completed", "content": [{"type": "diff", "path": path, "oldText": "one\ntwo\nthree\n", "newText": "one\nTWO\nthree\nfour\n"}]})
        w = ask("fs/write_text_file", {"sessionId": sid, "path": path, "content": "one\nTWO\nthree\nfour\n"})
        sys.stderr.write(f"fake: write {w.get('result', w.get('error'))}\n")
        if terminals[0]:
            # a command that takes a few seconds, so its output arrives
            # live and there is time to B2 Kill it
            script = os.environ.get("FAKE_CMD", "for i in 1 2 3; do echo tick $i; sleep 1; done; echo done")
            r = ask("terminal/create", {"sessionId": sid, "command": "sh", "args": ["-c", script]})
            tid = r.get("result", {}).get("terminalId")
            sys.stderr.write(f"fake: terminal {tid}\n")
            notify(sid, {"sessionUpdate": "tool_call", "toolCallId": "t3", "title": "sh -c " + script, "kind": "execute", "status": "in_progress", "content": [{"type": "terminal", "terminalId": tid}]})
            e = ask("terminal/wait_for_exit", {"sessionId": sid, "terminalId": tid}).get("result", {})
            o = ask("terminal/output", {"sessionId": sid, "terminalId": tid}).get("result", {})
            sys.stderr.write(f"fake: exit {e} output {o.get('output')!r}\n")
            ok = e.get("exitStatus", e).get("exitCode") == 0
            notify(sid, {"sessionUpdate": "tool_call_update", "toolCallId": "t3", "status": "completed" if ok else "failed"})
            ask("terminal/release", {"sessionId": sid, "terminalId": tid})
        notify(sid, {"sessionUpdate": "plan", "entries": [{"content": "read the file", "priority": "medium", "status": "completed"}, {"content": "answer", "priority": "medium", "status": "completed"}]})
        notify(sid, {"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "\nDone with " + path + "."}})
        send({"jsonrpc": "2.0", "id": m["id"], "result": {"stopReason": "end_turn"}})
    elif "id" in m:
        send({"jsonrpc": "2.0", "id": m["id"], "error": {"code": -32601, "message": "no " + str(method)}})

for line in sys.stdin:
    handle(json.loads(line))
