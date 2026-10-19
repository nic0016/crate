use std::path::Path;

use anyhow::{bail, Result};
use chrono::{Local, Utc};

use crate::repo::{author, Repo};
use crate::scan;
use crate::wal::{self, LockInfo, Op};

pub fn lock(cwd: &Path, paths: &[String], message: Option<String>) -> Result<()> {
    let repo = Repo::discover(cwd)?;
    let store = repo.lock_store()?;
    let me = author();
    let head = repo.head_manifest()?;
    for input in paths {
        let path = repo.to_repo_path(input)?;
        if !head.files.contains_key(&path) && !scan::abs_path(&repo.root, &path).is_file() {
            bail!("{path} existiert nicht");
        }
        if wal::load_index(store.as_ref())?.0.locks.get(&path).is_some_and(|l| l.owner == me) {
            println!("{path} ist bereits von dir gesperrt.");
            continue;
        }
        wal::append(store.as_ref(), &me, |idx| {
            if let Some(l) = idx.locks.get(&path) {
                bail!("{path} ist bereits gesperrt von {} (seit {})", l.owner, l.since.with_timezone(&Local).format("%d.%m.%Y %H:%M"));
            }
            Ok(Op::Lock { path: path.clone(), info: LockInfo { owner: me.clone(), since: Utc::now(), message: message.clone() } })
        })?;
        println!("gesperrt: {path}");
    }
    Ok(())
}

pub fn unlock(cwd: &Path, paths: &[String], force: bool) -> Result<()> {
    let repo = Repo::discover(cwd)?;
    let store = repo.lock_store()?;
    let me = author();
    for input in paths {
        let path = repo.to_repo_path(input)?;
        wal::append(store.as_ref(), &me, |idx| match idx.locks.get(&path) {
            None => bail!("{path} ist nicht gesperrt"),
            Some(l) if l.owner != me && !force => {
                bail!("{path} ist von {} gesperrt (mit --force trotzdem freigeben)", l.owner)
            }
            Some(_) => Ok(Op::Unlock { path: path.clone() }),
        })?;
        println!("freigegeben: {path}");
    }
    Ok(())
}

pub fn list(cwd: &Path) -> Result<()> {
    let repo = Repo::discover(cwd)?;
    let (index, _) = wal::load_index(repo.lock_store()?.as_ref())?;
    if index.locks.is_empty() {
        println!("Keine Sperren.");
    }
    for (path, l) in &index.locks {
        let msg = l.message.as_deref().map(|m| format!("  \"{m}\"")).unwrap_or_default();
        println!("{path}  {}  seit {}{msg}", l.owner, l.since.with_timezone(&Local).format("%d.%m.%Y %H:%M"));
    }
    Ok(())
}
