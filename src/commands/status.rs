use std::path::Path;

use anyhow::Result;

use super::{print_changes, short, WorkState};
use crate::repo::{author, Repo};

pub fn run(cwd: &Path) -> Result<()> {
    let repo = Repo::discover(cwd)?;
    let state = WorkState::collect(&repo)?;
    state.cache.save(&repo.cache_path())?;
    let index = repo.index()?;
    let head = repo.head()?;

    println!("Repo: {}", repo.root.display());
    match &head {
        Some(id) => {
            let c = repo.commit(id)?;
            let tags: Vec<&str> = index.tags.iter().filter(|(_, v)| *v == id).map(|(k, _)| k.as_str()).collect();
            let tags = if tags.is_empty() { String::new() } else { format!(" [{}]", tags.join(", ")) };
            println!("Stand: {} \"{}\"{tags}", short(id), c.message);
        }
        None => println!("Stand: noch kein Commit"),
    }
    if index.head.is_some() && index.head != head {
        println!(
            "Hinweis: Es gibt eine neuere Version ({}). `crate checkout latest` holt sie.",
            short(index.head.as_deref().unwrap_or(""))
        );
    }

    if state.changes.is_empty() {
        println!("\nKeine Änderungen ({} Dateien).", state.files.len());
    } else {
        println!("\nÄnderungen ({}):", state.changes.len());
        print_changes(&state.changes, |p| state.head.files.get(p).map(|f| f.size), |p| state.size_of(p));
    }

    if !index.locks.is_empty() {
        let me = author();
        println!("\nGesperrte Dateien:");
        for (path, info) in &index.locks {
            let who = if info.owner == me { "von dir".to_string() } else { format!("von {}", info.owner) };
            let changed = state.changes.iter().any(|(p, _)| p == path);
            let warn = if changed && info.owner != me { "  ACHTUNG: lokal geändert, aber fremd gesperrt" } else { "" };
            println!("  {path}  ({who}){warn}");
        }
    }
    Ok(())
}
