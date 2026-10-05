//! Deterministic, data-only `.penpkg` packages and filesystem registries.
use crate::asset;
use anyhow::{bail, Context, Result};
use ed25519_dalek::{Signer, Verifier};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, HashSet},
    fs,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};
use walkdir::WalkDir;
use zip::{write::SimpleFileOptions, CompressionMethod, ZipArchive, ZipWriter};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageManifest {
    #[serde(default = "schema")]
    pub schema: u32,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub license: String,
    #[serde(default)]
    pub repository: String,
    #[serde(default)]
    pub authors: Vec<String>,
    #[serde(default)]
    pub pentool: String,
    #[serde(default)]
    pub assets: BTreeMap<String, PackageAsset>,
    #[serde(default)]
    pub dependencies: BTreeMap<String, String>,
}
fn schema() -> u32 {
    1
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageAsset {
    pub path: String,
    pub hash: String,
    #[serde(default)]
    pub preview: Option<String>,
    /// Image blobs the asset document carries, with consumers, for audit.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub images: Vec<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockEntry {
    pub name: String,
    pub version: String,
    pub source: String,
    pub package_hash: String,
}
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Lockfile {
    #[serde(default = "schema")]
    pub schema: u32,
    #[serde(default)]
    pub packages: Vec<LockEntry>,
}

fn validate_name(name: &str) -> Result<()> {
    asset::validate_id(name).context("invalid package name")
}
fn read_manifest(dir: &Path) -> Result<PackageManifest> {
    let p = dir.join("pentool.package.json");
    let m: PackageManifest =
        serde_json::from_slice(&fs::read(&p).with_context(|| format!("missing {}", p.display()))?)?;
    validate_manifest(&m)?;
    Ok(m)
}
fn validate_manifest(m: &PackageManifest) -> Result<()> {
    if m.schema != 1 {
        bail!("unsupported package schema")
    };
    validate_name(&m.name)?;
    semver::Version::parse(&m.version).context("package version must use semantic versioning")?;
    for (id, a) in &m.assets {
        asset::validate_id(id)?;
        safe_relative(Path::new(&a.path))?;
        if let Some(p) = &a.preview {
            safe_relative(Path::new(p))?;
        }
    }
    for (name, range) in &m.dependencies {
        validate_name(name)?;
        semver::VersionReq::parse(range)?;
    }
    Ok(())
}
fn safe_relative(path: &Path) -> Result<()> {
    if path.is_absolute()
        || path.components().any(|c| {
            matches!(
                c,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        bail!("package paths must be safe relative paths");
    }
    Ok(())
}

pub fn init(dir: &Path, name: &str) -> Result<Value> {
    validate_name(name)?;
    fs::create_dir_all(dir.join("assets"))?;
    let path = dir.join("pentool.package.json");
    if path.exists() {
        bail!("package manifest already exists")
    }
    let m = PackageManifest {
        schema: 1,
        name: name.into(),
        version: "0.1.0".into(),
        description: String::new(),
        license: "MIT OR Apache-2.0".into(),
        repository: String::new(),
        authors: vec![],
        pentool: ">=0.6.0".into(),
        assets: BTreeMap::new(),
        dependencies: BTreeMap::new(),
    };
    asset::atomic_new(&path, &serde_json::to_vec_pretty(&m)?)?;
    Ok(json!({"ok":true,"manifest":path}))
}

pub fn pack(dir: &Path, output: &Path) -> Result<Value> {
    let mut m = read_manifest(dir)?;
    let mut seen = HashSet::new();
    // Discover metadata-bearing assets and make the manifest's asset table canonical.
    for e in WalkDir::new(dir.join("assets"))
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| {
            e.file_type().is_file() && e.path().extension().and_then(|s| s.to_str()) == Some("pen")
        })
    {
        let bytes = fs::read(e.path())?;
        let raw: Value = serde_json::from_slice(&bytes)?;
        let am = asset::manifest(&raw)?.context("package asset lacks asset metadata")?;
        asset::validate_properties(&raw)?;
        let images = check_asset_document(&raw)?;
        if !seen.insert(am.id.clone()) {
            bail!("duplicate asset ID {}", am.id)
        }
        let rel = slash(e.path().strip_prefix(dir).unwrap());
        m.assets.insert(
            am.id,
            PackageAsset {
                path: rel,
                hash: asset::hash_bytes(&bytes),
                preview: None,
                images,
            },
        );
    }
    if m.assets.is_empty() {
        bail!("package contains no assets")
    }
    let manifest_bytes = canonical_json(&serde_json::to_value(&m)?)?;
    let mut files: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    files.insert("pentool.package.json".into(), manifest_bytes);
    for a in m.assets.values() {
        files.insert(a.path.clone(), fs::read(dir.join(native(&a.path)))?);
        if let Some(p) = &a.preview {
            files.insert(p.clone(), fs::read(dir.join(native(p)))?);
        }
    }
    if dir.join("LICENSE").is_file() {
        files.insert("LICENSE".into(), fs::read(dir.join("LICENSE"))?);
    }
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    let temp = output.with_extension("penpkg.tmp");
    let file = fs::File::create(&temp)?;
    let mut zip = ZipWriter::new(file);
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .compression_level(Some(9))
        .unix_permissions(0o644);
    for (name, bytes) in files {
        zip.start_file(name, options)?;
        zip.write_all(&bytes)?;
    }
    zip.finish()?.sync_all()?;
    if output.exists() {
        fs::remove_file(output)?;
    }
    fs::rename(&temp, output)?;
    let bytes = fs::read(output)?;
    Ok(
        json!({"ok":true,"package":m.name,"version":m.version,"assets":m.assets.len(),"output":output,"hash":asset::hash_bytes(&bytes)}),
    )
}

/// Validate a packaged asset document with the shared format validators and
/// return its deterministic image table. Image blobs must be embedded so the
/// package is self-contained and verifiable offline; hashes, decode, pixel and
/// byte limits are enforced by the same validator used at import.
fn check_asset_document(raw: &Value) -> Result<Vec<Value>> {
    crate::transaction::validate_value(raw)?;
    let Some(images) = raw.get("image_assets").and_then(Value::as_object) else {
        return Ok(vec![]);
    };
    let mut consumers: BTreeMap<String, Vec<String>> = BTreeMap::new();
    fn walk(v: &Value, out: &mut BTreeMap<String, Vec<String>>) {
        match v {
            Value::Object(m) => {
                if m.get("kind").and_then(Value::as_str) == Some("image") {
                    if let (Some(a), Some(id)) = (
                        m.get("asset").and_then(Value::as_str),
                        m.get("id").and_then(Value::as_str),
                    ) {
                        out.entry(a.to_owned()).or_default().push(id.to_owned());
                    }
                }
                m.values().for_each(|c| walk(c, out));
            }
            Value::Array(a) => a.iter().for_each(|c| walk(c, out)),
            _ => {}
        }
    }
    walk(raw, &mut consumers);
    let mut table = vec![];
    let mut digests: Vec<_> = images.keys().collect();
    digests.sort();
    for digest in digests {
        let a = &images[digest];
        if a["storage"]["kind"] != "embedded" {
            bail!("[unsupported-capability] packaged image {digest} must be embedded; external image files are not packaged")
        }
        let mut users = consumers.remove(digest).unwrap_or_default();
        users.sort();
        table.push(json!({
            "digest": digest, "media_type": a["media_type"],
            "pixel_width": a["pixel_width"], "pixel_height": a["pixel_height"],
            "byte_length": a["byte_length"], "consumers": users
        }));
    }
    Ok(table)
}

fn canonical_json(v: &Value) -> Result<Vec<u8>> {
    Ok(serde_json::to_vec_pretty(v)?)
}
fn slash(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}
fn native(path: &str) -> PathBuf {
    PathBuf::from(path.replace('/', std::path::MAIN_SEPARATOR_STR))
}

pub fn verify(path: &Path) -> Result<Value> {
    const MAX_ARCHIVE: u64 = 512 * 1024 * 1024;
    const MAX_EXPANDED: u64 = 1024 * 1024 * 1024;
    if fs::metadata(path)?.len() > MAX_ARCHIVE {
        bail!("package exceeds 512 MiB")
    };
    let mut zip = ZipArchive::new(fs::File::open(path)?)?;
    let mut expanded = 0u64;
    let mut entries = HashSet::new();
    for i in 0..zip.len() {
        let f = zip.by_index(i)?;
        let enclosed = f.enclosed_name().context("unsafe package path")?.to_owned();
        if !entries.insert(enclosed) {
            bail!("duplicate package entry")
        };
        expanded = expanded.saturating_add(f.size());
        if expanded > MAX_EXPANDED {
            bail!("expanded package exceeds 1 GiB")
        }
    }
    let mut manifest_data = vec![];
    zip.by_name("pentool.package.json")
        .context("package manifest missing")?
        .read_to_end(&mut manifest_data)?;
    let m: PackageManifest = serde_json::from_slice(&manifest_data)?;
    validate_manifest(&m)?;
    for (id, a) in &m.assets {
        let mut data = vec![];
        zip.by_name(&a.path)
            .with_context(|| format!("asset missing: {id}"))?
            .read_to_end(&mut data)?;
        if asset::hash_bytes(&data) != a.hash {
            bail!("asset hash mismatch: {id}")
        }
        let raw: Value = serde_json::from_slice(&data)?;
        let am = asset::manifest(&raw)?.context("asset metadata missing")?;
        asset::validate_properties(&raw)?;
        if am.id != *id {
            bail!("asset ID mismatch: {id}")
        }
        if check_asset_document(&raw)? != a.images {
            bail!("asset image table does not match its document: {id}")
        }
    }
    Ok(
        json!({"ok":true,"package":m,"package_hash":asset::hash_bytes(&fs::read(path)?),"entries":entries.len(),"expanded_bytes":expanded}),
    )
}

pub fn inspect(path: &Path) -> Result<Value> {
    verify(path)
}

#[derive(Serialize, Deserialize)]
struct PackageSignature {
    schema: u32,
    algorithm: String,
    package_hash: String,
    public_key: String,
    signature: String,
}
pub fn keygen(prefix: &Path) -> Result<Value> {
    let key = ed25519_dalek::SigningKey::generate(&mut rand_core::OsRng);
    let secret = prefix.with_extension("key");
    let public = prefix.with_extension("pub");
    if secret.exists() || public.exists() {
        bail!("key output exists")
    };
    asset::atomic_new(&secret, hex::encode(key.to_bytes()).as_bytes())?;
    asset::atomic_new(
        &public,
        hex::encode(key.verifying_key().to_bytes()).as_bytes(),
    )?;
    Ok(
        json!({"ok":true,"private_key":secret,"public_key":public,"warning":"Keep the private key secret; publish only the .pub file"}),
    )
}
pub fn sign(package: &Path, key_path: &Path, output: Option<&Path>) -> Result<Value> {
    let raw = fs::read_to_string(key_path)?;
    let bytes = hex::decode(raw.trim()).context("private key must be 32-byte hex")?;
    let key = ed25519_dalek::SigningKey::from_bytes(
        &bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!("private key must be 32 bytes"))?,
    );
    let package_bytes = fs::read(package)?;
    let hash = asset::hash_bytes(&package_bytes);
    let signature = key.sign(hash.as_bytes());
    let record = PackageSignature {
        schema: 1,
        algorithm: "ed25519".into(),
        package_hash: hash.clone(),
        public_key: hex::encode(key.verifying_key().to_bytes()),
        signature: hex::encode(signature.to_bytes()),
    };
    let path = output
        .map(Path::to_owned)
        .unwrap_or_else(|| PathBuf::from(format!("{}.sig.json", package.display())));
    asset::atomic_new_replace(&path, &serde_json::to_vec_pretty(&record)?)?;
    Ok(json!({"ok":true,"package_hash":hash,"signature":path,"public_key":record.public_key}))
}
pub fn verify_signature(
    package: &Path,
    signature: &Path,
    public_key: Option<&Path>,
) -> Result<Value> {
    let record: PackageSignature = serde_json::from_slice(&fs::read(signature)?)?;
    if record.schema != 1 || record.algorithm != "ed25519" {
        bail!("unsupported signature format")
    };
    let hash = asset::hash_bytes(&fs::read(package)?);
    if hash != record.package_hash {
        bail!("signature package hash mismatch")
    };
    let public = if let Some(path) = public_key {
        fs::read_to_string(path)?
    } else {
        record.public_key.clone()
    };
    let public_bytes = hex::decode(public.trim())?;
    let key = ed25519_dalek::VerifyingKey::from_bytes(
        &public_bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!("public key must be 32 bytes"))?,
    )?;
    if hex::encode(key.to_bytes()) != record.public_key {
        bail!("signature signer does not match required public key")
    };
    let sig_bytes = hex::decode(&record.signature)?;
    let sig = ed25519_dalek::Signature::from_bytes(
        &sig_bytes
            .try_into()
            .map_err(|_| anyhow::anyhow!("signature must be 64 bytes"))?,
    );
    key.verify(hash.as_bytes(), &sig)?;
    Ok(json!({"ok":true,"package_hash":hash,"public_key":record.public_key,"algorithm":"ed25519"}))
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct RegistryIndex {
    #[serde(default = "schema")]
    schema: u32,
    #[serde(default)]
    packages: BTreeMap<String, BTreeMap<String, RegistryRelease>>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
struct RegistryRelease {
    path: String,
    hash: String,
    yanked: bool,
}
fn registry_index(root: &Path) -> PathBuf {
    root.join("index.json")
}
pub fn publish(package: &Path, registry: &Path, dry_run: bool) -> Result<Value> {
    let report = verify(package)?;
    let m: PackageManifest = serde_json::from_value(report["package"].clone())?;
    let hash = report["package_hash"].as_str().unwrap().to_owned();
    let rel = PathBuf::from("packages")
        .join(&m.name)
        .join(format!("{}.penpkg", m.version));
    let dst = registry.join(&rel);
    let mut idx: RegistryIndex = if registry_index(registry).exists() {
        serde_json::from_slice(&fs::read(registry_index(registry))?)?
    } else {
        RegistryIndex {
            schema: 1,
            packages: BTreeMap::new(),
        }
    };
    if let Some(old) = idx.packages.get(&m.name).and_then(|v| v.get(&m.version)) {
        if old.hash != hash {
            bail!("immutable package version already exists with different bytes")
        };
        return Ok(
            json!({"ok":true,"already_published":true,"package":m.name,"version":m.version,"hash":hash}),
        );
    }
    if dry_run {
        return Ok(
            json!({"ok":true,"dry_run":true,"package":m.name,"version":m.version,"hash":hash,"destination":dst}),
        );
    }
    fs::create_dir_all(dst.parent().unwrap())?;
    fs::copy(package, &dst)?;
    idx.packages.entry(m.name.clone()).or_default().insert(
        m.version.clone(),
        RegistryRelease {
            path: slash(&rel),
            hash: hash.clone(),
            yanked: false,
        },
    );
    fs::create_dir_all(registry)?;
    asset::atomic_new_replace(&registry_index(registry), &serde_json::to_vec_pretty(&idx)?)?;
    Ok(json!({"ok":true,"package":m.name,"version":m.version,"hash":hash,"registry":registry}))
}

pub fn registry_search(registry: &str, query: &str) -> Result<Value> {
    let idx = load_registry(registry)?;
    let q = query.to_ascii_lowercase();
    let results = idx
        .packages
        .into_iter()
        .filter(|(n, _)| n.to_ascii_lowercase().contains(&q))
        .map(
            |(name, versions)| json!({"name":name,"versions":versions.keys().collect::<Vec<_>>() }),
        )
        .collect::<Vec<_>>();
    Ok(json!({"registry":registry,"results":results}))
}

pub fn install(package: &Path, project: &Path, source: &str) -> Result<Value> {
    let report = verify(package)?;
    let m: PackageManifest = serde_json::from_value(report["package"].clone())?;
    let hash = report["package_hash"].as_str().unwrap().to_owned();
    let dst = project
        .join(".pentool")
        .join("packages")
        .join(&m.name)
        .join(&m.version);
    if dst.exists() {
        let marker = dst.join(".package-hash");
        if fs::read_to_string(marker).ok().as_deref() == Some(&hash) {
            return Ok(
                json!({"ok":true,"already_installed":true,"package":m.name,"version":m.version}),
            );
        }
        bail!("installed destination exists with different content")
    }
    let temp = dst.with_extension("installing");
    if temp.exists() {
        fs::remove_dir_all(&temp)?;
    }
    fs::create_dir_all(&temp)?;
    let mut zip = ZipArchive::new(fs::File::open(package)?)?;
    for i in 0..zip.len() {
        let mut f = zip.by_index(i)?;
        let rel = f.enclosed_name().context("unsafe package path")?.to_owned();
        let out = temp.join(rel);
        if f.is_dir() {
            fs::create_dir_all(&out)?;
        } else {
            if let Some(p) = out.parent() {
                fs::create_dir_all(p)?;
            }
            let mut w = fs::File::create(out)?;
            std::io::copy(&mut f, &mut w)?;
        }
    }
    fs::write(temp.join(".package-hash"), &hash)?;
    if let Some(p) = dst.parent() {
        fs::create_dir_all(p)?;
    }
    fs::rename(&temp, &dst)?;
    let lock_path = project.join("pentool.lock");
    let mut lock: Lockfile = if lock_path.exists() {
        serde_json::from_slice(&fs::read(&lock_path)?)?
    } else {
        Lockfile {
            schema: 1,
            packages: vec![],
        }
    };
    lock.packages.retain(|p| p.name != m.name);
    lock.packages.push(LockEntry {
        name: m.name.clone(),
        version: m.version.clone(),
        source: source.into(),
        package_hash: hash.clone(),
    });
    lock.packages.sort_by(|a, b| a.name.cmp(&b.name));
    asset::atomic_new_replace(&lock_path, &serde_json::to_vec_pretty(&lock)?)?;
    audit(
        project,
        &json!({"event":"install","package":m.name,"version":m.version,"hash":hash,"source":source}),
    )?;
    Ok(
        json!({"ok":true,"package":m.name,"version":m.version,"hash":hash,"installed":dst,"lockfile":lock_path}),
    )
}

pub fn install_from_registry(registry: &str, spec: &str, project: &Path) -> Result<Value> {
    let (name, wanted) = spec
        .rsplit_once('@')
        .context("package spec must be name@version")?;
    let idx = load_registry(registry)?;
    let release = idx
        .packages
        .get(name)
        .and_then(|v| v.get(wanted))
        .context("package version not found")?;
    if release.yanked {
        bail!("package version is yanked")
    };
    let package_path = if is_http(registry) {
        let url = format!("{}/{}", registry.trim_end_matches('/'), release.path);
        let bytes = http_get(&url, 512 * 1024 * 1024)?;
        if asset::hash_bytes(&bytes) != release.hash {
            bail!("registry artifact hash mismatch")
        }
        let cache = project
            .join(".pentool")
            .join("cache")
            .join("sha256")
            .join(release.hash.trim_start_matches("sha256:"));
        fs::create_dir_all(cache.parent().unwrap())?;
        if !cache.exists() {
            asset::atomic_new(&cache, &bytes)?;
        }
        cache
    } else {
        let source = PathBuf::from(registry).join(native(&release.path));
        let bytes = fs::read(&source)?;
        if asset::hash_bytes(&bytes) != release.hash {
            bail!("registry artifact hash mismatch")
        }
        let cache = project
            .join(".pentool")
            .join("cache")
            .join("sha256")
            .join(release.hash.trim_start_matches("sha256:"));
        fs::create_dir_all(cache.parent().unwrap())?;
        if !cache.exists() {
            asset::atomic_new(&cache, &bytes)?;
        }
        cache
    };
    install(&package_path, project, registry)
}

fn is_http(value: &str) -> bool {
    value.starts_with("https://") || value.starts_with("http://")
}
fn load_registry(registry: &str) -> Result<RegistryIndex> {
    let bytes = if is_http(registry) {
        http_get(
            &format!("{}/index.json", registry.trim_end_matches('/')),
            16 * 1024 * 1024,
        )?
    } else {
        fs::read(registry_index(Path::new(registry)))?
    };
    Ok(serde_json::from_slice(&bytes)?)
}
fn http_get(url: &str, limit: usize) -> Result<Vec<u8>> {
    // reqwest's blocking client owns a small Tokio runtime. Constructing or
    // dropping it inside our #[tokio::main] thread panics, so keep the entire
    // blocking request on a plain worker thread.
    let url = url.to_owned();
    std::thread::spawn(move || -> Result<Vec<u8>> {
        let response = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::limited(5))
            .build()?
            .get(url)
            .send()?
            .error_for_status()?;
        if response.content_length().is_some_and(|n| n > limit as u64) {
            bail!("download exceeds size limit")
        }
        let bytes = response.bytes()?;
        if bytes.len() > limit {
            bail!("download exceeds size limit")
        }
        Ok(bytes.to_vec())
    })
    .join()
    .map_err(|_| anyhow::anyhow!("HTTP download worker panicked"))?
}

pub fn lock_verify(project: &Path) -> Result<Value> {
    let p = project.join("pentool.lock");
    let lock: Lockfile = serde_json::from_slice(&fs::read(&p)?)?;
    let mut results = vec![];
    for e in &lock.packages {
        let marker = project
            .join(".pentool")
            .join("packages")
            .join(&e.name)
            .join(&e.version)
            .join(".package-hash");
        let actual = fs::read_to_string(&marker).unwrap_or_default();
        results.push(json!({"name":e.name,"version":e.version,"ok":actual==e.package_hash,"expected":e.package_hash,"actual":actual}));
    }
    let ok = results.iter().all(|r| r["ok"] == true);
    Ok(json!({"ok":ok,"lockfile":p,"packages":results}))
}

pub fn lock_sync(project: &Path, offline: bool) -> Result<Value> {
    let lock_path = project.join("pentool.lock");
    let lock: Lockfile = serde_json::from_slice(&fs::read(&lock_path)?)?;
    let mut results = vec![];
    for entry in lock.packages {
        let marker = project
            .join(".pentool")
            .join("packages")
            .join(&entry.name)
            .join(&entry.version)
            .join(".package-hash");
        if fs::read_to_string(&marker).ok().as_deref() == Some(&entry.package_hash) {
            results.push(json!({"name":entry.name,"version":entry.version,"status":"present"}));
            continue;
        }
        let cache = project
            .join(".pentool")
            .join("cache")
            .join("sha256")
            .join(entry.package_hash.trim_start_matches("sha256:"));
        if cache.exists() {
            install(&cache, project, &entry.source)?;
            results.push(
                json!({"name":entry.name,"version":entry.version,"status":"installed-from-cache"}),
            );
            continue;
        }
        if offline {
            bail!(
                "locked package is not available offline: {}@{}",
                entry.name,
                entry.version
            )
        }
        install_from_registry(
            &entry.source,
            &format!("{}@{}", entry.name, entry.version),
            project,
        )?;
        results.push(json!({"name":entry.name,"version":entry.version,"status":"downloaded"}));
    }
    Ok(json!({"ok":true,"offline":offline,"packages":results}))
}

pub fn list_installed(project: &Path) -> Result<Value> {
    let path = project.join("pentool.lock");
    let lock: Lockfile = if path.exists() {
        serde_json::from_slice(&fs::read(path)?)?
    } else {
        Lockfile {
            schema: 1,
            packages: vec![],
        }
    };
    Ok(json!({"packages":lock.packages}))
}
pub fn remove_installed(project: &Path, spec: &str, force: bool) -> Result<Value> {
    let (name, version) = spec
        .rsplit_once('@')
        .context("package spec must be name@version")?;
    let lock_path = project.join("pentool.lock");
    let mut lock: Lockfile = serde_json::from_slice(&fs::read(&lock_path)?)?;
    let locked = lock
        .packages
        .iter()
        .any(|p| p.name == name && p.version == version);
    if locked && !force {
        bail!("package is locked; pass --force to remove it and its lock entry")
    };
    let target = project
        .join(".pentool")
        .join("packages")
        .join(name)
        .join(version);
    let canonical_project = project.canonicalize()?;
    if target.exists() {
        let canonical_target = target.canonicalize()?;
        if !canonical_target.starts_with(&canonical_project) {
            bail!("refusing to remove package outside the project")
        };
        fs::remove_dir_all(&canonical_target)?;
    }
    lock.packages
        .retain(|p| !(p.name == name && p.version == version));
    asset::atomic_new_replace(&lock_path, &serde_json::to_vec_pretty(&lock)?)?;
    audit(
        project,
        &json!({"event":"remove","package":name,"version":version}),
    )?;
    Ok(json!({"ok":true,"removed":spec,"lock_updated":locked}))
}

fn audit(project: &Path, event: &Value) -> Result<()> {
    let path = project.join(".pentool").join("audit.jsonl");
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut record = event.clone();
    record["timestamp_ms"] = Value::from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_millis() as u64,
    );
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    serde_json::to_writer(&mut file, &record)?;
    file.write_all(b"\n")?;
    file.sync_data()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn paths_are_safe() {
        assert!(safe_relative(Path::new("assets/a.pen")).is_ok());
        assert!(safe_relative(Path::new("../x")).is_err());
    }
}
