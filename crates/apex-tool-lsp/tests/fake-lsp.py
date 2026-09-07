#!/usr/bin/env python3
"""A language server for the tests: enough JSON-RPC to check that apex lsp
opens and syncs documents, reads diagnostics, and acts on answers."""
import json, sys

docs = {}

def read():
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line:
            return None
        line = line.strip()
        if not line:
            break
        if line.lower().startswith(b"content-length:"):
            length = int(line.split(b":")[1])
    return json.loads(sys.stdin.buffer.read(length))

def send(msg):
    body = json.dumps(msg).encode()
    sys.stdout.buffer.write(b"Content-Length: %d\r\n\r\n" % len(body) + body)
    sys.stdout.buffer.flush()

def diagnose(uri):
    text = docs.get(uri, "")
    first = text.split()[0] if text.split() else ""
    send({"jsonrpc": "2.0", "method": "textDocument/publishDiagnostics", "params": {"uri": uri, "diagnostics": [
        {"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}}, "severity": 2,
         "message": "len=%d first=%s" % (len(text), first)}]}})

while True:
    m = read()
    if m is None:
        break
    method = m.get("method")
    id_ = m.get("id")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": id_, "result": {"capabilities": {"textDocumentSync": 2, "definitionProvider": True,
              "hoverProvider": True, "documentFormattingProvider": True, "referencesProvider": True}}})
    elif method == "textDocument/didOpen":
        td = m["params"]["textDocument"]
        docs[td["uri"]] = td["text"]
        diagnose(td["uri"])
    elif method == "textDocument/didChange":
        uri = m["params"]["textDocument"]["uri"]
        text = docs.get(uri, "")
        for ch in m["params"]["contentChanges"]:
            r = ch["range"]
            lines = text.split("\n")
            def off(p):
                return sum(len(l) + 1 for l in lines[:p["line"]]) + p["character"]
            text = text[:off(r["start"])] + ch["text"] + text[off(r["end"]):]
        docs[uri] = text
        diagnose(uri)
    elif method == "textDocument/definition":
        uri = m["params"]["textDocument"]["uri"]
        # the word on line 1, columns 5..6 of the same file
        send({"jsonrpc": "2.0", "id": id_, "result": [{"uri": uri, "range": {"start": {"line": 1, "character": 5}, "end": {"line": 1, "character": 6}}}]})
    elif method == "textDocument/hover":
        send({"jsonrpc": "2.0", "id": id_, "result": {"contents": {"kind": "plaintext", "value": "hover: f is a func"}}})
    elif method == "textDocument/formatting":
        uri = m["params"]["textDocument"]["uri"]
        text = docs.get(uri, "")
        lines = len(text.split("\n"))
        send({"jsonrpc": "2.0", "id": id_, "result": [{"range": {"start": {"line": 0, "character": 0}, "end": {"line": lines, "character": 0}}, "newText": "package main\n\nfunc f() {}\n"}]})
    elif method == "textDocument/references":
        send({"jsonrpc": "2.0", "id": id_, "result": []})
    elif id_ is not None:
        send({"jsonrpc": "2.0", "id": id_, "result": None})
