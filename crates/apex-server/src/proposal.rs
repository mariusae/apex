//! What the server asks the leader to do. In-process the client applies
//! these immediately; over the socket they travel as messages. Either way
//! the leader lowers them into its own entries (§5 of the design).

use serde::{Deserialize, Serialize};

use apex_core::*;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Proposal {
    /// Open a window on `col` showing a file's contents (or a listing).
    OpenWindow { col: ColumnId, name: String, text: String, hash: String, select_line: Option<usize> },
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
    ReplaceRange { col: ColumnId, buffer: BufferId, version: Version, q0: usize, q1: usize, text: String },
    /// Text for the column's `+Errors`.
    Errors { col: ColumnId, text: String },
    /// The outcome of an exec.
    Status { ctx: ExecCtx, exec: Seq, status: ExecStatusOp },
    /// B3 did not name a file: search the body instead.
    Look { ctx: ExecCtx, text: String },
    /// The file on disk changed under a dirty buffer.
    Stale { buffer: BufferId, hash: String },
    // ---- from tools (the control protocol) ----
    /// As if B2 on `text` in `ctx`.
    Exec { ctx: ExecCtx, text: String },
    /// Run an Edit program on a window's body.
    Edit { window: WindowId, program: String },
    Select { view: ViewId, q0: usize, q1: usize },
}

/// Apply a proposal through the leader. Returns the window it opened or
/// searched in, if any.
pub fn apply(node: &mut Node, log: &mut Log, p: Proposal) -> Result<Option<WindowId>, CoreError> {
    match p {
        Proposal::OpenWindow { col, name, text, hash, select_line } => {
            if let Some(w) = node.state.windows.keys().copied().find(|w| node.window_name(*w) == name) {
                select(node, log, w, select_line)?;
                node.seltext = Some(ViewId::Body(w));
                return Ok(Some(w));
            }
            let b = node.create_buffer(log, &name, &text, Some(hash))?;
            let w = node.open_window(log, col, b)?;
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
        Proposal::ReplaceRange { col, buffer, version, q0, q1, text } => {
            let ok = node.state.buffer(buffer).map(|b| b.version == version).unwrap_or(false);
            if ok {
                let view = node.state.buffer(buffer)?.views.keys().next().copied();
                if let Some(v) = view {
                    node.select(log, v, q0, q1)?;
                    node.replace_selection(log, v, &text)?;
                }
            } else {
                node.errors(log, col, &format!("pipe output not applied: buffer changed meanwhile\n{text}"))?;
            }
            Ok(None)
        }
        Proposal::Errors { col, text } => {
            node.errors(log, col, &text)?;
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
        Proposal::Look { ctx, text } => {
            let win = match ctx {
                ExecCtx::Window(w) => Some(w),
                _ => node.seltext.and_then(|v| v.window()),
            };
            if let Some(w) = win {
                if node.state.window(w).ok().and_then(|x| x.body_buffer()).is_some() {
                    node.look(log, w, &text)?;
                    return Ok(Some(w));
                }
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
                let col = node.column_of(window)?;
                node.errors(log, col, &run.output)?;
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
