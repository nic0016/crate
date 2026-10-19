use std::fs;
use std::path::Path;

use anyhow::{bail, Context, Result};

use crate::remote;
use crate::repo::Repo;

pub fn run(cwd: &Path, url: &str, dir: Option<&Path>) -> Result<()> {
    let url = remote::normalize_url(url)?;
    if !remote::store_path(&url)?.is_dir() {
        bail!("{url} ist nicht erreichbar");
    }
    let target = match dir {
        Some(d) => cwd.join(d),
        None => cwd.join(Path::new(&url).file_name().context("Bitte einen Zielordner angeben")?),
    };
    if target.exists() && fs::read_dir(&target)?.next().is_some() {
        bail!("{} existiert bereits und ist nicht leer", target.display());
    }
    fs::create_dir_all(&target)?;

    // No default .crateignore here: the project's own one arrives with the files.
    let mut repo = Repo::init(&target, None)?;
    repo.config.remotes.insert("origin".to_string(), url.clone());
    repo.save_config()?;
    println!("Klone {url} nach {}", target.display());
    super::pull::run(&target, "origin", None)
}
