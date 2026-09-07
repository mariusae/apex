//! What the server asks the leader to do. In-process the client applies
//! these immediately; over the socket they travel as messages. Either way
//! the leader lowers them into its own entries (§5 of the design).

use serde::{Deserialize, Serialize};

use apex_core::*;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Proposal {
    /// Open a window showing a file's contents (or a listing): in the
    /// active column, else the column of `from`, else `col` (acme's
    /// `makenewwindow`).
    OpenWindow { col: ColumnId, from: Option<WindowId>, name: String, text: String, hash: String, select_line: Option<usize> },
    /// A window on a new, empty buffer with this name (`New path`).
    NewWindow { col: ColumnId, name: String },
    /// A window on a terminal the server created.
    TermWindow { col: ColumnId, name: String, term: TermId },
    /// Replace a buffer's content: unconditionally (`Get`), or only if the
    /// buffer is still at `version` (a watched file changed) — a buffer
    /// edited meanwhile is flagged stale instead.
    SetContent { buffer: BufferId, version: Option<Version>, text: String, hash: String },
    /// The file on disk equals the buffer at `version` (`Put`).
    Clean { buffer: BufferId, version: Version, hash: String },
    /// `Put newname`: rename the buffer and its window's tag.
    Rename { buffer: BufferId, window: WindowId, name: String },
    /// Pipe output replacing a range, valid at `version`.
    ReplaceRange { dir: Option<String>, buffer: BufferId, version: Version, q0: usize, q1: usize, text: String },
    /// Text for `dir/+Errors` (acme's errorwin), or plain `+Errors`.
    Errors { dir: Option<String>, text: String },
    /// Filename completion (acme's ^F): insert `text` at `at` in `view`,
    /// if the insertion point is still there.
    Complete { view: ViewId, at: usize, text: String },
    /// acme's waitthread: a command started; its name goes to the front of
    /// the top row.
    CommandStart { name: String },
    /// ... and leaves it when the command ends.
    CommandExit { name: String },
    /// The outcome of an exec.
    Status { ctx: ExecCtx, exec: Seq, status: ExecStatusOp },
    /// Put this text in the snarf buffer (a terminal selection's text).
    Snarf { text: String },
    /// A terminal's shell labelled its window (acme's win): the tag's
    /// first word changes.
    TermName { window: WindowId, name: String },
    /// B3 did not name a file: search the body instead.
    Look { ctx: ExecCtx, text: String },
    /// A plumbing rule asks the UI to do something (`open` a URL, say).
    /// Only a UI can; a headless leader refuses, and the server tries the
    /// next rule.
    ClientDo { verb: String, args: String },
    /// The file on disk changed under a dirty buffer.
    Stale { buffer: BufferId, hash: String },
    // ---- from tools (the control protocol) ----
    /// As if B2 on `text` in `ctx`.
    Exec { ctx: ExecCtx, text: String },
    /// Run an Edit program on a window's body.
    Edit { window: WindowId, program: String },
    Select { view: ViewId, q0: usize, q1: usize },
    /// A tool's process is behind this window (`by` its attachment), or
    /// no longer is (`None`).
    Live { window: WindowId, by: Option<AttachmentId> },
    /// Insert at an address, valid at `version`, leaving the selection
    /// alone (what a tool writing output at a point wants; `ReplaceRange`
    /// selects what it put, as a pipe's output is selected).
    Insert { buffer: BufferId, version: Version, at: usize, text: String },
}

/// Apply a proposal through the leader. Returns the window it opened or
/// searched in, if any.
pub fn apply(node: &mut Node, log: &mut Log, p: Proposal) -> Result<Option<WindowId>, CoreError> {
    match p {
        Proposal::OpenWindow { col, from, name, text, hash, select_line } => {
            if let Some(w) = node.state.windows.keys().copied().find(|w| node.window_name(*w) == name) {
                // acme's openfile: show it (a window with no lines grows a
                // few), and jump the mouse to the selection
                node.reveal(log, w)?;
                select(node, log, w, select_line)?;
                node.seltext = Some(ViewId::Body(w));
                node.warp = Some(Warp::Sel(ViewId::Body(w)));
                return Ok(Some(w));
            }
            let b = node.create_buffer(log, &name, &text, Some(hash))?;
            let w = node.make_window(log, from, col, b)?;
            select(node, log, w, select_line)?;
            node.seltext = Some(ViewId::Body(w));
            Ok(Some(w))
        }
        Proposal::NewWindow { col, name } => {
            let w = node.new_window(log, col, &name, "")?;
            node.seltext = Some(ViewId::Body(w));
            Ok(Some(w))
        }
        Proposal::TermWindow { col, name, term } => {
            node.catch_up(log)?;
            let w = node.open_term_window(log, col, &name, term)?;
            Ok(Some(w))
        }
        Proposal::SetContent { buffer, version, text, hash } => {
            let b = node.state.buffer(buffer)?;
            if version.is_some_and(|v| v != b.version) && b.dirty() {
                if !b.stale {
                    node.append(log, Shard::Buffer(buffer), Op::Buffer(BufferOp::Stale { disk_hash: hash }))?;
                }
                return Ok(None);
            }
            node.set_content(log, buffer, &text)?;
            let version = node.state.buffer(buffer)?.version;
            node.append(log, Shard::Buffer(buffer), Op::Buffer(BufferOp::Clean { version, disk_hash: Some(hash) }))?;
            Ok(None)
        }
        Proposal::Clean { buffer, version, hash } => {
            node.append(log, Shard::Buffer(buffer), Op::Buffer(BufferOp::Clean { version, disk_hash: Some(hash) }))?;
            Ok(None)
        }
        Proposal::Rename { buffer, window, name } => {
            node.append(log, Shard::Buffer(buffer), Op::Buffer(BufferOp::Rename { name: name.clone() }))?;
            let tag = node.state.window(window)?.tag;
            let rest = node.state.buffer(tag).map(|t| t.text.to_string()).unwrap_or_default();
            let rest = rest.split_once(' ').map(|(_, r)| r.to_string()).unwrap_or_default();
            node.set_content(log, tag, &format!("{name} {rest}"))?;
            Ok(None)
        }
        Proposal::ReplaceRange { dir, buffer, version, q0, q1, text } => {
            let ok = node.state.buffer(buffer).map(|b| b.version == version).unwrap_or(false);
            if ok {
                let view = node.state.buffer(buffer)?.views.keys().next().copied();
                if let Some(v) = view {
                    node.select(log, v, q0, q1)?;
                    node.replace_selection(log, v, &text)?;
                }
            } else {
                node.errors(log, dir.as_deref(), &format!("pipe output not applied: buffer changed meanwhile\n{text}"))?;
            }
            Ok(None)
        }
        Proposal::Errors { dir, text } => {
            node.errors(log, dir.as_deref(), &text)?;
            Ok(None)
        }
        Proposal::CommandStart { name } => {
            if let Some(top) = node.state.layout.top {
                node.select(log, ViewId::Top, 0, 0)?;
                node.replace_selection(log, ViewId::Top, &format!("{name} "))?;
                node.select(log, ViewId::Top, 0, 0)?;
                let _ = top;
            }
            Ok(None)
        }
        Proposal::CommandExit { name } => {
            if node.state.layout.top.is_some() && node.look(log, ViewId::Top, &format!("{name} "))? {
                node.replace_selection(log, ViewId::Top, "")?;
                node.select(log, ViewId::Top, 0, 0)?;
            }
            Ok(None)
        }
        Proposal::Complete { view, at, text } => {
            if node.selection(view)? == (at, at) {
                node.insert(log, view, &text)?;
            }
            Ok(view.window())
        }
        Proposal::TermName { window, name } => {
            let tag = node.state.window(window)?.tag;
            let rest = node.state.buffer(tag).map(|t| t.text.to_string()).unwrap_or_default();
            let rest = rest.split_once(' ').map(|(_, r)| r.to_string()).unwrap_or_default();
            node.set_content(log, tag, &format!("{name} {rest}"))?;
            Ok(None)
        }
        Proposal::Snarf { text } => {
            node.append(log, apex_core::Shard::Layout, apex_core::Op::Layout(apex_core::LayoutOp::Snarf { text }))?;
            Ok(None)
        }
        Proposal::Status { ctx, exec, status } => {
            // the window may be gone (Del); the metalog is the record then
            let alive = match ctx {
                ExecCtx::Window(w) => node.state.window(w).is_ok(),
                _ => true,
            };
            if alive {
                node.append_status(log, ctx, exec, status)?;
            }
            Ok(None)
        }
        Proposal::ClientDo { verb, .. } => Err(CoreError::Missing(format!("no client here can {verb}"))),
        Proposal::Insert { buffer, version, at, text } => {
            let b = node.state.buffer(buffer)?;
            if b.version != version {
                return Err(CoreError::Missing("buffer changed meanwhile".into()));
            }
            let at = at.min(b.text.len());
            node.insert_text(log, buffer, at, &text)?;
            Ok(None)
        }
        Proposal::Live { window, by } => {
            node.append(log, Shard::Window(window), Op::Window(WindowOp::Live { by }))?;
            Ok(None)
        }
        Proposal::Look { ctx, text } => {
            // acme's look3: the search runs in seltext, the text last
            // selected with B1, not necessarily where B3 was clicked
            let view = node
                .seltext
                .filter(|v| node.view_buffer(*v).is_ok())
                .or_else(|| match ctx {
                    ExecCtx::Window(w) => Some(ViewId::Body(w)),
                    ExecCtx::Column(c) => Some(ViewId::ColTag(c)),
                    ExecCtx::Top => Some(ViewId::Top),
                });
            if let Some(v) = view {
                if node.view_buffer(v).is_ok() && node.look(log, v, &text)? {
                    if let Some(w) = v.window() {
                        node.reveal(log, w)?; // textshow grows a window with no lines
                    }
                    node.warp = Some(Warp::Sel(v)); // acme moves the mouse to what it found
                }
                return Ok(v.window());
            }
            Ok(None)
        }
        Proposal::Stale { buffer, hash } => {
            node.append(log, Shard::Buffer(buffer), Op::Buffer(BufferOp::Stale { disk_hash: hash }))?;
            Ok(None)
        }
        Proposal::Exec { ctx, text } => {
            match node.exec(log, ctx, &text)? {
                Executed::Failed(_, reason) => Err(CoreError::Missing(reason)),
                _ => Ok(match ctx {
                    ExecCtx::Window(w) => Some(w),
                    _ => None,
                }),
            }
        }
        Proposal::Edit { window, program } => {
            let run = node.run_edit(log, window, &program)?;
            if !run.output.is_empty() {
                let dir = node.error_dir(Some(window));
                node.errors(log, dir.as_deref(), &run.output)?;
            }
            Ok(Some(window))
        }
        Proposal::Select { view, q0, q1 } => {
            node.select(log, view, q0, q1)?;
            Ok(view.window())
        }
    }
}

fn select(node: &mut Node, log: &mut Log, w: WindowId, line: Option<usize>) -> Result<(), CoreError> {
    if let Some(n) = line {
        let b = node.view_buffer(ViewId::Body(w))?;
        if let Some((s, e)) = node.state.buffer(b)?.text.line_range(n.saturating_sub(1)) {
            let len = node.state.buffer(b)?.text.len();
            node.select(log, ViewId::Body(w), s, (e + 1).min(len))?;
        }
    }
    Ok(())
}
