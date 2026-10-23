//! Production packages: one directory holding the PDF/X draft, plates, the
//! preflight report, every verified profile dependency, the source document
//! and a digest manifest. See "Production packages (implemented subset)" in
//! `docs/professional-color-v1.md`.
//!
//! The package is staged beside the target and renamed into place, so a failed
//! build never leaves a partial package. Building refuses when preflight finds
//! errors; the corrective issues are listed in the error.
use super::{catalog, pdfx, preflight, roles, separation};
use anyhow::{bail, ensure, Context, Result};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

pub const MANIFEST: &str = "package.json";
pub const MAX_MANIFEST_BYTES: usize = 1 << 20;

pub struct Options {
    pub pdfx: pdfx::Options,
    pub preflight: preflight::Options,
    pub separation_scale: f32,
    /// Automatic trap width in document units for the package plates.
    pub auto_trap: Option<f64>,
    /// Plate PNG depth (8 or 16).
    pub plate_bits: u8,
}

fn digest(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn entry(path: &str, role: &str, bytes: &[u8]) -> Value {
    json!({"path":path,"role":role,"bytes":bytes.len(),"sha256":digest(bytes)})
}

/// Build the package into `out` (which must not exist). `source` is the exact
/// document bytes the package was produced from.
pub fn build(
    raw: &Value,
    document: &Path,
    source: &[u8],
    page_id: Option<&str>,
    out: &Path,
    options: &Options,
) -> Result<Value> {
    ensure!(
        !out.exists(),
        "[conflict] {} already exists; choose a new package directory",
        out.display()
    );
    let report = preflight::run(raw, document, page_id, &options.preflight)?;
    if report["ok"] != json!(true) {
        let codes: Vec<&str> = report["issues"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|i| i["severity"] == "error")
            .filter_map(|i| i["code"].as_str())
            .collect();
        bail!(
            "[invalid-input] preflight found {} error(s) ({}); resolve them (pentool color preflight) before packaging",
            codes.len(),
            codes.join(", ")
        );
    }
    let built = pdfx::build(raw, document, page_id, &options.pdfx)?;

    let mut files: Vec<(String, String, Vec<u8>)> = Vec::new();
    files.push(("document.pen".into(), "source".into(), source.to_vec()));
    files.push(("output.pdf".into(), "pdfx-draft".into(), built.bytes));
    let mut preflight_bytes = serde_json::to_vec_pretty(&report)?;
    preflight_bytes.push(b'\n');
    files.push(("preflight.json".into(), "preflight".into(), preflight_bytes));

    // Plates are optional: constructs outside the separation subset are
    // recorded, not hidden, and do not fail the whole package.
    let plates = match separation::separate(
        raw,
        document,
        page_id,
        &separation::Options {
            destination: options.pdfx.destination.clone(),
            intent: options.pdfx.intent,
            scale: options.separation_scale,
            auto_trap: options.auto_trap,
        },
    ) {
        Ok(separations) => {
            // Unique per call: packages of the same bytes may be built concurrently.
            static STAGE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let staged = std::env::temp_dir().join(format!(
                "pentool-plates-{}-{}-{}",
                std::process::id(),
                STAGE.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
                digest(source).get(..12).unwrap_or("x")
            ));
            let _ = std::fs::remove_dir_all(&staged);
            // Ink map: coverage per plate plus the heaviest total-ink pixel.
            let pixels = (separations.width as usize * separations.height as usize).max(1);
            let mut heaviest = 0.0_f64;
            for at in 0..pixels {
                let total: f64 = separations
                    .plates
                    .iter()
                    .filter(|plate| !plate.spot)
                    .map(|plate| plate.samples[at])
                    .sum();
                heaviest = heaviest.max(total);
            }
            let per_plate: Vec<Value> = separations
                .plates
                .iter()
                .map(|plate| {
                    let sum: f64 = plate.samples.iter().sum();
                    json!({"plate":plate.id,"kind":if plate.spot {"spot"} else {"process"},
                        "average_coverage_percent":sum / pixels as f64 * 100.0,
                        "max_percent":plate.samples.iter().fold(0.0_f64, |a, b| a.max(*b)) * 100.0})
                })
                .collect();
            let mut ink_map = serde_json::to_vec_pretty(&json!({
                "version":1,"scope":"ink-map","width":separations.width,
                "height":separations.height,"plates":per_plate,
                "max_total_process_ink_percent":heaviest * 100.0}))?;
            ink_map.push(b'\n');
            files.push(("ink-map.json".into(), "ink-map".into(), ink_map));
            let result = separations
                .write_with_depth(&staged, options.plate_bits)
                .and_then(|info| {
                    let mut names = Vec::new();
                    for item in std::fs::read_dir(&staged)? {
                        names.push(item?.file_name().to_string_lossy().into_owned());
                    }
                    names.sort();
                    for name in names {
                        let bytes = std::fs::read(staged.join(&name))?;
                        files.push((format!("plates/{name}"), "plate".into(), bytes));
                    }
                    Ok(info)
                });
            let _ = std::fs::remove_dir_all(&staged);
            result?;
            json!({"status":"written"})
        }
        Err(error) => json!({"status":"skipped","reason":error.to_string()}),
    };

    // Soft-proof image: recorded as skipped (with the reason) when the roles
    // cannot support one, never silently omitted.
    let proof = match super::proofimage::render(raw, document, page_id) {
        Ok(proof) => {
            files.push(("proof.png".into(), "soft-proof".into(), proof.png));
            proof.report
        }
        Err(error) => json!({"status":"skipped","reason":error.to_string()}),
    };

    // Linked image assets, hash-verified and kept at their document-relative
    // paths so the packaged document.pen resolves them unchanged.
    let (mut linked, mut embedded) = (0usize, 0usize);
    if let Some(assets) = raw["image_assets"].as_object() {
        for (id, asset) in assets {
            if asset["storage"]["kind"] != "external" {
                embedded += 1;
                continue;
            }
            let path = asset["storage"]["path"]
                .as_str()
                .context("[invalid-input] external image asset has no path")?
                .to_owned();
            crate::resource::safe_relative_path(Path::new(&path))?;
            let bytes = crate::image::load_asset_bytes_for_document(raw, document, id)?;
            if files.iter().any(|(p, _, _)| *p == path) {
                bail!("[conflict] linked image path {path} collides with a package file; rename the link");
            }
            files.push((path, "image-asset".into(), bytes));
            linked += 1;
        }
    }
    let assets_report = json!({"linked_images":linked,"embedded_images":embedded,
        "fonts":"not-collected"});

    // Every profile any color role depends on, verified by the catalog loader.
    let mut dependencies = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for role in roles::ALL {
        let Ok(setting) = roles::effective(raw, role) else {
            continue;
        };
        if let Some(reference) = setting.destination.profile() {
            if !seen.insert(reference.to_owned()) {
                continue;
            }
            let (identity, bytes) = catalog::load(raw, document, reference)?;
            let path = format!("profiles/{}.icc", digest(&bytes));
            dependencies.push(json!({"reference":reference,"identity":identity,
                "role":role.name(),"path":path,"sha256":digest(&bytes)}));
            if !files.iter().any(|(p, _, _)| *p == path) {
                files.push((path, "profile".into(), bytes));
            }
        }
    }

    files.sort_by(|a, b| a.0.cmp(&b.0));
    let manifest = json!({
        "version": 1,
        "scope": "production-package",
        "page": report["page"],
        "pdfx": built.report,
        "plates": plates,
        "proof": proof,
        "assets": assets_report,
        "dependencies": dependencies,
        "files": files.iter().map(|(p, r, b)| entry(p, r, b)).collect::<Vec<_>>(),
        "independent_validation": built.report["independent_validation"],
    });
    let mut manifest_bytes = serde_json::to_vec_pretty(&manifest)?;
    manifest_bytes.push(b'\n');
    ensure!(
        manifest_bytes.len() <= MAX_MANIFEST_BYTES,
        "[limit-exceeded] package manifest exceeds {MAX_MANIFEST_BYTES} bytes"
    );
    files.push((MANIFEST.into(), "manifest".into(), manifest_bytes));

    let parent = out.parent().filter(|p| !p.as_os_str().is_empty());
    let name = out
        .file_name()
        .context("[invalid-input] package path has no name")?
        .to_string_lossy()
        .into_owned();
    let stage: PathBuf = parent
        .unwrap_or_else(|| Path::new("."))
        .join(format!(".{name}.pentool-tmp-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&stage);
    let write = || -> Result<()> {
        for (path, _, bytes) in &files {
            let target = stage.join(path);
            std::fs::create_dir_all(target.parent().unwrap())?;
            std::fs::write(&target, bytes).with_context(|| format!("write {path}"))?;
        }
        std::fs::rename(&stage, out)
            .with_context(|| format!("move package into place at {}", out.display()))
    };
    if let Err(error) = write() {
        let _ = std::fs::remove_dir_all(&stage);
        return Err(error);
    }
    Ok(manifest)
}

/// Re-hash every file named by the manifest. Reads only the package directory.
pub fn verify(directory: &Path) -> Result<Value> {
    let manifest_bytes =
        crate::resource::read_bounded(&directory.join(MANIFEST), MAX_MANIFEST_BYTES)?;
    let manifest: Value =
        serde_json::from_slice(&manifest_bytes).context("invalid package manifest")?;
    ensure!(
        manifest["scope"] == "production-package" && manifest["version"] == 1,
        "[unsupported-version] not a version 1 production package manifest"
    );
    let mut problems = Vec::new();
    let mut checked = 0usize;
    let mut listed = std::collections::BTreeSet::new();
    for item in manifest["files"]
        .as_array()
        .context("manifest files missing")?
    {
        let path = item["path"].as_str().context("manifest path missing")?;
        ensure!(
            !path.is_empty()
                && !path.starts_with('/')
                && !path.contains('\\')
                && !path.contains(':')
                && path.split('/').all(|part| !part.is_empty() && part != ".."),
            "[invalid-input] unsafe package path {path}"
        );
        checked += 1;
        listed.insert(path.to_string());
        match std::fs::read(directory.join(path)) {
            Err(_) => problems.push(json!({"path":path,"problem":"missing"})),
            Ok(bytes) => {
                if digest(&bytes) != item["sha256"].as_str().unwrap_or_default()
                    || bytes.len() as u64 != item["bytes"].as_u64().unwrap_or(u64::MAX)
                {
                    problems.push(json!({"path":path,"problem":"digest-mismatch"}));
                }
            }
        }
    }
    let mut found = Vec::new();
    unlisted_files(directory, directory, &mut found)?;
    for path in found {
        if path != MANIFEST && !listed.contains(&path) {
            problems.push(json!({"path":path,"problem":"unlisted"}));
        }
    }
    Ok(json!({"ok":problems.is_empty(),"checked":checked,"problems":problems}))
}

/// Relative `/`-separated paths of every file below `dir`, sorted for stable output.
fn unlisted_files(root: &Path, dir: &Path, out: &mut Vec<String>) -> Result<()> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)?.collect::<std::io::Result<_>>()?;
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            unlisted_files(root, &path, out)?;
        } else if let Ok(rel) = path.strip_prefix(root) {
            out.push(
                rel.components()
                    .map(|c| c.as_os_str().to_string_lossy())
                    .collect::<Vec<_>>()
                    .join("/"),
            );
        }
    }
    Ok(())
}
