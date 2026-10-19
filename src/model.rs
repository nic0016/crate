use std::collections::BTreeMap;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::chunk;
use crate::store::BlobStore;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileEntry {
    pub size: u64,
    /// Modification time in nanoseconds since the Unix epoch, as seen at commit time.
    pub mtime: u64,
    pub hash: String,
    pub chunks: Vec<String>,
}

/// Snapshot of the whole project: repo-relative path (with `/`) -> file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Manifest {
    pub files: BTreeMap<String, FileEntry>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeStats {
    pub added: usize,
    pub modified: usize,
    pub deleted: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Commit {
    pub parent: Option<String>,
    pub manifest: String,
    pub author: String,
    pub timestamp: DateTime<Utc>,
    pub message: String,
    pub stats: ChangeStats,
    pub total_files: usize,
    pub total_size: u64,
}

pub fn manifest_key(id: &str) -> String {
    format!("manifests/{id}")
}

pub fn commit_key(id: &str) -> String {
    format!("commits/{id}")
}

/// Stores a value as compressed JSON under `<prefix>/<blake3>` and returns the id.
fn put_object<T: Serialize>(store: &dyn BlobStore, key_fn: fn(&str) -> String, value: &T) -> Result<String> {
    let json = serde_json::to_vec(value)?;
    let id = blake3::hash(&json).to_hex().to_string();
    store.put_if_absent(&key_fn(&id), &chunk::encode(&json)?)?;
    Ok(id)
}

fn get_object<T: DeserializeOwned>(store: &dyn BlobStore, key: &str) -> Result<T> {
    let stored = store.get(key)?.with_context(|| format!("Objekt {key} fehlt"))?;
    let json = chunk::decode(&stored)?;
    serde_json::from_slice(&json).with_context(|| format!("Objekt {key} ist ungültig"))
}

impl Manifest {
    pub fn save(&self, store: &dyn BlobStore) -> Result<String> {
        put_object(store, manifest_key, self)
    }

    pub fn load(store: &dyn BlobStore, id: &str) -> Result<Self> {
        get_object(store, &manifest_key(id))
    }

    pub fn total_size(&self) -> u64 {
        self.files.values().map(|f| f.size).sum()
    }
}

impl Commit {
    pub fn save(&self, store: &dyn BlobStore) -> Result<String> {
        put_object(store, commit_key, self)
    }

    pub fn load(store: &dyn BlobStore, id: &str) -> Result<Self> {
        get_object(store, &commit_key(id))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    Added,
    Modified,
    Deleted,
}

impl Change {
    pub fn symbol(&self) -> &'static str {
        match self {
            Change::Added => "neu      ",
            Change::Modified => "geändert ",
            Change::Deleted => "gelöscht ",
        }
    }
}

/// Compares two path->hash maps. Result is sorted by path.
pub fn diff_maps<'a>(
    old: impl IntoIterator<Item = (&'a String, &'a str)>,
    new: impl IntoIterator<Item = (&'a String, &'a str)>,
) -> Vec<(String, Change)> {
    let old: BTreeMap<&String, &str> = old.into_iter().collect();
    let new: BTreeMap<&String, &str> = new.into_iter().collect();
    let mut out = Vec::new();
    for (path, hash) in &new {
        match old.get(path) {
            None => out.push(((*path).clone(), Change::Added)),
            Some(h) if h != hash => out.push(((*path).clone(), Change::Modified)),
            _ => {}
        }
    }
    for path in old.keys() {
        if !new.contains_key(path) {
            out.push(((*path).clone(), Change::Deleted));
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

pub fn diff_manifests(old: &Manifest, new: &Manifest) -> Vec<(String, Change)> {
    diff_maps(
        old.files.iter().map(|(p, f)| (p, f.hash.as_str())),
        new.files.iter().map(|(p, f)| (p, f.hash.as_str())),
    )
}

pub fn stats_of(changes: &[(String, Change)]) -> ChangeStats {
    let mut s = ChangeStats::default();
    for (_, c) in changes {
        match c {
            Change::Added => s.added += 1,
            Change::Modified => s.modified += 1,
            Change::Deleted => s.deleted += 1,
        }
    }
    s
}
