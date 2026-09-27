# apex as a modern Mac app (the `modern-mac` branch)

An experiment in the UI, after Manifold (`~/src/manifold`). What makes
apex apex is untouched: acme's tiling and its rectangles (which are
shared state, in the log), tags that are text you type into and B2,
the three buttons and their chords, plumbing, the sessions and the
daemon. Everything here is drawn inside the rectangles the tiling
gives, or beside them, with one deliberate exception: the stash (below),
which changes what B2 and B3 do on a window's box, and so is the core's
(`Column::stash`, protocol 32).

## What changed

- **Look.** Four palettes to live with (View ▸ Theme, `theme.rs`), each a
  light and a dark, the appearance (View: Light, Dark, System) choosing
  which: Alabaster (tonsky's, light and dark), System (Xcode's Default
  Light and Dark, the system blue), Classic (acme's make -- cream paper,
  pale blue tags, a yellow selection -- toned down, in the hues of
  go.dev's playground) GitHub (Light Colorblind lifted off white,
  and Dark Dimmed) and Nova (Panic's standard Bright and Dark, sampled
  from Panic's own preview of them). Each is a handful of key colours from which the rest
  follows (`make`); every one keeps orange-for-red and blue-for-green
  where it matters (the terminal's ANSI, apex diff's lines) and B2's and
  B3's sweeps apart under a deuteranopia simulation. A grey header over
  each window, hairlines for acme's black borders.
- **Fonts** (View ▸ Font, `fonts.rs`), one choice for text windows and
  tags, mono windows and terminals, the sidebar, the palettes and menus, and
  pages (previews, apex diff, as `--apex-font` and `--apex-mono`):
  System (SF Pro with its high legibility set and tabular figures, 14/20;
  Terminal's own SF Mono at Medium, 12/16, the Regular being thin),
  Classic (Lucida Grande 13/17, Menlo 12/16), Nova (SF Pro legible
  14/20 for text and the interface, Menlo 12/16), Go (Go 14/20, Go Mono
  12/16, bundled) and Mona (Mona Sans 15/21, Monaspace Xenon 12/16 with
  Manifold's texture healing and stylistic sets 2, 3, 7 and 8, bundled).
- **Tags as title bars.** The window's name in the primary ink, a
  medium weight; the commands after it in the secondary. Column tags
  and the top row are all secondary. A header's caret is not drawn
  while it only rests at the start.
- **Handles as document dots** (`text_element::dot`): hollow clean,
  filled dirty, gold stale; live rings it in the accent and lights its
  middle; working turns a spinner's arc round it. A notified window's
  header takes a pale tint of the accent, as Mail tints a flagged row,
  with pjw's face in the accent at its end (and by the window's row in
  the sidebar, as by the session's). Column and session boxes are drag grips.
  All still acme's layout boxes: B1 on them as ever, B2 and B3 on a
  window's as the stash has them.
- **The caret says where the keys go.** Every text's caret is a plain
  dark line, but the one the keys go to (acme's rule: the text under the
  pointer, else the last selected in; none while apex is not in front)
  is the accent blue, a little wider, blinking as iOS's does -- solid for
  half a second after a key, a click or the pointer coming to it, then
  on and off every 530 ms. A header's shows at its start too while it is
  the one. The window is drawn again only when the caret changes.
  A terminal's cursor is the same caret: the accent's, blinking, where
  the keys go, and the plain dark one where they do not (a hollow box
  once its program has ended).
- **Pointers.** The system's, not Plan 9's: the arrow over text as over
  everything else (a click in apex's text does far more than place an
  insertion point, which is all the I-beam promises), the open hand over
  what drags (a window's handle, a column's box, the session's) and the
  closed hand everywhere while one is held; a page keeps its own.
- **Scrollers.** acme's lane, drawn as a slim rounded thumb with no
  track (`paint_scroller`), in text, terminal and page windows alike.
- **Sidebar, and no title bar** (`sidebar.rs`), as Manifold's: the
  sessions as vertical tabs in a card inset from the window's edges, the
  window's buttons on its top row. The shown session lists its windows,
  column by column, each with its dot; a click reveals one and lands on
  it, as taking a notification does. New Session opens the picker.
  Pinned (⌃⌘S, View ▸ Show/Hide Sidebar), it stands beside the content;
  unpinned, the content has the whole window and the sidebar floats over
  it without moving it, brought by the pointer at the very edge -- the
  window's last column of points, or leaving by it -- and never while a
  button is held (moving windows or sweeping text leftwards is not
  asking for it), and put away a tenth of a second after the pointer
  is 8 pixels past it, sliding in and out as Manifold's does. The
  window's buttons show only with it. There is no tab strip and no title
  bar, so full screen is the whole screen.
- **Palettes.** The picker (⌘T) and the finder are Manifold's command
  palette: a card 560 wide centred across the window, its top 30% of
  the way down, rounded 12 with a hairline and a soft shadow, over a
  light scrim; a 48-point search row with a magnifying glass, rows 34
  high with the title at 13.5 and what follows in the secondary ink, the
  chosen row in the accent with white words.
- **The stash** (`tiling::colstash` and its kin, the core's). There is no
  maximise: B3 on a window's box puts it away in its column's stash, its
  space going to a neighbour as a closed window's does, and B2 puts away
  every other window in the column, so the one has it all (B2 on one
  alone brings them all back). The stash shows as the edges of a stack
  of paper peeking out under the column's windows, a few pixels a sheet,
  which the tiling leaves room for (`tiling::floor`). The pointer resting
  there a quarter of a second, no button held, brings the stash out: the
  stashed windows' tags, live, stacked over the column's foot in the
  column's order as sheets drawn out of the pile, put away a moment
  after the pointer leaves. Their handles: B1 brings one back where it
  was (under the window it was under, at the share of the column it
  had), B2 back alone (the rest put away), a drag back where it is let
  go, in any column. Their text is a tag's: B2 runs Del or Put there, B3
  looks. A stashed window is never a dead end: whatever goes to it (a
  Look, the plumber, the finder, the sidebar, a notification) brings it
  back where it was; and a column with a stash is never blank -- when
  its last window laid out is put away or closed, the stashed one
  nearest it comes back. Folded windows (a tag squeezed in place by a
  neighbour's growth or a drag) are acme's, and plain tags. The sidebar
  and the finder list stashed windows where they stand, the sidebar's
  names in the secondary ink; `apex win list` does too.
- **Menus.** The B4 tools menu is a Mac context menu (`menu.rs`,
  `Menu::place`) with menuhit's ways: up while the button is held, run on
  release over an item, the last choice remembered and opened under the
  pointer (which lands on it), a part and a lane past 25 items. It looks
  like a Mac menu: 22-pixel rows in the system font, the highlight an
  accent pill in from the sides, a checkmark by the remembered item as a
  pop-up button marks its choice, the corners rounded and a shadow under
  it, the lane down its right with a slim thumb.
- **Columns on paper.** A column is the body's paper where its windows
  leave it (a body's part line at its foot, the gaps), with a hairline
  where each window meets the one above, as acme's column is white with
  black between.
- **Waiting.** A tab coming up shows the system's spinner over its words.

## Not done

Dragging sessions to reorder them in the sidebar, the sidebar sliding
out from the left edge when hidden (Manifold's), and anything that
would move the tiling's rectangles: cards with gaps and rounded
corners round every window would need the layout to leave room for
them, and the layout is the core's.
