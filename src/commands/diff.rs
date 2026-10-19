use std::path::Path;

use anyhow::Result;

use super::{print_changes, short, WorkState};
use crate::model::diff_manifests;
use crate::repo::Repo;

pub fn run(cwd: &Path, from: Option<&str>, to: Option<&str>) -> Result<()> {
    let repo = Repo::discover(cwd)?;
    let from_id = repo.resolve(from.unwrap_or("HEAD"))?;
    let old = repo.manifest_of(&from_id)?;

    match to {
        Some(to) => {
            let to_id = repo.resolve(to)?;
            let new = repo.manifest_of(&to_id)?;
            let changes = diff_manifests(&old, &new);
            println!("Vergleich {} → {}", short(&from_id), short(&to_id));
            if changes.is_empty() {
                println!("Keine Unterschiede.");
            } else {
                print_changes(&changes, |p| old.files.get(p).map(|f| f.size), |p| new.files.get(p).map(|f| f.size));
            }
        }
        None => {
            let state = WorkState::collect(&repo)?;
            state.cache.save(&repo.cache_path())?;
            let changes = crate::model::diff_maps(
                old.files.iter().map(|(p, f)| (p, f.hash.as_str())),
                state.hashes.iter().map(|(p, h)| (p, h.as_str())),
            );
            println!("Vergleich {} → Arbeitsordner", short(&from_id));
            if changes.is_empty() {
                println!("Keine Unterschiede.");
            } else {
                print_changes(&changes, |p| old.files.get(p).map(|f| f.size), |p| state.size_of(p));
            }
        }
    }
    Ok(())
}
