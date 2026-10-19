use std::path::Path;

use anyhow::{bail, Result};

use super::{materialize, remove_tracked, short, WorkState};
use crate::repo::Repo;
use crate::scan;

pub fn run(cwd: &Path, rev: &str, force: bool) -> Result<()> {
    let repo = Repo::discover(cwd)?;
    let target_id = repo.resolve(rev)?;
    let target = repo.manifest_of(&target_id)?;
    let mut state = WorkState::collect(&repo)?;

    if !state.changes.is_empty() && !force {
        let list: Vec<String> = state.changes.iter().take(10).map(|(p, c)| format!("{}{p}", c.symbol())).collect();
        bail!(
            "Es gibt {} ungespeicherte Änderungen:\n  {}{}\nZuerst committen oder mit --force verwerfen.",
            state.changes.len(),
            list.join("\n  "),
            if state.changes.len() > 10 { "\n  …" } else { "" }
        );
    }

    let mut written = 0;
    for (path, entry) in &target.files {
        if state.hashes.get(path) == Some(&entry.hash) {
            continue;
        }
        let fresh = materialize(&repo, entry, &scan::abs_path(&repo.root, path))?;
        state.cache.files.insert(path.clone(), fresh);
        written += 1;
    }

    // Remove files tracked in the old version but absent in the target. Untracked files stay.
    let mut removed = 0;
    for path in state.head.files.keys().filter(|p| !target.files.contains_key(*p)) {
        if remove_tracked(&repo, path)? {
            removed += 1;
        }
        state.cache.files.remove(path);
    }

    repo.set_head(Some(&target_id))?;
    state.cache.save(&repo.cache_path())?;

    let c = repo.commit(&target_id)?;
    println!("Stand {} \"{}\" hergestellt: {written} Dateien geschrieben, {removed} entfernt.", short(&target_id), c.message);
    let index = repo.index()?;
    if index.head.as_deref() != Some(target_id.as_str()) {
        println!(
            "Hinweis: Das ist nicht die neueste Version. Um von hier weiterzuarbeiten, nach dem Bearbeiten \
             `crate commit --force` verwenden (der alte Stand wird damit zur neuesten Version)."
        );
    }
    Ok(())
}
