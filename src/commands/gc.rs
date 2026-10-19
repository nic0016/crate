use std::path::Path;
use std::time::Duration;

use anyhow::Result;

use super::verify::reachable;
use crate::cad::human_size;
use crate::repo::Repo;

pub fn run(cwd: &Path, dry_run: bool, grace_minutes: u64) -> Result<()> {
    let repo = Repo::discover(cwd)?;
    let store = repo.store();
    let (commits, manifests, chunks) = reachable(&repo)?;
    // Objects of a commit that is being written right now are not referenced yet.
    let grace = Duration::from_secs(grace_minutes * 60);

    let mut count = 0usize;
    let mut bytes = 0u64;
    for obj in store.list("chunks")?.into_iter().chain(store.list("manifests")?).chain(store.list("commits")?) {
        let referenced = match obj.key.split_once('/') {
            Some(("chunks", rest)) => chunks.contains(&rest.replace('/', "")),
            Some(("manifests", id)) => manifests.contains(id),
            Some(("commits", id)) => commits.contains(id),
            _ => true,
        };
        if referenced || obj.modified.elapsed().unwrap_or_default() < grace {
            continue;
        }
        count += 1;
        bytes += obj.size;
        if !dry_run {
            store.delete(&obj.key)?;
        }
    }

    let verb = if dry_run { "würden entfernt" } else { "entfernt" };
    println!("{count} nicht referenzierte Objekte ({}) {verb}.", human_size(bytes));
    let total: u64 = store.list("")?.iter().map(|o| o.size).sum();
    println!("Speicherbedarf des Repos: {}", human_size(total));
    Ok(())
}
