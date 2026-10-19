//! Object storage abstraction. Keys are `/`-separated strings such as
//! `chunks/ab/cdef…`, `commits/<id>`, `wal/<seq>` or `index`.
//!
//! The semantics mirror what S3 offers (PUT, GET, conditional PUT via
//! `If-None-Match` / `If-Match`), so an S3 backend can implement the same trait.

mod local;

use std::time::SystemTime;

use anyhow::Result;

pub use local::LocalFsStore;

/// Opaque version tag of an object, used for compare-and-swap.
pub type Etag = String;

#[derive(Debug, Clone)]
pub struct ObjectInfo {
    pub key: String,
    pub size: u64,
    pub modified: SystemTime,
}

pub trait BlobStore: Send + Sync {
    fn get(&self, key: &str) -> Result<Option<Vec<u8>>>;

    /// Unconditional atomic write; readers see either the old or the new object.
    fn put(&self, key: &str, data: &[u8]) -> Result<()>;

    /// Writes only if the key does not exist yet. Returns `false` if it already existed.
    fn put_if_absent(&self, key: &str, data: &[u8]) -> Result<bool>;

    fn exists(&self, key: &str) -> Result<bool>;

    fn delete(&self, key: &str) -> Result<()>;

    fn list(&self, prefix: &str) -> Result<Vec<ObjectInfo>>;

    fn get_versioned(&self, key: &str) -> Result<Option<(Vec<u8>, Etag)>>;

    /// Replaces the object only if its current version equals `expected`
    /// (`None` = object must not exist). Returns `false` on conflict.
    fn cas_put(&self, key: &str, expected: Option<&Etag>, data: &[u8]) -> Result<bool>;
}
