//! Write-ahead log and index, following the design of Cursor's Continuity.
//!
//! Every state change of the repository (commit, tag, lock) is an immutable WAL
//! entry `wal/<seq>`. The entry only becomes visible once the small `index` object
//! is moved from sequence `n` to `n + 1` with a compare-and-swap. Concurrent
//! writers race on that single conditional write; losers re-read and retry. The
//! index is therefore a materialized view that can always be rebuilt from the WAL.

use std::collections::BTreeMap;

use anyhow::{bail, Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::store::{BlobStore, Etag};

pub const INDEX_KEY: &str = "index";
const MAX_CAS_RETRIES: usize = 100;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LockInfo {
    pub owner: String,
    pub since: DateTime<Utc>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Op {
    Commit { id: String, parent: Option<String> },
    Tag { name: String, commit: String },
    DeleteTag { name: String },
    Lock { path: String, info: LockInfo },
    Unlock { path: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalEntry {
    pub seq: u64,
    pub timestamp: DateTime<Utc>,
    pub author: String,
    #[serde(flatten)]
    pub op: Op,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Index {
    pub seq: u64,
    pub head: Option<String>,
    pub tags: BTreeMap<String, String>,
    pub locks: BTreeMap<String, LockInfo>,
}

impl Index {
    pub fn apply(&mut self, entry: &WalEntry) {
        self.seq = entry.seq;
        match &entry.op {
            Op::Commit { id, .. } => self.head = Some(id.clone()),
            Op::Tag { name, commit } => {
                self.tags.insert(name.clone(), commit.clone());
            }
            Op::DeleteTag { name } => {
                self.tags.remove(name);
            }
            Op::Lock { path, info } => {
                self.locks.insert(path.clone(), info.clone());
            }
            Op::Unlock { path } => {
                self.locks.remove(path);
            }
        }
    }
}

pub fn wal_key(seq: u64) -> String {
    format!("wal/{seq:020}")
}

pub fn load_index(store: &dyn BlobStore) -> Result<(Index, Option<Etag>)> {
    match store.get_versioned(INDEX_KEY)? {
        Some((data, etag)) => Ok((serde_json::from_slice(&data).context("Index ist ungültig")?, Some(etag))),
        None => Ok((Index::default(), None)),
    }
}

pub fn load_entry(store: &dyn BlobStore, seq: u64) -> Result<Option<WalEntry>> {
    store
        .get(&wal_key(seq))?
        .map(|d| serde_json::from_slice(&d).with_context(|| format!("WAL-Eintrag {seq} ist ungültig")))
        .transpose()
}

/// Appends one operation. `decide` sees the latest index and either returns the op
/// to append or an error (e.g. "file is locked by someone else"). It may be called
/// several times if other writers win the race.
pub fn append(
    store: &dyn BlobStore,
    author: &str,
    mut decide: impl FnMut(&Index) -> Result<Op>,
) -> Result<(WalEntry, Index)> {
    for _ in 0..MAX_CAS_RETRIES {
        let (mut index, etag) = load_index(store)?;
        let op = decide(&index)?;
        let entry = WalEntry { seq: index.seq + 1, timestamp: Utc::now(), author: author.to_string(), op };

        // The entry is durable before it is published. If the slot is taken, another
        // writer is mid-publish (or crashed before publishing): only the index decides.
        let bytes = serde_json::to_vec_pretty(&entry)?;
        if !store.put_if_absent(&wal_key(entry.seq), &bytes)? {
            let existing = store.get(&wal_key(entry.seq))?;
            if existing.as_deref() != Some(bytes.as_slice()) {
                repair_orphan(store, &index, etag.as_ref())?;
                continue;
            }
        }

        index.apply(&entry);
        if store.cas_put(INDEX_KEY, etag.as_ref(), &serde_json::to_vec_pretty(&index)?)? {
            return Ok((entry, index));
        }
    }
    bail!("Index konnte nach {MAX_CAS_RETRIES} Versuchen nicht aktualisiert werden (zu viele gleichzeitige Schreiber)")
}

/// A WAL slot `seq + 1` exists but the index still points at `seq`. Either a writer is
/// about to publish it, or it crashed in between. Unpublished entries are discarded
/// once the index has not moved for a while, mirroring how a lost CAS race is handled.
fn repair_orphan(store: &dyn BlobStore, index: &Index, etag: Option<&Etag>) -> Result<()> {
    std::thread::sleep(std::time::Duration::from_millis(50));
    let key = wal_key(index.seq + 1);
    let (_, now) = load_index(store)?;
    if now.as_ref() != etag {
        return Ok(());
    }
    let age = store
        .list(&key)?
        .first()
        .and_then(|o| o.modified.elapsed().ok())
        .unwrap_or_default();
    if age > std::time::Duration::from_secs(30) {
        store.delete(&key)?;
    }
    Ok(())
}

/// Rebuilds the index by replaying the WAL from the beginning.
pub fn replay(store: &dyn BlobStore, upto: u64) -> Result<Index> {
    let mut index = Index::default();
    for seq in 1..=upto {
        let entry = load_entry(store, seq)?.with_context(|| format!("WAL-Eintrag {seq} fehlt"))?;
        if entry.seq != seq {
            bail!("WAL-Eintrag {seq} trägt die Nummer {}", entry.seq);
        }
        index.apply(&entry);
    }
    Ok(index)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::LocalFsStore;
    use std::sync::Arc;

    #[test]
    fn append_and_replay() {
        let dir = tempfile::tempdir().unwrap();
        let s = LocalFsStore::open(dir.path()).unwrap();
        append(&s, "a", |_| Ok(Op::Commit { id: "c1".into(), parent: None })).unwrap();
        append(&s, "a", |i| Ok(Op::Commit { id: "c2".into(), parent: i.head.clone() })).unwrap();
        append(&s, "a", |_| Ok(Op::Tag { name: "Rev-A".into(), commit: "c1".into() })).unwrap();
        let (index, _) = load_index(&s).unwrap();
        assert_eq!(index.seq, 3);
        assert_eq!(index.head.as_deref(), Some("c2"));
        assert_eq!(replay(&s, index.seq).unwrap(), index);
    }

    #[test]
    fn concurrent_appends_are_linearized() {
        let dir = tempfile::tempdir().unwrap();
        let s = Arc::new(LocalFsStore::open(dir.path()).unwrap());
        let handles: Vec<_> = (0..6)
            .map(|t| {
                let s = s.clone();
                std::thread::spawn(move || {
                    for i in 0..10 {
                        append(s.as_ref(), "w", |idx| {
                            Ok(Op::Commit { id: format!("t{t}-{i}"), parent: idx.head.clone() })
                        })
                        .unwrap();
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        let (index, _) = load_index(s.as_ref()).unwrap();
        assert_eq!(index.seq, 60);
        // Every entry's parent is the previous entry's commit: a single linear history.
        let mut prev = None;
        for seq in 1..=60 {
            let e = load_entry(s.as_ref(), seq).unwrap().unwrap();
            let Op::Commit { id, parent } = e.op else { panic!() };
            assert_eq!(parent, prev);
            prev = Some(id);
        }
    }
}
