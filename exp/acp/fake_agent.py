#!/usr/bin/env python3
"""A fake ACP agent: streams a reply, makes a tool call, asks permission,
reads a file through the client, updates a plan. Logs to stderr."""
import json, sys, os

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
def handle(m):
    method = m.get("method")
    if method == "initialize":
        auth = [{"id": "fake-login", "name": "Log in to Fake"}] if os.environ.get("FAKE_NEEDS_AUTH") else []
        send({"jsonrpc": "2.0", "id": m["id"], "result": {"protocolVersion": 1, "agentCapabilities": {}, "authMethods": auth, "agentInfo": {"name": "fake-agent", "version": "0.1"}}})
    elif method == "authenticate":
        authed[0] = True
        send({"jsonrpc": "2.0", "id": m["id"], "result": {}})
    elif method == "session/new":
        if os.environ.get("FAKE_NEEDS_AUTH") and not authed[0]:
            send({"jsonrpc": "2.0", "id": m["id"], "error": {"code": -32000, "message": "Authentication required"}})
            return
        send({"jsonrpc": "2.0", "id": m["id"], "result": {"sessionId": "s1", "modes": {"currentModeId": "default", "availableModes": [{"id": "default", "name": "Always ask"}, {"id": "acceptEdits", "name": "Accept edits"}, {"id": "plan", "name": "Plan mode"}]}}})
        notify("s1", {"sessionUpdate": "available_commands_update", "availableCommands": [{"name": "review", "description": "review the diff"}, {"name": "compact", "description": "compact the context"}]})
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
        notify(sid, {"sessionUpdate": "plan", "entries": [{"content": "read the file", "priority": "medium", "status": "completed"}, {"content": "answer", "priority": "medium", "status": "completed"}]})
        notify(sid, {"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "\nDone with " + path + "."}})
        send({"jsonrpc": "2.0", "id": m["id"], "result": {"stopReason": "end_turn"}})
    elif "id" in m:
        send({"jsonrpc": "2.0", "id": m["id"], "error": {"code": -32601, "message": "no " + str(method)}})

for line in sys.stdin:
    handle(json.loads(line))
