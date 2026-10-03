# apex as a modern Mac app (the `modern-mac` branch)

An experiment in the UI, after Manifold (`~/src/manifold`). What makes
apex apex is untouched: acme's tiling and its rectangles (which are
shared state, in the log), tags that are text you type into and B2,
the three buttons and their chords, plumbing, the sessions and the
daemon. Everything here is drawn inside the rectangles the tiling
gives, or beside them, with one deliberate exception: the stash (below),
windows put away out of the tiling, which is the core's
(`Layout::stash`, protocol 37).

## What changed

- **Look.** Six palettes to live with (View ▸ Theme, `theme.rs`), each a
  light and a dark, the appearance (View: Light, Dark, System) choosing
  which: Alabaster (tonsky's, light and dark), Xcode (Xcode's Default
  Light and Dark, the system blue), Classic (acme's make -- cream paper,
  pale blue tags, a yellow selection -- toned down, in the hues of
  go.dev's playground) GitHub (Light Colorblind lifted off white,
  and Dark Dimmed), Nova (Panic's standard Bright and Dark, sampled
  from Panic's own preview of them) and rsms (Rasmus Andersson's Sublime
  Text theme: its bright scheme, and dark mono for the dark -- the
  editor's colours from its schemes, the chrome's as Sublime's Adaptive
  UI draws round them, sampled from its screenshots). Each is a handful of key colours from which the rest
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
  12/16, bundled), Mona (Mona Sans 15/21, Monaspace Xenon 12/16 with
  Manifold's texture healing and stylistic sets 2, 3, 7 and 8, bundled)
  Inter (Inter 14/20 and JetBrains Mono 12/16, as rsms sets his Sublime
  theme; both bundled), Geist (Vercel's Geist 14/20 and Geist Mono
  12/16, bundled), Styrene (Commercial Type's Styrene B 14/20 with
  JetBrains Mono 12/16: not bundled, the installed family) and H&Co
  (Ideal Sans 14/20, Operator Mono 12/16, the screen-smart cuts: not
  bundled, the installed families). ⌘+ and ⌘− (View ▸ Font ▸ Bigger,
  Smaller) take the text a pixel bigger or smaller a step, the mono
  faces in proportion, line heights to whole pixels; ⌘0 back to the
  set's own size. Kept, as the set is.
- **Tags as title bars.** The window's name in the primary ink, a
  medium weight; the commands after it in the secondary. Column tags
  and the top row are all secondary. A header's caret is not drawn
  while it only rests at the start.
- **Handles as document dots** (`text_element::dot`): hollow clean,
  filled dirty, gold stale; live rings it in the accent and lights its
  middle; working turns a spinner's arc round it. A notified window's
  header takes a pale tint of the accent, as Mail tints a flagged row.
  pjw's face is only ever for another session wanting the user: on the
  title bar's chevron, by its row in the sidebar and the chevron's list,
  on its card in the overview -- never for this one. Column boxes are
  drag grips; the
  session's square, having nothing to drag, is bare (red when fenced).
  B1 on the top row past its text drags the Mac window, as a title bar
  does.
  All still acme's layout boxes: B1, B2 and B3 on them as ever. A
  window grown to the whole column with others hidden behind it has a
  square handle; alone in its column it stays round.
- **The caret says where the keys go.** Every text's caret is a plain
  dark line, but the one the keys go to (acme's rule: the text under the
  pointer, else the last selected in; none while apex is not in front)
  is the accent blue, a little wider, blinking as iOS's does -- solid for
  half a second after a key, a click or the pointer coming to it, then
  on and off every 530 ms (View ▸ Blink Cursor off: steady). View ▸
  Smooth Cursor (off at first, to live with) has it glide where it
  moves, over 90 ms, as Neovide's and Ghostty's cursor shaders do -- the
  terminal's cursor too, where the keys go -- and go with the text at
  once when the text itself moves (a scroll, a window gliding). A
  header's shows at its start too while it is the one. It is as tall
  as the ink -- the tallest ascender to the deepest descender, a pixel
  over each way -- centred on the line, not the line's height, which a
  tag's air makes taller. The window is drawn again only when the caret
  changes.
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
  It stands at the right, as macOS's and Ghostty's do. In text and
  terminal windows it keeps no room of its own (`paint_overlay_scroller`):
  the text starts 8 in (`BODY_MARGIN`), and the scroller lays over the
  body's right edge. Shut, the lane is 6 wide to the pointer
  (`LANE_HIT`), and the thumb shows over the text while it moves; with
  the pointer in it, the lane opens to its full 12 with a gutter drawn
  over what is there (which does not move), and B1 B2 B3 in it are
  acme's scrollbar. A page keeps its bar beside it, at its right.
- **Cards on a ground.** Each window is a card inset in its space, its
  outer corners rounded, on a ground a step below it that shows where
  acme drew black borders and under the column tags and the top row; no
  hairlines, tone parting a tag from its body. A window's tag inks its
  name's directory in the secondary, its last part in the primary, and
  its commands faintly until the pointer is on the tag (the column tags
  and the top row likewise). Selections and sweeps are softly rounded.
  B2 and B3 sweeps (and their pills) are drawn as a chat app draws a
  link: a pale wash of the button's colour -- the blue for B3, the
  action's orange for B2 -- with the text in that colour, deepened to
  read on it, and corners a little rounder than a selection's.
  The window the keys go to has a soft ring in the accent.
- **The title bar is the top row** (`main.rs`, `title_bar`), as a
  modern Mac app's: across the whole window, the window's buttons, the
  sidebar's button, the session (`titlebar.rs`), a divider, then acme's
  top tag, as editable as ever. The session is its name in bold -- a
  click makes it a field, return renames the session on its daemon --
  and a chevron that drops the sessions down (this one checked, a
  notified one wearing pjw, New Session after them; a click goes to
  one); pjw sits on the chevron, a badge, while another session wants
  the user.
  Its bare parts move the window and a double click there zooms
  it; the tiling's line for the top tag lies above acme's area (the
  row's rectangle starts a line up), the tag drawn in the bar instead.
- **Sidebar** (`sidebar.rs`), as Manifold's: the
  sessions as vertical tabs in a card inset from the window's edges,
  running up round the window's buttons and its own toggle, one with
  them as Reflect's is (the title bar -- the session's name, the top
  row, the stash -- is the rest's, right of it) -- every known host's, under the host's name
  (this Mac first) when there is more than one: those open in the app
  as their tabs, the others fainter, a click opening one. Each host is
  asked for its sessions in the background as the sidebar shows and
  each minute it stays (what it had last shown until it answers, and
  kept, marked unreachable, if it does not). The shown session lists its windows,
  column by column, each with its dot; a click reveals one and lands on
  it, as taking a notification does. New Session opens the picker.
  Shown or not by the title bar's button (⌃⌘S, View ▸ Show/Hide
  Sidebar), it stands beside the content; hidden, the content has the
  whole window. Nothing brings it out by itself (the pointer at the
  window's edge once did, over the content, on Liquid Glass): the button
  is always there. There is no tab strip.
- **Palettes.** The picker (⌘T) and the finder are Manifold's command
  palette: a card 560 wide centred across the window, its top 30% of
  the way down, rounded 12 with a hairline and a soft shadow, over a
  light scrim; a 48-point search row with a magnifying glass, rows 34
  high with the title at 13.5 and what follows in the secondary ink, the
  chosen row in the accent with white words.
- **Maximizing** (`tiling::colfull`, `colmaximize` and their kin, the
  core's). B3 on a window's box is acme's again: the window grown to
  the whole column, keeping its place in it, the others hidden behind
  it (`Column::full`); B3 again or B1 gives them back exactly where
  they were. Its handle is square while others are hidden. B2
  maximizes as acme's B2 does: the others in the column down to their
  tags, which show, so it needs no mark; B1 on the maximized window's
  box gives each back the size it had (`Slot::premax`). Adding a
  window to the column, closing one, dragging or growing gives the
  hidden ones back first; going to a hidden window does too.
- **The stash** (`shelf.rs`, `tiling::stash`): one for the session, not
  a column's. ⌘M (View ▸ Stash Window) puts the window the keys go to
  away -- out of its column, its space going to a neighbour as a closed
  window's does; `Stash` typed and run in a tag does the same (it is
  not in the tags as drawn). The stashed windows show at the title
  bar's right end as their tags made small, bunched like a hand of
  cards, the latest on top. The pointer on them, or a scroll over
  them, fans them out, and the one under the pointer (or scrolled to)
  shows below the bar: the window itself, live, at the size it had
  (at least 480 wide and 320 or two fifths of the window tall: one put
  away as it was made, an errors window, may have had a line or two;
  such a window comes back with an even share of its column, too).
  The pointer can go onto it and work in it as in any window --
  select, snarf, B2, B3, type, scroll -- and it stays stashed (a Look in
  it is found and shown there, the mouse going to it there); the fan
  closes a quarter second after the pointer has left the cards and the
  preview, not while a button is held. A window worked in there (a
  click or a key in it, or a toast's Show All) is brought forward as
  the fan closes -- rightmost, the first card met next time -- not while
  it is open, where its card moving would take the preview away. A click on a card, or B1 on the
  handle in the preview, brings the window back where it was -- under the window it was under, at the share of its
  column it had, or at the foot of the active column if its own is
  gone. The sidebar lists them under Stashed, and a click there does
  the same. That is the only way back but one: whatever goes to a
  stashed window (a Look, the plumber, the finder, a notification)
  brings it back too. The finder and `apex win list` list stashed
  windows where they stood.
- **ctrl-tab** (`switcher.rs`) walks the sessions live: each press
  shows the next one in the window at once, sliding in from the right
  over the one it replaces (which slides out to the left, drawn from its
  replica for the moment it takes; ctrl-shift-tab the other way), and
  the walk goes on while control is held; letting go leaves the window
  where it came to, and escape goes back to where the walk began. The
  presses walk the sessions most recently settled on first; the ones
  passed on the way are not taken as settled on. A session not
  connected yet is being connected as it is passed.
- **⌘⇧\\ (or ⌘') shows every session** (View ▸ Show All Sessions), as Mission
  Control shows the windows: a grid of cards over the window, as large
  as the window allows, each its session's window drawn small and live
  from its replica (a notified one wearing pjw), in the sidebar's order,
  on an opaque ground. The window shrinks into its card as the grid
  comes up. One ring, the accent's, is on the card chosen -- this
  session's at first -- and is at once on the one under the pointer or
  the arrows' next. A click or return goes to it, its card growing to fill
  the window; escape, a click off the cards or ⌘⇧\\ again goes back to
  this one the same way.
- **Web windows** (`webbar.rs`) have a header of their own in the tag's
  place, as the Claude app's browser does: the handle (every button and
  drag of it as any window's -- it moves, grows, maximizes), back and
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
  column's, or the line between columns -- where it
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
- **Pills.** ⌘ held puts what a ⌘-click (B3) would take under the
  pointer -- the selection when the pointer is in it, else the word it
  would look for or open -- on a pill in B3's sweep colour, its text in
  the sweep's ink, the pointer a hand; ⌥ held, what a ⌥-click (B2) would
  run, on a pill in B2's. In tags as in bodies, and only with the
  modifier held.
- **Tag lines** are six pixels taller than a body's (`TAG_PAD`): the
  tiling's font height is theirs. The pad is the tag's, once, over its
  first row and under its last, not between them: a tag of `n` lines is
  a tag line and `n - 1` rows (`tiling::tag_height`, the client's
  `Info::tag_row` a body's line, `tag_row_height`), its rows a body's
  line apart, and a click in the pad is its nearest row's. What is drawn a
  tag's line high -- the handle's box, a selection, a sweep, a pill --
  still is, reaching into the pad over the first row and under the
  last, so a one-row tag is drawn as it always was. Text is centred by its ink, not by
  the face's ascent and descent (`ink_lift`: a face keeps room over its
  ascenders for accents, so text centred the usual way sits low), so
  there is as much air over the ascenders as under the descenders, in a
  pill as in a line; a folded window's card, a little shorter than the
  line, has its line and handle centred in it.
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
- **Empty columns** say what to do there, faintly, in the middle of
  them (where there is room): ⌘P to go to a file, ⌘N for a new window,
  B2 on Newterm for a shell, ⌘⇧P for every command. Drawing only: the
  column's ground takes the buttons as ever.
- **Errors as toasts** (`toasts.rs`): a command's errors still go to its
  errors window, but that window is made in the stash (as every
  diagnostic window is: a language server's, a script's
  `apex new -diagnostic`) and what is new in it shows in a toast at the
  app's lower right, with Show All (the
  window shown in the stash's preview and left stashed, the pointer on
  the toast's first line there, selected; B1 anywhere on the toast
  does the same) and ×; it goes after eight
  seconds unless the pointer is on it, and at once on a click anywhere
  off the toasts (in a page too). An errors window brought back is
  written to as before.
- **Session previews**: the pointer on a session in the sidebar (not the
  one shown) brings its window up beside the row, live, as ctrl-tab's
  cards draw it.
- **Prompt marks** (OSC 133): with `eval "$(apex shell-integration zsh)"`
  (or `bash`) in the shell's startup file, the shell marks each
  command's prompt, output and end, and the terminal carries the marks
  (`TermOp::Marks`, protocol 33): ⌘↑ ⌘↓ go from prompt to prompt, a
  command that failed has a mark in the gutter by its prompt, in the
  terminal's red, and ⌘⇧C copies the last command's output.
- **^F completes inline** (`completion.rs`): the path fragment before
  the caret goes to the server (`Candidates`, protocol 34, answered to
  the asker alone), what its candidates share is typed in at once, and
  when more than one is left they are listed under the caret (over its
  line, when the window's foot leaves no room below). Typing
  goes on into the text and narrows the list; ↑ ↓ choose; return or tab
  takes the one chosen; a slash typed, escape or a click elsewhere puts
  the list away. One candidate is simply typed in, a directory with its
  slash, a file with a space after, as acme's ^F does. A completion done
  is done: the next list (a directory's names, say) comes only with the
  next ^F. No more lists in the errors window.
- **A preview's contents as a scrubber** (`web.rs`, `TOC_SCRIPT`): down
  the left edge (the scrollbar is at the right) of a page rendered from a buffer (a Markdown preview,
  apex diff), each heading a tick where it stands in the page, longer
  the higher it is, a faint band for the part in view, and the heading
  of the part being read in the accent (a scrollspy). A press on the
  rail goes to that place, a drag scrubs through the page; the pointer
  on it brings the headings out beside it as a list, and a click on one
  goes there (a diff's files by their names alone, cut from the left
  when too long, since a path's end says most); the pointer on a tick
  marks its heading in the list, brought into view. The rail is at most
  half the view's height, in its middle. Where the page's content comes
  too near the left edge the page moves over to make room, and a view
  narrower than 420 has no rail. Only with two headings or more; it hangs off the page's
  root, so the live morph leaves it be, and is laid out again as the
  page changes.
- **A tag is what the window is, then your words.** The head is drawn
  from the window's state (DESIGN.md, *Windows are what they are*), none
  of it in the tag's text: the path, its folders dim and its name
  strong (Untitled for a new window), a wider space after it; the label
  on a chip (a terminal's title, a tool's name for its pane; an errors
  window's or a preview's kind when it has none); apex's verbs as
  icons, faint until the pointer is on the tag; a hairline. Then the
  user's words, a step stronger: all the tag's text holds.
- **The session's place in the title bar**: the host, dim, and the
  session's directory as crumbs; a crumb clicked lists the folders in it
  to cd into (`./` for the folder itself). Paths inside the directory
  are drawn in the tags from there on, unmarked (`src/main.rs`), and
  every other path whole; the directory itself is a drawn `./`.
- **Layout Animations** (View menu, on by default): windows and columns
  gliding to their places, a window opening down from its top, the
  pointer riding with the window it goes to. Off, the layout is where it
  goes at once; the overview, the stash's fan, a session sliding in, the
  scrolling and the status marks keep theirs.
- **Splitting a column by dragging**, as VS Code's and Zed's editors
  split: a window's box let go near a column's left or right edge (its
  outer eighth) makes a new column there, half the column wide, the
  window in it; the drag's shadow shows the window filling it. The
  window's own column too: at its left edge with the pointer pushed onto
  the edge itself (so a drag up or down drifting left still just moves
  it), at its right after a clear move right; never for a column's only
  window, nor a column too narrow to halve.
- **A column a drag empties goes.** Dragging its last window out (into
  another column, or out to split one) closes the column and gives its
  room to its neighbour; a column made empty (Newcol) stays.
- **⌘O: open a file or folder in the session's directory.** The host
  lists everything under it and matches what is typed there, so a huge
  tree never crosses the wire: the walk (breadth first, leaving out
  dot-directories, node_modules and their like, stopping at a million)
  streams into an index; the matcher (Zed's fuzzy scoring, across the
  cores) sends only the best page, again as the walk goes on; a newer
  query abandons an older one, a longer one narrows the last one's
  matches; scrolling near the end asks for the next page; closing it
  stops both on the host. The list fills in from the first moment.
- **Processes as pills.** What runs for the session is shown before
  the top row's text, a pill each: its name and a ×. The × ends it; B1
  on it goes to its output, B3 to the window it was run from. The
  sidebar lists them too, under the session's windows.
- **apex's verbs as icons** (`VERB_ICONS`, `Node::window_verbs`): `Del`
  (×), `Snarf` (copy), `Undo` and `Redo` (curved arrows), `Put` (into
  a tray), `Get` (reload), `Send` (a paper plane), `Back` and `Fwd`
  (chevrons). B1 or B2 on one runs it. After a window closes, the
  pointer goes to the next window's `Del`, as acme's does.
- **The path is a breadcrumb.** One click on a folder or the name
  brings a picker down under it, as VS Code's do: that folder's entries
  (the name's siblings), listed by the host as ^F's names are, so a
  remote session's too, narrowed as a query is typed -- in the tag
  itself: while the picker is down the path shows the folder listed and
  then what is typed, with its caret, where the rest of the path was;
  the names below are in the tag's face, under it. First come the windows open on the
  folder or a file in it that are not a plain file's or folder's -- its
  errors, a file's preview, a terminal or a tool's pane there -- errors
  first, then previews: the quick way to them. Return (or a
  click) opens the one chosen, a file or a folder, in a window of its
  own; ⌥return opens it here, in place of what this window shows -- a
  file in a file's window, a folder in a folder's (asking again when it
  is unsaved). → or tab (or the › at a folder's row's end) goes into a
  folder; ← or backspace with nothing typed goes up. A double-click on the
  path makes it a field (one click on Untitled): return renames the
  window, a relative path in the folder it was in. A double-click on
  the label edits it. B3 on a folder or the name plumbs the path to
  there.
- **Columns answer as windows do.** B1 on a column's box grows it a
  little. B2 maximizes it: the others minimized where they stand, as a
  maximized window leaves the others their tags, each remembering its
  width; B1 on the maximized column's box brings them all back. A column
  is minimized too when a drag on the line beside it takes it past half
  the least a column may be, as a window dragged over goes down to its
  tag, or when a neighbour's B1 squeezes it. A minimized column is a
  slim card on its side where it stands, among the others in their
  order, its outline rounded as a folded window's tag is, the column's
  grip at its top (it is the column's box, and says it is a column),
  each window's handle down it where the window stands: a click on a handle brings the
  column back and lands on that window, and B1 anywhere else on it
  brings it back where it stands.
  B3 gives a column the whole row, as it grows a window to the whole
  column: the others hidden behind it (`Layout::full`), their places
  kept, its grip framed and its dots square while they are; B3 again or
  B1 on its box gives them back where they stood, it at the width it
  had (its share of the row, `Column::restore`). Anything that changes
  the row -- a column added, closed or dragged, B2, going to a window in
  a hidden one -- gives them back first. Columns are not put away. A column an older apex put away at
  the row's right is still drawn as the edges of sheets on their sides,
  and B1 brings it back where it stood.
- **Long paths shortened** (`text_element::shortened`, `Head::elided`):
  a window's path that takes more than half its tag and is what makes the
  tag wrap is drawn from a suffix instead -- the fewest of its leading
  folders put away for the tag to fit one line, its name always kept --
  behind `⋯/`, which says so. `⋯/` is a folder like the others: the last
  one it puts away, which B1 lists and B3 plumbs. A tag of more lines
  than one (a newline in it) is long anyway and keeps its path whole, as
  does one that would not fit however short the path.
- **Looking as you type** (`look.rs`): the query is the first `Look`'s
  argument in a window's tag (every tag begins with `Look `), the result
  the window's selection, as acme's Look has them. While the caret is in
  that argument, each change looks again from where the selection was
  when the typing began: a letter more narrows there or further on, a
  letter less goes back, empty puts the selection back; nothing found,
  it stays and the argument is struck through. A click ends it. ⌘F takes
  the caret and the pointer to the argument (typing `Look ` first where
  a tag has none), selected to type over; ⌘G and ⌘⇧G look again forwards
  and back (⌘J is the next notification now). B3 on a word, found in its
  window, puts the word in the argument too. While a look goes on in a
  window -- live, or its selection one of the places -- every place the
  word is is washed faintly in B3's blue, under the selection; once the
  selection is elsewhere, the marks are gone. A tag's later `Look`s are
  words like any other.
- **A window over another** (`state::Cover`): `apex editor` run in a
  window -- a terminal, win's, a command from a tag: `$winid` -- ($EDITOR
  for git, say) opens the file over that window, in its place, rather
  than somewhere else: a window of its own, on the
  file's text if it is open elsewhere too (as Zerox makes one). The
  terminal is under it, out of the tiling and still running, and comes
  back in the place when the window over it goes -- wherever that window
  has been moved or stashed meanwhile. Its Del is drawn as an × on the
  top card of two (it closes this one only), and Swap (an arrow up and
  one down) changes the two over: the terminal on top, the file under
  it. Going to a covered window -- Look, the plumber, the finder, a
  notification taken -- brings it to the top the same way, and a
  notification on one under shows on the top's handle. Windows over one
  another are a stack: each covers the one under it, and closing one
  closes the stack over it. Only `apex editor` covers a window so;
  anything else opens as ever.
- **B4 on a box minimizes** -- shift-B1 on a laptop, which is B4
  everywhere (`tiling::colminimize`, `rowminimize`): on a
  window's box (or handle), the window down to its tag where it stands,
  its room to the window under it (over it, the last); on a column's
  box, the column a strip where it stands, its width to the nearest
  column with room, right of it first, and remembered for B1 on the
  strip to give back. B1 on a minimized window's box grows it again. A
  window alone in its column, or the last column with room, stays.
- **Menus.** The B4 tools menu (`menu.rs`, `Menu::place`) has
  menuhit's ways: up while the button is held, run on release over an
  item, the last choice remembered and opened under the pointer (which
  lands on it), a part and a lane past 25 items. It looks like a tag: a
  card of a tag's ground with its hairline and corners, lifted a little
  as a stash card is; rows a tag's lines in a tag's face and ink; the
  item under the pointer on B2's pill, as B2 on it in a tag would have
  it (the action's wash round the word, its ink), since choosing one is
  running it; no mark on the remembered one, which is where the menu
  opens; the lane down its right with a slim thumb.
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
