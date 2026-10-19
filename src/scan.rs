//! Scans the working directory and keeps a stat cache so that unchanged files
//! (same size and mtime) are never re-read.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use anyhow::{Context, Result};
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::cad::DEFAULT_IGNORES;
use crate::chunk;
use crate::model::Manifest;
use crate::repo::{IGNORE_FILE, REPO_DIR};

#[derive(Debug, Clone)]
pub struct WorkFile {
    pub path: String,
    pub abs: PathBuf,
    pub size: u64,
    pub mtime: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheEntry {
    pub size: u64,
    pub mtime: u64,
    pub hash: String,
    /// Known once the file content has been stored.
    pub chunks: Option<Vec<String>>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct StatCache {
    pub files: BTreeMap<String, CacheEntry>,
}

impl StatCache {
    pub fn load(path: &Path) -> Self {
        fs::read(path).ok().and_then(|d| serde_json::from_slice(&d).ok()).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let tmp = path.with_extension("tmp");
        fs::write(&tmp, serde_json::to_vec(self)?)?;
        fs::rename(&tmp, path)?;
        Ok(())
    }

    pub fn lookup(&self, f: &WorkFile) -> Option<&CacheEntry> {
        self.files.get(&f.path).filter(|c| c.size == f.size && c.mtime == f.mtime)
    }

    /// Seeds the cache from a manifest whose files are known to be on disk unchanged.
    pub fn absorb_manifest(&mut self, manifest: &Manifest) {
        for (path, e) in &manifest.files {
            self.files.insert(
                path.clone(),
                CacheEntry { size: e.size, mtime: e.mtime, hash: e.hash.clone(), chunks: Some(e.chunks.clone()) },
            );
        }
    }
}

pub fn build_ignore(root: &Path) -> Result<Gitignore> {
    let mut b = GitignoreBuilder::new(root);
    b.case_insensitive(true)?;
    for pat in DEFAULT_IGNORES {
        b.add_line(None, pat)?;
    }
    let custom = root.join(IGNORE_FILE);
    if custom.exists() {
        if let Some(err) = b.add(&custom) {
            return Err(err).context(".crateignore ist fehlerhaft");
        }
    }
    Ok(b.build()?)
}

pub fn mtime_of(meta: &fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

pub fn rel_path(root: &Path, abs: &Path) -> Result<String> {
    let rel = abs.strip_prefix(root)?;
    Ok(rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/"))
}

pub fn abs_path(root: &Path, rel: &str) -> PathBuf {
    rel.split('/').fold(root.to_path_buf(), |p, part| p.join(part))
}

/// Lists all versionable files below `root`, sorted by path.
pub fn scan(root: &Path) -> Result<Vec<WorkFile>> {
    let ignore = build_ignore(root)?;
    let mut out = Vec::new();
    let walker = WalkDir::new(root).min_depth(1).into_iter().filter_entry(|e| {
        if e.depth() == 1 && e.file_name() == REPO_DIR {
            return false;
        }
        let rel = e.path().strip_prefix(root).unwrap_or(e.path());
        !ignore.matched_path_or_any_parents(rel, e.file_type().is_dir()).is_ignore()
    });
    for entry in walker {
        let entry = entry?;
        if !entry.file_type().is_file() {
            continue;
        }
        let meta = entry.metadata()?;
        out.push(WorkFile {
            path: rel_path(root, entry.path())?,
            abs: entry.path().to_path_buf(),
            size: meta.len(),
            mtime: mtime_of(&meta),
        });
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(out)
}

/// Returns path -> content hash for every working file, hashing only files whose
/// size or mtime differ from the cache. Updates the cache in place.
pub fn hash_all(files: &[WorkFile], cache: &mut StatCache) -> Result<BTreeMap<String, String>> {
    let fresh: Vec<(String, CacheEntry)> = files
        .par_iter()
        .filter(|f| cache.lookup(f).is_none())
        .map(|f| {
            let hash = chunk::hash_file(&f.abs)?;
            Ok((f.path.clone(), CacheEntry { size: f.size, mtime: f.mtime, hash, chunks: None }))
        })
        .collect::<Result<_>>()?;
    for (path, entry) in fresh {
        cache.files.insert(path, entry);
    }
    let present: std::collections::HashSet<&str> = files.iter().map(|f| f.path.as_str()).collect();
    cache.files.retain(|p, _| present.contains(p.as_str()));
    Ok(files.iter().map(|f| (f.path.clone(), cache.files[&f.path].hash.clone())).collect())
}
