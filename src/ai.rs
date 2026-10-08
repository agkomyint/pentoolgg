//! Optional, explicit BYOK image-model tooling (v0.9.0).
//!
//! Everything here is inert until a provider is configured, and no model is ever
//! contacted without `--allow-model-call`. Credentials are stored as references
//! (an environment variable name or a file path), never as values.
use anyhow::{bail, Context, Result};
use base64::Engine;
use clap::{Args, Subcommand};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

const MAX_CANDIDATES: usize = 8;
const MAX_RUNS_KEPT: usize = 16;
const MAX_PROMPT_BYTES: u64 = 64 * 1024;
const MAX_SOURCE_BYTES: usize = 20 * 1024 * 1024;
const MAX_RESPONSE_BYTES: usize = 96 * 1024 * 1024;
const DEFAULT_MAX_PIXELS: u64 = 33_554_432;
const GEMINI_ASPECTS: [&str; 10] = [
    "1:1", "2:3", "3:2", "3:4", "4:3", "4:5", "5:4", "9:16", "16:9", "21:9",
];

/// A structured, agent-repairable failure: stable code plus the corrective action.
#[derive(Debug)]
pub struct AiError {
    pub code: &'static str,
    pub message: String,
    pub fix: String,
}

impl fmt::Display for AiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {} Fix: {}", self.code, self.message, self.fix)
    }
}

impl std::error::Error for AiError {}

fn err(code: &'static str, message: impl Into<String>, fix: impl Into<String>) -> anyhow::Error {
    AiError {
        code,
        message: message.into(),
        fix: fix.into(),
    }
    .into()
}

// ---------------------------------------------------------------------------
// Catalog and configuration
// ---------------------------------------------------------------------------

struct CatalogEntry {
    name: &'static str,
    adapter: &'static str,
    endpoint: &'static str,
    env: &'static [&'static str],
    default_model: &'static str,
}

/// A convenience list, not an allowlist: any compatible endpoint can be connected.
const CATALOG: [CatalogEntry; 2] = [
    CatalogEntry {
        name: "gemini",
        adapter: "gemini",
        endpoint: "https://generativelanguage.googleapis.com/v1beta",
        env: &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
        default_model: "gemini-2.5-flash-image",
    },
    CatalogEntry {
        name: "openai",
        adapter: "openai-compatible",
        endpoint: "https://api.openai.com/v1",
        env: &["OPENAI_API_KEY"],
        default_model: "gpt-image-1",
    },
];

fn catalog(name: &str) -> Option<&'static CatalogEntry> {
    CATALOG.iter().find(|entry| entry.name == name)
}

fn capabilities(adapter: &str) -> &'static [&'static str] {
    match adapter {
        "gemini" => &["generate", "edit", "remove-background"],
        "openai-compatible" => &["generate"],
        _ => &[],
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(tag = "source", rename_all = "lowercase")]
enum Credential {
    Env { name: String },
    File { path: String },
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
struct Profile {
    adapter: String,
    endpoint: String,
    credential: Credential,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    default_model: Option<String>,
}

#[derive(Serialize, Deserialize, Default, Clone, PartialEq, Debug)]
struct Config {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    providers: BTreeMap<String, Profile>,
    #[serde(default)]
    defaults: BTreeMap<String, String>,
    /// Cached model snapshots from an explicit `ai model refresh`.
    #[serde(default)]
    models: BTreeMap<String, Vec<String>>,
}

fn env_value(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

fn flag(name: &str) -> bool {
    env_value(name).is_some_and(|v| matches!(v.to_ascii_lowercase().as_str(), "1" | "true" | "yes"))
}

fn config_path() -> Result<PathBuf> {
    if let Some(path) = env_value("PENTOOL_AI_CONFIG") {
        return Ok(PathBuf::from(path));
    }
    Ok(directories::ProjectDirs::from("org", "pentool", "pentool")
        .context("no user configuration directory is available")?
        .config_dir()
        .join("ai.json"))
}

/// True when `PENTOOL_AI` switches the ai commands off.
pub fn disabled() -> bool {
    env_value("PENTOOL_AI").is_some_and(|v| {
        matches!(
            v.to_ascii_lowercase().as_str(),
            "off" | "0" | "false" | "disabled"
        )
    })
}

fn ensure_enabled() -> Result<()> {
    if disabled() {
        return Err(err(
            "policy-denied",
            "AI features are disabled by PENTOOL_AI=off.",
            "Unset PENTOOL_AI (or set PENTOOL_AI=on) to enable the ai commands.",
        ));
    }
    Ok(())
}

fn load_file_config() -> Result<Config> {
    let path = config_path()?;
    match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|error| {
            err(
                "malformed-config",
                format!(
                    "{} is not valid AI configuration ({error}).",
                    path.display()
                ),
                "Repair the file or delete it and run `pentool ai setup --from-env`.",
            )
        }),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Config::default()),
        Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
    }
}

fn save_config(config: &Config) -> Result<()> {
    let path = config_path()?;
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    let mut config = config.clone();
    config.version = 1;
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    {
        use std::io::Write;
        let mut options = fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(&serde_json::to_vec_pretty(&config)?)?;
        file.sync_all()?;
    }
    fs::rename(&temporary, &path).inspect_err(|_| {
        let _ = fs::remove_file(&temporary);
    })?;
    Ok(())
}

fn ephemeral(flag_set: bool) -> bool {
    flag_set || flag("PENTOOL_AI_EPHEMERAL")
}

/// The configuration commands see: the saved file, or env-derived profiles only
/// when running ephemerally (nothing is read from or written to disk).
fn effective_config(ephemeral_mode: bool) -> Result<Config> {
    if ephemeral_mode {
        let (profiles, _) = profiles_from_env()?;
        let mut config = Config {
            version: 1,
            providers: profiles,
            ..Config::default()
        };
        assign_defaults(&mut config);
        Ok(config)
    } else {
        load_file_config()
    }
}

/// Scan the documented environment variables. Returns profiles and a report.
fn profiles_from_env() -> Result<(BTreeMap<String, Profile>, Vec<Value>)> {
    let mut profiles = BTreeMap::new();
    let mut seen = Vec::new();
    let generic_credential = if let Some(path) = env_value("PENTOOL_AI_KEY_FILE") {
        Some(Credential::File { path })
    } else if env_value("PENTOOL_AI_KEY").is_some() {
        Some(Credential::Env {
            name: "PENTOOL_AI_KEY".into(),
        })
    } else {
        None
    };
    let named = env_value("PENTOOL_AI_PROVIDER");
    let model_selector = env_value("PENTOOL_AI_MODEL");
    let model_for = |provider: &str| {
        model_selector.as_deref().and_then(|selector| {
            selector
                .split_once('/')
                .filter(|(name, _)| *name == provider)
                .map(|(_, model)| model.to_owned())
        })
    };
    for entry in &CATALOG {
        let from_generic =
            named.as_deref() == Some(entry.name) && env_value("PENTOOL_AI_ENDPOINT").is_none();
        let credential = if from_generic && generic_credential.is_some() {
            generic_credential.clone()
        } else {
            entry
                .env
                .iter()
                .find(|name| env_value(name).is_some())
                .map(|name| Credential::Env {
                    name: (*name).into(),
                })
        };
        if let Some(credential) = credential {
            seen.push(json!({"provider":entry.name,"found":credential_label(&credential)}));
            profiles.insert(
                entry.name.to_owned(),
                Profile {
                    adapter: entry.adapter.into(),
                    endpoint: entry.endpoint.into(),
                    credential,
                    default_model: Some(
                        model_for(entry.name).unwrap_or_else(|| entry.default_model.into()),
                    ),
                },
            );
        }
    }
    if let Some(endpoint) = env_value("PENTOOL_AI_ENDPOINT") {
        let name = named.clone().unwrap_or_else(|| "custom".into());
        let credential = generic_credential.clone().ok_or_else(|| {
            err(
                "missing-credential",
                "PENTOOL_AI_ENDPOINT is set but no credential reference was found.",
                "Set PENTOOL_AI_KEY (or PENTOOL_AI_KEY_FILE=/path/to/key) in the environment.",
            )
        })?;
        let adapter = env_value("PENTOOL_AI_ADAPTER").unwrap_or_else(|| "openai-compatible".into());
        seen.push(json!({"provider":name,"found":credential_label(&credential)}));
        profiles.insert(
            name.clone(),
            Profile {
                adapter,
                endpoint,
                credential,
                default_model: model_for(&name),
            },
        );
    }
    Ok((profiles, seen))
}

fn credential_label(credential: &Credential) -> String {
    match credential {
        Credential::Env { name } => format!("env:{name}"),
        Credential::File { path } => format!("file:{path}"),
    }
}

fn assign_defaults(config: &mut Config) {
    for capability in ["generate", "edit", "remove-background"] {
        if config.defaults.contains_key(capability) {
            continue;
        }
        if let Some((name, model)) = config.providers.iter().find_map(|(name, profile)| {
            capabilities(&profile.adapter)
                .contains(&capability)
                .then(|| profile.default_model.clone().map(|m| (name.clone(), m)))
                .flatten()
        }) {
            config
                .defaults
                .insert(capability.into(), format!("{name}/{model}"));
        }
    }
}

// ---------------------------------------------------------------------------
// Policy
// ---------------------------------------------------------------------------

fn is_loopback(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "::1" | "[::1]")
}

/// An endpoint with userinfo, query and fragment removed, safe to print.
fn redact_endpoint(endpoint: &str) -> String {
    match url::Url::parse(endpoint) {
        Ok(mut url) => {
            let _ = url.set_username("");
            let _ = url.set_password(None);
            url.set_query(None);
            url.set_fragment(None);
            url.to_string()
        }
        Err(_) => endpoint
            .split(['?', '#'])
            .next()
            .unwrap_or_default()
            .to_owned(),
    }
}

/// Query parameter names that carry credentials (case, `-` and `_` ignored).
fn is_secret_param(name: &str) -> bool {
    let norm: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase();
    [
        "key",
        "token",
        "secret",
        "password",
        "passwd",
        "auth",
        "signature",
        "sig",
        "credential",
        "bearer",
    ]
    .iter()
    .any(|word| norm.contains(word))
}

fn check_endpoint(endpoint: &str) -> Result<url::Url> {
    let shown = redact_endpoint(endpoint);
    let url = url::Url::parse(endpoint).map_err(|error| {
        err(
            "invalid-endpoint",
            format!("endpoint {shown:?} is not a URL ({error})."),
            "Use an absolute https:// endpoint such as https://api.example.com/v1.",
        )
    })?;
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    let secure = url.scheme() == "https" || (url.scheme() == "http" && is_loopback(&host));
    if !secure || host.is_empty() {
        return Err(err(
            "policy-denied",
            format!("endpoint {shown} must use https (plain http is allowed only for loopback)."),
            "Use an https:// endpoint, or a loopback address for a local server.",
        ));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(err(
            "policy-denied",
            "endpoint URLs must not embed credentials.",
            "Remove the user:password@ part and use --credential-env or --credential-file.",
        ));
    }
    if url.fragment().is_some() {
        return Err(err(
            "policy-denied",
            format!("endpoint {shown} must not contain a #fragment."),
            "Remove the fragment; credentials belong in --credential-env or --credential-file.",
        ));
    }
    if let Some((name, _)) = url.query_pairs().find(|(name, _)| is_secret_param(name)) {
        return Err(err(
            "policy-denied",
            format!(
                "endpoint {shown} carries a credential-like query parameter ({name}); its value was not stored or printed."
            ),
            "Remove the query secret and use --credential-env or --credential-file.",
        ));
    }
    if let Some(list) = env_value("PENTOOL_AI_ALLOW_HOSTS") {
        let allowed = list.split(',').map(str::trim).any(|pattern| {
            pattern == host
                || pattern
                    .strip_prefix("*.")
                    .is_some_and(|suffix| host.ends_with(&format!(".{suffix}")))
        });
        if !allowed {
            return Err(err(
                "policy-denied",
                format!("host {host} is not in PENTOOL_AI_ALLOW_HOSTS."),
                "Add the host to PENTOOL_AI_ALLOW_HOSTS or choose an allowed provider.",
            ));
        }
    }
    Ok(url)
}

/// Read a numeric safety limit. A present but malformed or out-of-range value
/// fails closed instead of silently falling back to the permissive default.
fn env_limit(name: &str, default: u64, min: u64, max: u64) -> Result<u64> {
    let Some(text) = env_value(name) else {
        return Ok(default);
    };
    match text.trim().parse::<u64>() {
        Ok(value) if (min..=max).contains(&value) => Ok(value),
        _ => Err(err(
            "invalid-option",
            format!("{name} must be an integer from {min} to {max}, got {text:?}."),
            format!("Set {name} to a whole number from {min} to {max}, or unset it for the default ({default})."),
        )),
    }
}

fn max_calls() -> Result<usize> {
    Ok(
        env_limit("PENTOOL_AI_MAX_CALLS", MAX_CANDIDATES as u64, 0, 1_000_000)?
            .min(MAX_CANDIDATES as u64) as usize,
    )
}

fn max_pixels() -> Result<u64> {
    env_limit("PENTOOL_AI_MAX_PIXELS", DEFAULT_MAX_PIXELS, 1, u64::MAX)
}

// ---------------------------------------------------------------------------
// Credentials and resolution
// ---------------------------------------------------------------------------

fn credential_present(credential: &Credential) -> bool {
    match credential {
        Credential::Env { name } => env_value(name).is_some(),
        Credential::File { path } => Path::new(path).is_file(),
    }
}

fn read_credential(credential: &Credential) -> Result<String> {
    match credential {
        Credential::Env { name } => env_value(name).ok_or_else(|| {
            err(
                "missing-credential",
                format!("environment variable {name} is not set or empty."),
                format!("Export {name} in the agent's environment, then re-run."),
            )
        }),
        Credential::File { path } => {
            let metadata = fs::metadata(path).map_err(|_| {
                err(
                    "missing-credential",
                    format!("credential file {path} cannot be read."),
                    "Create the file (key only, user-readable) or change --credential-file.",
                )
            })?;
            if metadata.len() > 4096 {
                return Err(err(
                    "limit-exceeded",
                    format!("credential file {path} is larger than 4 KiB."),
                    "Keep only the API key in the file (no extra text), or use --credential-env.",
                ));
            }
            #[cfg(not(unix))]
            check_private_location(path)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                if metadata.permissions().mode() & 0o077 != 0 {
                    return Err(err(
                        "unsafe-credential-file",
                        format!("credential file {path} is readable by other users."),
                        format!("Run `chmod 600 {path}`."),
                    ));
                }
            }
            let value = fs::read_to_string(path)?.trim().to_owned();
            if value.is_empty() {
                return Err(err(
                    "missing-credential",
                    format!("credential file {path} is empty."),
                    "Write the key into the file.",
                ));
            }
            Ok(value)
        }
    }
}

/// Without POSIX mode bits there is no cheap per-file permission check, so
/// require the file to live under the user's profile and outside the working
/// project, where it could be committed, synced or shared.
#[cfg(not(unix))]
fn check_private_location(path: &str) -> Result<()> {
    let real = fs::canonicalize(path).map_err(|_| {
        err(
            "missing-credential",
            format!("credential file {path} cannot be read."),
            "Create the file (key only) or change --credential-file.",
        )
    })?;
    let strip = |p: PathBuf| PathBuf::from(p.to_string_lossy().trim_start_matches(r"\?\"));
    let real = strip(real);
    if let Ok(cwd) = std::env::current_dir().and_then(fs::canonicalize) {
        if real.starts_with(strip(cwd)) {
            return Err(err(
                "unsafe-credential-file",
                format!("credential file {path} is inside the current project directory."),
                "Move the key file into your user profile (outside any project), or use --credential-env.",
            ));
        }
    }
    let home = env_value("USERPROFILE").or_else(|| env_value("HOME"));
    let inside_home = home
        .and_then(|h| fs::canonicalize(h).ok())
        .is_some_and(|h| real.starts_with(strip(h)));
    if !inside_home {
        return Err(err(
            "unsafe-credential-file",
            format!("credential file {path} is outside your user profile."),
            "Store the key file under your user profile directory (private to your account), or use --credential-env.",
        ));
    }
    Ok(())
}

fn credential_report(credential: &Credential) -> Value {
    match credential {
        Credential::Env { name } => {
            json!({"source":"env","name":name,"present":credential_present(credential)})
        }
        Credential::File { path } => {
            json!({"source":"file","path":path,"present":credential_present(credential)})
        }
    }
}

struct Resolved {
    provider: String,
    profile: Profile,
    model: String,
}

fn valid_model_id(model: &str) -> bool {
    !model.is_empty()
        && model.len() <= 96
        && model
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | ':'))
}

fn resolve(
    config: &Config,
    capability: &str,
    provider: Option<&str>,
    model: Option<&str>,
) -> Result<Resolved> {
    let mut provider = provider.map(str::to_owned);
    let mut model = model.map(str::to_owned);
    if provider.is_none() {
        if let Some(selector) = model.clone() {
            if let Some((name, rest)) = selector.split_once('/') {
                if config.providers.contains_key(name) {
                    provider = Some(name.into());
                    model = Some(rest.into());
                }
            }
        }
    }
    if provider.is_none() && model.is_none() {
        let selector = config
            .defaults
            .get(capability)
            .cloned()
            .or_else(|| env_value("PENTOOL_AI_MODEL"));
        if let Some((name, rest)) = selector.as_deref().and_then(|s| s.split_once('/')) {
            if config.providers.contains_key(name) {
                provider = Some(name.into());
                model = Some(rest.into());
            }
        }
    }
    let name = match provider {
        Some(name) => name,
        None => config
            .providers
            .iter()
            .find(|(_, p)| capabilities(&p.adapter).contains(&capability) && p.default_model.is_some())
            .map(|(name, _)| name.clone())
            .ok_or_else(|| {
                err(
                    "no-provider",
                    format!("no configured provider supports {capability}."),
                    "Run `pentool ai setup --from-env --json` (or `ai connect`) first, then `pentool ai resolve --capability ".to_owned() + capability + "`.",
                )
            })?,
    };
    let profile = config.providers.get(&name).cloned().ok_or_else(|| {
        err(
            "unknown-provider",
            format!("provider {name} is not configured."),
            "Run `pentool ai provider list` to see configured providers.",
        )
    })?;
    if !capabilities(&profile.adapter).contains(&capability) {
        let supporting: Vec<_> = config
            .providers
            .iter()
            .filter(|(_, p)| capabilities(&p.adapter).contains(&capability))
            .map(|(n, _)| n.as_str())
            .collect();
        return Err(err(
            "unsupported-capability",
            format!(
                "provider {name} ({}) does not support {capability}.",
                profile.adapter
            ),
            if supporting.is_empty() {
                let known: Vec<&str> = CATALOG
                    .iter()
                    .filter(|e| capabilities(e.adapter).contains(&capability))
                    .map(|e| e.name)
                    .collect();
                if known.is_empty() {
                    format!("No adapter in this build supports {capability}; use a different operation.")
                } else {
                    format!("Connect a provider that supports {capability}, e.g. `ai connect --name {} ...`.", known[0])
                }
            } else {
                format!("Use --provider with one of: {}.", supporting.join(", "))
            },
        ));
    }
    let model = model.or_else(|| profile.default_model.clone()).ok_or_else(|| {
        err(
            "no-model",
            format!("provider {name} has no default model."),
            format!("Pass --model, or run `pentool ai default --capability {capability} {name}/MODEL`."),
        )
    })?;
    if !valid_model_id(&model) {
        return Err(err(
            "invalid-model",
            format!("model id {model:?} contains unsupported characters."),
            "Model ids may use letters, digits, '.', '-', '_' and ':'.",
        ));
    }
    Ok(Resolved {
        provider: name,
        profile,
        model,
    })
}

// ---------------------------------------------------------------------------
// HTTP
// ---------------------------------------------------------------------------

struct Http {
    status: u16,
    body: Vec<u8>,
}

fn redact(text: &str, secrets: &[String]) -> String {
    let mut out = text.to_owned();
    for secret in secrets.iter().filter(|s| s.len() >= 8) {
        out = out.replace(secret.as_str(), "[redacted]");
    }
    out
}

fn timeout() -> Result<std::time::Duration> {
    Ok(std::time::Duration::from_secs(env_limit(
        "PENTOOL_AI_TIMEOUT_SECS",
        180,
        1,
        3600,
    )?))
}

/// One bounded request on a plain worker thread (reqwest's blocking client owns a
/// Tokio runtime that must not live on the CLI's async thread). No redirects are
/// followed, so a credential can never be forwarded to another host.
fn http(
    method: &'static str,
    url: String,
    headers: Vec<(&'static str, String)>,
    body: Option<Vec<u8>>,
    secrets: Vec<String>,
) -> Result<Http> {
    let limit = timeout()?;
    std::thread::spawn(move || -> Result<Http> {
        let client = reqwest::blocking::Client::builder()
            .timeout(limit)
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        let mut request = if method == "POST" {
            client.post(url)
        } else {
            client.get(url)
        };
        for (name, value) in headers {
            request = request.header(name, value);
        }
        if let Some(body) = body {
            request = request
                .header("content-type", "application/json")
                .body(body);
        }
        let response = request.send().map_err(|error| {
            let kind = if error.is_timeout() {
                "timeout"
            } else {
                "unreachable-endpoint"
            };
            err(
                kind,
                format!(
                    "the provider request failed: {}",
                    redact(&error.without_url().to_string(), &secrets)
                ),
                "Check the endpoint, network access, and PENTOOL_AI_TIMEOUT_SECS.",
            )
        })?;
        let status = response.status().as_u16();
        if response
            .content_length()
            .is_some_and(|n| n > MAX_RESPONSE_BYTES as u64)
        {
            bail!("[limit-exceeded] provider response exceeds {MAX_RESPONSE_BYTES} bytes")
        }
        let bytes = response.bytes().map_err(|error| {
            err(
                "unreachable-endpoint",
                format!(
                    "reading the provider response failed: {}",
                    redact(&error.without_url().to_string(), &secrets)
                ),
                "Retry; if it persists check the network.",
            )
        })?;
        if bytes.len() > MAX_RESPONSE_BYTES {
            bail!("[limit-exceeded] provider response exceeds {MAX_RESPONSE_BYTES} bytes")
        }
        Ok(Http {
            status,
            body: bytes.to_vec(),
        })
    })
    .join()
    .map_err(|_| anyhow::anyhow!("HTTP worker panicked"))?
}

fn provider_message(body: &[u8], secrets: &[String]) -> String {
    let value: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
    let message = value["error"]["message"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| String::from_utf8_lossy(&body[..body.len().min(200)]).into_owned());
    let message: String = redact(&message, secrets).chars().take(300).collect();
    message.replace(['\n', '\r'], " ")
}

fn status_error(response: &Http, secrets: &[String]) -> anyhow::Error {
    let message = provider_message(&response.body, secrets);
    match response.status {
        401 | 403 => err(
            "provider-auth-failed",
            format!("the provider rejected the credential (HTTP {}): {message}", response.status),
            "Check the key, its API restrictions, and billing; re-run `pentool ai doctor --check --json`.",
        ),
        429 => err(
            "rate-limited",
            format!("the provider refused the request for quota or billing (HTTP 429): {message}"),
            "Check the account's quota and billing, or wait and retry.",
        ),
        400..=499 => err(
            "provider-rejected-request",
            format!("the provider rejected the request (HTTP {}): {message}", response.status),
            "Adjust the prompt, model, or options named in the message.",
        ),
        _ => err(
            "provider-unavailable",
            format!("the provider failed (HTTP {}): {message}", response.status),
            "Retry later, or switch provider with `--provider`.",
        ),
    }
}

// ---------------------------------------------------------------------------
// Adapters
// ---------------------------------------------------------------------------

struct Source {
    media_type: String,
    bytes: Vec<u8>,
}

struct Output {
    bytes: Vec<u8>,
    usage: Value,
}

struct Request<'a> {
    prompt: &'a str,
    aspect: Option<&'a str>,
    size: Option<&'a str>,
    sources: &'a [Source],
}

fn endpoint_base(profile: &Profile) -> Result<String> {
    Ok(check_endpoint(&profile.endpoint)?
        .as_str()
        .trim_end_matches('/')
        .to_owned())
}

fn call_model(resolved: &Resolved, key: &str, request: &Request<'_>) -> Result<Output> {
    let secrets = vec![key.to_owned()];
    let base = endpoint_base(&resolved.profile)?;
    match resolved.profile.adapter.as_str() {
        "gemini" => {
            let mut parts = vec![json!({"text": request.prompt})];
            for source in request.sources {
                parts.push(json!({"inlineData":{
                    "mimeType": source.media_type,
                    "data": base64::engine::general_purpose::STANDARD.encode(&source.bytes)
                }}));
            }
            let mut generation = json!({"responseModalities":["IMAGE"]});
            if let Some(aspect) = request.aspect {
                generation["imageConfig"] = json!({"aspectRatio": aspect});
            }
            let body = json!({"contents":[{"parts":parts}],"generationConfig":generation});
            let response = http(
                "POST",
                format!(
                    "{base}/models/{}:generateContent?key={}",
                    resolved.model,
                    urlencoding_key(key)
                ),
                vec![],
                Some(serde_json::to_vec(&body)?),
                secrets.clone(),
            )?;
            if !(200..300).contains(&response.status) {
                return Err(status_error(&response, &secrets));
            }
            parse_gemini(&response.body, &secrets)
        }
        "openai-compatible" => {
            if !request.sources.is_empty() {
                return Err(err(
                    "unsupported-capability",
                    "the openai-compatible adapter does not accept source images in this release.",
                    "Use a provider that supports edit, e.g. --provider gemini.",
                ));
            }
            let mut body = json!({"model":resolved.model,"prompt":request.prompt,"n":1});
            if let Some(size) = request.size {
                body["size"] = json!(size);
            }
            let response = http(
                "POST",
                format!("{base}/images/generations"),
                vec![("authorization", format!("Bearer {key}"))],
                Some(serde_json::to_vec(&body)?),
                secrets.clone(),
            )?;
            if !(200..300).contains(&response.status) {
                return Err(status_error(&response, &secrets));
            }
            let value: Value = serde_json::from_slice(&response.body).map_err(|_| {
                err(
                    "malformed-response",
                    "the provider returned non-JSON.",
                    "Check the endpoint is an images API.",
                )
            })?;
            let item = &value["data"][0];
            let encoded = item["b64_json"].as_str().ok_or_else(|| {
                err(
                    "malformed-response",
                    "the provider returned no inline image (URL results are never fetched).",
                    "Use a model/endpoint that returns b64_json.",
                )
            })?;
            Ok(Output {
                bytes: decode_b64(encoded)?,
                usage: value.get("usage").cloned().unwrap_or(Value::Null),
            })
        }
        other => Err(err(
            "unsupported-adapter",
            format!("adapter {other} is not available in this build."),
            "Use adapter gemini or openai-compatible.",
        )),
    }
}

/// API keys are URL-safe in practice; encode defensively without a new dependency.
fn urlencoding_key(key: &str) -> String {
    key.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn decode_b64(encoded: &str) -> Result<Vec<u8>> {
    base64::engine::general_purpose::STANDARD
        .decode(encoded.trim())
        .map_err(|_| {
            err(
                "malformed-response",
                "the provider returned invalid base64 image data.",
                "Retry; if it persists the model may not return images.",
            )
        })
}

fn parse_gemini(body: &[u8], secrets: &[String]) -> Result<Output> {
    let value: Value = serde_json::from_slice(body).map_err(|_| {
        err(
            "malformed-response",
            "the provider returned non-JSON.",
            "Check the endpoint and model.",
        )
    })?;
    if let Some(reason) = value["promptFeedback"]["blockReason"].as_str() {
        return Err(err(
            "provider-refusal",
            format!("the provider blocked the prompt ({reason})."),
            "Rephrase the prompt; this is a safety decision, not a transport error.",
        ));
    }
    let candidate = &value["candidates"][0];
    let parts = candidate["content"]["parts"].as_array();
    if let Some(data) = parts
        .into_iter()
        .flatten()
        .find_map(|part| part["inlineData"]["data"].as_str())
    {
        return Ok(Output {
            bytes: decode_b64(data)?,
            usage: value.get("usageMetadata").cloned().unwrap_or(Value::Null),
        });
    }
    let finish = candidate["finishReason"].as_str().unwrap_or("none");
    let text: String = candidate["content"]["parts"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| p["text"].as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let detail: String = redact(&text, secrets).chars().take(200).collect();
    if matches!(
        finish,
        "SAFETY" | "IMAGE_SAFETY" | "PROHIBITED_CONTENT" | "BLOCKLIST" | "RECITATION" | "SPII"
    ) {
        return Err(err(
            "provider-refusal",
            format!("the provider declined to produce an image ({finish}). {detail}"),
            "Rephrase the prompt; this is a safety decision, not a transport error.",
        ));
    }
    Err(err(
        "provider-no-image",
        format!("the model returned no image (finishReason {finish}). {detail}"),
        "Use an image-capable model (see `pentool ai model list`) and a prompt that asks for an image.",
    ))
}

// ---------------------------------------------------------------------------
// Image validation and the local key-colour cutout
// ---------------------------------------------------------------------------

struct Validated {
    bytes: Vec<u8>,
    sha256: String,
    media_type: String,
    width: u32,
    height: u32,
}

fn validate_output(bytes: Vec<u8>) -> Result<Validated> {
    let (info, _) = crate::image::decode_source_pixels(&bytes).map_err(|error| {
        err(
            "invalid-output",
            format!("the provider returned an unusable image: {error:#}"),
            "Retry, or choose a different model.",
        )
    })?;
    if u64::from(info.pixel_width) * u64::from(info.pixel_height) > max_pixels()? {
        return Err(err(
            "limit-exceeded",
            format!(
                "the returned image is {}x{}, above PENTOOL_AI_MAX_PIXELS.",
                info.pixel_width, info.pixel_height
            ),
            "Request a smaller size or raise PENTOOL_AI_MAX_PIXELS.",
        ));
    }
    Ok(Validated {
        sha256: info.digest,
        media_type: info.media_type,
        width: info.pixel_width,
        height: info.pixel_height,
        bytes,
    })
}

fn parse_color(text: &str) -> Result<[u8; 3]> {
    let hex = text.trim().trim_start_matches('#');
    let valid = hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit());
    if !valid {
        bail!("key colour must be #RRGGBB, got {text:?}")
    }
    let n = u32::from_str_radix(hex, 16)?;
    Ok([(n >> 16) as u8, (n >> 8) as u8, n as u8])
}

/// Remove a flat key colour: pixels within `tolerance` (RGB distance) become fully
/// transparent, alpha ramps up over `softness` more, and the key's spill is removed
/// from semi-transparent edge pixels. Deterministic and offline.
pub fn key_out(
    image: &::image::RgbaImage,
    key: [u8; 3],
    tolerance: f32,
    softness: f32,
) -> ::image::RgbaImage {
    let mut out = image.clone();
    let soft = softness.max(1.0);
    for pixel in out.pixels_mut() {
        let [r, g, b, a] = pixel.0;
        let distance = (f32::from(r) - f32::from(key[0])).powi(2)
            + (f32::from(g) - f32::from(key[1])).powi(2)
            + (f32::from(b) - f32::from(key[2])).powi(2);
        let distance = distance.sqrt();
        let coverage = ((distance - tolerance) / soft).clamp(0.0, 1.0);
        let alpha = (f32::from(a) * coverage).round() as u8;
        let (mut r, mut g, mut b) = (r, g, b);
        if coverage < 1.0 {
            // Pull the key colour out of blended edge pixels.
            let strength = 1.0 - coverage;
            for (channel, key_channel) in [(&mut r, key[0]), (&mut g, key[1]), (&mut b, key[2])] {
                let value = f32::from(*channel);
                let target = f32::from(key_channel);
                *channel = if target > 127.0 {
                    value - (value - 0.0).min(strength * (value * 0.5))
                } else {
                    value
                } as u8;
            }
            if key[0] > 127 && key[2] > 127 && key[1] < 128 {
                // Magenta-style key: clamp red and blue to the green channel plus slack.
                let cap = u32::from(g) + 40;
                r = r.min(cap as u8);
                b = b.min(cap as u8);
            }
        }
        *pixel = ::image::Rgba([r, g, b, alpha]);
    }
    out
}

fn encode_png(image: &::image::RgbaImage) -> Result<Vec<u8>> {
    let mut out = std::io::Cursor::new(Vec::new());
    ::image::DynamicImage::ImageRgba8(image.clone())
        .write_to(&mut out, ::image::ImageFormat::Png)?;
    Ok(out.into_inner())
}

// ---------------------------------------------------------------------------
// Run store
// ---------------------------------------------------------------------------

/// Runs are scoped to the originating document: documents sharing a directory
/// never see, accept, discard or prune each other's candidates.
fn runs_dir(document: &Path) -> PathBuf {
    let name = document
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let digest = crate::resource::sha256(name.as_bytes());
    let scope: String = digest
        .trim_start_matches("sha256:")
        .chars()
        .take(12)
        .collect();
    crate::resource::document_root(document)
        .join(".pentool")
        .join("ai-runs")
        .join(scope)
}

fn valid_run_id(id: &str) -> bool {
    id.len() == 12 && id.chars().all(|c| c.is_ascii_hexdigit())
}

fn extension(media_type: &str) -> &'static str {
    match media_type {
        "image/png" => "png",
        "image/webp" => "webp",
        _ => "jpg",
    }
}

fn store_run(document: &Path, record: &mut Value, candidates: &[Validated]) -> Result<String> {
    let mut hasher = sha2::Sha256::default();
    use sha2::Digest;
    hasher.update(record["provider"].as_str().unwrap_or_default());
    hasher.update(record["model"].as_str().unwrap_or_default());
    hasher.update(record["kind"].as_str().unwrap_or_default());
    hasher.update(record["prompt_sha256"].as_str().unwrap_or_default());
    for candidate in candidates {
        hasher.update(&candidate.sha256);
    }
    let id: String = hex::encode(hasher.finalize()).chars().take(12).collect();
    let dir = runs_dir(document).join(&id);
    fs::create_dir_all(&dir)?;
    let mut entries = Vec::new();
    for (index, candidate) in candidates.iter().enumerate() {
        let file = format!(
            "candidate-{}.{}",
            index + 1,
            extension(&candidate.media_type)
        );
        fs::write(dir.join(&file), &candidate.bytes)?;
        entries.push(json!({
            "index": index + 1,
            "file": file,
            "sha256": candidate.sha256,
            "media_type": candidate.media_type,
            "width": candidate.width,
            "height": candidate.height,
            "bytes": candidate.bytes.len(),
        }));
    }
    record["id"] = json!(id);
    record["candidates"] = Value::Array(entries);
    fs::write(dir.join("run.json"), serde_json::to_vec_pretty(&record)?)?;
    prune_runs(document);
    Ok(id)
}

/// Rejected candidates live in bounded temporary storage: keep the newest runs only.
fn prune_runs(document: &Path) {
    let Ok(read) = fs::read_dir(runs_dir(document)) else {
        return;
    };
    let mut runs: Vec<(std::time::SystemTime, PathBuf)> = read
        .flatten()
        .filter_map(|entry| {
            let modified = entry
                .path()
                .join("run.json")
                .metadata()
                .ok()?
                .modified()
                .ok()?;
            Some((modified, entry.path()))
        })
        .collect();
    runs.sort();
    while runs.len() > MAX_RUNS_KEPT {
        let (_, path) = runs.remove(0);
        let _ = fs::remove_dir_all(path);
    }
}

fn load_run(document: &Path, id: &str) -> Result<(Value, PathBuf)> {
    if !valid_run_id(id) {
        bail!("[invalid-run] run id must be 12 hexadecimal characters")
    }
    let dir = runs_dir(document).join(id);
    let bytes = fs::read(dir.join("run.json")).map_err(|_| {
        err(
            "unknown-run",
            format!("run {id} was not found beside {}.", document.display()),
            "List runs with `pentool ai run list DOCUMENT`; old runs are pruned automatically.",
        )
    })?;
    Ok((serde_json::from_slice(&bytes)?, dir))
}

// ---------------------------------------------------------------------------
// Document helpers
// ---------------------------------------------------------------------------

fn read_document(path: &Path) -> Result<Value> {
    let bytes = fs::read(path).with_context(|| format!("[missing-resource] {}", path.display()))?;
    serde_json::from_slice(&bytes)
        .with_context(|| format!("{} is not a .pen document", path.display()))
}

fn find_node<'a>(raw: &'a Value, page: Option<&str>, id: &str) -> Option<(String, &'a Value)> {
    fn walk<'a>(nodes: &'a [Value], id: &str) -> Option<&'a Value> {
        for node in nodes {
            if node["id"] == id {
                return Some(node);
            }
            if let Some(found) = node["children"].as_array().and_then(|c| walk(c, id)) {
                return Some(found);
            }
        }
        None
    }
    for p in raw["pages"].as_array()? {
        if page.is_some_and(|wanted| p["id"] != wanted) {
            continue;
        }
        for layer in p["layers"].as_array()? {
            if let Some(found) = layer["nodes"].as_array().and_then(|n| walk(n, id)) {
                return Some((layer["id"].as_str()?.to_owned(), found));
            }
        }
    }
    None
}

fn ensure_layer_open(raw: &Value, page: Option<&str>, layer: &str) -> Result<()> {
    let locked = raw["pages"].as_array().is_some_and(|pages| {
        pages
            .iter()
            .filter(|p| page.is_none_or(|wanted| p["id"] == wanted))
            .flat_map(|p| p["layers"].as_array().into_iter().flatten())
            .any(|l| l["id"] == layer && l["locked"].as_bool().unwrap_or(false))
    });
    if locked {
        return Err(err(
            "locked-layer",
            format!("layer {layer} is locked; nothing was written."),
            "Unlock the layer with `pentool layer DOCUMENT set LAYER --locked false`, then retry.",
        ));
    }
    Ok(())
}

/// Reject a change to a node whose layer, own flag or any ancestor is locked.
fn ensure_target_unlocked(raw: &Value, page: Option<&str>, id: &str) -> Result<()> {
    fn walk(nodes: &[Value], id: &str, locked: bool) -> Option<bool> {
        for node in nodes {
            let here = locked || node["locked"].as_bool().unwrap_or(false);
            if node["id"] == id {
                return Some(here);
            }
            if let Some(found) = node["children"].as_array().and_then(|c| walk(c, id, here)) {
                return Some(found);
            }
        }
        None
    }
    let (layer, _) = find_node(raw, page, id).context("target vanished")?;
    ensure_layer_open(raw, page, &layer)?;
    let node_locked = raw["pages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|p| page.is_none_or(|wanted| p["id"] == wanted))
        .flat_map(|p| p["layers"].as_array().into_iter().flatten())
        .filter_map(|l| l["nodes"].as_array().and_then(|n| walk(n, id, false)))
        .next()
        .unwrap_or(false);
    if node_locked {
        return Err(err(
            "locked-node",
            format!("{id} (or a group containing it) is locked; nothing was written."),
            "Unlock the node, then retry.",
        ));
    }
    Ok(())
}

fn find_node_mut<'a>(raw: &'a mut Value, page: Option<&str>, id: &str) -> Option<&'a mut Value> {
    fn walk<'a>(nodes: &'a mut [Value], id: &str) -> Option<&'a mut Value> {
        for node in nodes {
            if node["id"] == id {
                return Some(node);
            }
            if let Some(found) = node
                .get_mut("children")
                .and_then(Value::as_array_mut)
                .and_then(|c| walk(c, id))
            {
                return Some(found);
            }
        }
        None
    }
    for p in raw["pages"].as_array_mut()? {
        if page.is_some_and(|wanted| p["id"] != wanted) {
            continue;
        }
        for layer in p["layers"].as_array_mut()? {
            if let Some(found) = layer
                .get_mut("nodes")
                .and_then(Value::as_array_mut)
                .and_then(|n| walk(n, id))
            {
                return Some(found);
            }
        }
    }
    None
}

fn canvas_size(raw: &Value, page: Option<&str>) -> Result<(f64, f64)> {
    let pages = raw["pages"].as_array().context("document has no pages")?;
    let p = match page {
        Some(wanted) => pages.iter().find(|p| p["id"] == wanted),
        None => pages.first(),
    }
    .context("page not found")?;
    Ok((
        p["canvas"]["width"]
            .as_f64()
            .context("canvas width missing")?,
        p["canvas"]["height"]
            .as_f64()
            .context("canvas height missing")?,
    ))
}

fn first_layer(raw: &Value, page: Option<&str>) -> Result<String> {
    let pages = raw["pages"].as_array().context("document has no pages")?;
    let p = match page {
        Some(wanted) => pages.iter().find(|p| p["id"] == wanted),
        None => pages.first(),
    }
    .context("page not found")?;
    p["layers"][0]["id"]
        .as_str()
        .map(str::to_owned)
        .context("page has no layers")
}

fn source_for(
    raw: &Value,
    document: &Path,
    page: Option<&str>,
    node: &str,
) -> Result<(Source, Value)> {
    let (_, found) = find_node(raw, page, node).ok_or_else(|| {
        err(
            "not-found",
            format!("source node {node} was not found."),
            "List nodes with `pentool tree DOCUMENT --kind image`.",
        )
    })?;
    if found["kind"] != "image" {
        return Err(err(
            "invalid-source",
            format!("source node {node} is not an image."),
            "Pass an image node ID.",
        ));
    }
    let digest = found["asset"].as_str().context("image asset missing")?;
    let bytes =
        crate::image::load_asset_bytes(raw, crate::resource::document_root(document), digest)?;
    if bytes.len() > MAX_SOURCE_BYTES {
        return Err(err(
            "limit-exceeded",
            "the source image is larger than 20 MiB.",
            "Downscale the source before sending it to a model.",
        ));
    }
    let media_type = raw["image_assets"][digest]["media_type"]
        .as_str()
        .unwrap_or("image/png")
        .to_owned();
    Ok((Source { media_type, bytes }, found.clone()))
}

fn read_prompt(prompt: Option<&str>, file: Option<&Path>, required: bool) -> Result<String> {
    let text = match (prompt, file) {
        (Some(_), Some(_)) => bail!("pass either --prompt or --prompt-file, not both"),
        (Some(text), None) => text.to_owned(),
        (None, Some(path)) => {
            if fs::metadata(path)?.len() > MAX_PROMPT_BYTES {
                bail!("[limit-exceeded] prompt files are limited to 64 KiB")
            }
            fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?
        }
        (None, None) if required => {
            return Err(err(
                "missing-prompt",
                "a prompt is required.",
                "Pass --prompt TEXT or --prompt-file FILE.",
            ))
        }
        (None, None) => String::new(),
    };
    if text.len() as u64 > MAX_PROMPT_BYTES {
        bail!("[limit-exceeded] prompts are limited to 64 KiB")
    }
    Ok(text)
}

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

#[derive(Subcommand, Debug)]
pub enum AiAction {
    /// Configure providers from environment variables (non-interactive, idempotent).
    Setup(SetupArgs),
    /// Add or update one provider profile from flags (non-interactive, idempotent).
    Connect(ConnectArgs),
    /// List or remove configured providers and the bundled catalog.
    Provider {
        #[command(subcommand)]
        action: ProviderAction,
    },
    /// List cached models or explicitly refresh them from a provider.
    Model {
        #[command(subcommand)]
        action: ModelAction,
    },
    /// Set the default `provider/model` for a capability.
    Default(DefaultArgs),
    /// Choose the provider/model that would serve a capability.
    Resolve(ResolveArgs),
    /// Verify setup; `--check` adds one non-billable provider call.
    Doctor(DoctorArgs),
    /// Generate candidate images from a prompt (requires --allow-model-call).
    Generate(GenerateArgs),
    /// Edit or clean up an existing image node with a model.
    Edit(EditArgs),
    /// Remove a flat key colour locally, with no model and no network.
    Keyout(KeyoutArgs),
    /// Review, accept, or discard generated candidates.
    Run {
        #[command(subcommand)]
        action: RunAction,
    },
}

#[derive(Args, Debug)]
pub struct SetupArgs {
    /// Derive profiles from the documented environment variables.
    #[arg(long)]
    from_env: bool,
    /// Resolve for this process only and write nothing.
    #[arg(long)]
    ephemeral: bool,
}

#[derive(Args, Debug)]
pub struct ConnectArgs {
    #[arg(long)]
    name: String,
    /// gemini or openai-compatible (defaults from the catalog entry of the same name).
    #[arg(long)]
    adapter: Option<String>,
    #[arg(long)]
    endpoint: Option<String>,
    /// Name of the environment variable holding the key.
    #[arg(long, conflicts_with = "credential_file")]
    credential_env: Option<String>,
    /// Path of a user-only file holding the key.
    #[arg(long)]
    credential_file: Option<PathBuf>,
    #[arg(long)]
    default_model: Option<String>,
    /// Rejected: keys are never accepted on the command line.
    #[arg(long, hide = true)]
    api_key: Option<String>,
}

#[derive(Subcommand, Debug)]
pub enum ProviderAction {
    List,
    Remove { name: String },
}

#[derive(Subcommand, Debug)]
pub enum ModelAction {
    List {
        #[arg(long)]
        provider: Option<String>,
        #[arg(long)]
        capability: Option<String>,
    },
    /// Contact the provider's model listing (non-billable) and cache image models.
    Refresh {
        #[arg(long)]
        provider: String,
    },
}

#[derive(Args, Debug)]
pub struct DefaultArgs {
    #[arg(long)]
    capability: String,
    /// `provider/model`.
    selector: String,
}

#[derive(Args, Debug)]
pub struct ResolveArgs {
    #[arg(long)]
    capability: String,
    #[arg(long)]
    provider: Option<String>,
    #[arg(long)]
    model: Option<String>,
    #[arg(long)]
    ephemeral: bool,
}

#[derive(Args, Debug)]
pub struct DoctorArgs {
    #[arg(long)]
    check: bool,
    #[arg(long)]
    provider: Option<String>,
    #[arg(long)]
    ephemeral: bool,
}

#[derive(Args, Debug)]
pub struct GenerateArgs {
    document: PathBuf,
    /// Label for the run; the default ID of an accepted node.
    name: String,
    #[arg(long)]
    provider: Option<String>,
    #[arg(long)]
    model: Option<String>,
    #[arg(long)]
    prompt: Option<String>,
    #[arg(long)]
    prompt_file: Option<PathBuf>,
    /// Aspect ratio such as 16:9 (gemini).
    #[arg(long)]
    aspect: Option<String>,
    /// Pixel size such as 1024x1024 (openai-compatible).
    #[arg(long)]
    size: Option<String>,
    #[arg(long, default_value_t = 1)]
    candidates: usize,
    /// Acknowledge that this command contacts a paid model.
    #[arg(long)]
    allow_model_call: bool,
    /// Print exactly what would be sent; never contacts a provider.
    #[arg(long)]
    dry_run: bool,
    #[arg(long)]
    ephemeral: bool,
}

#[derive(Args, Debug)]
pub struct EditArgs {
    document: PathBuf,
    name: String,
    /// Image node whose asset is sent to the model.
    #[arg(long)]
    source: String,
    /// edit or remove-background.
    #[arg(long, default_value = "edit")]
    kind: String,
    #[arg(long)]
    provider: Option<String>,
    #[arg(long)]
    model: Option<String>,
    #[arg(long)]
    prompt: Option<String>,
    #[arg(long)]
    prompt_file: Option<PathBuf>,
    /// Key colour used for remove-background.
    #[arg(long, default_value = "#FF00FF")]
    key: String,
    #[arg(long, default_value_t = 1)]
    candidates: usize,
    #[arg(long)]
    allow_model_call: bool,
    #[arg(long)]
    dry_run: bool,
    #[arg(long)]
    ephemeral: bool,
}

#[derive(Args, Debug)]
pub struct KeyoutArgs {
    document: PathBuf,
    /// ID of the new transparent image node.
    id: String,
    #[arg(long)]
    source: String,
    #[arg(long, default_value = "#FF00FF")]
    key: String,
    /// RGB distance fully removed around the key colour (0-442).
    #[arg(long, default_value_t = 60.0)]
    tolerance: f32,
    /// Extra distance over which alpha ramps in.
    #[arg(long, default_value_t = 90.0)]
    softness: f32,
    #[arg(long)]
    dry_run: bool,
    #[arg(long = "if-revision")]
    if_revision: Option<String>,
}

#[derive(Subcommand, Debug)]
pub enum RunAction {
    List {
        document: PathBuf,
    },
    Show {
        document: PathBuf,
        run: String,
    },
    /// Commit one candidate into the document (one transaction, one undo entry).
    Accept {
        document: PathBuf,
        run: String,
        #[arg(long)]
        candidate: usize,
        /// ID of the new image node (default: the run name).
        #[arg(long)]
        id: Option<String>,
        #[arg(long)]
        layer: Option<String>,
        #[arg(long)]
        x: Option<f64>,
        #[arg(long)]
        y: Option<f64>,
        #[arg(long)]
        width: Option<f64>,
        #[arg(long)]
        height: Option<f64>,
        /// Swap the pixels of an existing image node, keeping its placement and effects.
        #[arg(long, conflicts_with_all = ["id", "layer", "x", "y", "width", "height"])]
        replace: Option<String>,
        /// Replace even though the target image changed since the run was made.
        #[arg(long, requires = "replace")]
        allow_stale_source: bool,
        #[arg(long)]
        dry_run: bool,
        #[arg(long = "if-revision")]
        if_revision: Option<String>,
    },
    Discard {
        document: PathBuf,
        run: String,
    },
}

pub fn run(action: AiAction, page: Option<&str>) -> Result<Value> {
    ensure_enabled()?;
    match action {
        AiAction::Setup(args) => setup(&args),
        AiAction::Connect(args) => connect(&args),
        AiAction::Provider { action } => provider(&action),
        AiAction::Model { action } => model(&action),
        AiAction::Default(args) => set_default(&args),
        AiAction::Resolve(args) => {
            let config = effective_config(ephemeral(args.ephemeral))?;
            let resolved = resolve(
                &config,
                &args.capability,
                args.provider.as_deref(),
                args.model.as_deref(),
            )?;
            Ok(json!({
                "capability": args.capability,
                "provider": resolved.provider,
                "model": resolved.model,
                "selector": format!("{}/{}", resolved.provider, resolved.model),
                "adapter": resolved.profile.adapter,
                "credential": credential_report(&resolved.profile.credential),
            }))
        }
        AiAction::Doctor(args) => doctor(&args),
        AiAction::Generate(args) => generate(&args),
        AiAction::Edit(args) => edit(&args, page),
        AiAction::Keyout(args) => keyout(&args, page),
        AiAction::Run { action } => run_action(&action, page),
    }
}

fn profile_summary(name: &str, profile: &Profile) -> Value {
    json!({
        "name": name,
        "adapter": profile.adapter,
        "endpoint": profile.endpoint,
        "credential": credential_report(&profile.credential),
        "default_model": profile.default_model,
        "capabilities": capabilities(&profile.adapter),
    })
}

fn setup(args: &SetupArgs) -> Result<Value> {
    if !args.from_env {
        return Err(err(
            "missing-option",
            "`ai setup` needs a source.",
            "Run `pentool ai setup --from-env --json` (non-interactive), or `pentool ai connect ...`.",
        ));
    }
    let (found, seen) = profiles_from_env()?;
    if found.is_empty() {
        let variables: Vec<&str> = CATALOG.iter().flat_map(|e| e.env.iter().copied()).collect();
        return Err(err(
            "no-provider-found",
            "no provider credentials were found in the environment.",
            format!(
                "Export one of {} (or PENTOOL_AI_ENDPOINT with PENTOOL_AI_KEY) and re-run.",
                variables.join(", ")
            ),
        ));
    }
    for profile in found.values() {
        check_endpoint(&profile.endpoint)?;
    }
    let ephemeral_mode = ephemeral(args.ephemeral);
    let mut config = if ephemeral_mode {
        Config::default()
    } else {
        load_file_config()?
    };
    let before = config.clone();
    let mut statuses = Vec::new();
    for (name, profile) in &found {
        let status = match before.providers.get(name) {
            None => "added",
            Some(existing) if existing == profile => "unchanged",
            Some(_) => "updated",
        };
        config.providers.insert(name.clone(), profile.clone());
        statuses.push(json!({"status":status,"profile":profile_summary(name, profile)}));
    }
    assign_defaults(&mut config);
    let changed = config != before;
    if changed && !ephemeral_mode {
        save_config(&config)?;
    }
    Ok(json!({
        "changed": changed && !ephemeral_mode,
        "ephemeral": ephemeral_mode,
        "config": if ephemeral_mode { Value::Null } else { json!(config_path()?) },
        "found": seen,
        "providers": statuses,
        "defaults": config.defaults,
        "next": "pentool ai doctor --json",
    }))
}

fn connect(args: &ConnectArgs) -> Result<Value> {
    if args.api_key.is_some() {
        return Err(err(
            "raw-secret-rejected",
            "keys are never accepted on the command line (they leak into process listings and shell history).",
            "Put the key in an environment variable and pass --credential-env NAME, or in a user-only file and pass --credential-file PATH.",
        ));
    }
    let entry = catalog(&args.name);
    let adapter = args
        .adapter
        .clone()
        .or_else(|| entry.map(|e| e.adapter.to_owned()))
        .ok_or_else(|| {
            err(
                "missing-option",
                "an adapter is required for a provider outside the catalog.",
                "Pass --adapter gemini or --adapter openai-compatible.",
            )
        })?;
    if capabilities(&adapter).is_empty() {
        return Err(err(
            "unsupported-adapter",
            format!("adapter {adapter} is not available in this build."),
            "Use --adapter gemini or --adapter openai-compatible.",
        ));
    }
    let endpoint = args
        .endpoint
        .clone()
        .or_else(|| entry.map(|e| e.endpoint.to_owned()))
        .ok_or_else(|| {
            err(
                "missing-option",
                "an endpoint is required for a provider outside the catalog.",
                "Pass --endpoint https://host/v1.",
            )
        })?;
    check_endpoint(&endpoint)?;
    let credential = match (&args.credential_env, &args.credential_file) {
        (Some(name), None) => Credential::Env { name: name.clone() },
        (None, Some(path)) => Credential::File {
            path: path.to_string_lossy().into_owned(),
        },
        _ => {
            return Err(err(
                "missing-credential",
                "a credential reference is required.",
                "Pass --credential-env NAME or --credential-file PATH.",
            ))
        }
    };
    let default_model = match &args.default_model {
        None => None,
        Some(model) => {
            let bare = match model.split_once('/') {
                Some((prefix, rest)) if prefix == args.name => rest,
                Some((prefix, _)) => {
                    return Err(err(
                        "invalid-model",
                        format!(
                        "model {model:?} names provider {prefix:?} but this connection is {:?}.",
                        args.name
                    ),
                        format!(
                            "Use --default-model {}/<model> or a bare model id.",
                            args.name
                        ),
                    ))
                }
                None => model.as_str(),
            };
            if !valid_model_id(bare) {
                return Err(err(
                    "invalid-model",
                    format!("model id {bare:?} contains unsupported characters."),
                    "Model ids may use letters, digits, '.', '-', '_' and ':' (optionally prefixed provider/).",
                ));
            }
            Some(bare.to_owned())
        }
    };
    let profile = Profile {
        adapter,
        endpoint,
        credential,
        default_model: default_model.or_else(|| entry.map(|e| e.default_model.to_owned())),
    };
    let mut config = load_file_config()?;
    let before = config.clone();
    let status = match config.providers.get(&args.name) {
        None => "added",
        Some(existing) if *existing == profile => "unchanged",
        Some(_) => "updated",
    };
    config.providers.insert(args.name.clone(), profile.clone());
    assign_defaults(&mut config);
    let changed = config != before;
    if changed {
        save_config(&config)?;
    }
    Ok(json!({
        "changed": changed,
        "status": status,
        "config": config_path()?,
        "profile": profile_summary(&args.name, &profile),
        "defaults": config.defaults,
        "next": "pentool ai doctor --json",
    }))
}

fn provider(action: &ProviderAction) -> Result<Value> {
    match action {
        ProviderAction::List => {
            let config = load_file_config()?;
            let configured: Vec<Value> = config
                .providers
                .iter()
                .map(|(name, profile)| profile_summary(name, profile))
                .collect();
            let known: Vec<Value> = CATALOG
                .iter()
                .map(|e| {
                    json!({"name":e.name,"adapter":e.adapter,"endpoint":e.endpoint,"env":e.env,"default_model":e.default_model,"capabilities":capabilities(e.adapter)})
                })
                .collect();
            Ok(json!({"configured":configured,"catalog":known,"defaults":config.defaults}))
        }
        ProviderAction::Remove { name } => {
            let mut config = load_file_config()?;
            if config.providers.remove(name).is_none() {
                return Err(err(
                    "unknown-provider",
                    format!("provider {name} is not configured."),
                    "Run `pentool ai provider list`.",
                ));
            }
            config.models.remove(name);
            config
                .defaults
                .retain(|_, selector| !selector.starts_with(&format!("{name}/")));
            save_config(&config)?;
            Ok(json!({"removed":name,"defaults":config.defaults}))
        }
    }
}

fn model(action: &ModelAction) -> Result<Value> {
    match action {
        ModelAction::List {
            provider,
            capability,
        } => {
            let config = load_file_config()?;
            let mut models = Vec::new();
            for (name, profile) in &config.providers {
                if provider.as_deref().is_some_and(|wanted| wanted != name) {
                    continue;
                }
                if capability
                    .as_deref()
                    .is_some_and(|c| !capabilities(&profile.adapter).contains(&c))
                {
                    continue;
                }
                let mut ids: Vec<String> = config.models.get(name).cloned().unwrap_or_default();
                if let Some(default) = &profile.default_model {
                    if !ids.contains(default) {
                        ids.insert(0, default.clone());
                    }
                }
                for id in ids {
                    models.push(json!({
                        "selector": format!("{name}/{id}"),
                        "provider": name,
                        "model": id,
                        "capabilities": capabilities(&profile.adapter),
                        "default": profile.default_model.as_deref() == Some(id.as_str()),
                    }));
                }
            }
            Ok(
                json!({"models":models,"source":"local snapshot; run `ai model refresh --provider NAME` to update"}),
            )
        }
        ModelAction::Refresh { provider } => {
            let mut config = load_file_config()?;
            let profile = config.providers.get(provider).cloned().ok_or_else(|| {
                err(
                    "unknown-provider",
                    format!("provider {provider} is not configured."),
                    "Run `pentool ai provider list`.",
                )
            })?;
            let ids = list_models(&profile)?;
            config.models.insert(provider.clone(), ids.clone());
            save_config(&config)?;
            Ok(json!({"provider":provider,"models":ids,"refreshed":true}))
        }
    }
}

/// Non-billable model listing; image-capable models only.
fn list_models(profile: &Profile) -> Result<Vec<String>> {
    let key = read_credential(&profile.credential)?;
    let secrets = vec![key.clone()];
    let base = endpoint_base(profile)?;
    let response = match profile.adapter.as_str() {
        "gemini" => http(
            "GET",
            format!("{base}/models?pageSize=200&key={}", urlencoding_key(&key)),
            vec![],
            None,
            secrets.clone(),
        )?,
        _ => http(
            "GET",
            format!("{base}/models"),
            vec![("authorization", format!("Bearer {key}"))],
            None,
            secrets.clone(),
        )?,
    };
    if !(200..300).contains(&response.status) {
        return Err(status_error(&response, &secrets));
    }
    let value: Value = serde_json::from_slice(&response.body)?;
    let mut ids: Vec<String> = if profile.adapter == "gemini" {
        value["models"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|m| {
                m["supportedGenerationMethods"]
                    .as_array()
                    .is_some_and(|methods| methods.iter().any(|x| x == "generateContent"))
            })
            .filter_map(|m| m["name"].as_str())
            .map(|name| name.trim_start_matches("models/").to_owned())
            .filter(|name| name.contains("image") && valid_model_id(name))
            .collect()
    } else {
        value["data"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|m| m["id"].as_str())
            .filter(|id| id.contains("image") && valid_model_id(id))
            .map(str::to_owned)
            .collect()
    };
    ids.sort();
    ids.dedup();
    Ok(ids)
}

fn set_default(args: &DefaultArgs) -> Result<Value> {
    let mut config = load_file_config()?;
    let (provider, model) = args.selector.split_once('/').ok_or_else(|| {
        err(
            "invalid-selector",
            format!("{:?} is not provider/model.", args.selector),
            "Use e.g. `gemini/gemini-2.5-flash-image`.",
        )
    })?;
    resolve(&config, &args.capability, Some(provider), Some(model))?;
    config
        .defaults
        .insert(args.capability.clone(), args.selector.clone());
    save_config(&config)?;
    Ok(json!({"capability":args.capability,"selector":args.selector,"defaults":config.defaults}))
}

fn doctor(args: &DoctorArgs) -> Result<Value> {
    let ephemeral_mode = ephemeral(args.ephemeral);
    let config = effective_config(ephemeral_mode)?;
    let mut providers = Vec::new();
    let mut all_ready = !config.providers.is_empty();
    let mut limit_problems = Vec::new();
    for check in [
        max_calls().map(|_| ()),
        max_pixels().map(|_| ()),
        timeout().map(|_| ()),
    ] {
        if let Err(error) = check {
            if let Some(ai) = error.downcast_ref::<AiError>() {
                limit_problems.push(json!({"code":ai.code,"message":ai.message,"fix":ai.fix}));
            }
        }
    }
    all_ready &= limit_problems.is_empty();
    for (name, profile) in &config.providers {
        if args
            .provider
            .as_deref()
            .is_some_and(|wanted| wanted != name)
        {
            continue;
        }
        let mut summary = profile_summary(name, profile);
        let credential_ok = credential_present(&profile.credential);
        let endpoint = check_endpoint(&profile.endpoint);
        let mut problems = Vec::new();
        if !credential_ok {
            problems.push(json!({"code":"missing-credential","fix":format!("Provide the credential referenced by {}.", credential_label(&profile.credential))}));
        }
        if let Err(error) = &endpoint {
            if let Some(ai) = error.downcast_ref::<AiError>() {
                problems.push(json!({"code":ai.code,"fix":ai.fix}));
            }
        }
        if args.check && problems.is_empty() {
            match list_models(profile) {
                Ok(ids) => summary["check"] = json!({"ok":true,"image_models":ids.len()}),
                Err(error) => {
                    let (code, fix) = error
                        .downcast_ref::<AiError>()
                        .map(|e| (e.code, e.fix.clone()))
                        .unwrap_or(("check-failed", "Inspect the message.".into()));
                    summary["check"] =
                        json!({"ok":false,"code":code,"message":format!("{error:#}"),"fix":fix});
                    problems.push(json!({"code":code,"fix":fix}));
                }
            }
        }
        let ready = problems.is_empty();
        all_ready &= ready;
        summary["ready"] = json!(ready);
        summary["problems"] = Value::Array(problems);
        providers.push(summary);
    }
    if config.providers.is_empty() {
        return Ok(json!({
            "ok": false,
            "enabled": true,
            "ephemeral": ephemeral_mode,
            "providers": [],
            "problems": [{"code":"no-provider","fix":"Run `pentool ai setup --from-env --json` or `pentool ai connect ...`."}],
        }));
    }
    Ok(json!({
        "ok": all_ready,
        "enabled": true,
        "ephemeral": ephemeral_mode,
        "config": if ephemeral_mode { Value::Null } else { json!(config_path()?) },
        "checked_network": args.check,
        "providers": providers,
        "defaults": config.defaults,
        "limits": {
            "max_calls": max_calls().ok(),
            "max_pixels": max_pixels().ok(),
            "allow_hosts": env_value("PENTOOL_AI_ALLOW_HOSTS"),
            "problems": limit_problems,
        },
        "note": "Model calls additionally require --allow-model-call.",
    }))
}

// ---------------------------------------------------------------------------
// Generation, editing, review
// ---------------------------------------------------------------------------

fn prompt_hash(prompt: &str) -> String {
    crate::resource::sha256(prompt.as_bytes())
}

fn host_of(profile: &Profile) -> String {
    url::Url::parse(&profile.endpoint)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .unwrap_or_default()
}

fn validate_candidates(count: usize) -> Result<()> {
    if count == 0 || count > MAX_CANDIDATES {
        return Err(err(
            "invalid-option",
            format!("--candidates must be 1-{MAX_CANDIDATES}."),
            "Pass a smaller --candidates value.",
        ));
    }
    let allowed = max_calls()?;
    if count > allowed {
        return Err(err(
            "policy-denied",
            format!("{count} calls exceed PENTOOL_AI_MAX_CALLS={allowed}."),
            "Lower --candidates or raise PENTOOL_AI_MAX_CALLS.",
        ));
    }
    Ok(())
}

struct Plan<'a> {
    kind: &'a str,
    name: &'a str,
    resolved: &'a Resolved,
    prompt: String,
    aspect: Option<String>,
    size: Option<String>,
    candidates: usize,
    sources: Vec<(String, Source)>,
    key: Option<[u8; 3]>,
}

fn disclosure(plan: &Plan<'_>) -> Value {
    json!({
        "provider": plan.resolved.provider,
        "model": plan.resolved.model,
        "adapter": plan.resolved.profile.adapter,
        "endpoint_host": host_of(&plan.resolved.profile),
        "capability": plan.kind,
        "prompt_chars": plan.prompt.chars().count(),
        "prompt_sha256": prompt_hash(&plan.prompt),
        "aspect": plan.aspect,
        "size": plan.size,
        "calls": plan.candidates,
        "sources": plan.sources.iter().map(|(node, s)| json!({
            "node": node,
            "sha256": crate::resource::sha256(&s.bytes),
            "media_type": s.media_type,
            "bytes": s.bytes.len(),
        })).collect::<Vec<_>>(),
        "credential": credential_report(&plan.resolved.profile.credential),
        "contacts_network": true,
    })
}

fn execute(document: &Path, plan: &Plan<'_>, allow: bool, dry_run: bool) -> Result<Value> {
    // Fail closed on malformed limits before any disclosure or network work.
    max_pixels()?;
    timeout()?;
    let disclosed = disclosure(plan);
    if dry_run {
        check_endpoint(&plan.resolved.profile.endpoint)?;
        return Ok(json!({"dry_run":true,"disclosure":disclosed,"contacted_provider":false}));
    }
    if !allow {
        return Err(err(
            "model-call-not-allowed",
            "this command contacts a paid model.",
            "Review the request with --dry-run, then re-run with --allow-model-call.",
        ));
    }
    check_endpoint(&plan.resolved.profile.endpoint)?;
    let key = read_credential(&plan.resolved.profile.credential)?;
    let sources: Vec<Source> = plan
        .sources
        .iter()
        .map(|(_, s)| Source {
            media_type: s.media_type.clone(),
            bytes: s.bytes.clone(),
        })
        .collect();
    let mut accepted: Vec<Validated> = Vec::new();
    let mut usage = Vec::new();
    let mut coverage: Vec<f64> = Vec::new();
    for _ in 0..plan.candidates {
        let request = Request {
            prompt: &plan.prompt,
            aspect: plan.aspect.as_deref(),
            size: plan.size.as_deref(),
            sources: &sources,
        };
        let output = call_model(plan.resolved, &key, &request)?;
        let mut validated = validate_output(output.bytes)?;
        if let Some(key_color) = plan.key {
            let (_, pixels) = crate::image::decode_source_pixels(&validated.bytes)?;
            let keyed = key_out(&pixels, key_color, 60.0, 90.0);
            let total = keyed.pixels().len().max(1) as f64;
            let clear = keyed.pixels().filter(|p| p[3] < 255).count() as f64 / total;
            if clear < 0.001 {
                return Err(err(
                    "keyout-failed",
                    format!(
                        "background removal left the image opaque (transparent coverage {:.2}%); the model did not paint the requested key background.",
                        clear * 100.0
                    ),
                    "Re-run (the call was billed), try another model, or remove the background manually.",
                ));
            }
            coverage.push((clear * 10000.0).round() / 10000.0);
            validated = validate_output(encode_png(&keyed)?)?;
        }
        usage.push(output.usage);
        if accepted.iter().any(|c| c.sha256 == validated.sha256) {
            continue;
        }
        accepted.push(validated);
    }
    let duplicates = plan.candidates - accepted.len();
    let warnings: Vec<String> = if duplicates > 0 {
        vec![format!(
            "{duplicates} of {} requested candidates were identical duplicates and were dropped; usage for all {} calls is recorded.",
            plan.candidates, plan.candidates
        )]
    } else {
        Vec::new()
    };
    let mut record = json!({
        "name": plan.name,
        "kind": plan.kind,
        "provider": plan.resolved.provider,
        "model": plan.resolved.model,
        "prompt": plan.prompt,
        "prompt_sha256": prompt_hash(&plan.prompt),
        "aspect": plan.aspect,
        "size": plan.size,
        "sources": disclosed["sources"],
        "usage": usage,
        "requested": plan.candidates,
        "unique": accepted.len(),
        "warnings": warnings,
        "keyout_transparent_coverage": coverage,
    });
    let id = store_run(document, &mut record, &accepted)?;
    Ok(json!({
        "run": id,
        "name": plan.name,
        "kind": plan.kind,
        "provider": plan.resolved.provider,
        "model": plan.resolved.model,
        "candidates": record["candidates"],
        "requested": plan.candidates,
        "unique": accepted.len(),
        "warnings": record["warnings"],
        "keyout_transparent_coverage": record["keyout_transparent_coverage"],
        "usage": record["usage"],
        "store": runs_dir(document).join(&id),
        "document_changed": false,
        "next": format!("pentool ai run accept {} {id} --candidate 1", document.display()),
    }))
}

fn aspect_for(resolved: &Resolved, aspect: Option<&str>) -> Result<Option<String>> {
    let Some(aspect) = aspect else {
        return Ok(None);
    };
    if resolved.profile.adapter == "gemini" && !GEMINI_ASPECTS.contains(&aspect) {
        return Err(err(
            "unsupported-option",
            format!("aspect {aspect} is not supported by gemini."),
            format!("Use one of {}.", GEMINI_ASPECTS.join(", ")),
        ));
    }
    Ok(Some(aspect.to_owned()))
}

fn generate(args: &GenerateArgs) -> Result<Value> {
    validate_candidates(args.candidates)?;
    let config = effective_config(ephemeral(args.ephemeral))?;
    let resolved = resolve(
        &config,
        "generate",
        args.provider.as_deref(),
        args.model.as_deref(),
    )?;
    read_document(&args.document)?;
    let prompt = read_prompt(args.prompt.as_deref(), args.prompt_file.as_deref(), true)?;
    let aspect = aspect_for(&resolved, args.aspect.as_deref())?;
    let size = match (&args.size, resolved.profile.adapter.as_str()) {
        (Some(size), "openai-compatible") => {
            let (w, h) = size
                .split_once('x')
                .and_then(|(w, h)| Some((w.parse::<u64>().ok()?, h.parse::<u64>().ok()?)))
                .ok_or_else(|| {
                    err(
                        "invalid-option",
                        "--size must look like 1024x1024.",
                        "Pass --size WIDTHxHEIGHT.",
                    )
                })?;
            if w * h > max_pixels()? {
                return Err(err(
                    "limit-exceeded",
                    "--size exceeds PENTOOL_AI_MAX_PIXELS.",
                    "Choose a smaller size.",
                ));
            }
            Some(size.clone())
        }
        (Some(_), _) => {
            return Err(err(
                "unsupported-option",
                "--size applies to openai-compatible providers.",
                "Use --aspect for gemini.",
            ))
        }
        (None, "openai-compatible") => Some("1024x1024".into()),
        (None, _) => None,
    };
    let plan = Plan {
        kind: "generate",
        name: &args.name,
        resolved: &resolved,
        prompt,
        aspect,
        size,
        candidates: args.candidates,
        sources: Vec::new(),
        key: None,
    };
    execute(&args.document, &plan, args.allow_model_call, args.dry_run)
}

fn edit(args: &EditArgs, page: Option<&str>) -> Result<Value> {
    let capability = match args.kind.as_str() {
        "edit" => "edit",
        "remove-background" => "remove-background",
        other => {
            return Err(err(
                "invalid-option",
                format!("--kind {other} is not supported."),
                "Use --kind edit or --kind remove-background.",
            ))
        }
    };
    validate_candidates(args.candidates)?;
    let config = effective_config(ephemeral(args.ephemeral))?;
    let resolved = resolve(
        &config,
        capability,
        args.provider.as_deref(),
        args.model.as_deref(),
    )?;
    let raw = read_document(&args.document)?;
    let (source, _) = source_for(&raw, &args.document, page, &args.source)?;
    let (prompt, key) = if capability == "remove-background" {
        let key = parse_color(&args.key)?;
        let extra = read_prompt(args.prompt.as_deref(), args.prompt_file.as_deref(), false)?;
        let mut prompt = format!(
            "Keep the main subject exactly as it is, unchanged in pose, size, position, lighting and detail. Replace the entire background with one perfectly flat solid {} background: no shadow, no gradient, no floor, no reflection, no texture, and none of that colour on the subject.",
            args.key.to_uppercase()
        );
        if !extra.trim().is_empty() {
            prompt.push(' ');
            prompt.push_str(extra.trim());
        }
        (prompt, Some(key))
    } else {
        (
            read_prompt(args.prompt.as_deref(), args.prompt_file.as_deref(), true)?,
            None,
        )
    };
    let plan = Plan {
        kind: capability,
        name: &args.name,
        resolved: &resolved,
        prompt,
        aspect: None,
        size: None,
        candidates: args.candidates,
        sources: vec![(args.source.clone(), source)],
        key,
    };
    execute(&args.document, &plan, args.allow_model_call, args.dry_run)
}

fn keyout(args: &KeyoutArgs, page: Option<&str>) -> Result<Value> {
    if !(0.0..=442.0).contains(&args.tolerance) || !(0.0..=442.0).contains(&args.softness) {
        return Err(err(
            "invalid-option",
            "--tolerance and --softness must be between 0 and 442.",
            "Use values such as --tolerance 60 --softness 90.",
        ));
    }
    let key = parse_color(&args.key)?;
    let mut raw = read_document(&args.document)?;
    let (source, node) = source_for(&raw, &args.document, page, &args.source)?;
    let (layer, _) = find_node(&raw, page, &args.source).context("source node vanished")?;
    let (_, pixels) = crate::image::decode_source_pixels(&source.bytes)?;
    let keyed = key_out(&pixels, key, args.tolerance, args.softness);
    let removed = keyed.pixels().filter(|p| p[3] == 0).count();
    let bytes = encode_png(&keyed)?;
    let result = crate::image::add(
        &mut raw,
        page,
        &layer,
        &args.id,
        &bytes,
        crate::image::embedded_storage(&bytes),
        node["x"].as_f64().unwrap_or(0.0),
        node["y"].as_f64().unwrap_or(0.0),
        node["width"].as_f64().unwrap_or(f64::from(keyed.width())),
        node["height"].as_f64().unwrap_or(f64::from(keyed.height())),
        crate::image::Fit::Fill,
    )?;
    let change = crate::transaction::commit_value(
        &args.document,
        "ai-keyout",
        args.dry_run,
        args.if_revision.as_deref(),
        &raw,
    )?;
    Ok(json!({
        "change": change,
        "result": result,
        "transparent_pixels": removed,
        "total_pixels": u64::from(keyed.width()) * u64::from(keyed.height()),
        "model_call": false,
    }))
}

fn run_action(action: &RunAction, page: Option<&str>) -> Result<Value> {
    match action {
        RunAction::List { document } => {
            let mut runs = Vec::new();
            if let Ok(read) = fs::read_dir(runs_dir(document)) {
                for entry in read.flatten() {
                    if let Ok(bytes) = fs::read(entry.path().join("run.json")) {
                        if let Ok(record) = serde_json::from_slice::<Value>(&bytes) {
                            runs.push(json!({
                                "run": record["id"],
                                "name": record["name"],
                                "kind": record["kind"],
                                "provider": record["provider"],
                                "model": record["model"],
                                "candidates": record["candidates"].as_array().map_or(0, Vec::len),
                            }));
                        }
                    }
                }
            }
            runs.sort_by_key(|r| r["run"].as_str().unwrap_or_default().to_owned());
            Ok(json!({"runs":runs}))
        }
        RunAction::Show { document, run } => {
            let (record, dir) = load_run(document, run)?;
            Ok(json!({"run":record,"store":dir}))
        }
        RunAction::Discard { document, run } => {
            let (_, dir) = load_run(document, run)?;
            fs::remove_dir_all(dir)?;
            Ok(json!({"discarded":run,"document_changed":false}))
        }
        RunAction::Accept {
            document,
            run,
            candidate,
            id,
            layer,
            x,
            y,
            width,
            height,
            replace,
            allow_stale_source,
            dry_run,
            if_revision,
        } => accept(
            document,
            run,
            *candidate,
            id.as_deref(),
            layer.as_deref(),
            [*x, *y, *width, *height],
            replace.as_deref(),
            *allow_stale_source,
            *dry_run,
            if_revision.as_deref(),
            page,
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn accept(
    document: &Path,
    run: &str,
    candidate: usize,
    id: Option<&str>,
    layer: Option<&str>,
    frame: [Option<f64>; 4],
    replace: Option<&str>,
    allow_stale_source: bool,
    dry_run: bool,
    if_revision: Option<&str>,
    page: Option<&str>,
) -> Result<Value> {
    let (record, dir) = load_run(document, run)?;
    let entry = record["candidates"]
        .as_array()
        .and_then(|c| c.get(candidate.checked_sub(1)?))
        .ok_or_else(|| {
            err(
                "unknown-candidate",
                format!("run {run} has no candidate {candidate}."),
                "See `pentool ai run show DOCUMENT RUN` for the candidate list.",
            )
        })?;
    let file = entry["file"].as_str().context("candidate file missing")?;
    if file.contains(['/', '\\']) || file.contains("..") {
        bail!("[unsafe-path] candidate file name is not a plain file")
    }
    let bytes = fs::read(dir.join(file))?;
    if crate::resource::sha256(&bytes) != entry["sha256"].as_str().unwrap_or_default() {
        return Err(err(
            "stale-candidate",
            "the stored candidate no longer matches its recorded hash.",
            "Discard the run and generate again.",
        ));
    }
    let info = crate::image::decode_source(&bytes)?;
    let mut raw = read_document(document)?;
    let provenance = json!({
        "run": run,
        "candidate": candidate,
        "kind": record["kind"],
        "provider": record["provider"],
        "model": record["model"],
        "prompt_sha256": record["prompt_sha256"],
        "asset": info.digest,
    });
    let node_id;
    let mode;
    if let Some(target) = replace {
        let migrated = crate::composite::is_document(&raw) || raw["version"].as_u64() >= Some(5);
        if !migrated {
            bail!("[unsupported-version] replace requires a document that already contains the image node")
        }
        let old = find_node(&raw, page, target)
            .map(|(_, n)| n.clone())
            .ok_or_else(|| {
                err(
                    "not-found",
                    format!("image node {target} was not found."),
                    "List nodes with `pentool tree DOCUMENT --kind image`.",
                )
            })?;
        if old["kind"] != "image" {
            return Err(err(
                "invalid-target",
                format!("{target} is not an image."),
                "Pass an image node ID to --replace.",
            ));
        }
        let old_digest = old["asset"].as_str().unwrap_or_default().to_owned();
        ensure_target_unlocked(&raw, page, target)?;
        if !allow_stale_source {
            if let Some(source) = record["sources"]
                .as_array()
                .and_then(|s| s.iter().find(|s| s["node"] == target))
            {
                let recorded = source["sha256"].as_str().unwrap_or_default();
                if !recorded.is_empty() && recorded != old_digest && old_digest != info.digest {
                    return Err(err(
                        "stale-source",
                        format!(
                            "{target} changed since run {run} was made (run source {recorded}, current {old_digest}); replacing would silently discard the newer image."
                        ),
                        "Re-run the edit from the current image, or pass --allow-stale-source to overwrite deliberately.",
                    ));
                }
            }
        }
        let assets = raw["image_assets"]
            .as_object_mut()
            .context("image_assets missing")?;
        assets.entry(info.digest.clone()).or_insert_with(|| {
            json!({
                "media_type": info.media_type,
                "byte_length": info.byte_length,
                "pixel_width": info.pixel_width,
                "pixel_height": info.pixel_height,
                "color_space": info.color_space,
                "orientation": info.orientation,
                "storage": crate::image::embedded_storage(&bytes),
            })
        });
        let node = find_node_mut(&mut raw, page, target).context("target vanished")?;
        // Keep placement, crop, transform, masks, effects, and the stable ID; stale
        // pixel-space operations (crops in source pixels) are reset by validation.
        node["asset"] = json!(info.digest);
        if let Some(object) = node.as_object_mut() {
            object.insert("ai".into(), provenance.clone());
        }
        if old_digest != info.digest && !crate::image::document_references_asset(&raw, &old_digest)
        {
            if let Some(assets) = raw["image_assets"].as_object_mut() {
                assets.remove(&old_digest);
            }
        }
        node_id = target.to_owned();
        mode = "replace";
    } else {
        let new_id = id
            .map(str::to_owned)
            .or_else(|| record["name"].as_str().map(str::to_owned))
            .context("a node id is required")?;
        let (canvas_w, canvas_h) = canvas_size(&raw, page)
            .unwrap_or((f64::from(info.pixel_width), f64::from(info.pixel_height)));
        let natural = (f64::from(info.pixel_width), f64::from(info.pixel_height));
        let scale = (canvas_w / natural.0).min(canvas_h / natural.1).min(1.0);
        let (w, h) = (
            frame[2].unwrap_or(natural.0 * scale),
            frame[3].unwrap_or(natural.1 * scale),
        );
        let (px, py) = (
            frame[0].unwrap_or((canvas_w - w) / 2.0),
            frame[1].unwrap_or((canvas_h - h) / 2.0),
        );
        let target_layer = match layer {
            Some(layer) => layer.to_owned(),
            None => first_layer(&raw, page)?,
        };
        ensure_layer_open(&raw, page, &target_layer)?;
        crate::image::add(
            &mut raw,
            page,
            &target_layer,
            &new_id,
            &bytes,
            crate::image::embedded_storage(&bytes),
            px,
            py,
            w,
            h,
            crate::image::Fit::Contain,
        )?;
        let node = find_node_mut(&mut raw, page, &new_id).context("new node vanished")?;
        if let Some(object) = node.as_object_mut() {
            object.insert("ai".into(), provenance.clone());
        }
        node_id = new_id;
        mode = "new";
    }
    let change =
        crate::transaction::commit_value(document, "ai-accept", dry_run, if_revision, &raw)?;
    Ok(json!({
        "change": change,
        "mode": mode,
        "node": node_id,
        "asset": info.digest,
        "provenance": provenance,
        "dry_run": dry_run,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_out_removes_flat_key_and_keeps_subject() {
        let mut image = ::image::RgbaImage::from_pixel(8, 8, ::image::Rgba([255, 0, 255, 255]));
        for x in 2..6 {
            for y in 2..6 {
                image.put_pixel(x, y, ::image::Rgba([200, 120, 30, 255]));
            }
        }
        let keyed = key_out(&image, [255, 0, 255], 60.0, 90.0);
        assert_eq!(keyed.get_pixel(0, 0)[3], 0);
        assert_eq!(keyed.get_pixel(3, 3).0, [200, 120, 30, 255]);
    }

    #[test]
    fn urlencoding_keeps_safe_characters_and_escapes_the_rest() {
        assert_eq!(urlencoding_key("AQ.Ab-_~9"), "AQ.Ab-_~9");
        assert_eq!(urlencoding_key("a b&c"), "a%20b%26c");
    }

    #[test]
    fn endpoints_require_https_except_loopback() {
        assert!(check_endpoint("https://api.example.com/v1").is_ok());
        assert!(check_endpoint("http://127.0.0.1:9/v1").is_ok());
        assert!(check_endpoint("http://api.example.com/v1").is_err());
        assert!(check_endpoint("https://user:pw@api.example.com/").is_err());
    }

    #[test]
    fn model_ids_cannot_inject_path_or_query() {
        assert!(valid_model_id("gemini-2.5-flash-image"));
        assert!(!valid_model_id("a/../b"));
        assert!(!valid_model_id("m?key=x"));
    }
}
