#!/usr/bin/env python3
"""A language server for the tests: enough JSON-RPC to check that apex lsp
opens and syncs documents, reads diagnostics, and acts on answers."""
import json, sys, time

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
        if "--delay-initialize" in sys.argv:
            time.sleep(2)
        send({"jsonrpc": "2.0", "id": id_, "result": {"capabilities": {"textDocumentSync": 2, "definitionProvider": True,
              "hoverProvider": True, "documentFormattingProvider": True, "referencesProvider": True,
              "documentSymbolProvider": True}}})
        # indexing, a third of the way, until the first document opens;
        # and a word for the user
        send({"jsonrpc": "2.0", "method": "$/progress", "params": {"token": "idx", "value": {"kind": "begin", "title": "indexing", "percentage": 30}}})
        send({"jsonrpc": "2.0", "method": "window/showMessage", "params": {"type": 3, "message": "fake ready"}})
        indexing = True
    elif method == "textDocument/didOpen":
        if "--hold-indexing" in sys.argv:
            time.sleep(1.5)
        send({"jsonrpc": "2.0", "method": "$/progress", "params": {"token": "idx", "value": {"kind": "end"}}})
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
    elif method == "textDocument/documentSymbol":
        # each `func NAME` a symbol, at its name; a `var NAME` in one its
        # child
        uri = m["params"]["textDocument"]["uri"]
        syms = []
        for n, line in enumerate(docs.get(uri, "").split("\n")):
            words = line.split()
            if len(words) >= 2 and words[0] in ("func", "var"):
                name = words[1].split("(")[0]
                col = line.index(name, line.index(words[0]) + len(words[0]))
                r = {"start": {"line": n, "character": col}, "end": {"line": n, "character": col + len(name)}}
                sym = {"name": name, "kind": 12, "range": r, "selectionRange": r, "children": []}
                if words[0] == "var" and line.startswith(("\t", " ")) and syms:
                    syms[-1]["children"].append(sym)
                else:
                    syms.append(sym)
        send({"jsonrpc": "2.0", "id": id_, "result": syms})
    elif method == "textDocument/references":
        send({"jsonrpc": "2.0", "id": id_, "result": []})
    elif id_ is not None:
        send({"jsonrpc": "2.0", "id": id_, "result": None})
