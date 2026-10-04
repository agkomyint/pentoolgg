//! Local library registration, indexing, and deterministic search.
use crate::{asset, document::Document};
use anyhow::{bail, Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, HashMap},
    fs,
    path::{Path, PathBuf},
};
use walkdir::WalkDir;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LibraryEntry {
    pub name: String,
    pub path: PathBuf,
    #[serde(default = "yes")]
    pub enabled: bool,
}
fn yes() -> bool {
    true
}
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct LibraryConfig {
    #[serde(default = "schema")]
    pub schema: u32,
    #[serde(default)]
    pub libraries: Vec<LibraryEntry>,
}
fn schema() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexedAsset {
    pub id: String,
    pub library: String,
    pub path: PathBuf,
    pub relative_path: PathBuf,
    pub name: String,
    pub description: String,
    pub version: String,
    pub kind: String,
    pub author: String,
    pub license: String,
    pub tags: Vec<String>,
    pub category: String,
    pub width: u32,
    pub height: u32,
    pub layers: usize,
    pub objects: usize,
    pub modified_ms: u128,
    pub size: u64,
    pub content_hash: String,
}
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct AssetIndex {
    #[serde(default = "schema")]
    pub schema: u32,
    #[serde(default)]
    pub assets: Vec<IndexedAsset>,
    #[serde(default)]
    pub warnings: Vec<Value>,
}

pub fn project_config(root: &Path) -> PathBuf {
    root.join(".pentool").join("libraries.json")
}
pub fn project_index(root: &Path) -> PathBuf {
    root.join(".pentool").join("index.json")
}
pub fn user_config() -> Result<PathBuf> {
    Ok(ProjectDirs::from("org", "pentool", "pentool")
        .context("user config directory unavailable")?
        .config_dir()
        .join("libraries.json"))
}
pub fn user_index() -> Result<PathBuf> {
    Ok(ProjectDirs::from("org", "pentool", "pentool")
        .context("user config directory unavailable")?
        .cache_dir()
        .join("index.json"))
}
fn load(path: &Path) -> Result<LibraryConfig> {
    if !path.exists() {
        return Ok(LibraryConfig {
            schema: 1,
            libraries: vec![],
        });
    }
    Ok(serde_json::from_slice(&fs::read(path)?)?)
}
fn save(path: &Path, value: &impl Serialize) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    crate::asset::atomic_new_replace(path, &serde_json::to_vec_pretty(value)?)
}

pub fn add(root: &Path, config: &Path, name: &str, folder: &Path) -> Result<Value> {
    if name.is_empty() || name.contains(char::is_whitespace) {
        bail!("library name must be a nonempty identifier");
    }
    let absolute = if folder.is_absolute() {
        folder.to_owned()
    } else {
        root.join(folder)
    };
    let canonical = absolute
        .canonicalize()
        .with_context(|| format!("library folder unavailable: {}", absolute.display()))?;
    if !canonical.is_dir() {
        bail!("library path is not a directory");
    }
    let mut cfg = load(config)?;
    if cfg.libraries.iter().any(|l| l.name == name) {
        bail!("library name already registered");
    }
    if cfg
        .libraries
        .iter()
        .any(|l| root.join(&l.path).canonicalize().ok().as_ref() == Some(&canonical))
    {
        bail!("library folder already registered");
    }
    let stored = if config.starts_with(root) {
        // macOS temp directories are commonly reached through /var while
        // canonicalize returns /private/var. Diff canonical paths so project
        // library entries do not accidentally point outside the project.
        let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_owned());
        pathdiff(&canonical_root, &canonical).unwrap_or(canonical.clone())
    } else {
        canonical.clone()
    };
    cfg.libraries.push(LibraryEntry {
        name: name.into(),
        path: stored.clone(),
        enabled: true,
    });
    cfg.libraries.sort_by(|a, b| a.name.cmp(&b.name));
    save(config, &cfg)?;
    Ok(json!({"ok":true,"name":name,"path":stored,"config":config}))
}

pub fn remove(config: &Path, name: &str) -> Result<Value> {
    let mut cfg = load(config)?;
    let before = cfg.libraries.len();
    cfg.libraries.retain(|l| l.name != name);
    if cfg.libraries.len() == before {
        bail!("library not found");
    }
    save(config, &cfg)?;
    Ok(json!({"ok":true,"removed":name}))
}
pub fn set_enabled(config: &Path, name: &str, enabled: bool) -> Result<Value> {
    let mut cfg = load(config)?;
    let item = cfg
        .libraries
        .iter_mut()
        .find(|l| l.name == name)
        .context("library not found")?;
    item.enabled = enabled;
    save(config, &cfg)?;
    Ok(json!({"ok":true,"name":name,"enabled":enabled}))
}

pub fn list(root: &Path, config: &Path) -> Result<Value> {
    let cfg = load(config)?;
    Ok(
        json!({"scope_root":root,"config":config,"libraries":cfg.libraries.into_iter().map(|l| {
        let p = if l.path.is_absolute(){l.path.clone()}else{root.join(&l.path)};
        json!({"name":l.name,"path":l.path,"enabled":l.enabled,"available":p.is_dir()})
    }).collect::<Vec<_>>() }),
    )
}

pub fn refresh(root: &Path, config: &Path, index_path: &Path, only: Option<&str>) -> Result<Value> {
    let cfg = load(config)?;
    if let Some(name) = only {
        if !cfg.libraries.iter().any(|l| l.name == name) {
            bail!("library not found");
        }
    }
    let mut index = if only.is_some() && index_path.exists() {
        load_index(index_path)?
    } else {
        AssetIndex {
            schema: 1,
            assets: vec![],
            warnings: vec![],
        }
    };
    if let Some(name) = only {
        index.assets.retain(|a| a.library != name);
        index
            .warnings
            .retain(|w| w.get("library").and_then(Value::as_str) != Some(name));
    }
    let mut ids: HashMap<String, PathBuf> = index
        .assets
        .iter()
        .map(|a| (a.id.clone(), a.path.clone()))
        .collect();
    for lib in cfg
        .libraries
        .iter()
        .filter(|l| l.enabled && only.is_none_or(|n| n == l.name))
    {
        let folder = if lib.path.is_absolute() {
            lib.path.clone()
        } else {
            root.join(&lib.path)
        };
        if !folder.is_dir() {
            index
                .warnings
                .push(json!({"library":lib.name,"code":"missing_folder","path":folder}));
            continue;
        }
        for entry in WalkDir::new(&folder).follow_links(false).into_iter() {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => {
                    index.warnings.push(
                        json!({"library":lib.name,"code":"walk_error","message":e.to_string()}),
                    );
                    continue;
                }
            };
            if !entry.file_type().is_file()
                || entry.path().extension().and_then(|s| s.to_str()) != Some("pen")
            {
                continue;
            }
            match index_file(&lib.name, &folder, entry.path()) {
                Ok(item) => {
                    if let Some(first) = ids.insert(item.id.clone(), item.path.clone()) {
                        index.warnings.push(json!({"library":lib.name,"code":"duplicate_asset_id","id":item.id,"first":first,"duplicate":item.path}));
                    } else { index.assets.push(item); }
                }
                Err(e) => index.warnings.push(json!({"library":lib.name,"code":"invalid_asset","path":entry.path(),"message":e.to_string()})),
            }
        }
    }
    index
        .assets
        .sort_by(|a, b| a.id.cmp(&b.id).then(a.library.cmp(&b.library)));
    save(index_path, &index)?;
    Ok(
        json!({"ok":true,"assets":index.assets.len(),"warnings":index.warnings.len(),"index":index_path}),
    )
}

fn index_file(library: &str, root: &Path, path: &Path) -> Result<IndexedAsset> {
    const MAX: u64 = 64 * 1024 * 1024;
    let meta = fs::metadata(path)?;
    if meta.len() > MAX {
        bail!("asset exceeds 64 MiB");
    }
    let bytes = fs::read(path)?;
    let raw: Value = serde_json::from_slice(&bytes)?;
    let m = asset::manifest(&raw)?.context("missing asset metadata")?;
    asset::validate_properties(&raw)?;
    let doc: Document = serde_json::from_value(raw)?;
    doc.validate().map_err(anyhow::Error::msg)?;
    let objects = doc
        .layers
        .iter()
        .map(|l| l.paths.len() + l.texts.len())
        .sum();
    let modified_ms = meta
        .modified()
        .ok()
        .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_millis());
    Ok(IndexedAsset {
        id: m.id,
        library: library.into(),
        path: path.to_owned(),
        relative_path: path.strip_prefix(root).unwrap_or(path).to_owned(),
        name: m.name,
        description: m.description,
        version: m.asset_version,
        kind: m.kind,
        author: m.author,
        license: m.license,
        tags: m.tags,
        category: m.category,
        width: doc.canvas.width,
        height: doc.canvas.height,
        layers: doc.layers.len(),
        objects,
        modified_ms,
        size: meta.len(),
        content_hash: asset::hash_bytes(&bytes),
    })
}

pub fn load_index(path: &Path) -> Result<AssetIndex> {
    Ok(serde_json::from_slice(&fs::read(path).with_context(
        || "asset index missing; run `pentool library refresh`",
    )?)?)
}
pub struct SearchOptions<'a> {
    pub query: Option<&'a str>,
    pub library: Option<&'a str>,
    pub tag: Option<&'a str>,
    pub category: Option<&'a str>,
    pub kind: Option<&'a str>,
    pub offset: usize,
    pub limit: usize,
}
pub fn search(index: &AssetIndex, options: &SearchOptions<'_>) -> Value {
    let q = options.query.unwrap_or("").to_ascii_lowercase();
    let mut scored: Vec<(u8, &IndexedAsset)> = index
        .assets
        .iter()
        .filter(|a| {
            options.library.is_none_or(|v| a.library == v)
                && options.tag.is_none_or(|v| a.tags.iter().any(|t| t == v))
                && options.category.is_none_or(|v| a.category == v)
                && options.kind.is_none_or(|v| a.kind == v)
        })
        .filter_map(|a| {
            let id = a.id.to_ascii_lowercase();
            let name = a.name.to_ascii_lowercase();
            let hay = format!(
                "{} {} {} {}",
                id,
                name,
                a.description.to_ascii_lowercase(),
                a.tags.join(" ").to_ascii_lowercase()
            );
            let score = if q.is_empty() {
                4
            } else if id == q {
                0
            } else if name == q {
                1
            } else if id.starts_with(&q) || name.starts_with(&q) {
                2
            } else if hay.contains(&q) {
                3
            } else {
                return None;
            };
            Some((score, a))
        })
        .collect();
    scored.sort_by(|(sa, a), (sb, b)| {
        sa.cmp(sb)
            .then(a.id.cmp(&b.id))
            .then(a.library.cmp(&b.library))
    });
    let total = scored.len();
    let items = scored
        .into_iter()
        .skip(options.offset)
        .take(options.limit.min(500))
        .map(|(_, a)| a)
        .collect::<Vec<_>>();
    json!({"matches":total,"returned":items.len(),"offset":options.offset,"limit":options.limit.min(500),"has_more":options.offset+items.len()<total,"assets":items})
}
pub fn resolve<'a>(index: &'a AssetIndex, spec: &str) -> Result<&'a IndexedAsset> {
    let mut found = index
        .assets
        .iter()
        .filter(|a| a.id == spec || format!("{}/{}", a.library, a.id) == spec);
    let first = found.next().context("asset not found")?;
    if found.next().is_some() {
        bail!("asset ID is ambiguous; use library/asset-id");
    }
    Ok(first)
}

fn pathdiff(base: &Path, target: &Path) -> Option<PathBuf> {
    let b: Vec<_> = base.components().collect();
    let t: Vec<_> = target.components().collect();
    if b.first() != t.first() {
        return None;
    }
    let common = b.iter().zip(&t).take_while(|(x, y)| x == y).count();
    let mut out = PathBuf::new();
    for _ in common..b.len() {
        out.push("..");
    }
    for c in &t[common..] {
        out.push(c.as_os_str());
    }
    Some(out)
}

pub fn merged_indexes(project: Option<&Path>, user: Option<&Path>) -> Result<AssetIndex> {
    let mut by = BTreeMap::new();
    let mut warnings = vec![];
    for p in [project, user].into_iter().flatten() {
        if p.exists() {
            let idx = load_index(p)?;
            for a in idx.assets {
                by.insert(format!("{}/{}", a.library, a.id), a);
            }
            warnings.extend(idx.warnings);
        }
    }
    Ok(AssetIndex {
        schema: 1,
        assets: by.into_values().collect(),
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn relative_paths() {
        assert_eq!(
            pathdiff(Path::new("C:/a/b"), Path::new("C:/a/c")).unwrap(),
            PathBuf::from("../c")
        );
    }
    #[test]
    fn ten_thousand_assets_search_deterministically() {
        let assets = (0..10_000)
            .map(|n| IndexedAsset {
                id: format!("icons/item-{n:05}"),
                library: "scale".into(),
                path: PathBuf::from(format!("item-{n}.pen")),
                relative_path: PathBuf::from(format!("item-{n}.pen")),
                name: format!("Item {n:05}"),
                description: String::new(),
                version: "1.0.0".into(),
                kind: "component".into(),
                author: String::new(),
                license: "MIT".into(),
                tags: vec!["icon".into()],
                category: "icons".into(),
                width: 24,
                height: 24,
                layers: 1,
                objects: 1,
                modified_ms: 0,
                size: 100,
                content_hash: format!("sha256:{n:064x}"),
            })
            .collect();
        let index = AssetIndex {
            schema: 1,
            assets,
            warnings: vec![],
        };
        let result = search(
            &index,
            &SearchOptions {
                query: Some("item-099"),
                library: Some("scale"),
                tag: Some("icon"),
                category: Some("icons"),
                kind: Some("component"),
                offset: 0,
                limit: 20,
            },
        );
        assert_eq!(result["returned"], 20);
        assert_eq!(result["assets"][0]["id"], "icons/item-09900");
    }
}
