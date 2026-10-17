//! `pentool design ...` command definitions and handlers.
use super::{
    audit, brand,
    library::{self, DiffOptions},
    page_args, paginate,
    plan::{self, Options},
    template, token_file,
};
use crate::{
    package, tokens as tok,
    tokens::{DesignError, Usage},
    transaction,
};
use clap::{Args, Subcommand};
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
};

type Result<T> = std::result::Result<T, DesignError>;

#[derive(Subcommand)]
pub enum DesignCommand {
    /// Inspect and resolve document design tokens.
    Token {
        #[command(subcommand)]
        action: TokenAction,
    },
    /// Package, install, lock, update and diff token libraries.
    Library {
        #[command(subcommand)]
        action: LibraryAction,
    },
    /// Inspect a document's brand kit or vendor a library's brand kit into it.
    Brand {
        #[command(subcommand)]
        action: BrandAction,
    },
    /// Check, list and fill parameterized templates.
    Template {
        #[command(subcommand)]
        action: TemplateAction,
    },
    /// Read-only audit of hardcoded values, token references and brand rules.
    Audit {
        input: PathBuf,
        /// Audit one page (default: the first page).
        #[arg(long)]
        page: Option<String>,
        /// Audit every page.
        #[arg(long)]
        all_pages: bool,
        /// Comma list of color,font,spacing,radius,refs,brand (default: all).
        #[arg(long)]
        rules: Option<String>,
        /// Only suggest and report tokens that came from this library.
        #[arg(long)]
        library: Option<String>,
        #[arg(long)]
        offset: Option<usize>,
        /// Findings per page (default 50, max 500).
        #[arg(long)]
        limit: Option<usize>,
        /// Add batch operations that bind exact matches to their tokens.
        #[arg(long)]
        fix_plan: bool,
        /// With --fix-plan, also bind near (deltaE <= 2) matches.
        #[arg(long)]
        fix_nearest: bool,
        /// Exit with status 3 when findings of this severity exist.
        #[arg(long, value_parser = ["warn", "error"])]
        fail_on: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum TemplateAction {
    /// Validate a template's design.template declaration (all problems are listed).
    Check {
        input: PathBuf,
        /// Page whose objects the targets refer to (default: first page).
        #[arg(long)]
        page: Option<String>,
    },
    /// List the templates (*.pen with design.template) in a directory, paginated.
    List {
        dir: PathBuf,
        #[arg(long)]
        offset: Option<usize>,
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Fill a template into a new document; the template is never modified.
    Fill {
        input: PathBuf,
        /// JSON object of {parameter: value}.
        #[arg(long, required_unless_present = "data", conflicts_with = "data")]
        values: Option<PathBuf>,
        #[arg(long, required_unless_present = "data", conflicts_with = "data")]
        output: Option<PathBuf>,
        /// Rows to fill (JSON array of objects, or .csv with a header of parameter names); max 10,000.
        #[arg(long, requires = "output_dir")]
        data: Option<PathBuf>,
        /// Directory receiving one <template>-NNNNN.pen per row (never overwrites).
        #[arg(long, requires = "data")]
        output_dir: Option<PathBuf>,
        #[arg(long)]
        page: Option<String>,
        #[arg(long)]
        dry_run: bool,
        /// Revision (SHA-256) the template must still have.
        #[arg(long)]
        if_revision: Option<String>,
        #[arg(long)]
        offset: Option<usize>,
        #[arg(long)]
        limit: Option<usize>,
    },
}

#[derive(Subcommand)]
pub enum BrandAction {
    /// Show the document's brand kit with resolved token values and profile state.
    Show { input: PathBuf },
    /// Vendor a library package's brand kit (its tokens and design.brand) into a document.
    Apply {
        package: PathBuf,
        input: PathBuf,
        #[command(flatten)]
        signing: Signing,
        #[command(flatten)]
        guard: Guard,
        #[command(flatten)]
        paging: Paging,
    },
}

#[derive(Subcommand)]
pub enum TokenAction {
    /// List tokens (sorted, paginated) with state and dependent counts.
    List {
        input: PathBuf,
        #[arg(long = "type")]
        token_type: Option<String>,
        /// Library name, `local`, or `override`.
        #[arg(long)]
        source: Option<String>,
        /// Add the resolved value, status and alias chain to every token.
        #[arg(long)]
        resolve: bool,
        #[arg(long)]
        offset: Option<usize>,
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Follow one token through its aliases using only the document.
    Resolve { input: PathBuf, token: String },
    /// Import a native or W3C token file into a document (dry run or commit).
    Import {
        input: PathBuf,
        /// Token file (`pentool.tokens` JSON or W3C design tokens).
        file: PathBuf,
        /// Force the file format (default: detected).
        #[arg(long, value_parser = ["native", "w3c"])]
        format: Option<String>,
        /// What to do when a token exists locally with a different value.
        #[arg(long, default_value = "fail", value_parser = ["fail", "skip", "overwrite"])]
        on_collision: String,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        if_revision: Option<String>,
        #[arg(long)]
        offset: Option<usize>,
        #[arg(long)]
        limit: Option<usize>,
    },
    /// Export a document's tokens as a native or W3C token file.
    Export {
        input: PathBuf,
        #[arg(long, default_value = "native", value_parser = ["native", "w3c"])]
        format: String,
        /// Write here instead of stdout.
        #[arg(long)]
        output: Option<PathBuf>,
        /// Omit tokens that cannot be represented and list them.
        #[arg(long)]
        skip_unmappable: bool,
    },
}

#[derive(Args)]
pub struct Paging {
    #[arg(long)]
    offset: Option<usize>,
    #[arg(long)]
    limit: Option<usize>,
    /// Include unchanged tokens in the report.
    #[arg(long)]
    all: bool,
}

#[derive(Args)]
pub struct Signing {
    /// Detached signature to require (see `pentool package sign`).
    #[arg(long)]
    signature: Option<PathBuf>,
    /// Public key file the signature must come from.
    #[arg(long)]
    public_key: Option<PathBuf>,
}

#[derive(Args)]
pub struct Guard {
    #[arg(long)]
    dry_run: bool,
    #[arg(long)]
    if_revision: Option<String>,
    /// Directory holding `pentool.lock` and `.pentool/` (default: the document's folder).
    #[arg(long)]
    project: Option<PathBuf>,
}

#[derive(Subcommand)]
pub enum LibraryAction {
    /// Create a library package directory with an empty design_library asset.
    Init { dir: PathBuf, name: String },
    /// Validate and pack a library directory into a deterministic .penpkg.
    Pack {
        dir: PathBuf,
        output: PathBuf,
        /// Earlier release; the declared version must satisfy semver for the changes.
        #[arg(long)]
        previous: Option<PathBuf>,
        /// Allow brand ICC profiles that are restricted or of unknown redistribution.
        #[arg(long)]
        allow_restricted_local: bool,
    },
    /// Verify a library package offline (hashes, schema, tokens).
    Verify {
        package: PathBuf,
        #[command(flatten)]
        signing: Signing,
    },
    /// Export a library package's tokens as a native or W3C token file.
    Export {
        package: PathBuf,
        #[arg(long, default_value = "native", value_parser = ["native", "w3c"])]
        format: String,
        /// Write here instead of stdout.
        #[arg(long)]
        output: Option<PathBuf>,
        /// Omit tokens that cannot be represented and list them.
        #[arg(long)]
        skip_unmappable: bool,
        #[command(flatten)]
        signing: Signing,
    },
    /// Show a library package's name, version, hash and token count.
    Inspect { package: PathBuf },
    /// Vendor a library's tokens into a document and lock the package.
    Install {
        package: PathBuf,
        input: PathBuf,
        /// Token ID globs (`*`) to vendor; aliases pull in their targets.
        #[arg(long = "tokens", num_args = 1..)]
        tokens: Vec<String>,
        #[command(flatten)]
        signing: Signing,
        #[command(flatten)]
        guard: Guard,
        #[command(flatten)]
        paging: Paging,
    },
    /// Plan (--dry-run) or commit a library update using the v0.6.24 resolutions.
    Update {
        input: PathBuf,
        /// New library package to update to.
        #[arg(long)]
        package: PathBuf,
        /// Assert the package's library name.
        #[arg(long)]
        library: Option<String>,
        /// Assert the package's version.
        #[arg(long)]
        to: Option<String>,
        /// JSON array of {"token","action","to"?}.
        #[arg(long)]
        resolutions: Option<PathBuf>,
        /// Update clean tokens and list every skipped one.
        #[arg(long)]
        continue_on_conflict: bool,
        #[command(flatten)]
        signing: Signing,
        #[command(flatten)]
        guard: Guard,
        #[command(flatten)]
        paging: Paging,
    },
    /// Compare document, lock and installed packages for each library (offline).
    Status {
        input: PathBuf,
        #[arg(long)]
        project: Option<PathBuf>,
    },
    /// Diff two packages, or a package against a document's vendored tokens.
    Diff {
        a: PathBuf,
        b: PathBuf,
        #[command(flatten)]
        paging: Paging,
    },
    /// Verify a library package and record it in the project's pentool.lock.
    Lock {
        package: PathBuf,
        #[arg(long)]
        project: Option<PathBuf>,
    },
}

fn invalid(message: impl Into<String>) -> DesignError {
    DesignError::new("invalid-input", message)
}

pub(super) fn read_document(path: &Path) -> Result<(Vec<u8>, Value)> {
    let bytes =
        fs::read(path).map_err(|e| invalid(format!("could not read {}: {e}", path.display())))?;
    let raw: Value = serde_json::from_slice(&bytes)
        .map_err(|e| invalid(format!("{} is not valid JSON: {e}", path.display())))?;
    if raw.get("version").and_then(Value::as_u64).unwrap_or(0) < 4 {
        return Err(DesignError::new(
            "unsupported-capability",
            "design tokens need a version 4 or newer scene document",
        )
        .action("migrate the document with `pentool migrate`"));
    }
    crate::scene::validate(&raw).map_err(|e| match e.downcast::<DesignError>() {
        Ok(design) => design,
        Err(other) => invalid(format!("{other:#}")),
    })?;
    Ok((bytes, raw))
}

fn operation_name(command: &DesignCommand) -> &'static str {
    match command {
        DesignCommand::Token {
            action: TokenAction::List { .. },
        } => "design token list",
        DesignCommand::Token {
            action: TokenAction::Resolve { .. },
        } => "design token resolve",
        DesignCommand::Token {
            action: TokenAction::Import { .. },
        } => "design token import",
        DesignCommand::Token {
            action: TokenAction::Export { .. },
        } => "design token export",
        DesignCommand::Template { action } => match action {
            TemplateAction::Check { .. } => "design template check",
            TemplateAction::List { .. } => "design template list",
            TemplateAction::Fill { .. } => "design template fill",
        },
        DesignCommand::Audit { .. } => "design audit",
        DesignCommand::Brand { action } => match action {
            BrandAction::Show { .. } => "design brand show",
            BrandAction::Apply { .. } => "design brand apply",
        },
        DesignCommand::Library { action } => match action {
            LibraryAction::Init { .. } => "design library init",
            LibraryAction::Pack { .. } => "design library pack",
            LibraryAction::Verify { .. } => "design library verify",
            LibraryAction::Inspect { .. } => "design library inspect",
            LibraryAction::Export { .. } => "design library export",
            LibraryAction::Install { .. } => "design library install",
            LibraryAction::Update { .. } => "design library update",
            LibraryAction::Status { .. } => "design library status",
            LibraryAction::Diff { .. } => "design library diff",
            LibraryAction::Lock { .. } => "design library lock",
        },
    }
}

/// Run a `design` command and print its JSON. A failure prints the structured
/// `{"error": {...}}` object to stderr and exits with status 1.
pub fn run_and_print(command: DesignCommand) -> anyhow::Result<()> {
    let operation = operation_name(&command);
    let fail_on = match &command {
        DesignCommand::Audit { fail_on, .. } => fail_on.clone(),
        _ => None,
    };
    match run(command) {
        Ok(value) => {
            println!("{}", serde_json::to_string_pretty(&value)?);
            if let Some(level) = fail_on {
                let status = audit::fail_status(&value, &level);
                if status != 0 {
                    std::process::exit(status);
                }
            }
            Ok(())
        }
        Err(mut error) => {
            if error.operation.is_none() {
                error.operation = Some(operation.into());
            }
            eprintln!("{}", serde_json::to_string_pretty(&error.to_json())?);
            std::process::exit(1)
        }
    }
}

pub fn run(command: DesignCommand) -> Result<Value> {
    match command {
        DesignCommand::Token { action } => token(action),
        DesignCommand::Library { action } => library_action(action),
        DesignCommand::Brand { action } => brand_action(action),
        DesignCommand::Template { action } => template_action(action),
        DesignCommand::Audit {
            input,
            page,
            all_pages,
            rules,
            library,
            offset,
            limit,
            fix_plan,
            fix_nearest,
            fail_on: _,
        } => {
            let (bytes, raw) = read_document(&input)?;
            audit::run(
                &bytes,
                &raw,
                &audit::Options {
                    document: &input.to_string_lossy(),
                    page: page.as_deref(),
                    all_pages,
                    rules: rules.as_deref(),
                    library: library.as_deref(),
                    offset,
                    limit,
                    fix_plan,
                    fix_nearest,
                },
            )
        }
    }
}

fn token(action: TokenAction) -> Result<Value> {
    match action {
        TokenAction::List {
            input,
            token_type,
            source,
            resolve,
            offset,
            limit,
        } => {
            let (offset, limit) = page_args(offset, limit)?;
            let (_, raw) = read_document(&input)?;
            let usage = Usage::build(&raw);
            let styles = raw.get("styles").cloned().unwrap_or_else(|| json!({}));
            let mut rows = Vec::new();
            let mut names: Vec<&String> = styles
                .as_object()
                .into_iter()
                .flat_map(|m| m.keys())
                .collect();
            names.sort();
            for name in names {
                let record = &styles[name];
                let kind = record.get("type").and_then(Value::as_str).unwrap_or("");
                if token_type.as_deref().is_some_and(|wanted| wanted != kind) {
                    continue;
                }
                let state = match record.pointer("/source/hash").and_then(Value::as_str) {
                    None => "local",
                    Some(base) if tok::record_hash(record).as_deref() == Some(base) => "clean",
                    Some(_) => "override",
                };
                if let Some(wanted) = &source {
                    let library = record.pointer("/source/library").and_then(Value::as_str);
                    let keep = match wanted.as_str() {
                        "local" => state == "local",
                        "override" => state == "override",
                        name => library == Some(name),
                    };
                    if !keep {
                        continue;
                    }
                }
                let (nodes, tokens) = usage.dependents(name);
                let mut row = json!({
                    "token": name, "type": kind, "value": record.get("value"),
                    "state": state, "dependents": nodes.len() + tokens.len(),
                });
                if let Some(source) = record.get("source") {
                    row["source"] = json!({
                        "library": source.get("library"), "version": source.get("version"),
                    });
                }
                if resolve {
                    let resolution = tok::resolve(&styles, name);
                    row["resolved"] = json!({
                        "status": resolution.status.as_str(), "value": resolution.value,
                        "chain": resolution.chain,
                    });
                }
                rows.push(row);
            }
            let (page, meta) = paginate(rows, offset, limit);
            let mut out = meta;
            out["tokens"] = json!(page);
            Ok(out)
        }
        TokenAction::Import {
            input,
            file,
            format,
            on_collision,
            dry_run,
            if_revision,
            offset,
            limit,
        } => token_file::import(&token_file::ImportOptions {
            input: &input,
            file: &file,
            format: format
                .as_deref()
                .map(token_file::Format::parse)
                .transpose()?,
            mode: token_file::Collision::parse(&on_collision)?,
            dry_run,
            if_revision: if_revision.as_deref(),
            offset,
            limit,
        }),
        TokenAction::Export {
            input,
            format,
            output,
            skip_unmappable,
        } => token_file::export_document(
            &input,
            token_file::Format::parse(&format)?,
            output.as_deref(),
            skip_unmappable,
        ),
        TokenAction::Resolve { input, token } => {
            let (_, raw) = read_document(&input)?;
            let styles = raw.get("styles").cloned().unwrap_or_else(|| json!({}));
            if styles.get(&token).is_none() {
                return Err(DesignError::new(
                    "unknown-token",
                    format!("token {token} does not exist"),
                )
                .token(token)
                .action("run `pentool design token list` for valid IDs"));
            }
            let resolution = tok::resolve(&styles, &token);
            let mut out = json!({
                "token": token, "type": resolution.token_type, "value": resolution.value,
                "chain": resolution.chain, "status": resolution.status.as_str(),
                "fallback_used": resolution.status != tok::Status::Resolved,
            });
            if let Some(reason) = resolution.reason {
                out["reason"] = json!(reason);
            }
            Ok(out)
        }
    }
}

fn library_action(action: LibraryAction) -> Result<Value> {
    match action {
        LibraryAction::Init { dir, name } => library::init(&dir, &name),
        LibraryAction::Pack {
            dir,
            output,
            previous,
            allow_restricted_local,
        } => library::pack(&dir, &output, previous.as_deref(), allow_restricted_local),
        LibraryAction::Verify { package, signing } => {
            library::check_signature(
                &package,
                signing.signature.as_deref(),
                signing.public_key.as_deref(),
            )?;
            Ok(library::summary(&library::load_package(&package)?))
        }
        LibraryAction::Export {
            package,
            format,
            output,
            skip_unmappable,
            signing,
        } => {
            library::check_signature(
                &package,
                signing.signature.as_deref(),
                signing.public_key.as_deref(),
            )?;
            let snapshot = library::load_package(&package)?;
            let format = token_file::Format::parse(&format)?;
            let (file, skipped) = token_file::render(&snapshot.tokens, format, skip_unmappable)?;
            let exported = snapshot.tokens.len() - skipped.len();
            token_file::emit(file, format, exported, skipped, output.as_deref())
        }
        LibraryAction::Inspect { package } => {
            let snapshot = library::load_package(&package)?;
            let mut out = library::summary(&snapshot);
            out["token_ids"] = json!(snapshot.tokens.keys().collect::<Vec<_>>());
            Ok(out)
        }
        LibraryAction::Install {
            package,
            input,
            tokens,
            signing,
            guard,
            paging,
        } => {
            library::check_signature(
                &package,
                signing.signature.as_deref(),
                signing.public_key.as_deref(),
            )?;
            let selection = if tokens.is_empty() {
                None
            } else {
                Some(tokens)
            };
            update(
                &input,
                &package,
                "design-library-install",
                Options {
                    selection,
                    continue_on_conflict: false,
                },
                &[],
                &guard,
                &paging,
                (None, None),
                false,
            )
        }
        LibraryAction::Update {
            input,
            package,
            library: name,
            to,
            resolutions,
            continue_on_conflict,
            signing,
            guard,
            paging,
        } => {
            library::check_signature(
                &package,
                signing.signature.as_deref(),
                signing.public_key.as_deref(),
            )?;
            let resolutions = match resolutions {
                Some(path) => {
                    let value: Value = serde_json::from_slice(
                        &fs::read(&path)
                            .map_err(|e| invalid(format!("{}: {e}", path.display())))?,
                    )
                    .map_err(|e| invalid(format!("resolutions file: {e}")))?;
                    plan::parse_resolutions(&value)?
                }
                None => vec![],
            };
            update(
                &input,
                &package,
                "design-library-update",
                Options {
                    selection: None,
                    continue_on_conflict,
                },
                &resolutions,
                &guard,
                &paging,
                (name, to),
                false,
            )
        }
        LibraryAction::Status { input, project } => status(&input, project.as_deref()),
        LibraryAction::Diff { a, b, paging } => library::diff(
            &a,
            &b,
            &DiffOptions {
                offset: paging.offset,
                limit: paging.limit,
                all: paging.all,
            },
        ),
        LibraryAction::Lock { package, project } => {
            let snapshot = library::load_package(&package)?;
            let project = project_dir(project.as_deref(), Path::new("."));
            let report = package::install(&package, &project, &source_string(&package, &project))
                .map_err(|e| invalid(format!("{e:#}")))?;
            Ok(
                json!({"ok": true, "library": snapshot.name, "version": snapshot.version,
                      "package_hash": snapshot.package_hash, "install": report}),
            )
        }
    }
}

fn template_action(action: TemplateAction) -> Result<Value> {
    match action {
        TemplateAction::Check { input, page } => template::check(&input, page.as_deref()),
        TemplateAction::List { dir, offset, limit } => template::list(&dir, offset, limit),
        TemplateAction::Fill {
            input,
            values,
            output,
            data,
            output_dir,
            page,
            dry_run,
            if_revision,
            offset,
            limit,
        } => template::fill(&template::FillOptions {
            input: &input,
            values: values.as_deref(),
            output: output.as_deref(),
            data: data.as_deref(),
            output_dir: output_dir.as_deref(),
            page: page.as_deref(),
            dry_run,
            if_revision: if_revision.as_deref(),
            offset,
            limit,
        }),
    }
}

fn brand_action(action: BrandAction) -> Result<Value> {
    match action {
        BrandAction::Show { input } => {
            let (_, raw) = read_document(&input)?;
            brand::show(&raw)
        }
        BrandAction::Apply {
            package,
            input,
            signing,
            guard,
            paging,
        } => {
            library::check_signature(
                &package,
                signing.signature.as_deref(),
                signing.public_key.as_deref(),
            )?;
            update(
                &input,
                &package,
                "design-brand-apply",
                Options::default(),
                &[],
                &guard,
                &paging,
                (None, None),
                true,
            )
        }
    }
}

fn project_dir(explicit: Option<&Path>, document: &Path) -> PathBuf {
    match explicit {
        Some(path) => path.to_path_buf(),
        None => match document.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent.to_path_buf(),
            _ => PathBuf::from("."),
        },
    }
}

/// Lock `source`: relative to the project when the package lives inside it.
fn source_string(package: &Path, project: &Path) -> String {
    let absolute = std::path::absolute(package).unwrap_or_else(|_| package.to_path_buf());
    let root = std::path::absolute(project).unwrap_or_else(|_| project.to_path_buf());
    match absolute.strip_prefix(&root) {
        Ok(relative) => relative.to_string_lossy().replace('\\', "/"),
        Err(_) => absolute.to_string_lossy().replace('\\', "/"),
    }
}

#[allow(clippy::too_many_arguments)]
fn update(
    input: &Path,
    package: &Path,
    operation: &str,
    options: Options,
    resolutions: &[plan::Resolution],
    guard: &Guard,
    paging: &Paging,
    assert: (Option<String>, Option<String>),
    brand_kit: bool,
) -> Result<Value> {
    let (offset, limit) = page_args(paging.offset, paging.limit)?;
    let (before, raw) = read_document(input)?;
    let revision = transaction::revision(&before);
    if guard
        .if_revision
        .as_deref()
        .is_some_and(|wanted| wanted != revision)
    {
        return Err(DesignError::new(
            "conflict",
            format!(
                "revision mismatch: expected {:?}, current {revision}",
                guard.if_revision
            ),
        )
        .action("re-read the document and plan again"));
    }
    let snapshot = library::load_package(package)?;
    if assert
        .0
        .as_deref()
        .is_some_and(|name| name != snapshot.name)
    {
        return Err(invalid(format!(
            "package is library {}, not {}",
            snapshot.name,
            assert.0.unwrap()
        )));
    }
    if assert
        .1
        .as_deref()
        .is_some_and(|version| version != snapshot.version)
    {
        return Err(invalid(format!(
            "package is version {}, not {}",
            snapshot.version,
            assert.1.unwrap()
        )));
    }
    let mut options = options;
    if brand_kit {
        let kit = snapshot.brand.as_ref().ok_or_else(|| {
            invalid(format!("library {} has no brand kit", snapshot.name))
                .action("add a design_library.brand block and pack the library again")
        })?;
        options.selection = Some(brand::referenced_tokens(kit));
    }
    let plan = plan::plan_update(&raw, &snapshot, resolutions, &options)?;
    let mut candidate = plan::apply_plan(&raw, &snapshot, &plan)?;
    if let (true, Some(kit)) = (brand_kit, &snapshot.brand) {
        brand::install_into(&mut candidate, &snapshot.name, kit)?;
    }
    crate::scene::validate(&candidate).map_err(|e| match e.downcast::<DesignError>() {
        Ok(design) => design,
        Err(other) => invalid(format!("{other:#}")),
    })?;
    let blocked = plan.blocked();
    let would_change = !blocked && candidate != raw;

    let rows = plan.token_rows(paging.all);
    let (page, meta) = paginate(rows, offset, limit);
    let mut report = json!({
        "library": plan.library, "from": plan.from, "to": plan.to,
        "package_hash": plan.package_hash,
        "dry_run": guard.dry_run, "would_change": would_change, "blocked": blocked,
        "counts": plan.counts(),
        "tokens": page,
        "conflicts": plan.conflicts, "resolutions_needed": plan.resolutions_needed,
        "writes": if blocked { json!([]) } else { json!(plan.writes) },
        "change": {"revision_before": revision, "revision_after": Value::Null},
    });
    for (key, value) in meta.as_object().unwrap() {
        report[key] = value.clone();
    }
    if let (true, Some(kit)) = (brand_kit, &snapshot.brand) {
        report["brand"] = json!({
            "colors": kit.get("colors").and_then(Value::as_object).map_or(0, |m| m.len()),
            "type": kit.get("type").and_then(Value::as_object).map_or(0, |m| m.len()),
            "profiles": brand::profile_report(&candidate, kit),
        });
    }
    if guard.dry_run {
        return Ok(report);
    }
    if blocked {
        return Err(DesignError::new(
            "conflict",
            format!(
                "{} token(s) need a resolution: {}",
                plan.resolutions_needed.len(),
                plan.resolutions_needed.join(", ")
            ),
        )
        .action(
            "rerun with --resolutions FILE or --continue-on-conflict; the document was not changed",
        ));
    }
    let project = project_dir(guard.project.as_deref(), input);
    let lock_path = project.join("pentool.lock");
    let old_lock = fs::read(&lock_path).ok();
    package::install(package, &project, &source_string(package, &project))
        .map_err(|e| DesignError::new("hash-mismatch", format!("{e:#}")))?;
    let restore_lock = |old: &Option<Vec<u8>>| match old {
        Some(bytes) => {
            let _ = crate::editing::atomic_write(&lock_path, bytes);
        }
        None => {
            let _ = fs::remove_file(&lock_path);
        }
    };
    let committed = transaction::commit_value(
        input,
        operation,
        false,
        guard.if_revision.as_deref(),
        &candidate,
    );
    match committed {
        Ok(summary) => {
            report["change"] = json!({
                "revision_before": summary.revision_before,
                "revision_after": summary.revision_after,
                "history": summary.history,
            });
            Ok(report)
        }
        Err(error) => {
            restore_lock(&old_lock);
            Err(invalid(format!("{error:#}")).action("the document and lock were left unchanged"))
        }
    }
}

fn status(input: &Path, project: Option<&Path>) -> Result<Value> {
    let (_, raw) = read_document(input)?;
    let project = project_dir(project, input);
    let lock: package::Lockfile = match fs::read(project.join("pentool.lock")) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).map_err(|e| invalid(format!("pentool.lock: {e}")))?
        }
        Err(_) => package::Lockfile::default(),
    };
    let mut rows = Vec::new();
    let libraries = raw.pointer("/design/libraries").and_then(Value::as_object);
    for (name, record) in libraries.into_iter().flatten() {
        let version = record.get("version").and_then(Value::as_str).unwrap_or("");
        let hash = record
            .get("package_hash")
            .and_then(Value::as_str)
            .unwrap_or("");
        let entry = lock.packages.iter().find(|p| &p.name == name);
        let (state, detail) = match entry {
            None => ("missing", "not in pentool.lock"),
            Some(entry) => {
                let marker = project
                    .join(".pentool/packages")
                    .join(&entry.name)
                    .join(&entry.version)
                    .join(".package-hash");
                let installed = fs::read_to_string(&marker).ok();
                let source = project.join(&entry.source);
                let source_hash = fs::read(&source).ok().map(|b| crate::asset::hash_bytes(&b));
                if installed.is_none() {
                    (
                        "missing",
                        "package is not installed under .pentool/packages",
                    )
                } else if installed.as_deref() != Some(entry.package_hash.as_str())
                    || source_hash
                        .as_deref()
                        .is_some_and(|h| h != entry.package_hash)
                {
                    (
                        "hash-mismatch",
                        "installed or source package differs from the lock",
                    )
                } else if entry.version != version || entry.package_hash != hash {
                    (
                        "stale",
                        "document records a different version than the lock",
                    )
                } else {
                    ("ok", "")
                }
            }
        };
        rows.push(json!({"library": name, "version": version, "package_hash": hash,
                         "locked_version": entry.map(|e| &e.version), "state": state, "detail": detail}));
    }
    let ok = rows.iter().all(|r| r["state"] == "ok");
    Ok(json!({"ok": ok, "libraries": rows}))
}
