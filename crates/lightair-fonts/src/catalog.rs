//! The font catalogue: every Google Fonts family Lightair may ship, with its files, their exact
//! hashes, and where to download them (docs/plans/fonts.md §4).
//!
//! `cargo xtask fonts-catalog` writes it from `google/fonts` at a pinned commit. Each app ships
//! the catalogue it was built with and may switch to a newer published one ([`Catalog::newer`]).

use serde::{Deserialize, Serialize};

use crate::fontref::Style;
use crate::{Error, Result, hash};

/// The format version this code reads and writes.
pub const VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Catalog {
    pub version: u32,
    /// Where the files are: the repository and the commit every path is read at.
    pub source: Source,
    /// When it was generated (RFC 3339).
    pub generated: String,
    pub families: Vec<Family>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    /// `owner/name` on GitHub: `google/fonts`.
    pub repo: String,
    pub commit: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Family {
    pub name: String,
    pub category: Category,
    #[serde(default)]
    pub designer: String,
    pub licence: Licence,
    /// The family's folder in the repository (`ofl/inter`).
    pub dir: String,
    /// The licence text that travels with the files (`ofl/inter/OFL.txt`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub licence_file: Option<Blob>,
    /// Scripts it covers (`latin`, `cyrillic`, `japanese`…).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scripts: Vec<String>,
    /// Google Fonts' popularity rank, 1 the most used; absent when the catalogue was built
    /// without the API.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub popularity: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added: Option<String>,
    pub files: Vec<FontFile>,
}

/// A file in the repository, named by its exact bytes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Blob {
    /// From the repository's root (`ofl/inter/Inter[opsz,wght].ttf`).
    pub path: String,
    /// Bytes; 0 when the catalogue couldn't learn it.
    pub size: u64,
    /// Git's blob hash of the file at the catalogue's commit.
    pub git_sha1: String,
    /// SHA-256 of the bytes behind `urls`, when those differ from the repository's file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// More places to get it (Google's own font server), used only when `sha256` says what they
    /// must deliver.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub urls: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FontFile {
    #[serde(flatten)]
    pub blob: Blob,
    #[serde(default)]
    pub style: Style,
    /// Its weight (a variable font's default; its range is in `axes`).
    pub weight: u16,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub axes: Vec<Axis>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Axis {
    /// `wght`, `wdth`, `slnt`, `opsz`…
    pub tag: String,
    pub min: f32,
    pub max: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Sans,
    Serif,
    Display,
    Handwriting,
    Mono,
}

impl Category {
    /// From `METADATA.pb`'s names (`SANS_SERIF`, `MONOSPACE`…).
    pub fn from_google(name: &str) -> Option<Category> {
        Some(match name {
            "SANS_SERIF" => Category::Sans,
            "SERIF" => Category::Serif,
            "DISPLAY" => Category::Display,
            "HANDWRITING" => Category::Handwriting,
            "MONOSPACE" => Category::Mono,
            _ => return None,
        })
    }
}

/// The three licences Google Fonts uses; all allow bundling, redistribution and commercial use
/// (docs/plans/fonts.md §2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Licence {
    /// SIL Open Font License 1.1.
    Ofl,
    /// Apache License 2.0.
    Apache,
    /// Ubuntu Font Licence 1.0.
    Ufl,
}

impl Licence {
    /// From the repository's top folder (`ofl`, `apache`, `ufl`); anything else isn't shipped.
    pub fn from_dir(dir: &str) -> Option<Licence> {
        Some(match dir {
            "ofl" => Licence::Ofl,
            "apache" => Licence::Apache,
            "ufl" => Licence::Ufl,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Licence::Ofl => "SIL Open Font License 1.1",
            Licence::Apache => "Apache License 2.0",
            Licence::Ufl => "Ubuntu Font Licence 1.0",
        }
    }
}

impl Catalog {
    pub fn from_json(text: &str) -> Result<Catalog> {
        Ok(serde_json::from_str(text)?)
    }

    pub fn to_json(&self) -> Result<String> {
        Ok(serde_json::to_string(self)?)
    }

    /// A family by name, ignoring case.
    pub fn family(&self, name: &str) -> Option<&Family> {
        self.families.iter().find(|f| f.name.eq_ignore_ascii_case(name))
    }

    /// Families matching a search, best first: names that start with the query, then names that
    /// contain it, each by popularity (then name). An empty query lists everything.
    pub fn search(&self, query: &str, category: Option<Category>, script: Option<&str>) -> Vec<&Family> {
        let q = query.trim().to_lowercase();
        let mut hits: Vec<(u8, &Family)> = self
            .families
            .iter()
            .filter(|f| category.is_none_or(|c| f.category == c))
            .filter(|f| script.is_none_or(|s| f.scripts.iter().any(|x| x.eq_ignore_ascii_case(s))))
            .filter_map(|f| {
                let name = f.name.to_lowercase();
                if q.is_empty() || name.starts_with(&q) {
                    Some((0, f))
                } else if name.contains(&q) {
                    Some((1, f))
                } else {
                    None
                }
            })
            .collect();
        hits.sort_by(|(ra, a), (rb, b)| {
            ra.cmp(rb)
                .then(a.popularity.unwrap_or(u32::MAX).cmp(&b.popularity.unwrap_or(u32::MAX)))
                .then(a.name.cmp(&b.name))
        });
        hits.into_iter().map(|(_, f)| f).collect()
    }

    /// Where to download a file from, in order: jsDelivr and GitHub serve the repository's exact
    /// bytes (checked against `git_sha1`); the blob's own `urls` (Google's server) only when the
    /// catalogue recorded their SHA-256.
    pub fn urls(&self, blob: &Blob) -> Vec<String> {
        let path = blob.path.split('/').map(escape_path_segment).collect::<Vec<_>>().join("/");
        let mut urls = vec![
            format!("https://cdn.jsdelivr.net/gh/{}@{}/{}", self.source.repo, self.source.commit, path),
            format!("https://raw.githubusercontent.com/{}/{}/{}", self.source.repo, self.source.commit, path),
        ];
        if blob.sha256.is_some() {
            urls.extend(blob.urls.iter().cloned());
        }
        urls
    }

    /// The newer of two catalogues (a published one replaces the shipped one only when it is
    /// newer and readable by this version).
    pub fn newer<'a>(&'a self, other: &'a Catalog) -> &'a Catalog {
        if other.version <= VERSION && other.generated > self.generated { other } else { self }
    }
}

/// Percent-encodes what a URL path can't carry as is: variable fonts' file names hold `[`, `]`
/// and `,` (`Inter[opsz,wght].ttf`).
fn escape_path_segment(segment: &str) -> String {
    let mut out = String::with_capacity(segment.len());
    for b in segment.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// Checks downloaded bytes against what the catalogue says they are.
pub fn verify(blob: &Blob, bytes: &[u8], from_repository: bool) -> Result<()> {
    let file = blob.path.clone();
    if from_repository {
        if blob.size > 0 && bytes.len() as u64 != blob.size {
            return Err(Error::Integrity { file, why: format!("{} bytes, expected {}", bytes.len(), blob.size) });
        }
        let got = hash::git_blob_sha1(bytes);
        if got != blob.git_sha1 {
            return Err(Error::Integrity { file, why: format!("git hash {got}, expected {}", blob.git_sha1) });
        }
    } else {
        let Some(want) = &blob.sha256 else {
            return Err(Error::Integrity { file, why: "no SHA-256 to check it against".into() });
        };
        let got = hash::sha256(bytes);
        if &got != want {
            return Err(Error::Integrity { file, why: format!("SHA-256 {got}, expected {want}") });
        }
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub fn sample() -> Catalog {
        let file = |path: &str, style, weight, axes: Vec<Axis>| FontFile {
            blob: Blob {
                path: path.into(),
                size: 6,
                git_sha1: hash::git_blob_sha1(b"hello\n"),
                sha256: None,
                urls: vec![],
            },
            style,
            weight,
            axes,
        };
        let wght = |min, max| vec![Axis { tag: "wght".into(), min, max }];
        Catalog {
            version: VERSION,
            source: Source { repo: "google/fonts".into(), commit: "abc123".into() },
            generated: "2026-11-20T12:00:00Z".into(),
            families: vec![
                Family {
                    name: "Inter".into(),
                    category: Category::Sans,
                    designer: "Rasmus Andersson".into(),
                    licence: Licence::Ofl,
                    dir: "ofl/inter".into(),
                    licence_file: None,
                    scripts: vec!["latin".into(), "cyrillic".into(), "greek".into()],
                    popularity: Some(20),
                    added: None,
                    files: vec![
                        file("ofl/inter/Inter[opsz,wght].ttf", Style::Normal, 400, wght(100.0, 900.0)),
                        file("ofl/inter/Inter-Italic[opsz,wght].ttf", Style::Italic, 400, wght(100.0, 900.0)),
                    ],
                },
                Family {
                    name: "Lato".into(),
                    category: Category::Sans,
                    designer: String::new(),
                    licence: Licence::Ofl,
                    dir: "ofl/lato".into(),
                    licence_file: None,
                    scripts: vec!["latin".into()],
                    popularity: Some(8),
                    added: None,
                    files: vec![
                        file("ofl/lato/Lato-Light.ttf", Style::Normal, 300, vec![]),
                        file("ofl/lato/Lato-Regular.ttf", Style::Normal, 400, vec![]),
                        file("ofl/lato/Lato-Bold.ttf", Style::Normal, 700, vec![]),
                        file("ofl/lato/Lato-Black.ttf", Style::Normal, 900, vec![]),
                        file("ofl/lato/Lato-Italic.ttf", Style::Italic, 400, vec![]),
                    ],
                },
                Family {
                    name: "Noto Sans JP".into(),
                    category: Category::Sans,
                    designer: "Google".into(),
                    licence: Licence::Ofl,
                    dir: "ofl/notosansjp".into(),
                    licence_file: None,
                    scripts: vec!["japanese".into(), "latin".into()],
                    popularity: Some(15),
                    added: None,
                    files: vec![file("ofl/notosansjp/NotoSansJP[wght].ttf", Style::Normal, 400, wght(100.0, 900.0))],
                },
                Family {
                    name: "Bebas Neue".into(),
                    category: Category::Display,
                    designer: String::new(),
                    licence: Licence::Ofl,
                    dir: "ofl/bebasneue".into(),
                    licence_file: None,
                    scripts: vec!["latin".into()],
                    popularity: None,
                    added: None,
                    files: vec![file("ofl/bebasneue/BebasNeue-Regular.ttf", Style::Normal, 400, vec![])],
                },
            ],
        }
    }

    #[test]
    fn search_orders_prefix_then_contains_then_popularity() {
        let c = sample();
        let names = |v: Vec<&Family>| v.into_iter().map(|f| f.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(c.search("", None, None)), ["Lato", "Noto Sans JP", "Inter", "Bebas Neue"]);
        assert_eq!(names(c.search("in", None, None)), ["Inter"]);
        assert_eq!(names(c.search("sans", None, None)), ["Noto Sans JP"]);
        assert_eq!(names(c.search("", Some(Category::Display), None)), ["Bebas Neue"]);
        assert_eq!(names(c.search("", None, Some("japanese"))), ["Noto Sans JP"]);
        assert!(c.family("inter").is_some());
    }

    #[test]
    fn urls_escape_variable_font_names_and_skip_unchecked_mirrors() {
        let c = sample();
        let mut blob = c.families[0].files[0].blob.clone();
        blob.urls = vec!["https://fonts.gstatic.com/s/inter/v1/x.ttf".into()];
        let urls = c.urls(&blob);
        assert_eq!(urls[0], "https://cdn.jsdelivr.net/gh/google/fonts@abc123/ofl/inter/Inter%5Bopsz%2Cwght%5D.ttf");
        assert_eq!(urls.len(), 2, "Google's link isn't used without a SHA-256");
        blob.sha256 = Some("00".into());
        assert_eq!(c.urls(&blob).len(), 3);
    }

    #[test]
    fn verify_checks_size_and_hash() {
        let c = sample();
        let blob = &c.families[0].files[0].blob;
        assert!(verify(blob, b"hello\n", true).is_ok());
        assert!(verify(blob, b"hellO\n", true).is_err());
        assert!(verify(blob, b"hello", true).is_err());
        assert!(verify(blob, b"hello\n", false).is_err(), "Google's bytes need a SHA-256");
    }

    #[test]
    fn json_round_trips() {
        let c = sample();
        let back = Catalog::from_json(&c.to_json().unwrap()).unwrap();
        assert_eq!(back.families.len(), 4);
        assert_eq!(back.families[0].files[0], c.families[0].files[0]);
    }
}
