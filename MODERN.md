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
  the sidebar, as by the session's). Column boxes are drag grips; the
  session's square, having nothing to drag, is bare (red when fenced).
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
  track (`paint_scroller`), in text, terminal and page windows alike --
  and, as macOS's overlay scrollers, only while its text moves, while the
  pointer is in the lane or dragging it, fading a second after
  (`Acme::scroller`). The lane works the same when the thumb is away.
- **Cards on a ground.** Each window is a card inset in its space, its
  outer corners rounded, on a ground a step below it that shows where
  acme drew black borders and under the column tags and the top row; no
  hairlines, tone parting a tag from its body. A window's tag inks its
  name's directory in the secondary, its last part in the primary, and
  its commands faintly until the pointer is on the tag (the column tags
  and the top row likewise). Selections and sweeps are softly rounded.
  The window the keys go to has a soft ring in the accent.
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
  which the tiling leaves room for (`tiling::floor`). The pointer on
  them, no button held, brings the stash out at once: the
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
- **ctrl-tab** (`switcher.rs`, `miniature.rs`) is Manifold's ⌘E over the
  sessions: while control is held the whole window is a row of cards,
  one a session, each its session's window drawn small and live from
  its replica (the shown one's and the parked ones'; pages are their
  paper and name, their native views being the shown session's alone),
  the chosen one in the middle with an accent edge and its neighbours a
  little smaller and fainter running off the sides, sliding as the
  choice moves. The presses walk the sessions most recently shown first
  (ctrl-shift-tab back); letting go of control switches to the chosen
  one, as does a click on a card; escape leaves things be. Nothing is
  switched while walking.
- **⌘E** (`switcher.rs`) brings a column's stash up as a stack of cards
  leaning back within the column, as Safari once showed its tabs, each a
  live preview (`miniature.rs` draws them through the lean: each row
  placed and narrowed as it goes down the card): the column tilts back
  into the front card and slides down while the stash rises behind it,
  the most recently put away chosen, the older ones behind showing their
  tags. More E's choose further back (⇧E forward), the cards before the
  choice sliding down to the foot and gathering there as their tags.
  Letting go of ⌘ brings the chosen window back where it was (B1's
  recall), its card settling flat onto where it lands as the rest fade;
  a click on a card does the same; escape settles the front card back
  as the column. The column is the one under the pointer, else the last
  worked in, else the first with a stash. View ▸ Bring Back from Stash
  does the same (a click or escape to finish).
- **Web windows** (`webbar.rs`) have a header of their own in the tag's
  place, as the Claude app's browser does: the handle (every button and
  drag of it as any window's -- it moves, grows, stashes), back and
  forward, and the address, and nothing else. A click in the address
  takes it for typing (all selected); return goes there (a bare host
  gets https://, a path is the host's file), escape or a click elsewhere
  leaves it. The tag is still the core's underneath -- its first word
  the address the page follows -- only not drawn. `Web` with nothing
  given or selected makes a blank page, its address field ready to type
  in (the core allows a web window with no address for it).
- **Letting a session go.** The pointer on a session's row in the
  sidebar shows an × (in place of the pjw a notified one wears), as the
  tabs had: the tab goes, the session stays on its host.
- **Column edges.** The line between two columns takes the ↔ pointer, and
  a drag of it moves only the line (`tiling::rowmovecol`, what the
  column box's resize does): the columns either side wider and narrower,
  never shuffled, and a click on it does nothing.
- **Placement preview.** While a handle is held -- a window's, a
  stashed one's, a column's, or the line between columns -- where it
  would land were it let go now is shaded in the accent, as Manifold
  shows where a dragged sheet would go. It is the drop itself, done on a
  copy of the layout (`Node::drag_window_preview` and its kin), so what
  is shown is what happens.
- **Restarting the server** (`restart.rs`), as Manifold offers: an
  attach that finds this machine's server of another version (which
  this Apex cannot talk to) offers, once a launch, to restart it, and
  Apex ▸ Restart Server… does whenever asked; both say first what goes
  with it (the sessions' windows, terminals and the programs in them,
  changes not saved -- counted, when it can) and that the tabs stay,
  each attaching to a fresh session of its name. A server of this
  protocol is asked to stop; one of another, which may not read our
  Stop as a Stop, is signalled, once the process at its socket's far
  end is seen to be an apex server on that socket
  (`remote::stop_any`). Nothing starts a second server over an old one
  any more: that took its socket and left it running, unreachable,
  with its sessions (`ensure_daemon`).
- **Force click is B3.** A trackpad pressed hard (macOS's "look up")
  turns the B1 press it began as into a B3 press where it is, before
  anything is swept: its release looks, or opens what the plumber finds.
- **⌘-hover** underlines, in the accent, what a ⌘-click (B3) would take
  under the pointer -- the selection when the pointer is in it, else the
  word it would look for or open -- and the pointer is a hand; ⌥-hover
  underlines, in the ink, what a ⌥-click (B2) would run.
- **⌘⇧P** (`commands.rs`) is a palette of commands to run in the window
  under the pointer: the words in its tag, its tools (the B4 menu's),
  what was run lately anywhere in the session, and apex's own, fuzzy-
  matched; return runs one there as B2 would, or what was typed. Go to
  in All Tabs moves to ⌘⇧O.
- **Windows glide** (`glide.rs`): what the tiling moves -- a grow, a
  stash or a recall, a drag, a close -- is drawn part of the way from
  where it was to where it is, for a sixth of a second; one that appears
  opens down from its top. Resizing the OS window or switching session
  snaps. A gliding window's terminal keeps its size, and its text its
  scroll, until it lands.
- **Errors as toasts** (`toasts.rs`): a command's errors still go to its
  +Errors window, but that window goes to its column's stash and what
  was written shows in a toast at the column's foot, with Show All (the
  window brought back) and ×; it goes after eight seconds unless the
  pointer is on it. An +Errors window brought back is written to as
  before.
- **Session previews**: the pointer on a session in the sidebar (not the
  one shown) brings its window up beside the row, live, as ctrl-tab's
  cards draw it.
- **Prompt marks** (OSC 133): with `eval "$(apex shell-integration zsh)"`
  (or `bash`) in the shell's startup file, the shell marks each
  command's prompt, output and end, and the terminal carries the marks
  (`TermOp::Marks`, protocol 33): ⌘↑ ⌘↓ go from prompt to prompt, a
  command that failed has a mark in the gutter by its prompt, in the
  terminal's red, and ⌘⇧C copies the last command's output.
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
