use std::path::Path;

use anyhow::{bail, Result};

use super::{materialize, matches_prefix, short};
use crate::cad::human_size;
use crate::repo::Repo;
use crate::scan;

pub fn run(cwd: &Path, paths: &[String], rev: &str, output: Option<&Path>) -> Result<()> {
    let repo = Repo::discover(cwd)?;
    let id = repo.resolve(rev)?;
    let manifest = repo.manifest_of(&id)?;

    let prefixes: Vec<String> = paths.iter().map(|p| repo.to_repo_path(p)).collect::<Result<_>>()?;
    let selected: Vec<(&String, _)> =
        manifest.files.iter().filter(|(p, _)| prefixes.iter().any(|pre| matches_prefix(p, pre))).collect();
    for pre in &prefixes {
        if !manifest.files.keys().any(|p| matches_prefix(p, pre)) {
            bail!("{pre} existiert in Version {} nicht", short(&id));
        }
    }

    if let Some(out) = output {
        if selected.len() != 1 {
            bail!("--output funktioniert nur mit genau einer Datei ({} ausgewählt)", selected.len());
        }
        let (path, entry) = selected[0];
        materialize(&repo, entry, out)?;
        println!("{path} @ {} → {} ({})", short(&id), out.display(), human_size(entry.size));
        return Ok(());
    }

    let mut cache = repo.load_cache();
    for (path, entry) in selected {
        let dest = scan::abs_path(&repo.root, path);
        let fresh = materialize(&repo, entry, &dest)?;
        cache.files.insert(path.clone(), fresh);
        println!("wiederhergestellt: {path} @ {} ({})", short(&id), human_size(entry.size));
    }
    cache.save(&repo.cache_path())?;
    Ok(())
}
