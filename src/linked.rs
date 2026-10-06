//! Hash-verified, offline linked-image maintenance and portable collection.
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::{fs, path::Path};

fn read_path(document: &Path, relative: &Path) -> Result<Vec<u8>> {
    let mut remaining = crate::resource::MAX_RESOURCE_BYTES;
    read_path_bounded(document, relative, &mut remaining)
}

fn read_path_bounded(document: &Path, relative: &Path, remaining: &mut usize) -> Result<Vec<u8>> {
    let relative = crate::resource::safe_relative_path(relative)?;
    let root = crate::resource::document_root(document).canonicalize()?;
    let file = root.join(relative).canonicalize()?;
    if !file.starts_with(&root) {
        bail!("[unsafe-path] linked source escapes document root")
    }
    let length =
        usize::try_from(fs::metadata(&file)?.len()).context("linked source size overflow")?;
    if length > crate::resource::MAX_RESOURCE_BYTES || length > *remaining {
        bail!("[limit-exceeded] linked source too large")
    }
    *remaining -= length;
    Ok(fs::read(file)?)
}

pub fn report(raw: &Value, document: &Path) -> Result<Value> {
    crate::transaction::validate_value(raw)?;
    let assets = raw["image_assets"]
        .as_object()
        .context("image assets missing")?;
    let mut remaining = crate::resource::MAX_RESOURCE_BYTES;
    let entries = assets
        .iter()
        .map(|(id, asset)| {
            let storage = &asset["storage"];
            let status = if storage["kind"] == "embedded" {
                "embedded"
            } else {
                match read_path_bounded(
                    document,
                    Path::new(storage["path"].as_str().unwrap()),
                    &mut remaining,
                ) {
                    Ok(bytes) if crate::resource::sha256(&bytes) == *id => "verified",
                    Ok(_) => "stale",
                    Err(error) if error.to_string().contains("limit-exceeded") => {
                        "not-checked-limit"
                    }
                    Err(_) => "missing-or-unsafe",
                }
            };
            json!({"asset":id,"storage":storage,"status":status,"byte_length":asset["byte_length"]})
        })
        .collect::<Vec<_>>();
    let portable = entries
        .iter()
        .all(|entry| entry["status"] == "embedded" || entry["status"] == "verified");
    Ok(json!({"dependencies":entries,"portable":portable,"offline":true}))
}

pub fn edit(
    raw: &mut Value,
    document: &Path,
    operation: &str,
    id: &str,
    path: Option<&Path>,
    dry_run: bool,
) -> Result<Value> {
    crate::transaction::validate_value(raw)?;
    raw["image_assets"]
        .get(id)
        .context("[missing-resource] asset not found")?;
    let mut candidate = raw.clone();
    match operation {
        "embed" => {
            let bytes =
                crate::image::load_asset_bytes(raw, crate::resource::document_root(document), id)?;
            candidate["image_assets"][id]["storage"] = crate::image::embedded_storage(&bytes);
        }
        "relink" => {
            let path = path.context("relink requires a document-relative path")?;
            let bytes = read_path(document, path)?;
            crate::resource::verify(&bytes, id)?;
            candidate["image_assets"][id]["storage"] = crate::image::external_storage(path)?;
        }
        "externalize" => {
            let path = path.context("externalize requires a document-relative path")?;
            let storage = crate::image::external_storage(path)?;
            let bytes =
                crate::image::load_asset_bytes(raw, crate::resource::document_root(document), id)?;
            let root = crate::resource::document_root(document).canonicalize()?;
            let target = root.join(path);
            // Existing parent must be contained; never follow a symlink outside the project.
            let parent = target
                .parent()
                .context("externalize parent missing")?
                .canonicalize()?;
            if !parent.starts_with(&root) {
                bail!("[unsafe-path] externalize parent escapes document root")
            }
            if target.exists() {
                crate::resource::verify(&read_path(document, path)?, id)?;
            } else if !dry_run {
                use std::io::Write;
                let mut file = fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&target)?;
                if let Err(error) = file.write_all(&bytes).and_then(|()| file.sync_all()) {
                    drop(file);
                    let _ = fs::remove_file(&target);
                    return Err(error.into());
                }
            }
            candidate["image_assets"][id]["storage"] = storage;
        }
        _ => bail!("linked asset operation must be embed, relink, or externalize"),
    }
    crate::transaction::validate_value(&candidate)?;
    *raw = candidate;
    Ok(json!({"operation":operation,"asset":id,"storage":raw["image_assets"][id]["storage"]}))
}

pub fn locate(raw: &Value, document: &Path, id: &str, paths: &[String]) -> Result<Value> {
    if paths.len() > 4096 {
        bail!("[limit-exceeded] locate accepts at most 4096 candidates")
    }
    if raw["image_assets"].get(id).is_none() {
        bail!("[missing-resource] asset not found")
    }
    let mut matches = Vec::new();
    let mut remaining = crate::resource::MAX_RESOURCE_BYTES;
    for path in paths {
        crate::resource::safe_relative_path(Path::new(path))?;
        match read_path_bounded(document, Path::new(path), &mut remaining) {
            Ok(bytes) if crate::resource::sha256(&bytes) == id => matches.push(path.clone()),
            Err(error) if error.to_string().contains("limit-exceeded") => return Err(error),
            _ => {}
        }
    }
    matches.sort();
    matches.dedup();
    Ok(json!({"asset":id,"matches":matches}))
}

pub fn collect(raw: &Value, document: &Path, output: &Path, dry_run: bool) -> Result<Value> {
    crate::transaction::validate_value(raw)?;
    if output.exists() {
        bail!("[output-exists] collection requires a new directory")
    }
    let assets = raw["image_assets"]
        .as_object()
        .context("image assets missing")?;
    let mut candidate = raw.clone();
    let mut encoded = Vec::new();
    let mut total = 0usize;
    for (id, _) in assets {
        let bytes =
            crate::image::load_asset_bytes(raw, crate::resource::document_root(document), id)?;
        total = total
            .checked_add(bytes.len())
            .context("collection size overflow")?;
        if total > crate::resource::MAX_RESOURCE_BYTES {
            bail!("[limit-exceeded] collection exceeds 512 MiB")
        }
        let relative = format!("assets/{}", id.strip_prefix("sha256:").unwrap());
        candidate["image_assets"][id]["storage"] =
            crate::image::external_storage(Path::new(&relative))?;
        encoded.push((relative, bytes));
    }
    crate::transaction::validate_value(&candidate)?;
    if !dry_run {
        // Publish the entire portable project by a single same-filesystem rename.
        let parent = output
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        use rand_core::RngCore;
        struct Staging(std::path::PathBuf);
        impl Drop for Staging {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let staging_path = parent.canonicalize()?.join(format!(
            ".pentool-collect-{:016x}",
            rand_core::OsRng.next_u64()
        ));
        fs::create_dir(&staging_path)?;
        let staging = Staging(staging_path);
        fs::create_dir(staging.0.join("assets"))?;
        for (path, bytes) in &encoded {
            fs::write(staging.0.join(path), bytes)?;
        }
        fs::write(
            staging.0.join("document.pen"),
            serde_json::to_vec_pretty(&candidate)?,
        )?;
        fs::rename(&staging.0, output)?;
    }
    Ok(
        json!({"output":output,"document":"document.pen","assets":encoded.len(),"byte_length":total,"dry_run":dry_run}),
    )
}
