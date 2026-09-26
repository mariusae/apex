# apex as a modern Mac app (the `modern-mac` branch)

An experiment in the UI alone, after Manifold (`~/src/manifold`). What
makes apex apex is untouched: acme's tiling and its rectangles (which
are shared state, in the log), tags that are text you type into and
B2, the three buttons and their chords, plumbing, the sessions and the
daemon. Everything here is drawn inside the rectangles the tiling
gives, or beside them; nothing asks the core for anything it did not
already do.

## What changed

- **Look.** A Mac app's palette (`theme.rs`): a near-white paper, a grey
  header over each window, hairlines for acme's black borders, the
  system's selection blue and label greys, one accent blue. B2's sweep
  is amber and B3's blue, apart for a reader with deuteranopia. Text is
  SF Pro 14/20 set as an editor sets it for code: its high legibility
  set (`ss06`: I, l and 1 each unlike the others) and tabular figures
  (`tnum`); mono is SF Mono 12/16 (loaded from the system's own file,
  which CoreText will not hand out by name); both with a slashed zero
  from the fonts' `zero` feature.
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
  A terminal's cursor says the same: the accent's block, the character
  on it in white, blinking with the caret, where the keys go; a hollow
  box, as Terminal's inactive cursor, where they do not.
- **Pointers.** The system's, not Plan 9's: the arrow over text as over
  everything else (a click in apex's text does far more than place an
  insertion point, which is all the I-beam promises), the open hand over
  what drags (a window's handle, a column's box, the session's) and the
  closed hand everywhere while one is held; a page keeps its own.
- **Scrollers.** acme's lane, drawn as a slim rounded thumb with no
  track (`paint_scroller`), in text, terminal and page windows alike.
- **Sidebar** (`sidebar.rs`, ⌃⌘S): the sessions as vertical tabs in a
  card inset from the window's edges, the traffic lights at its top.
  The shown session lists its windows, column by column, each with its
  dot; a click reveals one and lands on it, as taking a notification
  does. New Session opens the picker. Hidden, the tab strip comes back.
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
