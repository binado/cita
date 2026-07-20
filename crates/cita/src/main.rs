mod git;

use anyhow::{Context, Result, bail};
use cita_bibliography::parse as parse_bibtex;
use cita_core::{Locator, MetadataProvider, Reference, ReferenceSource};
use cita_documents::{
    DocumentStore, Error as DocumentError, FetchOutcome, FetchPolicy, arxiv_pdf_url,
};
use cita_inspire_client::{Client, RetryEvent};
use cita_manifest::{
    AddOutcome, BIBLIOGRAPHY_FILE, KeyRequest, MANIFEST_FILE, Manifest, PendingReference,
    SourceSnapshot,
};
use clap::{CommandFactory, Parser, Subcommand};
use std::{
    env, fmt, fs,
    fs::OpenOptions,
    io::{self, IsTerminal, Read, Write},
    path::{Path, PathBuf},
};

const CACHE_IGNORE_COMMENT: &str = "# Cita document cache";
const CACHE_IGNORE_RULE: &str = "/.cita/files/";

#[derive(Debug, Parser)]
#[command(name = "cita", version, about = "A Git-friendly bibliography database")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Initialize cita.toml and its generated references.bib
    Init,
    /// Import standalone BibTeX entries from a path or stdin (`-`)
    Import { path: String },
    /// Resolve and add one or more references through INSPIRE
    Add {
        /// Keep this local citation key (one locator only)
        #[arg(long)]
        key: Option<String>,
        #[arg(required = true)]
        locators: Vec<String>,
    },
    /// Refresh every INSPIRE-managed source snapshot by stable record id
    Sync,
    /// Remove references by local key or provider/DOI/arXiv identity
    Remove {
        #[arg(required = true)]
        selectors: Vec<String>,
    },
    /// List stored references
    List {
        #[arg(long, value_enum, default_value_t = SortBy::Key)]
        sort_by: SortBy,
        #[arg(long, value_enum, default_value_t = Order::Asc)]
        order: Order,
        #[arg(long)]
        no_wrap_title: bool,
    },
    /// Regenerate a missing or edited references.bib
    Generate,
    /// Fetch or resolve a reference's arXiv PDF
    Fetch {
        #[arg(long, conflicts_with_all = ["cache_only", "url"])]
        force: bool,
        /// Require an existing cached PDF without downloading
        #[arg(long, conflicts_with = "url")]
        cache_only: bool,
        /// Return the arXiv PDF URL without downloading
        #[arg(short = 'u', long)]
        url: bool,
        /// Open the returned path or URL with the system default application
        #[arg(long)]
        open: bool,
        #[arg(long)]
        save: bool,
        selector: String,
    },
    /// Commit cita.toml and references.bib, leaving unrelated files alone
    Commit,
}

#[derive(Clone, Copy, Debug, Default, clap::ValueEnum)]
enum SortBy {
    #[default]
    Key,
    Title,
    Author,
    Year,
}
#[derive(Clone, Copy, Debug, Default, clap::ValueEnum)]
enum Order {
    #[default]
    Asc,
    Desc,
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let cli = Cli::parse();
    let cwd = env::current_dir().context("could not determine current directory")?;
    match cli.command {
        None => {
            Cli::command().print_help()?;
            println!();
        }
        Some(Command::Init) => init(&cwd)?,
        Some(Command::Import { path }) => import(&cwd, &path)?,
        Some(Command::Add { key, locators }) => add(&cwd, key.as_deref(), &locators).await?,
        Some(Command::Sync) => sync(&cwd).await?,
        Some(Command::Remove { selectors }) => remove(&cwd, &selectors)?,
        Some(Command::List {
            sort_by,
            order,
            no_wrap_title,
        }) => list(&cwd, sort_by, order, !no_wrap_title)?,
        Some(Command::Generate) => generate(&cwd)?,
        Some(Command::Fetch {
            force,
            cache_only,
            url,
            open,
            save,
            selector,
        }) => {
            let policy = if force {
                FetchPolicy::Force
            } else if cache_only {
                FetchPolicy::CacheOnly
            } else {
                FetchPolicy::UseCache
            };
            fetch(&cwd, &selector, policy, url, open, save).await?;
        }
        Some(Command::Commit) => git::commit(&find_manifest(&cwd)?)?,
    }
    Ok(())
}

fn init(cwd: &Path) -> Result<()> {
    let directory = match git::repository_root(cwd) {
        Ok(Some(root)) => root,
        Ok(None) => cwd.to_path_buf(),
        Err(error) => {
            eprintln!("warning: {error:#}; initializing in the current directory");
            cwd.to_path_buf()
        }
    };
    let manifest_path = directory.join(MANIFEST_FILE);
    let bibliography_path = directory.join(BIBLIOGRAPHY_FILE);
    let existed = manifest_path.exists();
    if existed {
        Manifest::load_verified(&manifest_path)?;
    } else if bibliography_path.exists() {
        Manifest::import_existing(&directory)?;
    } else {
        Manifest::create(&directory)?;
    }
    ensure_cache_layout(&directory)?;
    if existed {
        println!("Already initialized {}", manifest_path.display());
    } else {
        println!("Initialized {}", manifest_path.display());
    }
    Ok(())
}

fn import(cwd: &Path, input: &str) -> Result<()> {
    let mut source = String::new();
    if input == "-" {
        io::stdin()
            .read_to_string(&mut source)
            .context("could not read BibTeX from stdin")?;
    } else {
        source = fs::read_to_string(input).with_context(|| format!("could not read {input}"))?;
    }
    let pending = parse_bibtex(&source)?
        .into_iter()
        .map(|(key, snapshot)| PendingReference {
            key: KeyRequest::Exact(key),
            source: SourceSnapshot::Import(snapshot),
        })
        .collect();
    let mut manifest = Manifest::load_verified(find_manifest(cwd)?)?;
    for outcome in manifest.add_batch(pending)? {
        print_add(outcome);
    }
    Ok(())
}

fn inspire_client() -> Result<Client> {
    let mut builder = Client::builder().on_retry(|event: &RetryEvent| {
        eprintln!(
            "INSPIRE rate limited the request for {}; retrying in {:?} (attempt {} of {})",
            event.resource, event.delay, event.attempt, event.max_retries
        );
    });
    if let Ok(base) = env::var("CITA_INSPIRE_BASE_URL") {
        builder = builder.base_url(base);
    }
    Ok(builder.build()?)
}

async fn add(cwd: &Path, explicit_key: Option<&str>, values: &[String]) -> Result<()> {
    if explicit_key.is_some() && values.len() != 1 {
        bail!("--key may only be used with one locator");
    }
    let locators = values
        .iter()
        .map(|value| value.parse::<Locator>())
        .collect::<Result<Vec<_>, _>>()?;
    let client = inspire_client()?;
    let mut pending = Vec::with_capacity(locators.len());
    for locator in &locators {
        let record = client.resolve(locator).await?;
        let key = match explicit_key {
            Some(key) => KeyRequest::Exact(key.to_owned()),
            None => KeyRequest::Suggested(record.texkey.clone()),
        };
        pending.push(PendingReference {
            key,
            source: SourceSnapshot::inspire(record),
        });
    }
    let mut manifest = Manifest::load_verified(find_manifest(cwd)?)?;
    for outcome in manifest.add_batch(pending)? {
        print_add(outcome);
    }
    Ok(())
}

fn add_message(outcome: &AddOutcome) -> String {
    match outcome {
        AddOutcome::Added(key) => format!("Added {key}"),
        AddOutcome::Existing(key) => format!("Already present: {key}"),
    }
}

fn print_add(outcome: AddOutcome) {
    println!("{}", add_message(&outcome));
}

async fn sync(cwd: &Path) -> Result<()> {
    let mut manifest = Manifest::load_verified(find_manifest(cwd)?)?;
    let ids = manifest.inspire_record_ids();
    let managed = ids.len();
    let unmanaged = manifest.references().len() - managed;
    let provider_ids = ids.iter().map(u64::to_string).collect::<Vec<_>>();
    let refreshed = inspire_client()?.refresh(&provider_ids).await?;
    let changed = manifest.replace_inspire(refreshed)?;
    if changed {
        println!("Synced {managed} managed references; left {unmanaged} imported unchanged");
    } else {
        println!("Already in sync: {managed} managed, {unmanaged} imported");
    }
    Ok(())
}

fn remove(cwd: &Path, selectors: &[String]) -> Result<()> {
    let mut manifest = Manifest::load_verified(find_manifest(cwd)?)?;
    for item in manifest.remove_batch(selectors)? {
        println!("Removed {}", item.key);
    }
    Ok(())
}

fn generate(cwd: &Path) -> Result<()> {
    let manifest = Manifest::load(find_manifest(cwd)?)?;
    manifest.generate()?;
    println!("Generated {}", manifest.bibliography_path().display());
    Ok(())
}

#[derive(Clone)]
struct Selected {
    key: String,
    reference: Reference,
    manifest_path: PathBuf,
    save_outcome: Option<AddOutcome>,
}

async fn select(cwd: &Path, selector: &str, save: bool) -> Result<Selected> {
    let path = find_manifest(cwd)?;
    let mut manifest = Manifest::load_verified(&path)?;
    if let Some(item) = manifest.find(selector)? {
        let save_outcome = save.then(|| AddOutcome::Existing(item.key.clone()));
        return Ok(Selected {
            key: item.key,
            reference: item.reference,
            manifest_path: path,
            save_outcome,
        });
    }
    let locator = selector.parse::<Locator>().map_err(|error| {
        anyhow::Error::from(error).context(format!("reference `{selector}` was not found"))
    })?;
    let client = inspire_client()?;
    if save {
        let record = client.resolve(&locator).await?;
        let key = record.texkey.clone();
        let reference = record.project()?;
        let outcome = manifest
            .add_batch(vec![PendingReference {
                key: KeyRequest::Suggested(key.clone()),
                source: SourceSnapshot::inspire(record),
            }])?
            .pop()
            .expect("one outcome");
        Ok(Selected {
            key,
            reference,
            manifest_path: path,
            save_outcome: Some(outcome),
        })
    } else {
        let reference = client.resolve_reference(&locator).await?;
        Ok(Selected {
            key: selector.into(),
            reference,
            manifest_path: path,
            save_outcome: None,
        })
    }
}

async fn fetch(
    cwd: &Path,
    selector: &str,
    policy: FetchPolicy,
    return_url: bool,
    open: bool,
    save: bool,
) -> Result<()> {
    let selected = select(cwd, selector, save).await?;
    if let Some(outcome) = &selected.save_outcome {
        eprintln!("{}", add_message(outcome));
    }
    let arxiv = selected
        .reference
        .identifiers
        .arxiv
        .first()
        .ok_or_else(|| anyhow::anyhow!("reference `{}` has no arXiv eprint", selected.key))?;
    let url = arxiv_pdf_url(arxiv)?.to_string();
    let target = if return_url {
        FetchTarget::Url(url)
    } else {
        let outcome = fetch_selected(&selected.manifest_path, arxiv, policy).await?;
        eprintln!("{}", fetch_message(&selected.key, &url, &outcome));
        FetchTarget::Path(outcome.path().to_owned())
    };
    if open {
        open_target(&target)?;
        eprintln!("Opened {target}");
    }
    println!("{target}");
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum FetchTarget {
    Url(String),
    Path(PathBuf),
}

impl fmt::Display for FetchTarget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Url(url) => formatter.write_str(url),
            Self::Path(path) => write!(formatter, "{}", path.display()),
        }
    }
}

fn open_target(target: &FetchTarget) -> Result<()> {
    match target {
        FetchTarget::Url(url) => opener::open(url).with_context(|| format!("could not open {url}")),
        FetchTarget::Path(path) => {
            opener::open(path).with_context(|| format!("could not open {}", path.display()))
        }
    }
}

async fn fetch_selected(path: &Path, arxiv: &str, policy: FetchPolicy) -> Result<FetchOutcome> {
    let root = path.parent().unwrap_or_else(|| Path::new("."));
    if policy != FetchPolicy::CacheOnly {
        ensure_cache_layout(root)?;
    }
    DocumentStore::new(root.join(".cita/files"))?
        .fetch(arxiv, policy)
        .await
        .map_err(document_error_with_hint)
}

fn document_error_with_hint(error: DocumentError) -> anyhow::Error {
    match error {
        error @ DocumentError::InvalidCachedPdf(_) => {
            anyhow::Error::from(error).context("retry with --force")
        }
        error @ DocumentError::NotCached(_) => {
            anyhow::Error::from(error).context("rerun without --cache-only")
        }
        error => error.into(),
    }
}
fn fetch_message(key: &str, url: &str, outcome: &FetchOutcome) -> String {
    match outcome {
        FetchOutcome::Downloaded(_) => format!("Fetched {key}: {url}"),
        FetchOutcome::Cached(_) => format!("Already fetched {key}: {url}"),
    }
}

struct Row {
    key: String,
    title: String,
    author: String,
    year_num: Option<i32>,
    year: String,
}
fn list(cwd: &Path, sort_by: SortBy, order: Order, wrap_title: bool) -> Result<()> {
    let manifest = Manifest::load_verified(find_manifest(cwd)?)?;
    let mut rows = manifest
        .projected()?
        .into_iter()
        .map(|item| {
            let author = match item.reference.authors.first() {
                Some(first) if item.reference.authors.len() > 1 => format!("{first} et al."),
                Some(first) => first.clone(),
                None => item
                    .reference
                    .collaborations
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "—".into()),
            };
            Row {
                key: item.key,
                title: item.reference.title,
                author,
                year_num: item.reference.year,
                year: item
                    .reference
                    .year
                    .map_or_else(|| "—".into(), |year| year.to_string()),
            }
        })
        .collect::<Vec<_>>();
    match sort_by {
        SortBy::Key => {
            if let Order::Desc = order {
                rows.reverse();
            }
        }
        SortBy::Title => rows.sort_by(|a, b| ordered(a.title.cmp(&b.title), order)),
        SortBy::Author => rows.sort_by(|a, b| ordered(a.author.cmp(&b.author), order)),
        SortBy::Year => rows.sort_by(|a, b| match (a.year_num, b.year_num) {
            (None, None) => std::cmp::Ordering::Equal,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (Some(_), None) => std::cmp::Ordering::Less,
            (Some(a), Some(b)) => ordered(a.cmp(&b), order),
        }),
    }
    print_rows(rows, wrap_title);
    Ok(())
}
fn ordered(value: std::cmp::Ordering, order: Order) -> std::cmp::Ordering {
    if matches!(order, Order::Desc) {
        value.reverse()
    } else {
        value
    }
}

fn print_rows(rows: Vec<Row>, wrap_title: bool) {
    if rows.is_empty() {
        return;
    }
    let headers = ["Key", "Title", "Author", "Year"];
    let key_width = column_width(headers[0], rows.iter().map(|row| row.key.as_str()));
    let author_width = column_width(headers[2], rows.iter().map(|row| row.author.as_str()));
    let year_width = column_width(headers[3], rows.iter().map(|row| row.year.as_str()));
    let title_width = terminal_size::terminal_size_of(io::stdout()).map(|(width, _)| {
        (width.0 as usize)
            .saturating_sub(key_width + author_width + year_width + 6)
            .max(10)
    });
    let displayed_title_width = title_width
        .unwrap_or_else(|| column_width(headers[1], rows.iter().map(|row| row.title.as_str())));
    let header = format!(
        "{:<kw$}  {:<tw$}  {:<aw$}  {:<yw$}",
        headers[0],
        headers[1],
        headers[2],
        headers[3],
        kw = key_width,
        tw = displayed_title_width,
        aw = author_width,
        yw = year_width
    );
    if io::stdout().is_terminal() && env::var_os("NO_COLOR").is_none_or(|value| value.is_empty()) {
        let style = anstyle::Style::new()
            .bold()
            .fg_color(Some(anstyle::AnsiColor::Cyan.into()));
        println!("{style}{header}{style:#}");
    } else {
        println!("{header}");
    }
    for row in rows {
        let titles = match title_width {
            Some(width) if wrap_title => wrap(&row.title, width),
            Some(width) => vec![truncate(&row.title, width)],
            None => vec![row.title],
        };
        for (index, title) in titles.into_iter().enumerate() {
            println!(
                "{:<kw$}  {:<tw$}  {:<aw$}  {:<yw$}",
                if index == 0 { row.key.as_str() } else { "" },
                title,
                if index == 0 { row.author.as_str() } else { "" },
                if index == 0 { row.year.as_str() } else { "" },
                kw = key_width,
                tw = displayed_title_width,
                aw = author_width,
                yw = year_width
            );
        }
    }
}
fn column_width<'a>(header: &str, values: impl Iterator<Item = &'a str>) -> usize {
    values.fold(header.chars().count(), |width, value| {
        width.max(value.chars().count())
    })
}
fn truncate(value: &str, width: usize) -> String {
    let chars = value.chars().collect::<Vec<_>>();
    if chars.len() <= width {
        return value.into();
    }
    if width == 0 {
        return String::new();
    }
    chars[..width - 1]
        .iter()
        .chain(std::iter::once(&'…'))
        .collect()
}
fn wrap(value: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![value.into()];
    };
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in value.split_whitespace() {
        let len = word.chars().count();
        if len > width {
            if !current.is_empty() {
                lines.push(std::mem::take(&mut current));
            }
            lines.extend(
                word.chars()
                    .collect::<Vec<_>>()
                    .chunks(width)
                    .map(|c| c.iter().collect()),
            );
        } else if !current.is_empty() && current.chars().count() + 1 + len > width {
            lines.push(std::mem::take(&mut current));
            current.push_str(word);
        } else {
            if !current.is_empty() {
                current.push(' ')
            }
            current.push_str(word)
        }
    }
    if !current.is_empty() {
        lines.push(current)
    }
    if lines.is_empty() {
        lines.push(String::new())
    }
    lines
}

fn find_manifest(start: &Path) -> Result<PathBuf> {
    for directory in start.ancestors() {
        let candidate = directory.join(MANIFEST_FILE);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    bail!(
        "no cita.toml found in {} or its parents; run `cita init`",
        start.display()
    )
}

fn ensure_cache_layout(directory: &Path) -> Result<()> {
    let cache = directory.join(".cita/files");
    fs::create_dir_all(&cache)
        .with_context(|| format!("could not create document cache {}", cache.display()))?;
    let ignore_path = directory.join(".gitignore");
    let existing = match fs::read_to_string(&ignore_path) {
        Ok(v) => v,
        Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            return Err(e).with_context(|| format!("could not read {}", ignore_path.display()));
        }
    };
    if existing
        .lines()
        .any(|line| line.trim() == CACHE_IGNORE_RULE)
    {
        return Ok(());
    }
    let separator = if existing.is_empty() || existing.ends_with("\n\n") {
        ""
    } else if existing.ends_with('\n') {
        "\n"
    } else {
        "\n\n"
    };
    let mut ignore = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&ignore_path)
        .with_context(|| format!("could not open {}", ignore_path.display()))?;
    write!(
        ignore,
        "{separator}{CACHE_IGNORE_COMMENT}\n{CACHE_IGNORE_RULE}\n"
    )
    .with_context(|| format!("could not update {}", ignore_path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn key_only_accepts_one_locator() {
        assert!(Cli::try_parse_from(["cita", "add", "--key", "X", "1207.7214"]).is_ok());
    }
    #[test]
    fn document_conflicts_are_enforced() {
        assert!(
            Cli::try_parse_from(["cita", "fetch", "--force", "--cache-only", "1207.7214"]).is_err()
        );
        assert!(Cli::try_parse_from(["cita", "fetch", "--url", "--force", "1207.7214"]).is_err());
        assert!(
            Cli::try_parse_from(["cita", "fetch", "--url", "--cache-only", "1207.7214"]).is_err()
        );
        assert!(Cli::try_parse_from(["cita", "fetch", "--url", "--open", "1207.7214"]).is_ok());
        assert!(
            Cli::try_parse_from(["cita", "fetch", "--cache-only", "--open", "1207.7214"]).is_ok()
        );
        assert!(Cli::try_parse_from(["cita", "fetch", "--url", "--save", "1207.7214"]).is_ok());
        assert!(Cli::try_parse_from(["cita", "open", "1207.7214"]).is_err());
        assert!(Cli::try_parse_from(["cita", "fetch", "--dry-run", "1207.7214"]).is_err());
    }
    #[test]
    fn fetch_targets_render_the_value_passed_to_the_opener() {
        assert_eq!(
            FetchTarget::Url("https://arxiv.org/pdf/1207.7214".into()).to_string(),
            "https://arxiv.org/pdf/1207.7214"
        );
        assert_eq!(
            FetchTarget::Path(PathBuf::from("/tmp/1207.7214.pdf")).to_string(),
            "/tmp/1207.7214.pdf"
        );
    }
    #[test]
    fn unicode_title_helpers() {
        assert_eq!(truncate("αβγ", 2), "α…");
        assert_eq!(wrap("one two three", 7), ["one two", "three"]);
    }
}
