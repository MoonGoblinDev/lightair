//! `cargo xtask fonts-catalog` and `cargo xtask fonts-bundle` (docs/plans/fonts.md §3, §4).
//!
//! The catalogue is read from `google/fonts` at a pinned commit without downloading its fonts: a
//! shallow, blob-less clone checks out only the families' `METADATA.pb` files and gives every
//! file's git hash; GitHub's tree API gives their sizes in one request per licence folder (asking
//! git for sizes would download every font). The bundle downloads the bundled families'
//! files from the same commit and checks each against the catalogue.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, bail};
use lightair_fonts::catalog::{self, Axis, Blob, Catalog, Category, Family, FontFile, Licence, Source};
use lightair_fonts::{Style, bundled};

const REPO: &str = "google/fonts";

/// The emoji font apps draw with: Noto Color Emoji's bitmap (CBDT) build, from Google's
/// `noto-emoji` repository (OFL, the same family name). `google/fonts` carries its COLRv1 build,
/// whose gradients swash (`lightair-render`'s rasteriser) doesn't draw.
const EMOJI_REPO: &str = "googlefonts/noto-emoji";
/// Tag v2.051.
const EMOJI_COMMIT: &str = "8998f5dd683424a73e2314a8c1f1e359c19e8742";
const EMOJI_PATH: &str = "fonts/NotoColorEmoji.ttf";

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().to_path_buf()
}

/// Where the shipped catalogue lives.
pub fn catalog_path() -> PathBuf {
    root().join("crates/lightair-fonts/catalog/fonts-catalog.json")
}

fn git(dir: &Path, args: &[&str]) -> anyhow::Result<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .with_context(|| format!("git {}", args.join(" ")))?;
    if !out.status.success() {
        bail!("git {} failed: {}", args.join(" "), String::from_utf8_lossy(&out.stderr));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// A shallow, blob-less checkout of `google/fonts` at `commit` (or its newest commit), with only
/// the families' `METADATA.pb` files on disk. Returns its folder and the commit.
fn checkout(commit: Option<&str>) -> anyhow::Result<(PathBuf, String)> {
    let dir = root().join("target/google-fonts");
    if !dir.join(".git").exists() {
        std::fs::create_dir_all(&dir)?;
        git(&dir, &["init", "-q"])?;
        git(&dir, &["remote", "add", "origin", &format!("https://github.com/{REPO}.git")])?;
        git(&dir, &["config", "remote.origin.promisor", "true"])?;
        git(&dir, &["config", "remote.origin.partialclonefilter", "blob:none"])?;
        git(
            &dir,
            &[
                "sparse-checkout",
                "set",
                "--no-cone",
                "/ofl/*/METADATA.pb",
                "/apache/*/METADATA.pb",
                "/ufl/*/METADATA.pb",
            ],
        )?;
    }
    let want = commit.unwrap_or("main");
    eprintln!("fetching {REPO} at {want} (metadata only)…");
    git(&dir, &["fetch", "-q", "--depth", "1", "--filter=blob:none", "origin", want])?;
    git(&dir, &["checkout", "-q", "--force", "FETCH_HEAD"])?;
    let commit = git(&dir, &["rev-parse", "HEAD"])?.trim().to_string();
    Ok((dir, commit))
}

/// The fields of a `METADATA.pb` the catalogue needs (protobuf text format, read line by line:
/// top-level fields, `fonts { }` and `axes { }` blocks; other blocks are skipped).
#[derive(Default, Debug)]
struct Metadata {
    name: String,
    designer: String,
    license: String,
    category: Option<String>,
    date_added: Option<String>,
    subsets: Vec<String>,
    fonts: Vec<MetaFont>,
    axes: Vec<Axis>,
}

#[derive(Default, Debug, Clone)]
struct MetaFont {
    style: String,
    weight: u16,
    filename: String,
}

fn unquote(v: &str) -> String {
    let v = v.trim();
    let v = v.strip_prefix('"').and_then(|s| s.strip_suffix('"')).unwrap_or(v);
    v.replace("\\\"", "\"").replace("\\'", "'")
}

fn parse_metadata(text: &str) -> Metadata {
    let mut m = Metadata::default();
    let mut stack: Vec<String> = Vec::new();
    let mut font = MetaFont::default();
    let (mut tag, mut min, mut max) = (String::new(), 0.0f32, 0.0f32);
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(name) = line.strip_suffix('{') {
            let name = name.trim().trim_end_matches(':').trim().to_string();
            if stack.is_empty() && name == "fonts" {
                font = MetaFont::default();
            }
            if stack.is_empty() && name == "axes" {
                (tag, min, max) = (String::new(), 0.0, 0.0);
            }
            stack.push(name);
            continue;
        }
        if line == "}" {
            match (stack.pop().as_deref(), stack.is_empty()) {
                (Some("fonts"), true) => m.fonts.push(font.clone()),
                (Some("axes"), true) => m.axes.push(Axis { tag: tag.clone(), min, max }),
                _ => {}
            }
            continue;
        }
        let Some((key, value)) = line.split_once(':') else { continue };
        let (key, value) = (key.trim(), value.trim());
        match (stack.as_slice(), key) {
            ([], "name") => m.name = unquote(value),
            ([], "designer") => m.designer = unquote(value),
            ([], "license") => m.license = unquote(value),
            ([], "category") if m.category.is_none() => m.category = Some(unquote(value)),
            ([], "date_added") => m.date_added = Some(unquote(value)),
            ([], "subsets") => m.subsets.push(unquote(value)),
            ([f], "style") if f == "fonts" => font.style = unquote(value),
            ([f], "weight") if f == "fonts" => font.weight = value.parse().unwrap_or(400),
            ([f], "filename") if f == "fonts" => font.filename = unquote(value),
            ([a], "tag") if a == "axes" => tag = unquote(value),
            ([a], "min_value") if a == "axes" => min = value.parse().unwrap_or(0.0),
            ([a], "max_value") if a == "axes" => max = value.parse().unwrap_or(0.0),
            _ => {}
        }
    }
    m
}

/// `cargo xtask fonts-catalog [--commit <sha>]`: writes `crates/lightair-fonts/catalog/fonts-catalog.json`.
/// With `GOOGLE_FONTS_API_KEY` set, families get Google Fonts' popularity ranks.
pub fn catalog(commit: Option<String>) -> anyhow::Result<()> {
    let (dir, commit) = checkout(commit.as_deref())?;
    // every file's git hash (local: trees came with the clone), then sizes from GitHub
    let mut blobs: HashMap<String, (u64, String)> = HashMap::new();
    for line in git(&dir, &["ls-tree", "-r", "HEAD", "--", "ofl", "apache", "ufl"])?.lines() {
        // <mode> blob <sha>\t<path>
        let Some((meta, path)) = line.split_once('\t') else { continue };
        let parts: Vec<&str> = meta.split_whitespace().collect();
        if parts.len() == 3 && parts[1] == "blob" {
            blobs.insert(path.to_string(), (0, parts[2].to_string()));
        }
    }
    let mut sized = 0;
    for top in ["ofl", "apache", "ufl"] {
        let tree = git(&dir, &["rev-parse", &format!("HEAD:{top}")])?.trim().to_string();
        match tree_sizes(&tree) {
            Ok(sizes) => {
                for (path, sha, size) in sizes {
                    if let Some(entry) = blobs.get_mut(&format!("{top}/{path}"))
                        && entry.1 == sha
                    {
                        entry.0 = size;
                        sized += 1;
                    }
                }
            }
            Err(e) => eprintln!("{top}: no sizes from GitHub ({e}); files in it are listed without one"),
        }
    }
    eprintln!("{} files, {sized} with sizes", blobs.len());
    let blob = |path: &str| {
        blobs.get(path).map(|(size, sha)| Blob {
            path: path.to_string(),
            size: *size,
            git_sha1: sha.clone(),
            sha256: None,
            urls: vec![],
        })
    };
    let popularity = popularity_ranks();
    let mut families = Vec::new();
    let mut skipped = 0;
    for top in ["ofl", "apache", "ufl"] {
        let licence = Licence::from_dir(top).unwrap();
        let Ok(entries) = std::fs::read_dir(dir.join(top)) else { continue };
        for entry in entries.flatten() {
            let meta_path = entry.path().join("METADATA.pb");
            let Ok(text) = std::fs::read_to_string(&meta_path) else { continue };
            let m = parse_metadata(&text);
            let folder = format!("{top}/{}", entry.file_name().to_string_lossy());
            let Some(category) = m.category.as_deref().and_then(Category::from_google) else {
                skipped += 1;
                continue;
            };
            let files: Vec<FontFile> = m
                .fonts
                .iter()
                .filter_map(|f| {
                    let b = blob(&format!("{folder}/{}", f.filename))?;
                    let axes = if f.filename.contains('[') { m.axes.clone() } else { vec![] };
                    Some(FontFile { blob: b, style: Style::from_google(&f.style), weight: f.weight, axes })
                })
                .collect();
            if m.name.is_empty() || files.is_empty() {
                skipped += 1;
                continue;
            }
            let licence_file =
                ["OFL.txt", "LICENSE.txt", "UFL.txt"].iter().find_map(|n| blob(&format!("{folder}/{n}")));
            families.push(Family {
                popularity: popularity.get(&m.name).copied(),
                name: m.name,
                category,
                designer: m.designer,
                licence,
                dir: folder,
                licence_file,
                scripts: m.subsets.into_iter().filter(|s| s != "menu").collect(),
                added: m.date_added,
                files,
            });
        }
    }
    families.sort_by(|a, b| a.name.cmp(&b.name));
    // two folders can carry one name (a family moved between licences); keep the first
    families.dedup_by(|a, b| a.name == b.name);
    use_bitmap_emoji(&mut families)?;
    let missing: Vec<&str> = bundled::FAMILIES
        .iter()
        .copied()
        .filter(|f| !families.iter().any(|x| x.name.eq_ignore_ascii_case(f)))
        .collect();
    if !missing.is_empty() {
        bail!("bundled families missing from {REPO} at {commit}: {}", missing.join(", "));
    }
    let catalog = Catalog {
        version: catalog::VERSION,
        source: Source { repo: REPO.into(), commit: commit.clone() },
        generated: now_rfc3339(),
        families,
    };
    let out = catalog_path();
    std::fs::create_dir_all(out.parent().unwrap())?;
    std::fs::write(&out, catalog.to_json()?)?;
    let files: usize = catalog.families.iter().map(|f| f.files.len()).sum();
    let bytes: u64 = catalog.families.iter().flat_map(|f| &f.files).map(|f| f.blob.size).sum();
    eprintln!(
        "{} families ({} files, {:.1} GB of fonts) from {REPO}@{} → {} ({} KB); {skipped} folders without a usable METADATA.pb skipped",
        catalog.families.len(),
        files,
        bytes as f64 / 1e9,
        &commit[..12],
        out.display(),
        std::fs::metadata(&out)?.len() / 1024
    );
    Ok(())
}

/// Points the emoji family at the bitmap build (see [`EMOJI_REPO`]): its files come from that
/// repository, checked by SHA-256 (they aren't in `google/fonts`); its licence stays the family's.
fn use_bitmap_emoji(families: &mut [Family]) -> anyhow::Result<()> {
    let Some(family) = families.iter_mut().find(|f| f.name == bundled::EMOJI) else {
        bail!("{} isn't in {REPO}", bundled::EMOJI)
    };
    let urls = vec![
        format!("https://cdn.jsdelivr.net/gh/{EMOJI_REPO}@{EMOJI_COMMIT}/{EMOJI_PATH}"),
        format!("https://raw.githubusercontent.com/{EMOJI_REPO}/{EMOJI_COMMIT}/{EMOJI_PATH}"),
    ];
    let bytes = urls
        .iter()
        .find_map(|u| fetch(u).ok())
        .with_context(|| format!("couldn't download {EMOJI_PATH} from {EMOJI_REPO}"))?;
    let blob = Blob {
        path: format!("{EMOJI_REPO}/{EMOJI_PATH}"),
        size: bytes.len() as u64,
        git_sha1: lightair_fonts::hash::git_blob_sha1(&bytes),
        sha256: Some(lightair_fonts::hash::sha256(&bytes)),
        urls,
    };
    family.files = vec![FontFile { blob, style: Style::Normal, weight: 400, axes: vec![] }];
    Ok(())
}

/// Every blob under a tree, from GitHub's tree API: (path within the tree, git hash, size).
fn tree_sizes(tree: &str) -> anyhow::Result<Vec<(String, String, u64)>> {
    let url = format!("https://api.github.com/repos/{REPO}/git/trees/{tree}?recursive=1");
    let out = Command::new("curl").args(["-sfL", "-H", "Accept: application/vnd.github+json", &url]).output()?;
    if !out.status.success() {
        bail!("GitHub answered with an error (rate limit?)");
    }
    let json: serde_json::Value = serde_json::from_slice(&out.stdout)?;
    if json["truncated"].as_bool() == Some(true) {
        eprintln!("GitHub truncated the tree {tree}; some sizes are missing");
    }
    Ok(json["tree"]
        .as_array()
        .map(|t| {
            t.iter()
                .filter(|e| e["type"] == "blob")
                .filter_map(|e| {
                    Some((e["path"].as_str()?.to_string(), e["sha"].as_str()?.to_string(), e["size"].as_u64()?))
                })
                .collect()
        })
        .unwrap_or_default())
}

/// Google Fonts' popularity ranks, by family name, when `GOOGLE_FONTS_API_KEY` is set.
fn popularity_ranks() -> HashMap<String, u32> {
    let Ok(key) = std::env::var("GOOGLE_FONTS_API_KEY") else { return HashMap::new() };
    let url = format!("https://www.googleapis.com/webfonts/v1/webfonts?key={key}&sort=popularity");
    let Ok(out) = Command::new("curl").args(["-sfL", &url]).output() else { return HashMap::new() };
    let Ok(json) = serde_json::from_slice::<serde_json::Value>(&out.stdout) else {
        eprintln!("the Google Fonts API didn't answer; no popularity ranks");
        return HashMap::new();
    };
    json["items"]
        .as_array()
        .map(|items| {
            items
                .iter()
                .enumerate()
                .filter_map(|(i, it)| Some((it["family"].as_str()?.to_string(), i as u32 + 1)))
                .collect()
        })
        .unwrap_or_default()
}

fn now_rfc3339() -> String {
    let out = Command::new("date").args(["-u", "+%Y-%m-%dT%H:%M:%SZ"]).output();
    out.ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default()
}

fn fetch(url: &str) -> Result<Vec<u8>, String> {
    let out = Command::new("curl").args(["-sfL", "--retry", "2", url]).output().map_err(|e| e.to_string())?;
    if out.status.success() { Ok(out.stdout) } else { Err(format!("curl exited with {}", out.status)) }
}

/// `cargo xtask fonts-bundle [<folder>]`: downloads the bundled families (every file of each, and
/// its licence) from the catalogue's commit into `target/fonts/bundled` (or `<folder>`), checked
/// against the catalogue, and writes `bundled.json`.
pub fn bundle(dest: Option<PathBuf>) -> anyhow::Result<()> {
    let catalog =
        Catalog::from_json(&std::fs::read_to_string(catalog_path()).context("run `cargo xtask fonts-catalog` first")?)?;
    let dest = dest.unwrap_or_else(|| root().join("target/fonts/bundled"));
    std::fs::create_dir_all(&dest)?;
    let mut manifest = bundled::Manifest { commit: catalog.source.commit.clone(), families: Vec::new() };
    let mut total = 0u64;
    let mut report = Vec::new();
    for name in bundled::FAMILIES {
        let family = catalog.family(name).with_context(|| format!("{name} isn't in the catalogue"))?;
        let folder = family.dir.rsplit('/').next().unwrap().to_string();
        std::fs::create_dir_all(dest.join(&folder))?;
        let mut files = Vec::new();
        let mut family_bytes = 0u64;
        let mut blobs: Vec<&Blob> = bundled::files_to_bundle(family).into_iter().map(|f| &f.blob).collect();
        blobs.extend(family.licence_file.iter());
        for blob in blobs {
            let file_name = Path::new(&blob.path).file_name().unwrap().to_string_lossy().into_owned();
            let rel = format!("{folder}/{file_name}");
            let path = dest.join(&rel);
            let have = std::fs::read(&path)
                .ok()
                .filter(|b| catalog::verify(blob, b, true).is_ok() || catalog::verify(blob, b, false).is_ok());
            let bytes = match have {
                Some(b) => b,
                None => {
                    let mut got = None;
                    for (i, url) in catalog.urls(blob).into_iter().enumerate() {
                        match fetch(&url) {
                            Ok(b) if catalog::verify(blob, &b, i < 2).is_ok() => {
                                got = Some(b);
                                break;
                            }
                            Ok(_) => eprintln!("{url}: bytes don't match the catalogue"),
                            Err(e) => eprintln!("{url}: {e}"),
                        }
                    }
                    let b = got.with_context(|| format!("couldn't download {}", blob.path))?;
                    std::fs::write(&path, &b)?;
                    b
                }
            };
            if Some(blob) == family.licence_file.as_ref() {
                continue;
            }
            family_bytes += bytes.len() as u64;
            files.push(rel);
        }
        total += family_bytes;
        report.push((name.to_string(), family_bytes));
        manifest.families.push(bundled::BundledFamily {
            name: family.name.clone(),
            licence: family.licence,
            files,
            licence_file: family
                .licence_file
                .as_ref()
                .map(|l| format!("{folder}/{}", Path::new(&l.path).file_name().unwrap().to_string_lossy())),
        });
        eprintln!("{name}: {:.0} KB", family_bytes as f64 / 1024.0);
    }
    std::fs::write(dest.join("bundled.json"), serde_json::to_vec_pretty(&manifest)?)?;
    report.sort_by_key(|(_, b)| std::cmp::Reverse(*b));
    let emoji = report.iter().find(|(n, _)| n == bundled::EMOJI).map(|(_, b)| *b).unwrap_or(0);
    eprintln!(
        "\n{} families, {:.1} MB in {} ({:.1} MB of text fonts, {:.1} MB of emoji)",
        manifest.families.len(),
        total as f64 / 1e6,
        dest.display(),
        (total - emoji) as f64 / 1e6,
        emoji as f64 / 1e6
    );
    eprintln!(
        "largest: {}",
        report.iter().take(5).map(|(n, b)| format!("{n} {:.1} MB", *b as f64 / 1e6)).collect::<Vec<_>>().join(", ")
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_metadata() {
        let text = r#"
name: "Inter"
designer: "Rasmus Andersson"
license: "OFL"
category: "SANS_SERIF"
date_added: "2020-01-01"
fonts {
  name: "Inter"
  style: "normal"
  weight: 400
  filename: "Inter[opsz,wght].ttf"
  copyright: "Copyright 2020 \"The Inter Project\""
}
fonts {
  name: "Inter"
  style: "italic"
  weight: 400
  filename: "Inter-Italic[opsz,wght].ttf"
}
subsets: "cyrillic"
subsets: "latin"
subsets: "menu"
axes {
  tag: "opsz"
  min_value: 14.0
  max_value: 32.0
}
axes {
  tag: "wght"
  min_value: 100.0
  max_value: 900.0
}
source {
  repository_url: "https://github.com/rsms/inter"
  files {
    source_file: "x"
    dest_file: "y"
  }
}
"#;
        let m = parse_metadata(text);
        assert_eq!(m.name, "Inter");
        assert_eq!(m.category.as_deref(), Some("SANS_SERIF"));
        assert_eq!(m.fonts.len(), 2);
        assert_eq!(m.fonts[1].style, "italic");
        assert_eq!(m.fonts[0].filename, "Inter[opsz,wght].ttf");
        assert_eq!(m.axes.len(), 2);
        assert_eq!(m.axes[1].tag, "wght");
        assert_eq!(m.axes[1].max, 900.0);
        assert_eq!(m.subsets, ["cyrillic", "latin", "menu"]);
    }
}
