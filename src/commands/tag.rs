use std::path::Path;

use anyhow::{bail, Result};

use super::short;
use crate::repo::{author, Repo};
use crate::wal::{self, Op};

pub fn run(cwd: &Path, name: Option<&str>, rev: Option<&str>, delete: bool, force: bool) -> Result<()> {
    let repo = Repo::discover(cwd)?;
    let Some(name) = name else {
        let index = repo.index()?;
        if index.tags.is_empty() {
            println!("Keine Tags.");
        }
        for (tag, id) in &index.tags {
            let c = repo.commit(id)?;
            println!("{tag:<20} {}  {}", short(id), c.message);
        }
        return Ok(());
    };

    if name.is_empty() || name.chars().any(|c| c.is_whitespace() || c == '~' || c == '/') {
        bail!("Ungültiger Tag-Name {name:?} (keine Leerzeichen, '~' oder '/')");
    }
    if matches!(name, "HEAD" | "latest" | "main") {
        bail!("{name} ist reserviert");
    }

    if delete {
        wal::append(repo.store(), &author(), |idx| {
            if !idx.tags.contains_key(name) {
                bail!("Tag {name} existiert nicht");
            }
            Ok(Op::DeleteTag { name: name.to_string() })
        })?;
        println!("Tag {name} gelöscht.");
        return Ok(());
    }

    let id = repo.resolve(rev.unwrap_or("HEAD"))?;
    wal::append(repo.store(), &author(), |idx| {
        if idx.tags.contains_key(name) && !force {
            bail!("Tag {name} existiert bereits (mit --force verschieben)");
        }
        Ok(Op::Tag { name: name.to_string(), commit: id.clone() })
    })?;
    println!("Tag {name} → {}", short(&id));
    Ok(())
}
