use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use walkdir::WalkDir;

use super::{BlobStore, Etag, ObjectInfo};

const LOCK_STALE_AFTER: Duration = Duration::from_secs(30);
const LOCK_WAIT_MAX: Duration = Duration::from_secs(20);

/// Stores every object as a file below `root`. Works on local disks and network shares.
pub struct LocalFsStore {
    root: PathBuf,
}

impl LocalFsStore {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self> {
        let root = root.into();
        fs::create_dir_all(&root)
            .with_context(|| format!("Speicherordner {} kann nicht angelegt werden", root.display()))?;
        Ok(Self { root })
    }

    fn path(&self, key: &str) -> Result<PathBuf> {
        if key.is_empty() || key.split('/').any(|p| p.is_empty() || p == "." || p == "..") {
            bail!("ungültiger Objektschlüssel: {key:?}");
        }
        Ok(key.split('/').fold(self.root.clone(), |p, part| p.join(part)))
    }

    fn write_tmp(&self, dest: &Path, data: &[u8]) -> Result<PathBuf> {
        let dir = dest.parent().expect("object path has a parent");
        fs::create_dir_all(dir)?;
        let tmp = dir.join(format!(".tmp-{}", unique_suffix()));
        let mut f = File::create(&tmp).with_context(|| format!("{} schreiben", tmp.display()))?;
        f.write_all(data)?;
        f.sync_all()?;
        Ok(tmp)
    }
}

fn unique_suffix() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    format!("{}-{}-{}", std::process::id(), nanos, COUNTER.fetch_add(1, Ordering::Relaxed))
}

pub(crate) fn etag_of(data: &[u8]) -> Etag {
    blake3::hash(data).to_hex().to_string()
}

struct FileLock {
    path: PathBuf,
}

impl FileLock {
    fn acquire(path: PathBuf) -> Result<Self> {
        let start = Instant::now();
        loop {
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut f) => {
                    let _ = writeln!(f, "{}", std::process::id());
                    return Ok(Self { path });
                }
                // Windows reports PermissionDenied while another process is deleting the file.
                Err(e) if matches!(e.kind(), ErrorKind::AlreadyExists | ErrorKind::PermissionDenied) => {
                    let stale = fs::metadata(&path)
                        .and_then(|m| m.modified())
                        .ok()
                        .and_then(|m| m.elapsed().ok())
                        .is_some_and(|age| age > LOCK_STALE_AFTER);
                    if stale {
                        let _ = fs::remove_file(&path);
                        continue;
                    }
                    if start.elapsed() > LOCK_WAIT_MAX {
                        bail!("Sperrdatei {} wird seit über {}s gehalten", path.display(), LOCK_WAIT_MAX.as_secs());
                    }
                    thread::sleep(Duration::from_millis(20));
                }
                Err(e) => return Err(e).with_context(|| format!("Sperrdatei {}", path.display())),
            }
        }
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

impl BlobStore for LocalFsStore {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>> {
        let path = self.path(key)?;
        match fs::read(&path) {
            Ok(data) => Ok(Some(data)),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("{} lesen", path.display())),
        }
    }

    fn put(&self, key: &str, data: &[u8]) -> Result<()> {
        let dest = self.path(key)?;
        let tmp = self.write_tmp(&dest, data)?;
        fs::rename(&tmp, &dest).with_context(|| format!("{} ersetzen", dest.display()))
    }

    fn put_if_absent(&self, key: &str, data: &[u8]) -> Result<bool> {
        let dest = self.path(key)?;
        if dest.exists() {
            return Ok(false);
        }
        let tmp = self.write_tmp(&dest, data)?;
        // hard_link fails atomically if the destination exists; rename would overwrite.
        let result = match fs::hard_link(&tmp, &dest) {
            Ok(()) => Ok(true),
            Err(e) if e.kind() == ErrorKind::AlreadyExists => Ok(false),
            Err(_) => {
                // Filesystems without hard links: fall back to exclusive create + copy.
                match OpenOptions::new().write(true).create_new(true).open(&dest) {
                    Ok(mut f) => {
                        f.write_all(data)?;
                        f.sync_all()?;
                        Ok(true)
                    }
                    Err(e) if e.kind() == ErrorKind::AlreadyExists => Ok(false),
                    Err(e) => Err(e.into()),
                }
            }
        };
        let _ = fs::remove_file(&tmp);
        result
    }

    fn exists(&self, key: &str) -> Result<bool> {
        Ok(self.path(key)?.exists())
    }

    fn delete(&self, key: &str) -> Result<()> {
        let path = self.path(key)?;
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e).with_context(|| format!("{} löschen", path.display())),
        }
    }

    fn list(&self, prefix: &str) -> Result<Vec<ObjectInfo>> {
        let base = if prefix.is_empty() { self.root.clone() } else { self.path(prefix.trim_end_matches('/'))? };
        if !base.exists() {
            return Ok(Vec::new());
        }
        let mut out = Vec::new();
        for entry in WalkDir::new(&base) {
            let entry = entry?;
            if !entry.file_type().is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy();
            if name.starts_with(".tmp-") || name.ends_with(".lock") {
                continue;
            }
            let rel = entry.path().strip_prefix(&self.root)?;
            let key = rel.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/");
            let meta = entry.metadata()?;
            out.push(ObjectInfo { key, size: meta.len(), modified: meta.modified()? });
        }
        out.sort_by(|a, b| a.key.cmp(&b.key));
        Ok(out)
    }

    fn get_versioned(&self, key: &str) -> Result<Option<(Vec<u8>, Etag)>> {
        Ok(self.get(key)?.map(|d| {
            let tag = etag_of(&d);
            (d, tag)
        }))
    }

    fn cas_put(&self, key: &str, expected: Option<&Etag>, data: &[u8]) -> Result<bool> {
        let dest = self.path(key)?;
        if let Some(dir) = dest.parent() {
            fs::create_dir_all(dir)?;
        }
        let mut lock_path = dest.clone().into_os_string();
        lock_path.push(".lock");
        let _lock = FileLock::acquire(PathBuf::from(lock_path))?;

        let current = self.get(key)?.map(|d| etag_of(&d));
        if current.as_ref() != expected {
            return Ok(false);
        }
        self.put(key, data)?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn put_get_list_delete() {
        let dir = tempfile::tempdir().unwrap();
        let s = LocalFsStore::open(dir.path()).unwrap();
        s.put("a/b/c", b"hello").unwrap();
        assert_eq!(s.get("a/b/c").unwrap().unwrap(), b"hello");
        assert!(!s.put_if_absent("a/b/c", b"other").unwrap());
        assert_eq!(s.get("a/b/c").unwrap().unwrap(), b"hello");
        assert!(s.put_if_absent("a/x", b"1").unwrap());
        let keys: Vec<_> = s.list("a").unwrap().into_iter().map(|o| o.key).collect();
        assert_eq!(keys, vec!["a/b/c", "a/x"]);
        s.delete("a/x").unwrap();
        assert!(!s.exists("a/x").unwrap());
        assert!(s.get("../evil").is_err());
    }

    #[test]
    fn cas_detects_conflicts() {
        let dir = tempfile::tempdir().unwrap();
        let s = LocalFsStore::open(dir.path()).unwrap();
        assert!(s.cas_put("index", None, b"v1").unwrap());
        assert!(!s.cas_put("index", None, b"v1b").unwrap());
        let (_, tag) = s.get_versioned("index").unwrap().unwrap();
        assert!(s.cas_put("index", Some(&tag), b"v2").unwrap());
        assert!(!s.cas_put("index", Some(&tag), b"v3").unwrap());
        assert_eq!(s.get("index").unwrap().unwrap(), b"v2");
    }

    #[test]
    fn concurrent_cas_is_linearizable() {
        let dir = tempfile::tempdir().unwrap();
        let s = std::sync::Arc::new(LocalFsStore::open(dir.path()).unwrap());
        s.put("counter", b"0").unwrap();
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let s = s.clone();
                thread::spawn(move || {
                    for _ in 0..25 {
                        loop {
                            let (d, tag) = s.get_versioned("counter").unwrap().unwrap();
                            let n: u64 = String::from_utf8(d).unwrap().parse().unwrap();
                            if s.cas_put("counter", Some(&tag), (n + 1).to_string().as_bytes()).unwrap() {
                                break;
                            }
                        }
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(s.get("counter").unwrap().unwrap(), b"200");
    }
}
