//! Getting a font into the cache (docs/plans/fonts.md §4): each source in turn, the bytes checked
//! against the catalogue before they're kept. The download itself is the app's: the desktop uses
//! its HTTP client, the phones their platform's, so proxies, VPNs and background rules apply.

use std::path::PathBuf;

use crate::cache::Cache;
use crate::catalog::{self, Blob, Catalog, Family, FontFile};
use crate::fontref::FontRef;
use crate::{Error, Result, resolve};

/// Downloads a URL. Implemented by each app.
pub trait Fetcher {
    fn fetch(&self, url: &str) -> std::result::Result<Vec<u8>, String>;
}

impl<F: Fn(&str) -> std::result::Result<Vec<u8>, String>> Fetcher for F {
    fn fetch(&self, url: &str) -> std::result::Result<Vec<u8>, String> {
        self(url)
    }
}

/// Makes sure one file is in the cache, and returns where.
pub fn ensure_blob(
    catalog: &Catalog,
    family: &str,
    blob: &Blob,
    cache: &mut Cache,
    fetcher: &dyn Fetcher,
) -> Result<PathBuf> {
    if let Some(p) = cache.path(blob) {
        cache.touch(blob, family)?;
        return Ok(p);
    }
    let mut tried = Vec::new();
    let repository_urls = 2; // the first two of `Catalog::urls` serve the repository's bytes
    for (i, url) in catalog.urls(blob).into_iter().enumerate() {
        match fetcher.fetch(&url) {
            Ok(bytes) => match catalog::verify(blob, &bytes, i < repository_urls) {
                Ok(()) => return cache.insert(family, blob, &bytes),
                Err(e) => {
                    tracing::warn!("{url}: {e}");
                    tried.push(format!("{url}: {e}"));
                }
            },
            Err(e) => tried.push(format!("{url}: {e}")),
        }
    }
    Err(Error::NoSource { file: blob.path.clone(), tried: tried.join("; ") })
}

/// Makes sure a family's file and its licence are in the cache; returns the font's path.
pub fn ensure_file(
    catalog: &Catalog,
    family: &Family,
    file: &FontFile,
    cache: &mut Cache,
    fetcher: &dyn Fetcher,
) -> Result<PathBuf> {
    let path = ensure_blob(catalog, &family.name, &file.blob, cache, fetcher)?;
    if let Some(licence) = &family.licence_file
        && let Err(e) = ensure_blob(catalog, &family.name, licence, cache, fetcher)
    {
        // the font is usable; the licence comes on the next attempt
        tracing::warn!("{}: licence not fetched: {e}", family.name);
    }
    Ok(path)
}

/// Makes sure the file a reference needs is in the cache; returns its path.
pub fn ensure(catalog: &Catalog, want: &FontRef, cache: &mut Cache, fetcher: &dyn Fetcher) -> Result<PathBuf> {
    let (family, file) = resolve::in_catalog(catalog, want).ok_or_else(|| Error::NotInCatalog(want.family.clone()))?;
    ensure_file(catalog, family, file, cache, fetcher)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::tests::sample;
    use std::cell::RefCell;

    #[test]
    fn tries_sources_in_order_and_keeps_only_checked_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let mut cache = Cache::open(dir.path()).unwrap();
        let c = sample();
        let asked = RefCell::new(Vec::new());
        // jsDelivr sends the wrong bytes, GitHub the right ones
        let fetcher = |url: &str| {
            asked.borrow_mut().push(url.to_string());
            if url.contains("jsdelivr") { Ok(b"wrong\n".to_vec()) } else { Ok(b"hello\n".to_vec()) }
        };
        let p = ensure(&c, &FontRef::new("Lato", 700), &mut cache, &fetcher).unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"hello\n");
        assert_eq!(asked.borrow().len(), 2);
        // cached now: no more requests
        ensure(&c, &FontRef::new("Lato", 700), &mut cache, &fetcher).unwrap();
        assert_eq!(asked.borrow().len(), 2);
    }

    #[test]
    fn says_why_nothing_worked() {
        let dir = tempfile::tempdir().unwrap();
        let mut cache = Cache::open(dir.path()).unwrap();
        let c = sample();
        let fetcher = |_: &str| Err::<Vec<u8>, _>("offline".to_string());
        let e = ensure(&c, &FontRef::new("Inter", 400), &mut cache, &fetcher).unwrap_err();
        assert!(matches!(e, Error::NoSource { .. }));
        assert!(matches!(ensure(&c, &FontRef::new("Nope", 400), &mut cache, &fetcher), Err(Error::NotInCatalog(_))));
    }
}
