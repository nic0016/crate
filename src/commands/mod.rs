pub mod checkout;
pub mod clone;
pub mod commit;
pub mod diff;
pub mod gc;
pub mod init;
pub mod lock;
pub mod log;
pub mod pull;
pub mod push;
pub mod remote;
pub mod restore;
pub mod status;
pub mod tag;
pub mod verify;

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::BufWriter;
use std::path::Path;

use anyhow::{Context, Result};

use crate::cad::{human_delta, human_size, Category};
use crate::chunk;
use crate::model::{diff_manifests, diff_maps, Change, FileEntry, Manifest};
use crate::repo::Repo;
use crate::scan::{self, CacheEntry, StatCache, WorkFile};

pub fn short(id: &str) -> &str {
    &id[..id.len().min(12)]
}

/// Working directory compared against the HEAD manifest.
pub struct WorkState {
    pub files: Vec<WorkFile>,
    pub hashes: BTreeMap<String, String>,
    pub cache: StatCache,
    pub head: Manifest,
    pub changes: Vec<(String, Change)>,
}

impl WorkState {
    pub fn collect(repo: &Repo) -> Result<Self> {
        let files = scan::scan(&repo.root)?;
        let mut cache = repo.load_cache();
        let hashes = scan::hash_all(&files, &mut cache)?;
        let head = repo.head_manifest()?;
        let changes = diff_maps(
            head.files.iter().map(|(p, f)| (p, f.hash.as_str())),
            hashes.iter().map(|(p, h)| (p, h.as_str())),
        );
        Ok(Self { files, hashes, cache, head, changes })
    }

    pub fn size_of(&self, path: &str) -> Option<u64> {
        self.cache.files.get(path).map(|c| c.size)
    }
}

/// Prints a change list grouped by CAD category, with size deltas.
pub fn print_changes(changes: &[(String, Change)], old_size: impl Fn(&str) -> Option<u64>, new_size: impl Fn(&str) -> Option<u64>) {
    let mut by_cat: BTreeMap<Category, Vec<&(String, Change)>> = BTreeMap::new();
    for c in changes {
        by_cat.entry(Category::of(&c.0)).or_default().push(c);
    }
    for (cat, items) in by_cat {
        println!("  {} ({}):", cat.label(), items.len());
        for (path, change) in items {
            let size = match change {
                Change::Added => format!("{}", human_size(new_size(path).unwrap_or(0))),
                Change::Deleted => format!("{}", human_size(old_size(path).unwrap_or(0))),
                Change::Modified => human_delta(old_size(path).unwrap_or(0), new_size(path).unwrap_or(0)),
            };
            println!("    {} {path}  ({size})", change.symbol());
        }
    }
}

/// Writes a stored file atomically to `dest` and returns its new cache entry.
pub fn materialize(repo: &Repo, entry: &FileEntry, dest: &Path) -> Result<CacheEntry> {
    if let Some(dir) = dest.parent() {
        fs::create_dir_all(dir)?;
    }
    let file_name = dest.file_name().context("ungültiger Zielpfad")?.to_string_lossy();
    let tmp = dest.with_file_name(format!(".{file_name}.crate-tmp"));
    {
        let mut out = BufWriter::new(File::create(&tmp).with_context(|| format!("{} schreiben", tmp.display()))?);
        chunk::restore_to(repo.store(), &entry.chunks, &entry.hash, &mut out)?;
        out.into_inner().map_err(|e| e.into_error())?.sync_all()?;
    }
    if let Err(e) = fs::rename(&tmp, dest) {
        let _ = fs::remove_file(&tmp);
        return Err(e).with_context(|| format!("{} ersetzen (in CAD noch geöffnet?)", dest.display()));
    }
    let meta = fs::metadata(dest)?;
    Ok(CacheEntry { size: meta.len(), mtime: scan::mtime_of(&meta), hash: entry.hash.clone(), chunks: Some(entry.chunks.clone()) })
}

/// Deletes a tracked file and any directories that became empty.
pub fn remove_tracked(repo: &Repo, path: &str) -> Result<bool> {
    let abs = scan::abs_path(&repo.root, path);
    if !abs.exists() {
        return Ok(false);
    }
    fs::remove_file(&abs)?;
    let mut dir = abs.parent();
    while let Some(d) = dir {
        if d == repo.root || fs::remove_dir(d).is_err() {
            break;
        }
        dir = d.parent();
    }
    Ok(true)
}

/// Brings the files that differ between `old` and `new` to the state of `new`.
/// All other files, including local edits to them, are left alone.
pub fn apply_update(repo: &Repo, state: &mut WorkState, old: &Manifest, new: &Manifest) -> Result<(usize, usize)> {
    let (mut written, mut removed) = (0, 0);
    for (path, _) in diff_manifests(old, new) {
        match new.files.get(&path) {
            Some(entry) => {
                if state.hashes.get(&path) != Some(&entry.hash) {
                    let fresh = materialize(repo, entry, &scan::abs_path(&repo.root, &path))?;
                    state.cache.files.insert(path, fresh);
                    written += 1;
                }
            }
            None => {
                if remove_tracked(repo, &path)? {
                    removed += 1;
                }
                state.cache.files.remove(&path);
            }
        }
    }
    Ok((written, removed))
}

/// Selects manifest entries matching `prefix` (exact file or directory).
pub fn matches_prefix(path: &str, prefix: &str) -> bool {
    prefix.is_empty() || path == prefix || path.starts_with(&format!("{prefix}/"))
}
