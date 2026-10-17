//! Library packages: the `design_library` block, offline verification, packing
//! against a previous release, and diff reports (spec sections 1.3 and 5).
use super::{page_args, paginate};
use crate::{
    asset, package,
    tokens::{self, DesignError},
};
use serde_json::{json, Map, Value};
use std::{collections::BTreeMap, fs, io::Read, path::Path};

type Result<T> = std::result::Result<T, DesignError>;

#[derive(Debug, Clone)]
pub struct LibToken {
    pub token_type: String,
    pub value: Value,
    pub description: String,
    pub hash: String,
}

#[derive(Debug, Clone)]
pub struct Snapshot {
    pub name: String,
    pub version: String,
    pub package_hash: String,
    pub tokens: BTreeMap<String, LibToken>,
}

fn invalid(message: impl Into<String>) -> DesignError {
    DesignError::new("invalid-input", message)
}

/// Parse and strictly validate a `design_library` block of an asset document.
/// Returns `None` when the document has no block.
pub fn parse_block(doc: &Value) -> Result<Option<Snapshot>> {
    let Some(block) = doc.get("design_library") else {
        return Ok(None);
    };
    let object = block
        .as_object()
        .ok_or_else(|| invalid("design_library must be an object"))?;
    if object.get("schema").and_then(Value::as_u64) != Some(1) {
        return Err(invalid("design_library.schema must be 1").action("set \"schema\": 1"));
    }
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("design_library.name is required"))?;
    asset::validate_id(name).map_err(|e| invalid(format!("design_library.name: {e}")))?;
    let version = object
        .get("version")
        .and_then(Value::as_str)
        .ok_or_else(|| invalid("design_library.version is required"))?;
    semver::Version::parse(version)
        .map_err(|_| invalid("design_library.version must use semantic versioning"))?;
    let raw_tokens = match object.get("tokens") {
        None => Map::new(),
        Some(Value::Object(map)) => map.clone(),
        Some(_) => return Err(invalid("design_library.tokens must be an object")),
    };
    if raw_tokens.len() > tokens::MAX_TOKENS {
        return Err(DesignError::new(
            "limit-exceeded",
            format!(
                "library has {} tokens; the limit is {}",
                raw_tokens.len(),
                tokens::MAX_TOKENS
            ),
        )
        .action("split the library"));
    }
    let mut out = BTreeMap::new();
    let mut store = Map::new();
    for (id, record) in &raw_tokens {
        if !tokens::valid_library_id(id) {
            return Err(DesignError::new(
                "invalid-token-id",
                format!("token ID {id:?} does not match the library ID grammar"),
            )
            .token(id.clone())
            .action("use lowercase dotted segments such as color.accent"));
        }
        let record = record
            .as_object()
            .ok_or_else(|| invalid(format!("token {id} must be an object")).token(id.clone()))?;
        for key in record.keys() {
            if !matches!(key.as_str(), "type" | "value" | "description") {
                return Err(
                    invalid(format!("token {id} has unknown key {key:?}")).token(id.clone())
                );
            }
        }
        let token_type = record
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| invalid(format!("token {id} needs a string type")).token(id.clone()))?;
        // `type` is an import alias of `typography` (decision Q4); stored canonically.
        let token_type = if token_type == "type" {
            "typography"
        } else {
            token_type
        };
        if !tokens::TYPES.contains(&token_type) || token_type == "stroke" {
            return Err(invalid(format!(
                "token {id}: type {token_type:?} is not allowed in a library"
            ))
            .token(id.clone())
            .action("use color, typography, spacing, radius or shadow"));
        }
        let value = record
            .get("value")
            .ok_or_else(|| invalid(format!("token {id} needs a value")).token(id.clone()))?;
        if value.get("ref").is_none() {
            tokens::validate_value(token_type, value, true).map_err(|reason| {
                DesignError::new("invalid-token-value", format!("token {id}: {reason}"))
                    .token(id.clone())
                    .action("fix the token value")
            })?;
        } else if tokens::canonical(value).len() > tokens::MAX_VALUE_BYTES {
            return Err(DesignError::new(
                "invalid-token-value",
                format!("token {id}: value too large"),
            )
            .token(id.clone()));
        }
        store.insert(id.clone(), json!({"type": token_type, "value": value}));
        out.insert(
            id.clone(),
            LibToken {
                token_type: token_type.to_owned(),
                value: value.clone(),
                description: record
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_owned(),
                hash: tokens::value_hash(token_type, value),
            },
        );
    }
    let store = Value::Object(store);
    // Alias closure: every alias names a token of this library with the same type.
    for (id, token) in &out {
        if let Some(target) = token.value.get("ref").and_then(Value::as_str) {
            match out.get(target) {
                None => {
                    return Err(DesignError::new(
                        "unknown-token",
                        format!("token {id} aliases {target}, which is not in the library"),
                    )
                    .token(id.clone())
                    .action("add the target token or use a literal value"))
                }
                Some(other) if other.token_type != token.token_type => {
                    return Err(DesignError::new(
                        "token-type-mismatch",
                        format!(
                            "token {id} ({}) aliases {target} ({})",
                            token.token_type, other.token_type
                        ),
                    )
                    .token(id.clone())
                    .action("alias a token of the same type"))
                }
                _ => {}
            }
        }
    }
    tokens::check_write(&store, None)?;
    Ok(Some(Snapshot {
        name: name.to_owned(),
        version: version.to_owned(),
        package_hash: String::new(),
        tokens: out,
    }))
}

fn map_package_error(error: anyhow::Error) -> DesignError {
    let text = format!("{error:#}");
    let lower = text.to_ascii_lowercase();
    if let Some(design) = error.downcast_ref::<DesignError>() {
        return design.clone();
    }
    let code = if lower.contains("hash mismatch") || lower.contains("checksum") {
        "hash-mismatch"
    } else if lower.contains("unsafe") {
        "unsafe-path"
    } else {
        "invalid-input"
    };
    DesignError::new(code, text).action("re-download or rebuild the package; it was not installed")
}

/// Verify a `.penpkg` offline and load its single library block.
pub fn load_package(path: &Path) -> Result<Snapshot> {
    let report = package::verify(path).map_err(map_package_error)?;
    let manifest: package::PackageManifest =
        serde_json::from_value(report["package"].clone()).map_err(|e| invalid(e.to_string()))?;
    let package_hash = report["package_hash"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let file = fs::File::open(path).map_err(|e| invalid(format!("{}: {e}", path.display())))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| map_package_error(e.into()))?;
    let mut found: Vec<Snapshot> = Vec::new();
    for (id, entry) in &manifest.assets {
        let mut data = Vec::new();
        zip.by_name(&entry.path)
            .map_err(|e| map_package_error(e.into()))?
            .read_to_end(&mut data)
            .map_err(|e| map_package_error(e.into()))?;
        let doc: Value = serde_json::from_slice(&data).map_err(|e| invalid(e.to_string()))?;
        if let Some(snapshot) = parse_block(&doc).map_err(|e| e.token(String::new()))? {
            let _ = id;
            found.push(snapshot);
        }
    }
    if found.len() != 1 {
        return Err(invalid(format!(
            "package {} must contain exactly one design_library asset, found {}",
            manifest.name,
            found.len()
        )));
    }
    let mut snapshot = found.pop().unwrap();
    if snapshot.name != manifest.name || snapshot.version != manifest.version {
        return Err(invalid(format!(
            "design_library {}@{} does not match package {}@{}",
            snapshot.name, snapshot.version, manifest.name, manifest.version
        ))
        .action("make design_library.name/version equal the package manifest"));
    }
    snapshot.package_hash = package_hash;
    Ok(snapshot)
}

pub fn summary(snapshot: &Snapshot) -> Value {
    json!({
        "ok": true,
        "package_hash": snapshot.package_hash,
        "name": snapshot.name,
        "version": snapshot.version,
        "tokens": snapshot.tokens.len(),
        "templates": 0,
        "warnings": [],
    })
}

/// Optional offline signature requirement (`--signature`, `--public-key`).
pub fn check_signature(package: &Path, signature: Option<&Path>, key: Option<&Path>) -> Result<()> {
    match (signature, key) {
        (None, None) => Ok(()),
        (Some(signature), key) => package::verify_signature(package, signature, key)
            .map(|_| ())
            .map_err(|e| {
                DesignError::new("hash-mismatch", format!("{e:#}"))
                    .action("obtain a correctly signed package")
            }),
        (None, Some(_)) => Err(invalid("--public-key needs --signature")),
    }
}

pub fn init(dir: &Path, name: &str) -> Result<Value> {
    asset::validate_id(name).map_err(|e| invalid(format!("library name: {e}")))?;
    package::init(dir, name).map_err(map_package_error)?;
    let mut doc = crate::scene::new_document(100, 100);
    doc["name"] = json!(name);
    doc["asset"] =
        json!({"schema":1,"id":name,"name":name,"asset_version":"0.1.0","kind":"library"});
    doc["design_library"] =
        json!({"schema":1,"name":name,"version":"0.1.0","description":"","license":"","tokens":{}});
    let file = dir
        .join("assets")
        .join(format!("{}.pen", name.replace('/', "-")));
    asset::atomic_new(&file, &serde_json::to_vec_pretty(&doc).unwrap())
        .map_err(|e| invalid(format!("{e:#}")))?;
    Ok(json!({"ok":true,"library":name,"asset":file,"manifest":dir.join("pentool.package.json")}))
}

fn rank(level: &str) -> i32 {
    match level {
        "major" => 3,
        "minor" => 2,
        "patch" => 1,
        "none" => 0,
        _ => -1,
    }
}

pub struct Diff {
    pub entries: Vec<Value>,
    pub required: &'static str,
}

/// Compare two token sets with the section 4 vocabulary and derive the semver
/// bump the change requires.
pub fn diff_tokens(
    a: &BTreeMap<String, LibToken>,
    b: &BTreeMap<String, LibToken>,
    all: bool,
) -> Diff {
    let mut required = "none";
    let mut bump = |level: &'static str| {
        if rank(level) > rank(required) {
            required = level;
        }
    };
    let mut entries = Vec::new();
    let names: std::collections::BTreeSet<&String> = a.keys().chain(b.keys()).collect();
    for name in names {
        let (status, row) = match (a.get(name), b.get(name)) {
            (None, Some(new)) => {
                bump("minor");
                (
                    "added",
                    json!({"type":new.token_type,"to":new.value,"incoming":new.hash}),
                )
            }
            (Some(old), None) => {
                bump("major");
                (
                    "removed",
                    json!({"type":old.token_type,"from":old.value,"base":old.hash}),
                )
            }
            (Some(old), Some(new)) if old.token_type != new.token_type => {
                bump("major");
                (
                    "type-changed",
                    json!({"type":new.token_type,"from_type":old.token_type,"from":old.value,"to":new.value}),
                )
            }
            (Some(old), Some(new)) if old.hash != new.hash => {
                bump("minor");
                (
                    "changed",
                    json!({"type":new.token_type,"from":old.value,"to":new.value,"base":old.hash,"incoming":new.hash}),
                )
            }
            (Some(old), Some(new)) if old.description != new.description => {
                bump("patch");
                ("metadata", json!({"type":new.token_type}))
            }
            _ => {
                if !all {
                    continue;
                }
                ("unchanged", json!({"type":a[name].token_type}))
            }
        };
        let mut row = row;
        row["token"] = json!(name);
        row["status"] = json!(status);
        entries.push(row);
    }
    Diff { entries, required }
}

fn declared(a: &str, b: &str) -> &'static str {
    let (Ok(a), Ok(b)) = (semver::Version::parse(a), semver::Version::parse(b)) else {
        return "invalid";
    };
    if b.major > a.major {
        "major"
    } else if b.major < a.major {
        "downgrade"
    } else if b.minor > a.minor {
        "minor"
    } else if b.minor < a.minor {
        "downgrade"
    } else if b.patch > a.patch {
        "patch"
    } else if b.patch < a.patch {
        "downgrade"
    } else {
        "none"
    }
}

/// Tokens a document vendored from `library`, at their current values.
fn document_tokens(doc: &Value, library: &str) -> BTreeMap<String, LibToken> {
    let mut out = BTreeMap::new();
    for (name, record) in doc
        .get("styles")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
    {
        if record.pointer("/source/library").and_then(Value::as_str) != Some(library) {
            continue;
        }
        let (Some(token_type), Some(value)) = (
            record.get("type").and_then(Value::as_str),
            record.get("value"),
        ) else {
            continue;
        };
        out.insert(
            name.clone(),
            LibToken {
                token_type: token_type.to_owned(),
                value: value.clone(),
                description: String::new(),
                hash: tokens::value_hash(token_type, value),
            },
        );
    }
    out
}

pub struct DiffOptions {
    pub offset: Option<usize>,
    pub limit: Option<usize>,
    pub all: bool,
}

/// `design library diff A B`: A is a package; B is a package or a `.pen` document.
pub fn diff(a: &Path, b: &Path, options: &DiffOptions) -> Result<Value> {
    let (offset, limit) = page_args(options.offset, options.limit)?;
    let left = load_package(a)?;
    let is_doc = b.extension().and_then(|e| e.to_str()) == Some("pen");
    let (right_tokens, right_info, verdict) = if is_doc {
        let doc: Value = serde_json::from_slice(
            &fs::read(b).map_err(|e| invalid(format!("{}: {e}", b.display())))?,
        )
        .map_err(|e| invalid(format!("invalid .pen document: {e}")))?;
        let version = doc
            .pointer(&format!(
                "/design/libraries/{}/version",
                left.name.replace('/', "~1")
            ))
            .and_then(Value::as_str)
            .map(str::to_owned);
        (
            document_tokens(&doc, &left.name),
            json!({"document": b, "name": left.name, "version": version}),
            false,
        )
    } else {
        let right = load_package(b)?;
        let info =
            json!({"name":right.name,"version":right.version,"package_hash":right.package_hash});
        (right.tokens.clone(), info, true)
    };
    let result = diff_tokens(&left.tokens, &right_tokens, options.all);
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for entry in &result.entries {
        *counts
            .entry(entry["status"].as_str().unwrap().to_owned())
            .or_default() += 1;
    }
    let (page, meta) = paginate(result.entries, offset, limit);
    let mut report = json!({
        "a": {"name":left.name,"version":left.version,"package_hash":left.package_hash},
        "b": right_info,
        "counts": counts,
        "tokens": page,
    });
    if let (true, Some(right_version)) = (verdict, report["b"]["version"].as_str()) {
        let declared = declared(&left.version, right_version);
        report["semver"] = json!({
            "required": result.required,
            "declared": declared,
            "ok": rank(declared) >= rank(result.required),
        });
    } else {
        report["semver"] = Value::Null;
    }
    if let (Some(target), Some(source)) = (report.as_object_mut(), meta.as_object()) {
        for (key, value) in source {
            target.insert(key.clone(), value.clone());
        }
    }
    Ok(report)
}

/// Pack a library directory (`design library pack`). With `previous`, refuse a
/// version that is lower than the bump the token changes require (section 1.3).
pub fn pack(dir: &Path, output: &Path, previous: Option<&Path>) -> Result<Value> {
    let manifest_bytes = fs::read(dir.join("pentool.package.json")).map_err(|e| {
        invalid(format!(
            "missing pentool.package.json in {}: {e}",
            dir.display()
        ))
    })?;
    let manifest: package::PackageManifest =
        serde_json::from_slice(&manifest_bytes).map_err(|e| invalid(e.to_string()))?;
    let mut blocks: Vec<Snapshot> = Vec::new();
    for entry in walkdir::WalkDir::new(dir.join("assets"))
        .follow_links(false)
        .sort_by_file_name()
    {
        let entry = entry.map_err(|e| invalid(e.to_string()))?;
        if !entry.file_type().is_file()
            || entry.path().extension().and_then(|e| e.to_str()) != Some("pen")
        {
            continue;
        }
        let doc: Value =
            serde_json::from_slice(&fs::read(entry.path()).map_err(|e| invalid(e.to_string()))?)
                .map_err(|e| invalid(format!("{}: {e}", entry.path().display())))?;
        if let Some(block) = parse_block(&doc)? {
            blocks.push(block);
        }
    }
    let [block] = blocks.as_slice() else {
        return Err(invalid(format!(
            "{} must contain exactly one asset with a design_library block, found {}",
            dir.display(),
            blocks.len()
        )));
    };
    if block.name != manifest.name || block.version != manifest.version {
        return Err(invalid(format!(
            "design_library {}@{} does not match manifest {}@{}",
            block.name, block.version, manifest.name, manifest.version
        ))
        .action("make design_library.name/version equal the package manifest"));
    }
    if let Some(previous) = previous {
        let old = load_package(previous)?;
        let result = diff_tokens(&old.tokens, &block.tokens, false);
        let declared = declared(&old.version, &block.version);
        if rank(declared) < rank(result.required) {
            return Err(invalid(format!(
                "changes since {} require a {} version bump; {} -> {} is {declared}",
                old.version, result.required, old.version, block.version
            ))
            .action(format!(
                "raise the version to a {} release",
                result.required
            )));
        }
    }
    package::pack(dir, output).map_err(map_package_error)?;
    let packed = load_package(output)?;
    Ok(summary(&packed))
}
