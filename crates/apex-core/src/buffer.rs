//! A buffer (acme's `File`): text, version, undo history, and the views
//! (selections and origins) of the windows showing it. Views live here,
//! sequenced with the edits that move them, so every replica agrees.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::ids::*;
use crate::text::Text;

/// One window's selection and scroll origin on a buffer (acme's `Text`).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct View {
    pub q0: usize,
    pub q1: usize,
    pub origin: usize,
}

/// One edit as recorded for undo: what was there and what replaced it.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Rec {
    pub q0: usize,
    pub deleted: String,
    pub inserted: String,
}

impl Rec {
    fn deleted_len(&self) -> usize {
        self.deleted.chars().count()
    }
    fn inserted_len(&self) -> usize {
        self.inserted.chars().count()
    }
}

/// The edits of one command or run of typing, undone together.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Group {
    pub id: GroupId,
    pub recs: Vec<Rec>,
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Buffer {
    pub id: BufferId,
    pub name: String,
    pub text: Text,
    /// Number of modifying entries applied.
    pub version: Version,
    /// Version at the last load or Put.
    pub clean_version: Version,
    pub disk_hash: Option<String>,
    /// The file on disk changed underneath while the buffer was dirty.
    pub stale: bool,
    pub views: BTreeMap<ViewId, View>,
    pub undo: Vec<Group>,
    pub redo: Vec<Group>,
}

/// Bound on undo history, in groups.
pub const MAX_UNDO: usize = 1000;

impl Buffer {
    pub fn new(id: BufferId, name: &str, text: &str, disk_hash: Option<String>) -> Buffer {
        Buffer {
            id,
            name: name.to_string(),
            text: Text::new(text),
            version: 0,
            clean_version: 0,
            disk_hash,
            stale: false,
            views: BTreeMap::new(),
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    pub fn dirty(&self) -> bool {
        self.version != self.clean_version
    }

    /// Apply a concrete edit and record it under `group`.
    pub fn edit(&mut self, q0: usize, nd: usize, text: &str, group: GroupId) {
        let rec = self.splice(q0, nd, text);
        match self.undo.last_mut() {
            Some(g) if g.id == group => g.recs.push(rec),
            _ => {
                self.undo.push(Group { id: group, recs: vec![rec] });
                if self.undo.len() > MAX_UNDO {
                    self.undo.remove(0);
                }
            }
        }
        self.redo.clear();
        self.version += 1;
    }

    /// Splice the text and adjust every view; returns the record.
    fn splice(&mut self, q0: usize, nd: usize, text: &str) -> Rec {
        let q0 = q0.min(self.text.len());
        let nd = nd.min(self.text.len() - q0);
        let deleted = self.text.slice(q0, q0 + nd);
        self.text.replace(q0, nd, text);
        let ni = text.chars().count();
        self.adjust_views(q0, nd, ni);
        Rec { q0, deleted, inserted: text.to_string() }
    }

    /// acme's `textdelete` then `textinsert` rules, for every view.
    fn adjust_views(&mut self, q0: usize, nd: usize, ni: usize) {
        for v in self.views.values_mut() {
            if nd > 0 {
                if q0 < v.q0 {
                    v.q0 -= nd.min(v.q0 - q0);
                }
                if q0 < v.q1 {
                    v.q1 -= nd.min(v.q1 - q0);
                }
                if q0 + nd <= v.origin {
                    v.origin -= nd;
                } else if q0 < v.origin {
                    v.origin = q0;
                }
            }
            if ni > 0 {
                if q0 < v.q1 {
                    v.q1 += ni;
                }
                if q0 < v.q0 {
                    v.q0 += ni;
                }
                if q0 < v.origin {
                    v.origin += ni;
                }
            }
        }
    }

    /// Undo the most recent group. Returns the range of the last record
    /// restored (acme selects that) so the leader can select it, or `None`
    /// if there was nothing to undo.
    pub fn undo(&mut self) -> Option<(usize, usize)> {
        let g = self.undo.pop()?;
        let mut range = (0, 0);
        for rec in g.recs.iter().rev() {
            self.text.replace(rec.q0, rec.inserted_len(), &rec.deleted);
            self.adjust_views(rec.q0, rec.inserted_len(), rec.deleted_len());
            range = (rec.q0, rec.q0 + rec.deleted_len());
        }
        self.redo.push(g);
        self.version += 1;
        Some(range)
    }

    pub fn redo(&mut self) -> Option<(usize, usize)> {
        let g = self.redo.pop()?;
        let mut range = (0, 0);
        for rec in &g.recs {
            self.text.replace(rec.q0, rec.deleted_len(), &rec.inserted);
            self.adjust_views(rec.q0, rec.deleted_len(), rec.inserted_len());
            range = (rec.q0, rec.q0 + rec.inserted_len());
        }
        self.undo.push(g);
        self.version += 1;
        Some(range)
    }

    pub fn view(&self, id: ViewId) -> View {
        self.views.get(&id).copied().unwrap_or_default()
    }

    pub fn set_select(&mut self, id: ViewId, q0: usize, q1: usize) {
        let n = self.text.len();
        let v = self.views.entry(id).or_default();
        v.q0 = q0.min(n);
        v.q1 = q1.min(n).max(v.q0);
    }

    pub fn set_origin(&mut self, id: ViewId, origin: usize) {
        let n = self.text.len();
        self.views.entry(id).or_default().origin = origin.min(n);
    }

    pub fn hash_into(&self, h: &mut blake3::Hasher) {
        h.update(&self.id.0.to_le_bytes());
        h.update(self.name.as_bytes());
        h.update(&[0]);
        self.text.hash_into(h);
        h.update(&self.version.to_le_bytes());
        h.update(&self.clean_version.to_le_bytes());
        h.update(&[self.stale as u8]);
        if let Some(d) = &self.disk_hash {
            h.update(d.as_bytes());
        }
        for (id, v) in &self.views {
            h.update(format!("{id}").as_bytes());
            h.update(&v.q0.to_le_bytes());
            h.update(&v.q1.to_le_bytes());
            h.update(&v.origin.to_le_bytes());
        }
        for stack in [&self.undo, &self.redo] {
            h.update(&(stack.len() as u64).to_le_bytes());
            for g in stack {
                h.update(&g.id.0.to_le_bytes());
                for r in &g.recs {
                    h.update(&r.q0.to_le_bytes());
                    h.update(r.deleted.as_bytes());
                    h.update(&[0]);
                    h.update(r.inserted.as_bytes());
                    h.update(&[0]);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buf(s: &str) -> Buffer {
        Buffer::new(BufferId(1), "x", s, None)
    }

    #[test]
    fn views_follow_edits_like_acme() {
        let mut b = buf("hello world");
        let v = ViewId::Body(WindowId(1));
        b.set_select(v, 6, 11); // "world"
        b.edit(0, 0, "XX", GroupId(1)); // insert before
        assert_eq!(b.view(v), View { q0: 8, q1: 13, origin: 0 });
        b.edit(0, 2, "", GroupId(2)); // delete it again
        assert_eq!(b.view(v), View { q0: 6, q1: 11, origin: 0 });
        b.edit(11, 0, "!", GroupId(3)); // insert after: unchanged
        assert_eq!(b.view(v), View { q0: 6, q1: 11, origin: 0 });
        b.edit(8, 2, "", GroupId(4)); // delete inside: shrinks
        assert_eq!(b.view(v), View { q0: 6, q1: 9, origin: 0 });
        // insertion exactly at q0 does not move it (strict <), as in acme
        b.set_select(v, 3, 3);
        b.edit(3, 0, "abc", GroupId(5));
        assert_eq!(b.view(v), View { q0: 3, q1: 3, origin: 0 });
    }

    #[test]
    fn undo_redo_round_trip() {
        let mut b = buf("abc");
        b.edit(1, 1, "XY", GroupId(1));
        b.edit(3, 0, "Z", GroupId(1)); // same group
        assert_eq!(b.text.to_string(), "aXYZc");
        assert_eq!(b.undo(), Some((1, 2)));
        assert_eq!(b.text.to_string(), "abc");
        assert_eq!(b.redo(), Some((3, 4))); // the last record redone
        assert_eq!(b.text.to_string(), "aXYZc");
        assert_eq!(b.version, 4);
        assert!(b.dirty());
    }

    #[test]
    fn origin_tracks_deletions() {
        let mut b = buf("0123456789");
        let v = ViewId::Body(WindowId(1));
        b.set_origin(v, 5);
        b.edit(0, 2, "", GroupId(1));
        assert_eq!(b.view(v).origin, 3);
        b.edit(2, 5, "", GroupId(2)); // deletion spans the origin
        assert_eq!(b.view(v).origin, 2);
    }
}
