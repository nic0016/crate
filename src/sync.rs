//! Moves commits between two stores (`push`, `pull`, `clone`).
//!
//! Objects are content-addressed, so copying is idempotent and an interrupted
//! transfer can simply be repeated. Objects are copied bottom-up (chunks, manifests,
//! commits) and only then published with a WAL entry, so the target never shows a
//! version whose data is incomplete.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{bail, Context, Result};
use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;

use crate::chunk::{self, chunk_key};
use crate::commands::short;
use crate::model::{commit_key, diff_manifests, manifest_key, Commit, Manifest};
use crate::store::BlobStore;
use crate::wal::{self, Index, Op};

pub type History = Vec<(String, Commit)>;

/// Commits after `stop` up to and including `tip`, oldest first. `None` if `stop`
/// is not an ancestor of `tip`; `stop = None` means from the very first commit.
pub fn commits_since(store: &dyn BlobStore, tip: Option<&str>, stop: Option<&str>) -> Result<Option<History>> {
    let mut out = Vec::new();
    let mut cur = tip.map(str::to_string);
    while let Some(id) = cur {
        if stop == Some(id.as_str()) {
            out.reverse();
            return Ok(Some(out));
        }
        let c = Commit::load(store, &id)?;
        cur = c.parent.clone();
        out.push((id, c));
    }
    if stop.is_some() {
        return Ok(None);
    }
    out.reverse();
    Ok(Some(out))
}

/// Newest commit in the history of `their_tip` that is also in the history of `our_tip`.
pub fn merge_base(
    ours: &dyn BlobStore,
    our_tip: Option<&str>,
    theirs: &dyn BlobStore,
    their_tip: &str,
) -> Result<Option<String>> {
    let known: HashSet<String> =
        commits_since(ours, our_tip, None)?.unwrap_or_default().into_iter().map(|(id, _)| id).collect();
    let mut cur = Some(their_tip.to_string());
    while let Some(id) = cur {
        if known.contains(&id) {
            return Ok(Some(id));
        }
        cur = Commit::load(theirs, &id)?.parent;
    }
    Ok(None)
}

/// Paths changed by `commit` compared to its parent.
pub fn changed_paths(store: &dyn BlobStore, commit: &Commit) -> Result<Vec<String>> {
    let before = match &commit.parent {
        Some(p) => Manifest::load(store, &Commit::load(store, p)?.manifest)?,
        None => Manifest::default(),
    };
    let after = Manifest::load(store, &commit.manifest)?;
    Ok(diff_manifests(&before, &after).into_iter().map(|(p, _)| p).collect())
}

pub struct Transfer {
    pub chunks: usize,
    pub bytes: u64,
}

/// Copies everything `commits` need from `src` to `dst`, skipping what `dst` has.
pub fn copy_commits(src: &dyn BlobStore, dst: &dyn BlobStore, commits: &[(String, Commit)]) -> Result<Transfer> {
    let manifests: Vec<(String, Manifest)> = commits
        .iter()
        .map(|(_, c)| Ok((c.manifest.clone(), Manifest::load(src, &c.manifest)?)))
        .collect::<Result<_>>()?;
    let wanted: BTreeSet<&str> =
        manifests.iter().flat_map(|(_, m)| m.files.values()).flat_map(|f| f.chunks.iter().map(String::as_str)).collect();
    let wanted: Vec<&str> = wanted.into_iter().collect();
    let present: Vec<bool> = wanted.par_iter().map(|h| dst.exists(&chunk_key(h))).collect::<Result<_>>()?;
    let missing: Vec<&str> = wanted.iter().zip(present).filter(|(_, p)| !p).map(|(h, _)| *h).collect();

    let bar = ProgressBar::new(missing.len() as u64);
    bar.set_style(ProgressStyle::with_template("{msg} [{bar:30}] {pos}/{len} ({eta})").unwrap().progress_chars("=> "));
    bar.set_message("Übertrage Datenblöcke");
    let bytes = AtomicU64::new(0);
    missing.par_iter().try_for_each(|hash| -> Result<()> {
        let key = chunk_key(hash);
        let data = src.get(&key)?.with_context(|| format!("Chunk {hash} fehlt in der Quelle"))?;
        if blake3::hash(&chunk::decode(&data)?).to_hex().as_str() != *hash {
            bail!("Chunk {hash} ist in der Quelle beschädigt");
        }
        if dst.put_if_absent(&key, &data)? {
            bytes.fetch_add(data.len() as u64, Ordering::Relaxed);
        }
        bar.inc(1);
        Ok(())
    })?;
    bar.finish_and_clear();

    for (id, _) in &manifests {
        copy_object(src, dst, &manifest_key(id))?;
    }
    for (id, _) in commits {
        copy_object(src, dst, &commit_key(id))?;
    }
    Ok(Transfer { chunks: missing.len(), bytes: bytes.into_inner() })
}

fn copy_object(src: &dyn BlobStore, dst: &dyn BlobStore, key: &str) -> Result<()> {
    if !dst.exists(key)? {
        let data = src.get(key)?.with_context(|| format!("Objekt {key} fehlt in der Quelle"))?;
        dst.put_if_absent(key, &data)?;
    }
    Ok(())
}

/// Appends a `Commit` entry for each of `commits` (id, parent; oldest first). Fails
/// with `conflict` if the head of `store` is no longer `expected`. `check` sees the
/// index and the position of the commit and can veto it.
pub fn publish(
    store: &dyn BlobStore,
    author: &str,
    mut expected: Option<String>,
    commits: &[(String, Option<String>)],
    conflict: &str,
    check: impl Fn(&Index, usize) -> Result<()>,
) -> Result<()> {
    for (i, (id, parent)) in commits.iter().enumerate() {
        wal::append(store, author, |idx| {
            if idx.head != expected {
                bail!("{conflict}");
            }
            check(idx, i)?;
            Ok(Op::Commit { id: id.clone(), parent: parent.clone() })
        })?;
        expected = Some(id.clone());
    }
    Ok(())
}

/// Adds tags that `dst` does not have yet, if `dst` has their commit. Tags that point
/// somewhere else in `dst` are never moved. `where_` reads like "auf origin" / "lokal".
pub fn copy_tags(tags: &BTreeMap<String, String>, dst: &dyn BlobStore, author: &str, where_: &str) -> Result<usize> {
    let existing = wal::load_index(dst)?.0.tags;
    let mut copied = 0;
    for (name, id) in tags {
        if let Some(other) = existing.get(name) {
            if other != id {
                println!("Hinweis: Tag {name} zeigt {where_} auf {}, hier auf {} – nicht geändert.", short(other), short(id));
            }
            continue;
        }
        if !dst.exists(&commit_key(id))? {
            println!("Hinweis: Tag {name} übersprungen, Version {} fehlt {where_}.", short(id));
            continue;
        }
        wal::append(dst, author, |idx| {
            if idx.tags.contains_key(name) {
                bail!("Tag {name} wurde gleichzeitig {where_} angelegt");
            }
            Ok(Op::Tag { name: name.clone(), commit: id.clone() })
        })?;
        copied += 1;
    }
    Ok(copied)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ChangeStats;
    use crate::store::LocalFsStore;
    use chrono::Utc;

    fn commit(store: &dyn BlobStore, parent: Option<&str>, msg: &str) -> String {
        let manifest = Manifest::default().save(store).unwrap();
        Commit {
            parent: parent.map(str::to_string),
            manifest,
            author: "t".into(),
            timestamp: Utc::now(),
            message: msg.into(),
            stats: ChangeStats::default(),
            total_files: 0,
            total_size: 0,
        }
        .save(store)
        .unwrap()
    }

    #[test]
    fn history_and_merge_base() {
        let dir = tempfile::tempdir().unwrap();
        let a = LocalFsStore::open(dir.path().join("a")).unwrap();
        let b = LocalFsStore::open(dir.path().join("b")).unwrap();
        let c1 = commit(&a, None, "1");
        let c2 = commit(&a, Some(&c1), "2");
        let c3 = commit(&a, Some(&c2), "3");

        let all = commits_since(&a, Some(&c3), None).unwrap().unwrap();
        assert_eq!(all.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(), vec![&c1, &c2, &c3]);
        assert_eq!(commits_since(&a, Some(&c3), Some(&c1)).unwrap().unwrap().len(), 2);
        assert!(commits_since(&a, Some(&c3), Some(&c3)).unwrap().unwrap().is_empty());
        assert!(commits_since(&a, Some(&c2), Some("nope")).unwrap().is_none());

        copy_commits(&a, &b, &all[..2]).unwrap();
        let x = commit(&b, Some(&c2), "x");
        assert_eq!(merge_base(&a, Some(&c3), &b, &x).unwrap().as_deref(), Some(c2.as_str()));
    }
}
