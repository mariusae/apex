//! The log store: one in-memory log per shard, plus the lease table that
//! fences appends. This is the server's authority; the metalog entries it
//! appends are how lease decisions become visible to everyone.

use std::collections::BTreeMap;

use crate::entry::*;
use crate::ids::*;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LogError {
    #[error("{shard}: fenced: lease is held by {holder} at epoch {epoch}")]
    Fenced { shard: Shard, holder: AttachmentId, epoch: Epoch },
    #[error("{0}: no such shard")]
    NoShard(Shard),
    #[error("{0}: shard already exists")]
    ShardExists(Shard),
    #[error("{shard}: op does not fit")]
    WrongShard { shard: Shard },
    #[error("{shard}: pinned; its lease cannot move")]
    Pinned { shard: Shard },
    #[error("{shard}: lease not released by {holder}")]
    NotReleased { shard: Shard, holder: AttachmentId },
}

#[derive(Debug, Clone, Default)]
struct ShardLog {
    /// `entries[i].seq == base + 1 + i`.
    entries: Vec<Entry>,
    base: Seq,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LeaseState {
    pub holder: AttachmentId,
    pub epoch: Epoch,
    pub released: Option<Seq>,
}

/// The in-memory log store of one session.
#[derive(Debug, Clone)]
pub struct Log {
    shards: BTreeMap<Shard, ShardLog>,
    leases: BTreeMap<Shard, LeaseState>,
    next_attachment: u64,
}

impl Default for Log {
    fn default() -> Self {
        Self::new()
    }
}

impl Log {
    /// A new session's store, with its metalog initialised.
    pub fn new() -> Log {
        let mut log = Log { shards: BTreeMap::new(), leases: BTreeMap::new(), next_attachment: 1 };
        log.shards.insert(Shard::Meta, ShardLog::default());
        log.leases.insert(Shard::Meta, LeaseState { holder: SERVER, epoch: 0, released: None });
        log.push_meta(MetaOp::Init);
        log
    }

    fn push_meta(&mut self, op: MetaOp) -> Entry {
        let sl = self.shards.get_mut(&Shard::Meta).expect("meta exists");
        let seq = sl.base + sl.entries.len() as Seq + 1;
        let e = Entry { seq, attachment: SERVER, epoch: 0, op: Op::Meta(op) };
        sl.entries.push(e.clone());
        e
    }

    pub fn shards(&self) -> impl Iterator<Item = Shard> + '_ {
        self.shards.keys().copied()
    }

    pub fn has(&self, shard: Shard) -> bool {
        self.shards.contains_key(&shard)
    }

    pub fn lease(&self, shard: Shard) -> Option<LeaseState> {
        self.leases.get(&shard).copied()
    }

    pub fn last_seq(&self, shard: Shard) -> Seq {
        self.shards.get(&shard).map(|s| s.base + s.entries.len() as Seq).unwrap_or(0)
    }

    /// Entries of `shard` with sequence greater than `after`.
    pub fn since(&self, shard: Shard, after: Seq) -> &[Entry] {
        match self.shards.get(&shard) {
            Some(s) => {
                let skip = after.saturating_sub(s.base) as usize;
                &s.entries[skip.min(s.entries.len())..]
            }
            None => &[],
        }
    }

    /// Append an entry as `attachment` at `epoch`; fenced against the lease.
    pub fn append(&mut self, shard: Shard, attachment: AttachmentId, epoch: Epoch, op: Op) -> Result<Entry, LogError> {
        if !op.fits(shard) {
            return Err(LogError::WrongShard { shard });
        }
        let lease = self.leases.get(&shard).copied().ok_or(LogError::NoShard(shard))?;
        if lease.holder != attachment || lease.epoch != epoch || lease.released.is_some() {
            return Err(LogError::Fenced { shard, holder: lease.holder, epoch: lease.epoch });
        }
        let sl = self.shards.get_mut(&shard).ok_or(LogError::NoShard(shard))?;
        let seq = sl.base + sl.entries.len() as Seq + 1;
        let e = Entry { seq, attachment, epoch, op };
        sl.entries.push(e.clone());
        Ok(e)
    }

    /// Create a shard. Leasable shards go to their creator at epoch 1;
    /// pinned shards stay with the server. Returns the metalog entries.
    pub fn create_shard(&mut self, shard: Shard, creator: AttachmentId) -> Result<Vec<Entry>, LogError> {
        if self.shards.contains_key(&shard) {
            return Err(LogError::ShardExists(shard));
        }
        self.shards.insert(shard, ShardLog::default());
        self.leases.insert(shard, LeaseState { holder: SERVER, epoch: 0, released: None });
        let mut out = vec![self.push_meta(MetaOp::ShardNew { shard })];
        if !shard.is_pinned() && creator != SERVER {
            out.push(self.grant_unchecked(shard, creator, 0));
        }
        Ok(out)
    }

    pub fn delete_shard(&mut self, shard: Shard) -> Result<Entry, LogError> {
        if shard == Shard::Meta {
            return Err(LogError::Pinned { shard });
        }
        self.shards.remove(&shard).ok_or(LogError::NoShard(shard))?;
        self.leases.remove(&shard);
        Ok(self.push_meta(MetaOp::ShardDel { shard }))
    }

    /// Register an attachment.
    pub fn attach(&mut self, kind: AttachmentKind, name: &str) -> (AttachmentId, Entry) {
        let id = AttachmentId(self.next_attachment);
        self.next_attachment += 1;
        let e = self.push_meta(MetaOp::Attach { attachment: id, kind, name: name.to_string() });
        (id, e)
    }

    pub fn detach(&mut self, attachment: AttachmentId) -> Entry {
        self.push_meta(MetaOp::Detach { attachment })
    }

    /// Ask the holder of `shard` to hand over to `to`.
    pub fn request(&mut self, shard: Shard, to: AttachmentId) -> Result<Entry, LogError> {
        if shard.is_pinned() {
            return Err(LogError::Pinned { shard });
        }
        if !self.leases.contains_key(&shard) {
            return Err(LogError::NoShard(shard));
        }
        Ok(self.push_meta(MetaOp::LeaseRequest { shard, to }))
    }

    /// The holder has flushed everything up to `seq` and lets go.
    pub fn release(&mut self, shard: Shard, from: AttachmentId, seq: Seq) -> Result<Entry, LogError> {
        let l = self.leases.get_mut(&shard).ok_or(LogError::NoShard(shard))?;
        if l.holder != from {
            return Err(LogError::Fenced { shard, holder: l.holder, epoch: l.epoch });
        }
        l.released = Some(seq);
        Ok(self.push_meta(MetaOp::LeaseRelease { shard, from, seq }))
    }

    /// Grant the lease to `to`. Allowed when the server holds it or the
    /// holder has released; otherwise the holder must release or be
    /// reclaimed first.
    pub fn grant(&mut self, shard: Shard, to: AttachmentId) -> Result<Entry, LogError> {
        if shard.is_pinned() {
            return Err(LogError::Pinned { shard });
        }
        let l = self.leases.get(&shard).copied().ok_or(LogError::NoShard(shard))?;
        if l.holder != SERVER && l.released.is_none() {
            return Err(LogError::NotReleased { shard, holder: l.holder });
        }
        Ok(self.grant_unchecked(shard, to, l.epoch))
    }

    fn grant_unchecked(&mut self, shard: Shard, to: AttachmentId, epoch: Epoch) -> Entry {
        let seq = self.last_seq(shard);
        let epoch = epoch + 1;
        self.leases.insert(shard, LeaseState { holder: to, epoch, released: None });
        self.push_meta(MetaOp::LeaseGrant { shard, to, epoch, seq })
    }

    /// Take the lease back from a holder that did not answer. The old epoch
    /// is dead from here; the shard is the server's until granted again.
    pub fn reclaim(&mut self, shard: Shard) -> Result<Entry, LogError> {
        if shard.is_pinned() {
            return Err(LogError::Pinned { shard });
        }
        let l = self.leases.get(&shard).copied().ok_or(LogError::NoShard(shard))?;
        let seq = self.last_seq(shard);
        let epoch = l.epoch + 1;
        self.leases.insert(shard, LeaseState { holder: SERVER, epoch, released: None });
        Ok(self.push_meta(MetaOp::LeaseReclaim { shard, from: l.holder, epoch, seq }))
    }

    /// Forget entries up to and including `upto` (a snapshot covers them).
    pub fn compact(&mut self, shard: Shard, upto: Seq) {
        if let Some(sl) = self.shards.get_mut(&shard) {
            if upto > sl.base {
                let n = ((upto - sl.base) as usize).min(sl.entries.len());
                sl.entries.drain(..n);
                sl.base += n as Seq;
            }
        }
    }

    /// Total entries held, for tests and metrics.
    pub fn len(&self) -> usize {
        self.shards.values().map(|s| s.entries.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
