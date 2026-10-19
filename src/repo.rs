use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::model::{Commit, Manifest};
use crate::remote;
use crate::scan::StatCache;
use crate::store::{BlobStore, LocalFsStore};
use crate::wal::{self, Index, LockInfo};

pub const REPO_DIR: &str = ".crate";
pub const IGNORE_FILE: &str = ".crateignore";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StoreConfig {
    /// Absolute path, or relative to the `.crate` directory.
    Local { path: String },
}

impl StoreConfig {
    pub fn resolve(&self, repo_dir: &Path) -> PathBuf {
        match self {
            StoreConfig::Local { path } => {
                let p = Path::new(path);
                if p.is_absolute() {
                    p.to_path_buf()
                } else {
                    repo_dir.join(p)
                }
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub version: u32,
    pub store: StoreConfig,
    /// Name -> URL of other stores used by `push` and `pull`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub remotes: BTreeMap<String, String>,
}

impl Config {
    pub fn load(path: &Path) -> Result<Config> {
        let data = fs::read(path).with_context(|| format!("{} lesen", path.display()))?;
        serde_json::from_slice(&data).context("config.json ist ungültig")
    }
}

pub struct Repo {
    pub root: PathBuf,
    pub dir: PathBuf,
    pub config: Config,
    pub store_path: PathBuf,
    pub store: Arc<dyn BlobStore>,
}

impl Repo {
    pub fn init(root: &Path, store_path: Option<&Path>) -> Result<Repo> {
        let root = std::path::absolute(root)?;
        let dir = root.join(REPO_DIR);
        if dir.exists() {
            bail!("{} ist bereits ein crate-Repo", root.display());
        }
        fs::create_dir_all(&dir)?;
        let path = match store_path {
            Some(p) => std::path::absolute(p)?.to_string_lossy().into_owned(),
            None => "objects".to_string(),
        };
        let config = Config { version: 1, store: StoreConfig::Local { path }, remotes: BTreeMap::new() };
        fs::write(dir.join("config.json"), serde_json::to_vec_pretty(&config)?)?;
        fs::write(dir.join("HEAD"), "")?;
        Self::open_at(root)
    }

    /// Finds the repository containing `start` (searching parent directories).
    pub fn discover(start: &Path) -> Result<Repo> {
        let start = std::path::absolute(start)?;
        let mut cur = start.as_path();
        loop {
            if cur.join(REPO_DIR).join("config.json").is_file() {
                return Self::open_at(cur.to_path_buf());
            }
            match cur.parent() {
                Some(p) => cur = p,
                None => bail!("Kein crate-Repo gefunden (in {} oder darüber). Zuerst `crate init` ausführen.", start.display()),
            }
        }
    }

    fn open_at(root: PathBuf) -> Result<Repo> {
        let dir = root.join(REPO_DIR);
        let config = Config::load(&dir.join("config.json"))?;
        let store_path = config.store.resolve(&dir);
        let store: Arc<dyn BlobStore> = Arc::new(LocalFsStore::open(store_path.clone())?);
        Ok(Repo { root, dir, config, store_path, store })
    }

    pub fn save_config(&self) -> Result<()> {
        fs::write(self.dir.join("config.json"), serde_json::to_vec_pretty(&self.config)?)?;
        Ok(())
    }

    pub fn store(&self) -> &dyn BlobStore {
        self.store.as_ref()
    }

    /// Opens the store behind a configured remote. Only `push` may create it.
    pub fn open_remote(&self, name: &str, create: bool) -> Result<Arc<dyn BlobStore>> {
        let url = self.config.remotes.get(name).with_context(|| {
            format!("Remote {name} ist nicht eingerichtet. Einrichten mit `crate remote add {name} <ordner>`.")
        })?;
        let path = remote::store_path(url)?;
        if !create && !path.is_dir() {
            bail!("Remote {name} ({url}) ist nicht erreichbar");
        }
        let store = LocalFsStore::open(path.clone())?;
        if fs::canonicalize(&path)? == fs::canonicalize(&self.store_path)? {
            bail!("Remote {name} zeigt auf den eigenen Speicher dieses Repos");
        }
        Ok(Arc::new(store))
    }

    /// Where locks live: on `origin` if configured, so the whole team sees them.
    pub fn lock_store(&self) -> Result<Arc<dyn BlobStore>> {
        if self.config.remotes.contains_key("origin") {
            self.open_remote("origin", false)
        } else {
            Ok(self.store.clone())
        }
    }

    /// Current locks; falls back to the local view if `origin` is unreachable.
    pub fn locks(&self) -> Result<BTreeMap<String, LockInfo>> {
        match self.lock_store().and_then(|s| Ok(wal::load_index(s.as_ref())?.0.locks)) {
            Ok(locks) => Ok(locks),
            Err(e) => {
                eprintln!("Warnung: Sperren auf origin nicht abrufbar ({e:#}).");
                Ok(self.index()?.locks)
            }
        }
    }

    /// The commit the working directory is based on (local, like Git's HEAD).
    pub fn head(&self) -> Result<Option<String>> {
        let s = fs::read_to_string(self.dir.join("HEAD")).unwrap_or_default();
        let s = s.trim();
        Ok(if s.is_empty() { None } else { Some(s.to_string()) })
    }

    pub fn set_head(&self, id: Option<&str>) -> Result<()> {
        fs::write(self.dir.join("HEAD"), id.unwrap_or(""))?;
        Ok(())
    }

    pub fn index(&self) -> Result<Index> {
        Ok(wal::load_index(self.store())?.0)
    }

    pub fn cache_path(&self) -> PathBuf {
        self.dir.join("stat-cache.json")
    }

    pub fn load_cache(&self) -> StatCache {
        StatCache::load(&self.cache_path())
    }

    pub fn head_manifest(&self) -> Result<Manifest> {
        match self.head()? {
            Some(id) => self.manifest_of(&id),
            None => Ok(Manifest::default()),
        }
    }

    pub fn commit(&self, id: &str) -> Result<Commit> {
        Commit::load(self.store(), id)
    }

    pub fn manifest_of(&self, commit_id: &str) -> Result<Manifest> {
        Manifest::load(self.store(), &self.commit(commit_id)?.manifest)
    }

    /// Resolves `HEAD`, `HEAD~n`, `latest`/`main`, a tag name or a (prefix of a) commit id.
    pub fn resolve(&self, rev: &str) -> Result<String> {
        let (base, back) = match rev.split_once('~') {
            Some((b, n)) => (b, n.parse::<usize>().with_context(|| format!("ungültige Revision {rev}"))?),
            None => (rev, 0),
        };
        let index = self.index()?;
        let mut id = match base {
            "HEAD" | "" => self.head()?.context("Es gibt noch keinen Commit")?,
            "latest" | "main" => index.head.clone().context("Es gibt noch keinen Commit")?,
            _ => {
                if let Some(id) = index.tags.get(base) {
                    id.clone()
                } else {
                    let base = base.to_ascii_lowercase();
                    if base.len() < 4 || !base.chars().all(|c| c.is_ascii_hexdigit()) {
                        bail!("Unbekannte Revision oder Tag: {rev}");
                    }
                    let matches: Vec<String> = self
                        .history_from(index.head.as_deref())?
                        .into_iter()
                        .map(|(id, _)| id)
                        .filter(|id| id.starts_with(&base))
                        .collect();
                    match matches.len() {
                        0 => bail!("Kein Commit beginnt mit {base}"),
                        1 => matches.into_iter().next().unwrap(),
                        _ => bail!("{base} ist mehrdeutig, bitte mehr Zeichen angeben"),
                    }
                }
            }
        };
        for _ in 0..back {
            id = self.commit(&id)?.parent.with_context(|| format!("{rev}: so weit reicht die Historie nicht zurück"))?;
        }
        Ok(id)
    }

    /// All commits reachable from `start`, newest first.
    pub fn history_from(&self, start: Option<&str>) -> Result<Vec<(String, Commit)>> {
        let mut out = Vec::new();
        let mut cur = start.map(str::to_string);
        while let Some(id) = cur {
            let c = self.commit(&id)?;
            cur = c.parent.clone();
            out.push((id, c));
        }
        Ok(out)
    }

    /// Converts a user-supplied path (relative to the cwd or absolute) to a repo path.
    pub fn to_repo_path(&self, input: &str) -> Result<String> {
        let abs = std::path::absolute(input)?;
        let rel = match abs.strip_prefix(&self.root) {
            Ok(r) => r.to_path_buf(),
            Err(_) => PathBuf::from(input),
        };
        let parts: Vec<String> = rel
            .components()
            .filter(|c| !matches!(c, std::path::Component::CurDir))
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect();
        if parts.iter().any(|p| p == ".." || p.contains(':')) {
            bail!("{input} liegt außerhalb des Repos");
        }
        Ok(parts.join("/"))
    }
}

pub fn author() -> String {
    let user = std::env::var("CRATE_AUTHOR")
        .or_else(|_| std::env::var("USERNAME"))
        .or_else(|_| std::env::var("USER"))
        .unwrap_or_else(|_| "unbekannt".into());
    if user.contains('@') {
        return user;
    }
    match std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")) {
        Ok(host) => format!("{user}@{host}"),
        Err(_) => user,
    }
}
