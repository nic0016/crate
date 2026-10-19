use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{bail, Result};
use chrono::Utc;
use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;

use super::short;
use crate::cad::human_size;
use crate::chunk;
use crate::model::{diff_manifests, stats_of, Commit, FileEntry, Manifest};
use crate::repo::{author, Repo};
use crate::scan::{self, WorkFile};
use crate::wal::{self, Op};

pub fn run(cwd: &Path, message: &str, force: bool) -> Result<()> {
    if message.trim().is_empty() {
        bail!("Bitte eine Commit-Nachricht angeben (-m \"...\")");
    }
    let repo = Repo::discover(cwd)?;
    let me = author();
    let index = repo.index()?;
    let local_head = repo.head()?;
    if index.head != local_head && !force {
        bail!(
            "Dein Arbeitsstand basiert auf {}, die neueste Version ist {}.\n\
             Ein Commit würde die neueren Änderungen überschreiben. Entweder `crate checkout latest` \
             (lokale Änderungen vorher sichern) oder bewusst `crate commit --force`.",
            local_head.as_deref().map(short).unwrap_or("nichts"),
            index.head.as_deref().map(short).unwrap_or("nichts"),
        );
    }
    let parent = index.head.clone();
    let parent_manifest = match &parent {
        Some(id) => repo.manifest_of(id)?,
        None => Manifest::default(),
    };

    let files = scan::scan(&repo.root)?;
    let mut cache = repo.load_cache();

    // Reuse chunk lists for files whose content is already stored.
    let mut manifest = Manifest::default();
    let mut to_store: Vec<&WorkFile> = Vec::new();
    for f in &files {
        let reuse = cache.lookup(f).and_then(|c| match &c.chunks {
            Some(chunks) => Some(chunks.clone()),
            None => parent_manifest.files.get(&f.path).filter(|p| p.hash == c.hash).map(|p| p.chunks.clone()),
        });
        match (reuse, cache.lookup(f)) {
            (Some(chunks), Some(c)) => {
                manifest.files.insert(f.path.clone(), FileEntry { size: f.size, mtime: f.mtime, hash: c.hash.clone(), chunks });
            }
            _ => to_store.push(f),
        }
    }

    let total: u64 = to_store.iter().map(|f| f.size).sum();
    let bar = ProgressBar::new(total);
    bar.set_style(
        ProgressStyle::with_template("{msg} [{bar:30}] {bytes}/{total_bytes} ({bytes_per_sec}, {eta})")
            .unwrap()
            .progress_chars("=> "),
    );
    bar.set_message(format!("Speichere {} Dateien", to_store.len()));
    let new_bytes = AtomicU64::new(0);
    let store = repo.store();
    let stored: Vec<(String, FileEntry)> = to_store
        .par_iter()
        .map(|f| {
            let s = chunk::store_file(store, &f.abs, &|n| bar.inc(n))?;
            new_bytes.fetch_add(s.new_bytes, Ordering::Relaxed);
            Ok((f.path.clone(), FileEntry { size: s.size, mtime: f.mtime, hash: s.hash, chunks: s.chunks }))
        })
        .collect::<Result<_>>()?;
    bar.finish_and_clear();
    manifest.files.extend(stored);

    let changes = diff_manifests(&parent_manifest, &manifest);
    if changes.is_empty() {
        cache.absorb_manifest(&manifest);
        cache.save(&repo.cache_path())?;
        println!("Keine Änderungen – nichts zu committen.");
        return Ok(());
    }

    let foreign: Vec<String> = changes
        .iter()
        .filter_map(|(p, _)| index.locks.get(p).filter(|l| l.owner != me).map(|l| format!("{p} (gesperrt von {})", l.owner)))
        .collect();
    if !foreign.is_empty() && !force {
        bail!("Geänderte Dateien sind von anderen gesperrt:\n  {}\nMit --force trotzdem committen.", foreign.join("\n  "));
    }

    let manifest_id = manifest.save(store)?;
    let stats = stats_of(&changes);
    let commit = Commit {
        parent: parent.clone(),
        manifest: manifest_id,
        author: me.clone(),
        timestamp: Utc::now(),
        message: message.trim().to_string(),
        stats: stats.clone(),
        total_files: manifest.files.len(),
        total_size: manifest.total_size(),
    };
    let id = commit.save(store)?;

    wal::append(store, &me, |idx| {
        if idx.head != parent {
            bail!("Während des Commits wurde eine neue Version veröffentlicht. Bitte `crate status` prüfen und erneut versuchen.");
        }
        Ok(Op::Commit { id: id.clone(), parent: parent.clone() })
    })?;

    repo.set_head(Some(&id))?;
    cache.files.retain(|p, _| manifest.files.contains_key(p));
    cache.absorb_manifest(&manifest);
    cache.save(&repo.cache_path())?;

    println!("[{}] {}", short(&id), commit.message);
    println!(
        "  {} neu, {} geändert, {} gelöscht · {} Dateien, {} · {} neu gespeichert",
        stats.added,
        stats.modified,
        stats.deleted,
        commit.total_files,
        human_size(commit.total_size),
        human_size(new_bytes.load(Ordering::Relaxed)),
    );
    Ok(())
}
