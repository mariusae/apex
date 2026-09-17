# Vendored assets

- `mermaid.min.js.gz`: [Mermaid](https://mermaid.js.org) 12.0.0,
  `dist/mermaid.min.js` from the npm package, gzipped (`gzip -9 -n`).
  MIT licence. Previews draw ```` ```mermaid ```` blocks with it; the
  client decompresses it once and gives it to a page that has such a
  block (`web.rs`). The uncompressed file's SHA-256 is
  `28fca7ae6ebc7ed7bb63bde63136a74bfef14f296a57e403657eeb8b32836073`.

  To update: fetch `https://cdn.jsdelivr.net/npm/mermaid@VERSION/dist/mermaid.min.js`,
  gzip it as above, and change the version and hash here.
