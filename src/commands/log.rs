use std::path::Path;

use anyhow::Result;
use chrono::Local;

use super::{matches_prefix, short};
use crate::cad::human_size;
use crate::model::Manifest;
use crate::repo::Repo;

pub fn run(cwd: &Path, path: Option<&str>, limit: Option<usize>) -> Result<()> {
    let repo = Repo::discover(cwd)?;
    let index = repo.index()?;
    let head = repo.head()?;
    let history = repo.history_from(index.head.as_deref())?;
    if history.is_empty() {
        println!("Noch keine Commits.");
        return Ok(());
    }
    let filter = path.map(|p| repo.to_repo_path(p)).transpose()?;
    let limit = limit.unwrap_or(usize::MAX);

    // Manifests are only needed when filtering by path; load each one once.
    let manifests: Vec<Option<Manifest>> = match &filter {
        Some(_) => history.iter().map(|(_, c)| Manifest::load(repo.store(), &c.manifest).map(Some)).collect::<Result<_>>()?,
        None => vec![None; history.len()],
    };
    let empty = Manifest::default();

    let mut shown = 0;
    for (i, (id, c)) in history.iter().enumerate() {
        if shown >= limit {
            break;
        }
        let mut detail = Vec::new();
        if let Some(prefix) = &filter {
            let cur = manifests[i].as_ref().unwrap();
            let prev = manifests.get(i + 1).and_then(|m| m.as_ref()).unwrap_or(&empty);
            for (p, e) in cur.files.iter().filter(|(p, _)| matches_prefix(p, prefix)) {
                match prev.files.get(p) {
                    None => detail.push(format!("neu       {p} ({})", human_size(e.size))),
                    Some(o) if o.hash != e.hash => detail.push(format!("geändert  {p} ({})", human_size(e.size))),
                    _ => {}
                }
            }
            for p in prev.files.keys().filter(|p| matches_prefix(p, prefix) && !cur.files.contains_key(*p)) {
                detail.push(format!("gelöscht  {p}"));
            }
            if detail.is_empty() {
                continue;
            }
        }

        let mut marks = Vec::new();
        if head.as_deref() == Some(id.as_str()) {
            marks.push("HEAD".to_string());
        }
        if i == 0 {
            marks.push("latest".to_string());
        }
        marks.extend(index.tags.iter().filter(|(_, v)| *v == id).map(|(k, _)| format!("Tag: {k}")));
        let marks = if marks.is_empty() { String::new() } else { format!("  ({})", marks.join(", ")) };

        println!("commit {}{marks}", short(id));
        println!("Autor: {}   Datum: {}", c.author, c.timestamp.with_timezone(&Local).format("%d.%m.%Y %H:%M"));
        println!("    {}", c.message);
        if detail.is_empty() {
            println!(
                "    {} neu, {} geändert, {} gelöscht · {} Dateien, {}",
                c.stats.added,
                c.stats.modified,
                c.stats.deleted,
                c.total_files,
                human_size(c.total_size)
            );
        } else {
            for d in detail {
                println!("    {d}");
            }
        }
        println!();
        shown += 1;
    }
    if shown == 0 {
        println!("Keine Commits betreffen {}.", filter.unwrap_or_default());
    }
    Ok(())
}
