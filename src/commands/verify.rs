use std::collections::BTreeSet;
use std::path::Path;

use anyhow::{bail, Result};
use chrono::Local;
use rayon::prelude::*;

use super::short;
use crate::chunk::{chunk_key, decode};
use crate::model::{commit_key, manifest_key, Manifest};
use crate::repo::Repo;
use crate::wal::{self, Op};

pub fn wal(cwd: &Path, limit: Option<usize>) -> Result<()> {
    let repo = Repo::discover(cwd)?;
    let index = repo.index()?;
    let limit = limit.unwrap_or(usize::MAX) as u64;
    let first = index.seq.saturating_sub(limit) + 1;
    for seq in (first..=index.seq).rev() {
        let Some(e) = wal::load_entry(repo.store(), seq)? else {
            println!("{seq:>6}  (fehlt!)");
            continue;
        };
        let what = match &e.op {
            Op::Commit { id, .. } => format!("commit  {}", short(id)),
            Op::Tag { name, commit } => format!("tag     {name} → {}", short(commit)),
            Op::DeleteTag { name } => format!("untag   {name}"),
            Op::Lock { path, .. } => format!("lock    {path}"),
            Op::Unlock { path } => format!("unlock  {path}"),
        };
        println!("{seq:>6}  {}  {:<24} {what}", e.timestamp.with_timezone(&Local).format("%d.%m.%Y %H:%M:%S"), e.author);
    }
    Ok(())
}

/// Collects every commit, manifest and chunk referenced by the WAL.
pub fn reachable(repo: &Repo) -> Result<(BTreeSet<String>, BTreeSet<String>, BTreeSet<String>)> {
    let index = repo.index()?;
    let mut commits = BTreeSet::new();
    let mut manifests = BTreeSet::new();
    let mut chunks = BTreeSet::new();
    let mut starts: Vec<String> = index.head.iter().cloned().collect();
    starts.extend(index.tags.values().cloned());
    for seq in 1..=index.seq {
        if let Some(e) = wal::load_entry(repo.store(), seq)? {
            if let Op::Commit { id, .. } = e.op {
                starts.push(id);
            }
        }
    }
    for start in starts {
        let mut cur = Some(start);
        while let Some(id) = cur {
            if !commits.insert(id.clone()) {
                break;
            }
            let c = repo.commit(&id)?;
            if manifests.insert(c.manifest.clone()) {
                let m = Manifest::load(repo.store(), &c.manifest)?;
                for f in m.files.values() {
                    chunks.extend(f.chunks.iter().cloned());
                }
            }
            cur = c.parent;
        }
    }
    Ok((commits, manifests, chunks))
}

pub fn run(cwd: &Path, full: bool) -> Result<()> {
    let repo = Repo::discover(cwd)?;
    let store = repo.store();
    let mut problems = Vec::new();

    let index = repo.index()?;
    match wal::replay(store, index.seq) {
        Ok(replayed) if replayed == index => println!("WAL: {} Einträge, Index stimmt überein.", index.seq),
        Ok(_) => problems.push("Index weicht vom WAL ab (Index lässt sich aus dem WAL neu aufbauen)".to_string()),
        Err(e) => problems.push(format!("WAL: {e:#}")),
    }

    let (commits, manifests, chunks) = reachable(&repo)?;
    for id in &commits {
        if !store.exists(&commit_key(id))? {
            problems.push(format!("Commit {} fehlt", short(id)));
        }
    }
    for id in &manifests {
        if !store.exists(&manifest_key(id))? {
            problems.push(format!("Manifest {} fehlt", short(id)));
        }
    }
    let chunk_list: Vec<&String> = chunks.iter().collect();
    let bad: Vec<String> = chunk_list
        .par_iter()
        .filter_map(|h| {
            let key = chunk_key(h);
            if !full {
                return match store.exists(&key) {
                    Ok(true) => None,
                    _ => Some(format!("Chunk {} fehlt", short(h))),
                };
            }
            match store.get(&key) {
                Ok(Some(d)) => match decode(&d) {
                    Ok(data) if blake3::hash(&data).to_hex().as_str() == h.as_str() => None,
                    _ => Some(format!("Chunk {} ist beschädigt", short(h))),
                },
                _ => Some(format!("Chunk {} fehlt", short(h))),
            }
        })
        .collect();
    problems.extend(bad);

    println!(
        "{} Commits, {} Manifeste, {} Chunks {}.",
        commits.len(),
        manifests.len(),
        chunks.len(),
        if full { "vollständig gelesen und geprüft" } else { "vorhanden geprüft (--full prüft alle Prüfsummen)" }
    );
    if problems.is_empty() {
        println!("Alles in Ordnung.");
        Ok(())
    } else {
        for p in &problems {
            println!("  PROBLEM: {p}");
        }
        bail!("{} Probleme gefunden", problems.len())
    }
}
