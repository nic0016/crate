//! Content-defined chunking, hashing and compression of file contents.
//!
//! A file is split with FastCDC into chunks of ~1 MB (256 KB – 4 MB). Each chunk is
//! addressed by its BLAKE3 hash and stored once, so unchanged regions of large
//! assemblies or STEP exports are shared between versions and between files.

use std::fs::File;
use std::io::{BufReader, Read, Write};
use std::path::Path;

use anyhow::{bail, Context, Result};
use fastcdc::v2020::StreamCDC;
use flate2::read::DeflateDecoder;
use flate2::write::DeflateEncoder;
use flate2::Compression;

use crate::store::BlobStore;

pub const MIN_CHUNK: u32 = 256 * 1024;
pub const AVG_CHUNK: u32 = 1024 * 1024;
pub const MAX_CHUNK: u32 = 4 * 1024 * 1024;

const CODEC_RAW: u8 = 0;
const CODEC_DEFLATE: u8 = 1;

pub fn chunk_key(hash: &str) -> String {
    format!("chunks/{}/{}", &hash[..2], &hash[2..])
}

/// Prefixes the payload with a codec byte. Falls back to raw storage when compression
/// does not help (e.g. ZIP archives or already compressed CAD containers).
pub fn encode(data: &[u8]) -> Result<Vec<u8>> {
    let mut enc = DeflateEncoder::new(Vec::with_capacity(data.len() / 2 + 1), Compression::new(6));
    enc.write_all(data)?;
    let compressed = enc.finish()?;
    let mut out = Vec::with_capacity(compressed.len().min(data.len()) + 1);
    if compressed.len() < data.len() * 95 / 100 {
        out.push(CODEC_DEFLATE);
        out.extend_from_slice(&compressed);
    } else {
        out.push(CODEC_RAW);
        out.extend_from_slice(data);
    }
    Ok(out)
}

pub fn decode(stored: &[u8]) -> Result<Vec<u8>> {
    let (&codec, payload) = stored.split_first().context("leeres Objekt")?;
    match codec {
        CODEC_RAW => Ok(payload.to_vec()),
        CODEC_DEFLATE => {
            let mut out = Vec::new();
            DeflateDecoder::new(payload).read_to_end(&mut out)?;
            Ok(out)
        }
        other => bail!("unbekannter Codec {other}"),
    }
}

/// Result of storing one file.
pub struct StoredFile {
    pub hash: String,
    pub size: u64,
    pub chunks: Vec<String>,
    /// Bytes actually written to the store (after compression, only new chunks).
    pub new_bytes: u64,
}

/// Splits `path` into chunks, uploads missing chunks and returns the chunk list.
/// `on_progress` is called with the number of input bytes processed.
pub fn store_file(store: &dyn BlobStore, path: &Path, on_progress: &(dyn Fn(u64) + Sync)) -> Result<StoredFile> {
    let file = File::open(path).with_context(|| format!("{} öffnen (in CAD noch geöffnet?)", path.display()))?;
    let reader = BufReader::with_capacity(1 << 20, file);
    let mut file_hasher = blake3::Hasher::new();
    let mut chunks = Vec::new();
    let mut size = 0u64;
    let mut new_bytes = 0u64;

    for chunk in StreamCDC::new(reader, MIN_CHUNK, AVG_CHUNK, MAX_CHUNK) {
        let chunk = chunk.map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
        file_hasher.update(&chunk.data);
        let hash = blake3::hash(&chunk.data).to_hex().to_string();
        let key = chunk_key(&hash);
        if !store.exists(&key)? {
            let encoded = encode(&chunk.data)?;
            if store.put_if_absent(&key, &encoded)? {
                new_bytes += encoded.len() as u64;
            }
        }
        size += chunk.length as u64;
        on_progress(chunk.length as u64);
        chunks.push(hash);
    }

    Ok(StoredFile { hash: file_hasher.finalize().to_hex().to_string(), size, chunks, new_bytes })
}

/// Hashes a file without storing it (used by `status`).
pub fn hash_file(path: &Path) -> Result<String> {
    let mut file = File::open(path).with_context(|| format!("{} öffnen", path.display()))?;
    let mut hasher = blake3::Hasher::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher.finalize().to_hex().to_string())
}

/// Reassembles a file from its chunks into `out`, verifying every chunk and the whole file.
pub fn restore_to(store: &dyn BlobStore, chunks: &[String], expected_hash: &str, out: &mut dyn Write) -> Result<u64> {
    let mut file_hasher = blake3::Hasher::new();
    let mut size = 0u64;
    for hash in chunks {
        let stored = store.get(&chunk_key(hash))?.with_context(|| format!("Chunk {hash} fehlt im Speicher"))?;
        let data = decode(&stored)?;
        if blake3::hash(&data).to_hex().as_str() != hash {
            bail!("Chunk {hash} ist beschädigt");
        }
        file_hasher.update(&data);
        out.write_all(&data)?;
        size += data.len() as u64;
    }
    if file_hasher.finalize().to_hex().as_str() != expected_hash {
        bail!("Prüfsumme der wiederhergestellten Datei stimmt nicht");
    }
    Ok(size)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::LocalFsStore;

    fn pseudo_random(len: usize, seed: u64) -> Vec<u8> {
        let mut x = seed;
        (0..len)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                x as u8
            })
            .collect()
    }

    #[test]
    fn codec_roundtrip() {
        let text = b"SolidWorks ".repeat(1000);
        let enc = encode(&text).unwrap();
        assert_eq!(enc[0], CODEC_DEFLATE);
        assert_eq!(decode(&enc).unwrap(), text);
        let noise = pseudo_random(10_000, 7);
        let enc = encode(&noise).unwrap();
        assert_eq!(enc[0], CODEC_RAW);
        assert_eq!(decode(&enc).unwrap(), noise);
    }

    #[test]
    fn store_and_restore_with_dedup() {
        let dir = tempfile::tempdir().unwrap();
        let store = LocalFsStore::open(dir.path().join("objects")).unwrap();
        let mut data = pseudo_random(9 * 1024 * 1024, 42);
        let file = dir.path().join("teil.SLDPRT");
        std::fs::write(&file, &data).unwrap();

        let a = store_file(&store, &file, &|_| {}).unwrap();
        assert_eq!(a.size, data.len() as u64);
        assert!(a.chunks.len() >= 3);

        // Small edit near the end: most chunks must be reused.
        let n = data.len();
        data[n - 1000..n - 900].fill(0xAB);
        std::fs::write(&file, &data).unwrap();
        let b = store_file(&store, &file, &|_| {}).unwrap();
        let shared = b.chunks.iter().filter(|c| a.chunks.contains(c)).count();
        assert!(shared >= b.chunks.len() - 2, "shared {shared} of {}", b.chunks.len());
        assert!(b.new_bytes < a.new_bytes / 2);

        let mut out = Vec::new();
        restore_to(&store, &b.chunks, &b.hash, &mut out).unwrap();
        assert_eq!(out, data);
        assert_eq!(hash_file(&file).unwrap(), b.hash);
    }
}
