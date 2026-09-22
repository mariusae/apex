# Vendored assets

- `pjw.svg`: Peter J. Weinberger's face, the mark Plan 9 shows when
  something wants you. Converted from plan9port's
  `postscript/prologues/pjw.char.ps` (a MetaPost outline of 1994): its
  two subpaths' `moveto`/`lineto`/`curveto` turned into SVG path data,
  filled even-odd as the PostScript does (`eofill`), with the
  coordinates flipped for SVG's downward y and moved to the origin.
  MIT licence (the Plan 9 Foundation's relicence of Plan 9; plan9port's
  own LICENSE). A tab whose session wants the user draws it before the
  name (`shell::pjw`), tinted by gpui like any other SVG.

- `mermaid.min.js.gz`: [Mermaid](https://mermaid.js.org) 12.0.0,
  `dist/mermaid.min.js` from the npm package, gzipped (`gzip -9 -n`).
  MIT licence. Previews draw ```` ```mermaid ```` blocks with it; the
  client decompresses it once and gives it to a page that has such a
  block (`web.rs`). The uncompressed file's SHA-256 is
  `28fca7ae6ebc7ed7bb63bde63136a74bfef14f296a57e403657eeb8b32836073`.

  To update: fetch `https://cdn.jsdelivr.net/npm/mermaid@VERSION/dist/mermaid.min.js`,
  gzip it as above, and change the version and hash here.
