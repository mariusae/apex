# apex as a modern Mac app (the `modern-mac` branch)

An experiment in the UI alone, after Manifold (`~/src/manifold`). What
makes apex apex is untouched: acme's tiling and its rectangles (which
are shared state, in the log), tags that are text you type into and
B2, the three buttons and their chords, plumbing, the sessions and the
daemon. Everything here is drawn inside the rectangles the tiling
gives, or beside them; nothing asks the core for anything it did not
already do.

## What changed

- **Look.** GitHub's palettes, both a step in from the extremes
  (`theme.rs`): light is GitHub Light Colorblind with its canvas.subtle
  (#f6f8fa) for the paper and a softer ink (#32383f), sheets and menus
  white above it; dark is GitHub Dark Dimmed (#22272e, ink #adbac7) with
  the colour-blind themes' orange-for-red and blue-for-green kept. From
  Primer, as the GitHub VS Code theme draws them: the canvas and its subtle
  grey, fg.default and fg.muted, accent.fg as the accent and caret and,
  at 20%, the selection; and, where GitHub's other themes have red and
  green, orange and blue -- B2's sweep orange and B3's blue, the
  terminal's ANSI red and green orange and blue, apex diff's removed and
  added lines orange and blue. A grey header over each window, hairlines
  for acme's black borders. (rsms's Sublime schemes were the palette
  before; they are in the history.)
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
  All still acme's layout boxes: B1, B2 and B3 on them as ever.
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
  it without moving it, brought by the pointer at the window's left edge
  (or leaving by it) and put away a tenth of a second after the pointer
  is 8 pixels past it, sliding in and out as Manifold's does. The
  window's buttons show only with it. There is no tab strip and no title
  bar, so full screen is the whole screen.
- **Sheets.** The picker and the finder hang from the top edge of the
  content, square above and rounded below, and slide down into place.
- **Stacks.** A window folded to its tag is drawn as the edge of a sheet
  in a stack, Manifold's stack of paper, on the body's paper.
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
