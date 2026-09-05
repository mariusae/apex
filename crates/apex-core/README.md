# apex-core

The editor's replicated state machine, headless. A session's state is a
pure function of its shards' logs; this crate holds the entries, `apply`,
the in-memory log store with leases and fencing, and a `Node` that leads
shards: it types, selects, runs the built-in commands, and lowers Edit
programs into entries. See `../../DESIGN.md`.

| module | what |
|---|---|
| `ids`, `entry` | shards (`buffer`, `window`, `layout`, `term`, `meta`) and their entry types |
| `text` | rune-indexed text on a rope; the `apex_edit::Text` view of it |
| `buffer` | a buffer (acme's `File`): text, version, undo groups, and its **views** — selections and origins, adjusted by acme's `textinsert`/`textdelete` rules |
| `state` | `State::apply`, deterministic and pure; `hash()` for divergence checks; postcard snapshots |
| `log` | `Log`: one log per shard, leases with fence epochs, transfer (request/release/grant) and reclaim |
| `node` | `Node`: catch up as a follower; as leader append, type, cut/paste/undo, `Look`, `Edit`, `New`/`Del`/`Zerox`/`Newcol`/`Delcol`/`Sort`/`Font`, `+Errors` |

Design points visible in the code:

- **Views live in the buffer shard.** A buffer edit adjusts every view on
  the buffer, so `Select`/`Origin` entries are sequenced with `Edit`
  entries in one log; otherwise replicas could interleave them
  differently. Windows keep creation, kind and execs.
- **Undo is an entry.** `Undo`/`Redo` carry only the expected version; the
  inverse is derived from the history in the state, which is itself a
  function of the log. Consecutive typing shares one undo group until a
  mouse action or a command.
- **Execs are entries** with a handler. Built-ins run in the leader and get
  a `Done`/`Failed` status entry; everything else is recorded for the
  server and stays `Pending` until it reports.
- **Ids need no coordination**: each attachment allocates from its own
  range (`attachment << 40 | counter`).

## Tests

`tests/core.rs`: scenario tests (typing, Zerox, cut/paste/undo, Edit
lowering, commands and their status, Del's warning, columns, fencing and
transfer, snapshot plus tail) and property tests: for random action
sequences a follower replaying the log, and a node restored from a mid-way
snapshot plus the tail, must equal the leader byte for byte; undo/redo
round-trips; Edit lowering matches `apex_edit::apply`.

## Benchmarks (`cargo bench`)

On an Apple laptop, a 1M-rune buffer: typing a keystroke (edit + select
entries) ~260 ns; an edit with 100 views on the buffer ~0.7 µs; hashing
the state ~0.9 ms; a snapshot round trip ~0.3 ms; replaying 20k entries
into a follower ~2.2 ms; lowering `,x/fox/ c/cat/` (20k changes) ~50 ms.
