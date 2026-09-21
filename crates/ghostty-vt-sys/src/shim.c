// A narrow C face on libghostty-vt, for apex's terminals.
//
// Everything here could be done from Rust, but the enums, sized structs
// and tagged unions of the VT API are C's, and are read from its headers
// at compile time here rather than copied into Rust by hand -- the API
// is not stable yet, and a copy would go quietly wrong. So the shim does
// the walking of the grid and hands back what a term shard is made of:
// a cell of a character, a foreground, a background, flags and a link.

#include <stdbool.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>

#include <ghostty/vt.h>

// ---- what apex's term shard is made of -------------------------------

// A cell as `apex_core::Cell` carries it: colours are packed as apex
// packs them -- 0xff000000|rgb for an exact colour, 0xfe0000|i for one
// of the palette (the client's theme colours those), 0 for the default
// ink, and `APEX_DEFAULT_BG` for the default paper.
typedef struct {
  uint32_t ch;
  uint32_t fg;
  uint32_t bg;
  uint8_t flags;
  uint16_t link;
} ApexCell;

#define APEX_FLAG_BOLD 1
#define APEX_FLAG_ITALIC 2
#define APEX_FLAG_UNDERLINE 4

#define APEX_DEFAULT_FG 0xfd000000u
#define APEX_DEFAULT_BG 0xfd000001u

// What a program did that apex answers: bytes for the pty, a title, the
// clipboard, a bell.
#define APEX_EVENT_WRITE_PTY 1
#define APEX_EVENT_TITLE 2
#define APEX_EVENT_CLIPBOARD 3
#define APEX_EVENT_BELL 4

typedef struct ApexEvent {
  int kind;
  uint8_t *data;
  size_t len;
  struct ApexEvent *next;
} ApexEvent;

struct ApexVt {
  GhosttyTerminal term;
  GhosttyRenderState render;
  GhosttyRenderStateRowIterator rows;
  GhosttyRenderStateRowCells cells;
  ApexEvent *head;
  ApexEvent *tail;
  bool titled;
  // the links of the last snapshot, so a cell can name one by index
  char **links;
  size_t links_len;
  size_t links_cap;
};

typedef struct ApexVt ApexVt;

// ---- the events ------------------------------------------------------

static void push(ApexVt *vt, int kind, const uint8_t *data, size_t len) {
  ApexEvent *e = calloc(1, sizeof(ApexEvent));
  if (!e) return;
  e->kind = kind;
  e->len = len;
  if (len > 0) {
    e->data = malloc(len);
    if (!e->data) {
      free(e);
      return;
    }
    memcpy(e->data, data, len);
  }
  if (vt->tail) {
    vt->tail->next = e;
  } else {
    vt->head = e;
  }
  vt->tail = e;
}

static void on_write_pty(GhosttyTerminal t, void *ud, const uint8_t *data, size_t len) {
  (void)t;
  push((ApexVt *)ud, APEX_EVENT_WRITE_PTY, data, len);
}

static void on_bell(GhosttyTerminal t, void *ud) {
  (void)t;
  push((ApexVt *)ud, APEX_EVENT_BELL, NULL, 0);
}

// The title is there once the callback has returned, so it is read
// after the write that changed it rather than here.
static void on_title(GhosttyTerminal t, void *ud) {
  (void)t;
  ((ApexVt *)ud)->titled = true;
}

// OSC 52: the first text/plain content is what apex snarfs.
static void on_clipboard(GhosttyTerminal t, void *ud, const GhosttyClipboardWrite *w) {
  (void)t;
  if (!w || w->contents_len == 0) return;
  const GhosttyClipboardContent *c = &w->contents[0];
  push((ApexVt *)ud, APEX_EVENT_CLIPBOARD, c->data.ptr, c->data.len);
}

// The next event, its bytes borrowed until the call after it.
int apex_vt_next_event(ApexVt *vt, const uint8_t **out_data, size_t *out_len) {
  static _Thread_local ApexEvent *held = NULL;
  if (held) {
    free(held->data);
    free(held);
    held = NULL;
  }
  ApexEvent *e = vt->head;
  if (!e) return 0;
  vt->head = e->next;
  if (!vt->head) vt->tail = NULL;
  held = e;
  *out_data = e->data;
  *out_len = e->len;
  return e->kind;
}

// ---- the terminal ----------------------------------------------------

ApexVt *apex_vt_new(uint16_t cols, uint16_t rows, size_t scrollback) {
  ApexVt *vt = calloc(1, sizeof(ApexVt));
  if (!vt) return NULL;
  if (ghostty_terminal_new(NULL, &vt->term, cols, rows) != GHOSTTY_SUCCESS) {
    free(vt);
    return NULL;
  }
  if (ghostty_render_state_new(NULL, &vt->render) != GHOSTTY_SUCCESS ||
      ghostty_render_state_row_iterator_new(NULL, &vt->rows) != GHOSTTY_SUCCESS ||
      ghostty_render_state_row_cells_new(NULL, &vt->cells) != GHOSTTY_SUCCESS) {
    ghostty_terminal_free(vt->term);
    free(vt);
    return NULL;
  }
  size_t lines = scrollback;
  ghostty_terminal_set(vt->term, GHOSTTY_TERMINAL_OPT_SCROLLBACK_MAX_LINES, &lines);
  ghostty_terminal_set(vt->term, GHOSTTY_TERMINAL_OPT_USERDATA, vt);
  // a callback option takes the function itself, not its address
  ghostty_terminal_set(vt->term, GHOSTTY_TERMINAL_OPT_WRITE_PTY, (const void *)on_write_pty);
  ghostty_terminal_set(vt->term, GHOSTTY_TERMINAL_OPT_BELL, (const void *)on_bell);
  ghostty_terminal_set(vt->term, GHOSTTY_TERMINAL_OPT_TITLE_CHANGED, (const void *)on_title);
  ghostty_terminal_set(vt->term, GHOSTTY_TERMINAL_OPT_CLIPBOARD_WRITE, (const void *)on_clipboard);
  return vt;
}

void apex_vt_free(ApexVt *vt) {
  if (!vt) return;
  const uint8_t *d;
  size_t l;
  while (apex_vt_next_event(vt, &d, &l)) {
  }
  for (size_t i = 0; i < vt->links_len; i++) free(vt->links[i]);
  free(vt->links);
  ghostty_render_state_row_cells_free(vt->cells);
  ghostty_render_state_row_iterator_free(vt->rows);
  ghostty_render_state_free(vt->render);
  ghostty_terminal_free(vt->term);
  free(vt);
}

void apex_vt_write(ApexVt *vt, const uint8_t *data, size_t len) {
  ghostty_terminal_vt_write(vt->term, data, len);
  if (vt->titled) {
    vt->titled = false;
    GhosttyString s = {0};
    if (ghostty_terminal_get(vt->term, GHOSTTY_TERMINAL_DATA_TITLE, &s) == GHOSTTY_SUCCESS) {
      push(vt, APEX_EVENT_TITLE, s.ptr, s.len);
    }
  }
}

void apex_vt_resize(ApexVt *vt, uint16_t cols, uint16_t rows) {
  ghostty_terminal_resize(vt->term, cols, rows, 0, 0);
}

// The viewport moved: `delta` rows, up into the history when negative.
void apex_vt_scroll(ApexVt *vt, intptr_t delta) {
  GhosttyTerminalScrollViewport s = {.tag = GHOSTTY_SCROLL_VIEWPORT_DELTA, .value = {.delta = delta}};
  ghostty_terminal_scroll_viewport(vt->term, s);
}

void apex_vt_scroll_bottom(ApexVt *vt) {
  GhosttyTerminalScrollViewport s = {.tag = GHOSTTY_SCROLL_VIEWPORT_BOTTOM};
  ghostty_terminal_scroll_viewport(vt->term, s);
}

// ---- what the shard shows -------------------------------------------

static uint32_t pack_rgb(GhosttyColorRgb c) {
  return 0xff000000u | ((uint32_t)c.r << 16) | ((uint32_t)c.g << 8) | (uint32_t)c.b;
}

// A cell's colour as apex packs it: one of the sixteen by its index, so
// the client's theme colours it; anything else exactly.
static uint32_t pack_color(GhosttyStyleColor c, const GhosttyRenderStateColors *colors, uint32_t none) {
  switch (c.tag) {
    case GHOSTTY_STYLE_COLOR_RGB:
      return pack_rgb(c.value.rgb);
    case GHOSTTY_STYLE_COLOR_PALETTE:
      if (c.value.palette < 16) return 0xfe000000u | c.value.palette;
      return pack_rgb(colors->palette[c.value.palette]);
    default:
      return none;
  }
}

static uint16_t link_of(ApexVt *vt, const GhosttyGridRef *ref) {
  uint8_t buf[2048];
  size_t len = 0;
  if (ghostty_grid_ref_hyperlink_uri(ref, buf, sizeof(buf), &len) != GHOSTTY_SUCCESS || len == 0) return 0;
  for (size_t i = 0; i < vt->links_len; i++) {
    if (strlen(vt->links[i]) == len && memcmp(vt->links[i], buf, len) == 0) return (uint16_t)(i + 1);
  }
  if (vt->links_len == vt->links_cap) {
    size_t cap = vt->links_cap ? vt->links_cap * 2 : 8;
    char **grown = realloc(vt->links, cap * sizeof(char *));
    if (!grown) return 0;
    vt->links = grown;
    vt->links_cap = cap;
  }
  char *s = malloc(len + 1);
  if (!s) return 0;
  memcpy(s, buf, len);
  s[len] = 0;
  vt->links[vt->links_len++] = s;
  return (uint16_t)vt->links_len;
}

// The viewport as cells, `cells` holding cols*rows of them, with the
// cursor and the row the viewport starts at counted from the first row
// of the history. Returns 0, or -1 if the viewport does not fit.
int apex_vt_snapshot(ApexVt *vt,
                     ApexCell *cells,
                     size_t cap,
                     uint16_t *out_cols,
                     uint16_t *out_rows,
                     uint16_t *out_cursor_x,
                     uint16_t *out_cursor_y,
                     int *out_cursor_visible,
                     uint64_t *out_top,
                     uint32_t *out_links) {
  uint16_t cols = 0, rows = 0;
  ghostty_terminal_get(vt->term, GHOSTTY_TERMINAL_DATA_COLS, &cols);
  ghostty_terminal_get(vt->term, GHOSTTY_TERMINAL_DATA_ROWS, &rows);
  *out_cols = cols;
  *out_rows = rows;
  if ((size_t)cols * (size_t)rows > cap) return -1;

  for (size_t i = 0; i < vt->links_len; i++) free(vt->links[i]);
  vt->links_len = 0;

  for (size_t i = 0; i < (size_t)cols * (size_t)rows; i++) {
    cells[i] = (ApexCell){.ch = ' ', .fg = 0, .bg = 0, .flags = 0, .link = 0};
  }

  if (ghostty_render_state_update(vt->render, vt->term) != GHOSTTY_SUCCESS) return -1;

  GhosttyRenderStateColors colors = GHOSTTY_INIT_SIZED(GhosttyRenderStateColors);
  ghostty_render_state_get(vt->render, GHOSTTY_RENDER_STATE_DATA_COLORS, &colors);

  GhosttyRenderStateCursor cursor = GHOSTTY_INIT_SIZED(GhosttyRenderStateCursor);
  ghostty_render_state_get(vt->render, GHOSTTY_RENDER_STATE_DATA_CURSOR, &cursor);
  *out_cursor_x = cursor.viewport_x;
  *out_cursor_y = cursor.viewport_y;
  *out_cursor_visible = cursor.visible && cursor.viewport_has_value;

  // where the viewport sits in the whole screen, history and all
  *out_top = 0;
  GhosttyGridRef ref = GHOSTTY_INIT_SIZED(GhosttyGridRef);
  GhosttyPoint top = {.tag = GHOSTTY_POINT_TAG_VIEWPORT, .value = {.coordinate = {.x = 0, .y = 0}}};
  if (ghostty_terminal_grid_ref(vt->term, top, &ref) == GHOSTTY_SUCCESS) {
    GhosttyPointCoordinate at = {0};
    if (ghostty_terminal_point_from_grid_ref(vt->term, &ref, GHOSTTY_POINT_TAG_SCREEN, &at) == GHOSTTY_SUCCESS) {
      *out_top = at.y;
    }
  }

  if (ghostty_render_state_get(vt->render, GHOSTTY_RENDER_STATE_DATA_ROW_ITERATOR, &vt->rows) != GHOSTTY_SUCCESS) return -1;
  uint16_t y = 0;
  while (ghostty_render_state_row_iterator_next(vt->rows) && y < rows) {
    if (ghostty_render_state_row_get(vt->rows, GHOSTTY_RENDER_STATE_ROW_DATA_CELLS, &vt->cells) != GHOSTTY_SUCCESS) {
      y++;
      continue;
    }
    uint16_t x = 0;
    while (ghostty_render_state_row_cells_next(vt->cells) && x < cols) {
      ApexCell *out = &cells[(size_t)y * cols + x];
      GhosttyCell raw = 0;
      ghostty_render_state_row_cells_get(vt->cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_RAW, &raw);

      GhosttyCellWide wide = GHOSTTY_CELL_WIDE_NARROW;
      uint32_t cp = 0;
      bool has_link = false;
      if (raw != 0) {
        ghostty_cell_get(raw, GHOSTTY_CELL_DATA_WIDE, &wide);
        ghostty_cell_get(raw, GHOSTTY_CELL_DATA_CODEPOINT, &cp);
        ghostty_cell_get(raw, GHOSTTY_CELL_DATA_HAS_HYPERLINK, &has_link);
      }
      // the cell after a wide one is not a cell of its own
      if (wide == GHOSTTY_CELL_WIDE_SPACER_TAIL || wide == GHOSTTY_CELL_WIDE_SPACER_HEAD) {
        x++;
        continue;
      }

      GhosttyStyle style = GHOSTTY_INIT_SIZED(GhosttyStyle);
      ghostty_render_state_row_cells_get(vt->cells, GHOSTTY_RENDER_STATE_ROW_CELLS_DATA_STYLE, &style);

      uint32_t fg = pack_color(style.fg_color, &colors, 0);
      uint32_t bg = pack_color(style.bg_color, &colors, 0);
      if (style.inverse) {
        uint32_t b = bg == 0 ? APEX_DEFAULT_BG : bg;
        bg = fg == 0 ? APEX_DEFAULT_FG : fg;
        fg = b;
      }
      if (style.faint) fg = 0xff777777u;

      uint8_t flags = 0;
      if (style.bold) flags |= APEX_FLAG_BOLD;
      if (style.italic) flags |= APEX_FLAG_ITALIC;
      if (style.underline != GHOSTTY_SGR_UNDERLINE_NONE) flags |= APEX_FLAG_UNDERLINE;

      out->ch = (style.invisible || cp == 0) ? ' ' : cp;
      out->fg = fg;
      out->bg = bg;
      out->flags = flags;
      out->link = 0;
      if (has_link) {
        GhosttyGridRef cref = GHOSTTY_INIT_SIZED(GhosttyGridRef);
        GhosttyPoint p = {.tag = GHOSTTY_POINT_TAG_VIEWPORT, .value = {.coordinate = {.x = x, .y = y}}};
        if (ghostty_terminal_grid_ref(vt->term, p, &cref) == GHOSTTY_SUCCESS) out->link = link_of(vt, &cref);
      }
      x++;
    }
    y++;
  }
  *out_links = (uint32_t)vt->links_len;
  return 0;
}

// The URI of link `i` (one-based, as a cell names it), borrowed until
// the next snapshot.
const char *apex_vt_link(ApexVt *vt, uint32_t i) {
  if (i == 0 || i > vt->links_len) return NULL;
  return vt->links[i - 1];
}

// The colours the embedder draws with: a program asking (OSC 4, 10, 11)
// is answered from these, and a cell with no colour of its own is drawn
// in them. Each is 0xRRGGBB.
void apex_vt_set_colors(ApexVt *vt, uint32_t fg, uint32_t bg, const uint32_t *palette) {
  GhosttyColorRgb f = {.r = (uint8_t)(fg >> 16), .g = (uint8_t)(fg >> 8), .b = (uint8_t)fg};
  GhosttyColorRgb b = {.r = (uint8_t)(bg >> 16), .g = (uint8_t)(bg >> 8), .b = (uint8_t)bg};
  ghostty_terminal_set(vt->term, GHOSTTY_TERMINAL_OPT_COLOR_FOREGROUND, &f);
  ghostty_terminal_set(vt->term, GHOSTTY_TERMINAL_OPT_COLOR_BACKGROUND, &b);
  GhosttyColorRgb all[256];
  for (size_t i = 0; i < 256; i++) {
    uint32_t c;
    if (i < 16) {
      c = palette[i];
    } else if (i < 232) {
      size_t k = i - 16;
      uint8_t lv[6] = {0, 95, 135, 175, 215, 255};
      c = ((uint32_t)lv[k / 36] << 16) | ((uint32_t)lv[(k / 6) % 6] << 8) | lv[k % 6];
    } else {
      uint8_t v = (uint8_t)(8 + (i - 232) * 10);
      c = ((uint32_t)v << 16) | ((uint32_t)v << 8) | v;
    }
    all[i].r = (uint8_t)(c >> 16);
    all[i].g = (uint8_t)(c >> 8);
    all[i].b = (uint8_t)c;
  }
  ghostty_terminal_set(vt->term, GHOSTTY_TERMINAL_OPT_COLOR_PALETTE, all);
}

// ---- modes, text -----------------------------------------------------

#define APEX_MODE_APP_CURSOR 1
#define APEX_MODE_BRACKETED_PASTE 2
#define APEX_MODE_FOCUS_IN_OUT 3
#define APEX_MODE_ALT_SCREEN 4
#define APEX_MODE_MOUSE 5
#define APEX_MODE_SGR_MOUSE 6
#define APEX_MODE_ALT_SCROLL 7

static bool mode_on(ApexVt *vt, GhosttyMode m) {
  GhosttyTerminalModeConfig q = {.mode = m, .value = false};
  if (ghostty_terminal_get(vt->term, GHOSTTY_TERMINAL_DATA_MODE, &q) != GHOSTTY_SUCCESS) return false;
  return q.value;
}

bool apex_vt_mode(ApexVt *vt, int which) {
  switch (which) {
    case APEX_MODE_APP_CURSOR:
      return mode_on(vt, GHOSTTY_MODE_DECCKM);
    case APEX_MODE_BRACKETED_PASTE:
      return mode_on(vt, GHOSTTY_MODE_BRACKETED_PASTE);
    case APEX_MODE_FOCUS_IN_OUT:
      return mode_on(vt, GHOSTTY_MODE_FOCUS_EVENT);
    case APEX_MODE_ALT_SCREEN: {
      GhosttyTerminalScreen s = GHOSTTY_TERMINAL_SCREEN_PRIMARY;
      ghostty_terminal_get(vt->term, GHOSTTY_TERMINAL_DATA_ACTIVE_SCREEN, &s);
      return s == GHOSTTY_TERMINAL_SCREEN_ALTERNATE;
    }
    case APEX_MODE_MOUSE:
      return mode_on(vt, GHOSTTY_MODE_X10_MOUSE) || mode_on(vt, GHOSTTY_MODE_NORMAL_MOUSE) || mode_on(vt, GHOSTTY_MODE_BUTTON_MOUSE) ||
             mode_on(vt, GHOSTTY_MODE_ANY_MOUSE);
    case APEX_MODE_SGR_MOUSE:
      return mode_on(vt, GHOSTTY_MODE_SGR_MOUSE);
    case APEX_MODE_ALT_SCROLL:
      return mode_on(vt, GHOSTTY_MODE_ALT_SCROLL);
    default:
      return false;
  }
}

// How many rows the history holds, and how many the whole screen does.
void apex_vt_size(ApexVt *vt, uint64_t *out_scrollback, uint64_t *out_total, int *out_at_bottom) {
  size_t sb = 0, total = 0;
  bool at_bottom = true;
  ghostty_terminal_get(vt->term, GHOSTTY_TERMINAL_DATA_SCROLLBACK_ROWS, &sb);
  ghostty_terminal_get(vt->term, GHOSTTY_TERMINAL_DATA_TOTAL_ROWS, &total);
  ghostty_terminal_get(vt->term, GHOSTTY_TERMINAL_DATA_VIEWPORT_ACTIVE, &at_bottom);
  *out_scrollback = sb;
  *out_total = total;
  *out_at_bottom = at_bottom;
}

// The text between two points of the whole screen (history first), the
// end exclusive; `out` holds it, and the length is returned, or -1.
intptr_t apex_vt_text(ApexVt *vt, uint16_t x0, uint32_t y0, uint16_t x1, uint32_t y1, uint8_t *out, size_t cap) {
  GhosttyGridRef start = GHOSTTY_INIT_SIZED(GhosttyGridRef);
  GhosttyGridRef end = GHOSTTY_INIT_SIZED(GhosttyGridRef);
  GhosttyPoint p0 = {.tag = GHOSTTY_POINT_TAG_SCREEN, .value = {.coordinate = {.x = x0, .y = y0}}};
  GhosttyPoint p1 = {.tag = GHOSTTY_POINT_TAG_SCREEN, .value = {.coordinate = {.x = x1, .y = y1}}};
  if (ghostty_terminal_grid_ref(vt->term, p0, &start) != GHOSTTY_SUCCESS) return -1;
  if (ghostty_terminal_grid_ref(vt->term, p1, &end) != GHOSTTY_SUCCESS) return -1;

  GhosttySelection sel = GHOSTTY_INIT_SIZED(GhosttySelection);
  sel.start = start;
  sel.end = end;
  sel.rectangle = false;

  GhosttyTerminalSelectionFormatOptions opts = GHOSTTY_INIT_SIZED(GhosttyTerminalSelectionFormatOptions);
  opts.emit = GHOSTTY_FORMATTER_FORMAT_PLAIN;
  opts.unwrap = true;
  opts.trim = false;
  opts.selection = &sel;

  uint8_t *buf = NULL;
  size_t len = 0;
  if (ghostty_terminal_selection_format_alloc(vt->term, NULL, opts, &buf, &len) != GHOSTTY_SUCCESS) return -1;
  intptr_t got = (intptr_t)len;
  if (len > cap) {
    got = -(intptr_t)len; // say how much room it wants
  } else if (len > 0) {
    memcpy(out, buf, len);
  }
  ghostty_free(NULL, buf, len);
  return got;
}
