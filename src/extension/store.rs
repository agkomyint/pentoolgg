//! Extension packages (`.penext`): deterministic, content-addressed zip files
//! verified entirely offline, plus the per-project install store with recorded
//! capability approvals.
use super::manifest::{safe_relative, Manifest, CAPABILITIES, MANIFEST_FILE};
use crate::asset;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
};
use zip::{write::SimpleFileOptions, CompressionMethod, ZipArchive, ZipWriter};

pub const FILES_INDEX: &str = "files.json";
const MAX_FILES: usize = 256;
const MAX_TOTAL_BYTES: u64 = 32 * 1024 * 1024;
const INSTALL_RECORD: &str = ".install.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct FilesIndex {
    schema: u32,
    files: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallRecord {
    pub name: String,
    pub version: String,
    pub package_hash: String,
    pub entry_hash: String,
    pub approved_capabilities: Vec<String>,
}

pub fn extensions_dir(project: &Path) -> PathBuf {
    project.join(".pentool").join("extensions")
}

fn collect(
    dir: &Path,
    root: &Path,
    out: &mut BTreeMap<String, Vec<u8>>,
    total: &mut u64,
) -> Result<()> {
    let mut entries: Vec<_> = fs::read_dir(dir)?.collect::<std::io::Result<_>>()?;
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let kind = entry.file_type()?;
        let path = entry.path();
        let rel = path
            .strip_prefix(root)?
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        if kind.is_symlink() {
            bail!("[unsafe-path] `{rel}` is a symbolic link; extensions contain regular files only")
        }
        if kind.is_dir() {
            if rel == ".pentool" || rel.starts_with('.') {
                continue;
            }
            collect(&path, root, out, total)?;
        } else if kind.is_file() {
            if rel == MANIFEST_FILE || rel == FILES_INDEX {
                continue;
            }
            safe_relative(&rel)?;
            let bytes = fs::read(&path)?;
            *total += bytes.len() as u64;
            if out.len() >= MAX_FILES || *total > MAX_TOTAL_BYTES {
                bail!("[limit-exceeded] an extension is limited to {MAX_FILES} files and 32 MiB")
            }
            out.insert(rel, bytes);
        }
    }
    Ok(())
}

/// Pack `dir` (containing `extension.json` and its files) into a `.penext`.
pub fn pack(dir: &Path, output: &Path) -> Result<Value> {
    let manifest_bytes = fs::read(dir.join(MANIFEST_FILE))
        .with_context(|| format!("{} has no {MANIFEST_FILE}", dir.display()))?;
    let manifest = Manifest::parse(&manifest_bytes)?;
    let mut files = BTreeMap::new();
    let mut total = 0;
    collect(dir, dir, &mut files, &mut total)?;
    let wasm = files.get(&manifest.entry).with_context(|| {
        format!(
            "entry `{}` is not in the extension directory",
            manifest.entry
        )
    })?;
    super::host::check_module(wasm)?;
    let index = FilesIndex {
        schema: 1,
        files: files
            .iter()
            .map(|(path, bytes)| (path.clone(), asset::hash_bytes(bytes)))
            .collect(),
    };
    let index_bytes = serde_json::to_vec_pretty(&index)?;
    let mut buffer = Vec::new();
    {
        let mut zip = ZipWriter::new(std::io::Cursor::new(&mut buffer));
        let options = SimpleFileOptions::default()
            .compression_method(CompressionMethod::Deflated)
            .last_modified_time(zip::DateTime::default());
        zip.start_file(MANIFEST_FILE, options)?;
        zip.write_all(&manifest_bytes)?;
        zip.start_file(FILES_INDEX, options)?;
        zip.write_all(&index_bytes)?;
        for (path, bytes) in &files {
            zip.start_file(path, options)?;
            zip.write_all(bytes)?;
        }
        zip.finish()?;
    }
    asset::atomic_new(output, &buffer)?;
    Ok(json!({
        "ok": true,
        "output": output,
        "name": manifest.name,
        "version": manifest.version,
        "package_hash": asset::hash_bytes(&buffer),
        "files": files.len() + 2,
        "capabilities": manifest.capabilities,
    }))
}

struct Opened {
    manifest: Manifest,
    manifest_bytes: Vec<u8>,
    files: BTreeMap<String, Vec<u8>>,
    package_hash: String,
}

fn open(package: &Path) -> Result<Opened> {
    let package_bytes = fs::read(package)
        .with_context(|| format!("[missing-resource] could not read {}", package.display()))?;
    let package_hash = asset::hash_bytes(&package_bytes);
    let mut zip = ZipArchive::new(std::io::Cursor::new(package_bytes))
        .context("[malformed-resource] not a valid .penext archive")?;
    if zip.len() > MAX_FILES + 2 {
        bail!("[limit-exceeded] archive has too many entries")
    }
    let mut all = BTreeMap::new();
    let mut total = 0u64;
    for i in 0..zip.len() {
        let mut file = zip.by_index(i)?;
        let name = file
            .enclosed_name()
            .context("[unsafe-path] archive entry escapes the extension")?
            .to_string_lossy()
            .replace('\\', "/");
        if file.is_dir() {
            continue;
        }
        safe_relative(&name)?;
        total += file.size();
        if total > MAX_TOTAL_BYTES {
            bail!("[limit-exceeded] extension expands beyond 32 MiB")
        }
        let mut bytes = Vec::new();
        file.by_ref()
            .take(MAX_TOTAL_BYTES + 1)
            .read_to_end(&mut bytes)?;
        if all.insert(name.clone(), bytes).is_some() {
            bail!("[malformed-resource] duplicate archive entry `{name}`")
        }
    }
    let manifest_bytes = all
        .remove(MANIFEST_FILE)
        .context("[malformed-resource] archive has no extension.json")?;
    let manifest = Manifest::parse(&manifest_bytes)?;
    let index: FilesIndex = serde_json::from_slice(
        &all.remove(FILES_INDEX)
            .context("[malformed-resource] archive has no files.json digest index")?,
    )?;
    if index.files.keys().ne(all.keys()) {
        bail!("[hash-mismatch] archive files do not match files.json (extra or missing entries)")
    }
    for (path, bytes) in &all {
        if index.files[path] != asset::hash_bytes(bytes) {
            bail!("[hash-mismatch] `{path}` does not match its recorded digest")
        }
    }
    let wasm = all.get(&manifest.entry).with_context(|| {
        format!(
            "[missing-resource] entry `{}` is not in the package",
            manifest.entry
        )
    })?;
    super::host::check_module(wasm)?;
    Ok(Opened {
        manifest,
        manifest_bytes,
        files: all,
        package_hash,
    })
}

/// Offline verification: structure, digests, manifest and module validity.
pub fn verify(package: &Path) -> Result<Value> {
    let opened = open(package)?;
    Ok(json!({
        "ok": true,
        "name": opened.manifest.name,
        "version": opened.manifest.version,
        "package_hash": opened.package_hash,
        "capabilities": opened.manifest.capabilities,
        "limits": opened.manifest.limits,
        "files": opened.files.len() + 2,
    }))
}

/// Install a verified package. Without `approve` nothing is written and the
/// requested capabilities are returned so a person (or agent) can review them.
pub fn add(package: &Path, project: &Path, approve: bool) -> Result<Value> {
    let opened = open(package)?;
    let manifest = &opened.manifest;
    let prompt = json!({
        "name": manifest.name,
        "version": manifest.version,
        "package_hash": opened.package_hash,
        "capabilities": manifest.capabilities,
        "limits": manifest.limits,
        "known_capabilities": CAPABILITIES,
    });
    if !approve {
        return Ok(json!({
            "ok": true,
            "installed": false,
            "approval_required": true,
            "message": "review the capabilities, then re-run with --approve to install",
            "extension": prompt,
        }));
    }
    let root = extensions_dir(project);
    let target = root.join(&manifest.name);
    let staging = root.join(format!("{}.installing", manifest.name));
    if staging.exists() {
        fs::remove_dir_all(&staging)?;
    }
    fs::create_dir_all(&staging)?;
    fs::write(staging.join(MANIFEST_FILE), &opened.manifest_bytes)?;
    for (path, bytes) in &opened.files {
        let out = staging.join(path);
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(out, bytes)?;
    }
    let record = InstallRecord {
        name: manifest.name.clone(),
        version: manifest.version.clone(),
        package_hash: opened.package_hash.clone(),
        entry_hash: asset::hash_bytes(&opened.files[&manifest.entry]),
        approved_capabilities: manifest.capabilities.clone(),
    };
    fs::write(
        staging.join(INSTALL_RECORD),
        serde_json::to_vec_pretty(&record)?,
    )?;
    let replaced = target.exists();
    if replaced {
        fs::remove_dir_all(&target)?;
    }
    fs::rename(&staging, &target)?;
    Ok(json!({
        "ok": true,
        "installed": true,
        "replaced": replaced,
        "extension": prompt,
        "approved_capabilities": record.approved_capabilities,
        "path": target,
    }))
}

/// An installed extension whose files and approvals have just been re-verified.
pub struct Installed {
    pub manifest: Manifest,
    pub wasm: Vec<u8>,
    pub record: InstallRecord,
}

pub fn load(project: &Path, name: &str) -> Result<Installed> {
    super::manifest::validate_name(name)?;
    let dir = extensions_dir(project).join(name);
    if !dir.is_dir() {
        bail!(
            "[missing-resource] extension `{name}` is not installed in {}",
            project.display()
        )
    }
    let manifest = Manifest::parse(&fs::read(dir.join(MANIFEST_FILE))?)?;
    let record: InstallRecord = serde_json::from_slice(&fs::read(dir.join(INSTALL_RECORD))?)
        .context("[malformed-resource] install record is unreadable; reinstall the extension")?;
    let wasm = fs::read(dir.join(&manifest.entry))?;
    if asset::hash_bytes(&wasm) != record.entry_hash {
        bail!("[hash-mismatch] installed module for `{name}` was modified after install; reinstall it")
    }
    for capability in &manifest.capabilities {
        if !record.approved_capabilities.contains(capability) {
            bail!("[capability-denied] `{capability}` was never approved for `{name}`; reinstall with --approve")
        }
    }
    Ok(Installed {
        manifest,
        wasm,
        record,
    })
}

pub fn list(project: &Path) -> Result<Value> {
    let root = extensions_dir(project);
    let mut items = Vec::new();
    if root.is_dir() {
        let mut names: Vec<_> = fs::read_dir(&root)?
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| !n.ends_with(".installing"))
            .collect();
        names.sort();
        for name in names {
            items.push(match load(project, &name) {
                Ok(i) => json!({
                    "name": i.manifest.name,
                    "version": i.manifest.version,
                    "description": i.manifest.description,
                    "capabilities": i.manifest.capabilities,
                    "limits": i.manifest.limits,
                    "commands": i.manifest.commands,
                    "package_hash": i.record.package_hash,
                    "status": "ok",
                }),
                Err(e) => json!({"name": name, "status": "broken", "error": format!("{e:#}")}),
            });
        }
    }
    Ok(json!({"ok": true, "count": items.len(), "extensions": items}))
}

pub fn remove(project: &Path, name: &str) -> Result<Value> {
    super::manifest::validate_name(name)?;
    let dir = extensions_dir(project).join(name);
    if !dir.is_dir() {
        bail!("[missing-resource] extension `{name}` is not installed")
    }
    fs::remove_dir_all(&dir)?;
    Ok(json!({"ok": true, "removed": name}))
}

#[derive(Debug, Default)]
pub struct RunOptions {
    pub command: Option<String>,
    pub args: Option<Value>,
    pub event: Option<Value>,
    pub page: Option<String>,
    pub dry_run: bool,
    pub if_revision: Option<String>,
}

/// Run an installed extension against a document.
pub fn run(project: &Path, name: &str, document: &Path, options: RunOptions) -> Result<Value> {
    let installed = load(project, name)?;
    super::host::run(super::host::RunRequest {
        manifest: &installed.manifest,
        wasm: &installed.wasm,
        document,
        command: options.command,
        args: options.args.unwrap_or(Value::Null),
        event: options.event.unwrap_or(Value::Null),
        page: options.page,
        dry_run: options.dry_run,
        if_revision: options.if_revision,
    })
}
