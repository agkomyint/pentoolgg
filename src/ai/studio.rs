//! Browser requests deliberately cannot supply document paths or raw credentials.
use super::*;
use anyhow::ensure;

#[derive(Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GenerateRequest {
    pub name: String,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub prompt: Option<String>,
    pub aspect: Option<String>,
    pub size: Option<String>,
    #[serde(default = "one")]
    pub candidates: usize,
    pub source: Option<String>,
    pub kind: Option<String>,
    pub page: Option<String>,
    pub job: Option<String>,
    pub reviewed_disclosure: Option<Value>,
    #[serde(default)]
    pub dry_run: bool,
    #[serde(default)]
    pub allow_model_call: bool,
}
fn one() -> usize {
    1
}

pub fn generate_request(document: PathBuf, request: GenerateRequest) -> Result<Value> {
    ensure_enabled()?;
    if request.allow_model_call && !request.dry_run {
        let mut review = request.clone();
        review.dry_run = true;
        let current = execute_request(document.clone(), review)?;
        ensure!(
            request.reviewed_disclosure.as_ref() == Some(&current["disclosure"]),
            "[conflict] the request changed or was not reviewed; review it again before confirming"
        );
    }
    execute_request(document, request)
}

fn execute_request(document: PathBuf, request: GenerateRequest) -> Result<Value> {
    if let Some(source) = request.source {
        edit(
            &EditArgs {
                document,
                name: request.name,
                source,
                kind: request.kind.unwrap_or_else(|| "edit".into()),
                provider: request.provider,
                model: request.model,
                prompt: request.prompt,
                prompt_file: None,
                key: "#FF00FF".into(),
                candidates: request.candidates,
                allow_model_call: request.allow_model_call,
                dry_run: request.dry_run,
                ephemeral: false,
            },
            request.page.as_deref(),
        )
    } else {
        ensure!(
            request.kind.is_none(),
            "[invalid-input] an edit kind requires a source image"
        );
        generate(&GenerateArgs {
            document,
            name: request.name,
            provider: request.provider,
            model: request.model,
            prompt: request.prompt,
            prompt_file: None,
            aspect: request.aspect,
            size: request.size,
            candidates: request.candidates,
            allow_model_call: request.allow_model_call,
            dry_run: request.dry_run,
            ephemeral: false,
        })
    }
}

pub fn runs(document: PathBuf) -> Result<Value> {
    run(
        AiAction::Run {
            action: RunAction::List { document },
        },
        None,
    )
}

/// Bounded thumbnails, decoded from verified candidates; never accept a filesystem path.
pub fn candidate(document: &Path, run: &str, index: usize) -> Result<Vec<u8>> {
    ensure_enabled()?;
    let (record, dir) = load_run(document, run)?;
    let entry = record["candidates"]
        .as_array()
        .and_then(|items| {
            items
                .iter()
                .find(|item| item["index"].as_u64() == Some(index as u64))
        })
        .context("[missing-resource] candidate not found")?;
    // Reconstruct the filename rather than trusting run metadata with a path.
    let file = format!(
        "candidate-{}.{}",
        index,
        extension(entry["media_type"].as_str().unwrap_or(""))
    );
    let path = dir.join(file);
    ensure!(
        fs::metadata(&path)?.len() <= MAX_RESPONSE_BYTES as u64,
        "[limit-exceeded] candidate file is too large"
    );
    let bytes = fs::read(path)?;
    ensure!(
        crate::resource::sha256(&bytes) == entry["sha256"],
        "[stale-source] candidate bytes changed"
    );
    let validated = validate_output(bytes)?;
    let image = ::image::load_from_memory(&validated.bytes)?;
    let image = if image.width() > 512 || image.height() > 512 {
        image.thumbnail(512, 512)
    } else {
        image
    };
    let image = image.to_rgba8();
    encode_png(&image)
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AcceptRequest {
    pub candidate: usize,
    pub id: Option<String>,
    pub layer: Option<String>,
    pub page: Option<String>,
    pub replace: Option<String>,
    pub if_revision: String,
    #[serde(default)]
    pub dry_run: bool,
}
pub fn accept_request(document: PathBuf, run_id: String, request: AcceptRequest) -> Result<Value> {
    ensure!(
        !request.if_revision.is_empty(),
        "[invalid-input] if_revision is required"
    );
    ensure!(
        request.replace.is_none() || (request.id.is_none() && request.layer.is_none()),
        "[invalid-input] replace cannot be combined with id or layer"
    );
    run(
        AiAction::Run {
            action: RunAction::Accept {
                document,
                run: run_id,
                candidate: request.candidate,
                id: request.id,
                layer: request.layer,
                x: None,
                y: None,
                width: None,
                height: None,
                replace: request.replace,
                allow_stale_source: false,
                dry_run: request.dry_run,
                if_revision: Some(request.if_revision),
            },
        },
        request.page.as_deref(),
    )
}
pub fn discard(document: PathBuf, run_id: String) -> Result<Value> {
    run(
        AiAction::Run {
            action: RunAction::Discard {
                document,
                run: run_id,
            },
        },
        None,
    )
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct KeyoutRequest {
    pub source: String,
    pub id: String,
    pub page: Option<String>,
    pub if_revision: String,
}
pub fn keyout_request(document: PathBuf, request: KeyoutRequest) -> Result<Value> {
    ensure_enabled()?;
    ensure!(
        !request.if_revision.is_empty(),
        "[invalid-input] if_revision is required"
    );
    let raw = read_document(&document)?;
    ensure_target_unlocked(&raw, request.page.as_deref(), &request.source)?;
    keyout(
        &KeyoutArgs {
            document,
            id: request.id,
            source: request.source,
            key: "#FF00FF".into(),
            tolerance: 60.0,
            softness: 90.0,
            dry_run: false,
            if_revision: Some(request.if_revision),
        },
        request.page.as_deref(),
    )
}
