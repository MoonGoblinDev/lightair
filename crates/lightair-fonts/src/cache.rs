//! Downloaded font files and their licences, kept by content (docs/plans/fonts.md §5). Files sit
//! in `<root>/files/<git hash>.<ext>`, written atomically after they were checked; an index
//! records what each is and when it was last used. Fonts a saved project uses are never evicted.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::Result;
use crate::catalog::Blob;

/// The default size the cache is trimmed to (1 GB).
pub const DEFAULT_LIMIT: u64 = 1 << 30;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    /// The family it belongs to.
    pub family: String,
    /// Its path in the catalogue's repository.
    pub path: String,
    pub size: u64,
    /// Unix seconds.
    pub last_used: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Index {
    /// By git hash.
    entries: BTreeMap<String, Entry>,
}

pub struct Cache {
    root: PathBuf,
    index: Index,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn extension(path: &str) -> &str {
    Path::new(path).extension().and_then(|e| e.to_str()).unwrap_or("bin")
}

impl Cache {
    /// Opens (or starts) a cache at `root`. An unreadable index starts empty; files without an
    /// entry are re-adopted the next time they're asked for.
    pub fn open(root: impl Into<PathBuf>) -> Result<Cache> {
        let root = root.into();
        std::fs::create_dir_all(root.join("files"))?;
        let index = std::fs::read_to_string(root.join("index.json"))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        Ok(Cache { root, index })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn file_path(&self, blob: &Blob) -> PathBuf {
        self.root.join("files").join(format!("{}.{}", blob.git_sha1, extension(&blob.path)))
    }

    /// Where the file is, if it's here.
    pub fn path(&self, blob: &Blob) -> Option<PathBuf> {
        let p = self.file_path(blob);
        p.is_file().then_some(p)
    }

    /// Adds checked bytes (the caller verified them, [`crate::catalog::verify`]) and returns
    /// where they are.
    pub fn insert(&mut self, family: &str, blob: &Blob, bytes: &[u8]) -> Result<PathBuf> {
        let dest = self.file_path(blob);
        let tmp = dest.with_extension("part");
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, &dest)?;
        self.index.entries.insert(
            blob.git_sha1.clone(),
            Entry { family: family.to_string(), path: blob.path.clone(), size: bytes.len() as u64, last_used: now() },
        );
        self.save()?;
        Ok(dest)
    }

    /// Marks a file used now (eviction keeps recently used fonts longest).
    pub fn touch(&mut self, blob: &Blob, family: &str) -> Result<()> {
        let size = self.path(blob).and_then(|p| std::fs::metadata(p).ok()).map(|m| m.len()).unwrap_or(blob.size);
        let e = self.index.entries.entry(blob.git_sha1.clone()).or_insert_with(|| Entry {
            family: family.to_string(),
            path: blob.path.clone(),
            size,
            last_used: 0,
        });
        e.last_used = now();
        self.save()
    }

    pub fn entries(&self) -> impl Iterator<Item = (&String, &Entry)> {
        self.index.entries.iter()
    }

    /// Bytes held.
    pub fn size(&self) -> u64 {
        self.index.entries.values().map(|e| e.size).sum()
    }

    /// Removes least-recently-used files until the cache is within `limit` bytes, never one whose
    /// git hash is in `keep` (fonts saved projects use, and their licences). Returns bytes freed.
    pub fn evict(&mut self, limit: u64, keep: &HashSet<String>) -> Result<u64> {
        let mut order: Vec<(String, u64, u64)> = self
            .index
            .entries
            .iter()
            .filter(|(k, _)| !keep.contains(*k))
            .map(|(k, e)| (k.clone(), e.last_used, e.size))
            .collect();
        order.sort_by_key(|(_, used, _)| *used);
        let mut total = self.size();
        let mut freed = 0;
        for (key, _, size) in order {
            if total <= limit {
                break;
            }
            if let Some(e) = self.index.entries.remove(&key) {
                let p = self.root.join("files").join(format!("{key}.{}", extension(&e.path)));
                let _ = std::fs::remove_file(p);
                total -= size;
                freed += size;
            }
        }
        self.save()?;
        Ok(freed)
    }

    fn save(&self) -> Result<()> {
        let tmp = self.root.join("index.json.part");
        std::fs::write(&tmp, serde_json::to_vec(&self.index)?)?;
        std::fs::rename(tmp, self.root.join("index.json"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hash::git_blob_sha1;

    fn blob(path: &str, bytes: &[u8]) -> Blob {
        Blob { path: path.into(), size: bytes.len() as u64, git_sha1: git_blob_sha1(bytes), sha256: None, urls: vec![] }
    }

    #[test]
    fn keeps_files_by_content_and_survives_reopening() {
        let dir = tempfile::tempdir().unwrap();
        let a = blob("ofl/a/A.ttf", b"aaaa");
        {
            let mut c = Cache::open(dir.path()).unwrap();
            assert!(c.path(&a).is_none());
            let p = c.insert("A", &a, b"aaaa").unwrap();
            assert_eq!(std::fs::read(p).unwrap(), b"aaaa");
        }
        let c = Cache::open(dir.path()).unwrap();
        assert!(c.path(&a).is_some());
        assert_eq!(c.size(), 4);
    }

    #[test]
    fn evicts_oldest_first_but_never_kept_fonts() {
        let dir = tempfile::tempdir().unwrap();
        let mut c = Cache::open(dir.path()).unwrap();
        let (a, b, k) = (blob("a.ttf", b"1111"), blob("b.ttf", b"2222"), blob("k.ttf", b"3333"));
        c.insert("A", &a, b"1111").unwrap();
        c.insert("B", &b, b"2222").unwrap();
        c.insert("K", &k, b"3333").unwrap();
        // a is the oldest; k is kept even though it's in the way
        c.index.entries.get_mut(&a.git_sha1).unwrap().last_used = 1;
        c.index.entries.get_mut(&k.git_sha1).unwrap().last_used = 0;
        let keep: HashSet<String> = [k.git_sha1.clone()].into();
        let freed = c.evict(8, &keep).unwrap();
        assert_eq!(freed, 4);
        assert!(c.path(&a).is_none());
        assert!(c.path(&b).is_some() && c.path(&k).is_some());
    }
}
