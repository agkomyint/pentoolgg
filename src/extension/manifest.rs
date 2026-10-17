//! Extension manifest: identity, declared capabilities, entry point, limits.
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Component, Path};

/// The only extension API revision this build implements.
pub const API_VERSION: u32 = 1;
/// Deterministic CPU budget: wasm fuel units granted per declared `cpu_ms`.
pub const FUEL_PER_MS: u64 = 100_000;
pub const DEFAULT_CPU_MS: u64 = 5_000;
pub const MAX_CPU_MS: u64 = 60_000;
pub const DEFAULT_MEMORY_MB: u64 = 64;
pub const MAX_MEMORY_MB: u64 = 256;
pub const MANIFEST_FILE: &str = "extension.json";

/// Everything an extension may ask for. Anything not listed here does not exist.
pub const CAPABILITIES: &[&str] = &["document.read", "document.edit"];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    #[serde(default = "default_cpu")]
    pub cpu_ms: u64,
    #[serde(default = "default_memory")]
    pub memory_mb: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            cpu_ms: DEFAULT_CPU_MS,
            memory_mb: DEFAULT_MEMORY_MB,
        }
    }
}
fn default_cpu() -> u64 {
    DEFAULT_CPU_MS
}
fn default_memory() -> u64 {
    DEFAULT_MEMORY_MB
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Command {
    pub id: String,
    pub title: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub name: String,
    pub version: String,
    pub api: u32,
    pub capabilities: Vec<String>,
    pub entry: String,
    #[serde(default)]
    pub limits: Limits,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub commands: Vec<Command>,
}

pub fn validate_name(name: &str) -> Result<()> {
    if valid_id(name, 64) {
        Ok(())
    } else {
        bail!(
            "[invalid-operation] extension name `{name}` must be 1-64 chars of a-z, 0-9, `-`, `_`"
        )
    }
}

fn valid_id(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        && value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// A package-relative path that cannot escape its root.
pub fn safe_relative(path: &str) -> Result<()> {
    let p = Path::new(path);
    if path.is_empty()
        || path.contains('\\')
        || p.is_absolute()
        || p.components().any(|c| !matches!(c, Component::Normal(_)))
    {
        bail!("[unsafe-path] `{path}` must be a safe relative path inside the extension")
    }
    Ok(())
}

impl Manifest {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        crate::json::reject_duplicate_keys(bytes)?;
        let manifest: Manifest = serde_json::from_slice(bytes)
            .context("[malformed-resource] extension.json is not a valid manifest")?;
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn validate(&self) -> Result<()> {
        if !valid_id(&self.name, 64) {
            bail!(
                "[invalid-operation] extension name `{}` must be 1-64 chars of a-z, 0-9, `-`, `_`",
                self.name
            )
        }
        semver::Version::parse(&self.version).with_context(|| {
            format!(
                "[invalid-operation] version `{}` is not semver",
                self.version
            )
        })?;
        if self.api != API_VERSION {
            bail!(
                "[unsupported-capability] extension targets api {} but this build implements api {API_VERSION}",
                self.api
            )
        }
        let mut seen = Vec::new();
        for capability in &self.capabilities {
            if !CAPABILITIES.contains(&capability.as_str()) {
                bail!(
                    "[unsupported-capability] unknown capability `{capability}`; known: {}",
                    CAPABILITIES.join(", ")
                )
            }
            if seen.contains(&capability) {
                bail!("[invalid-operation] capability `{capability}` is listed twice")
            }
            seen.push(capability);
        }
        if self.capabilities.contains(&"document.edit".to_string())
            && !self.capabilities.contains(&"document.read".to_string())
        {
            bail!("[invalid-operation] `document.edit` requires `document.read`")
        }
        safe_relative(&self.entry)?;
        if !self.entry.ends_with(".wasm") {
            bail!(
                "[invalid-operation] entry `{}` must be a .wasm module",
                self.entry
            )
        }
        if self.limits.cpu_ms == 0 || self.limits.cpu_ms > MAX_CPU_MS {
            bail!("[limit-exceeded] limits.cpu_ms must be 1..={MAX_CPU_MS}")
        }
        if self.limits.memory_mb == 0 || self.limits.memory_mb > MAX_MEMORY_MB {
            bail!("[limit-exceeded] limits.memory_mb must be 1..={MAX_MEMORY_MB}")
        }
        if self.description.len() > 1024 || self.commands.len() > 32 {
            bail!("[limit-exceeded] description is limited to 1024 bytes and 32 commands")
        }
        for command in &self.commands {
            if !valid_id(&command.id, 64) || command.title.is_empty() || command.title.len() > 128 {
                bail!(
                    "[invalid-operation] command `{}` needs a valid id and a 1-128 byte title",
                    command.id
                )
            }
        }
        Ok(())
    }

    pub fn fuel(&self) -> u64 {
        self.limits.cpu_ms * FUEL_PER_MS
    }

    pub fn can(&self, capability: &str) -> bool {
        self.capabilities.iter().any(|c| c == capability)
    }
}
