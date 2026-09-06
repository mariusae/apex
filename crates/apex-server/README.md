# apex-server

Everything with an effect on the world: terminals (pinned `Term` shards),
files, external commands and pipes, and the execs that name the server as
handler. It never writes a shard it does not lead; what it wants done to
buffers, windows or the layout is a `Proposal` for the leader (`proposal.rs`).

Two ways to run it:

- **In-process** (`apex [files]`): the client owns the `Log`, the `Server`
  lives in the same process, and proposals are applied on the spot.
- **Daemon** (`apexd`, then `apex --attach [SOCKET] [files]`): `daemon.rs`
  owns the authoritative log, keeps a follower replica of the whole session,
  and speaks the attach protocol (`proto.rs`) over a Unix socket. The client
  side (`remote.rs`) runs the same `Node` over a *mirror* `Log`: it assigns
  the sequence numbers the server will store, so it leads without waiting
  for anything, ships its entries on `flush`, and takes the shards it
  follows (terminals, the metalog) as they arrive. Shard creation is a
  fire-and-forget request the server fulfils before the appends that
  follow it. Attaching while another client holds the leases reclaims them
  (single-player mode); the earlier client stays attached but fenced.

Wire format: postcard, `u32` little-endian length prefix per frame. The
same framing works over any byte stream (ssh's stdio next).

## Measurements

`apex-bench` (release build, M-series laptop, daemon on a thread of the
same process, one round trip = client → kernel → daemon thread → kernel →
client):

| what | p50 | p99 |
|---|---|---|
| ping round trip | 21 µs | 42 µs |
| one keystroke: local apply, flush, server ack | 14 µs | 23 µs |
| 100 000 keystrokes in one flush | 1.8 M entries/s | |
| attach: connect, snapshot, decode, ~120 KB text (770 KB snapshot) | 4.2 ms | |
| attach with 1.2 M runes (1.9 MB snapshot) | 6.3 ms | |
| attach with 11.5 M runes (11.9 MB snapshot) | 34 ms | |
| terminal key → new rows on the client | 180 µs | 570 µs |

The transport is not where time goes. Two things stand out:

- The snapshot is several times the text because it carries the undo
  history (every keystroke is a record). Compacting undo before snapshotting,
  or excluding it from the attach snapshot, would bring a large session's
  attach down to the cost of the text itself.
- Terminal latency is dominated by the pty and alacritty's parser, not by
  the socket: the whole grid is published as `Rows` entries per wakeup.
  Diffing rows would shrink the stream but the latency is already fine.

Through the stdio bridge (`apex-bench --socket P --via "apex --socket P
attach --stdio"`, the ssh path without the network): ping 26 µs,
keystroke → ack 21 µs, attach +4.5 ms flat, terminal 195 µs.

Run it against a live daemon with `apex-bench --socket PATH`. Note that
macOS limits socket paths to 104 bytes.

## Commands

B2 commands and pipes run as acme's do: `rc -c command` (mariusae/rustrc,
carried with the app; `sh` if there is none), in the window's directory,
with `$winid`, `$%` and `$samfile` set. `Kill name` ends what the server
started; the top row names running commands.

## Files

`watch.rs` watches the parent directories of open files. A change to a
clean buffer is proposed as a base-versioned `SetContent`; the leader
turns it into `Stale` if the buffer has moved on, so a lagging replica
never overwrites typing. `Put` on a stale buffer refuses once.
