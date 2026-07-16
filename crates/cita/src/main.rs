mod git;

use anyhow::{Context, Result, bail};
use cita_core::{Locator, MetadataProvider, PaperRecord, ResolvedPaper, fallback_key};
use cita_documents::{
    DocumentStore, Error as DocumentError, FetchOutcome, FetchPolicy, arxiv_pdf_url,
};
use cita_inspire_client::InspireProvider;
use cita_manifest::{AddOutcome, Manifest, export_bibtex};
use clap::{CommandFactory, Parser, Subcommand};
use std::{
    env, fs,
    fs::OpenOptions,
    io::{IsTerminal, Write},
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
    /// Initialize cita.toml and the local document cache
    Init,
    /// Resolve and add one or more papers through INSPIRE
    Add {
        /// Override the citation key (only valid with one locator)
        #[arg(long)]
        key: Option<String>,
        /// Overwrite stored metadata when the paper is already present by identity
        #[arg(short = 'f', long)]
        force: bool,
        /// arXiv id, or an arxiv:, doi:, or inspire: locator
        #[arg(required = true)]
        locators: Vec<String>,
    },
    /// Remove papers by citation key or locator
    Remove {
        #[arg(required = true)]
        selectors: Vec<String>,
    },
    /// List stored papers
    List {
        /// Field to sort the listing by
        #[arg(long, value_enum, default_value_t = SortBy::Key)]
        sort_by: SortBy,
        /// Sort direction
        #[arg(long, value_enum, default_value_t = Order::Asc)]
        order: Order,
        /// Truncate long titles instead of wrapping them across lines
        #[arg(long)]
        no_wrap_title: bool,
    },
    /// Fetch a paper's arXiv PDF into the local cache
    Fetch {
        /// Download even if a cached PDF already exists
        #[arg(long, conflicts_with = "dry_run")]
        force: bool,
        /// Print the arXiv PDF URL without downloading it
        #[arg(long, conflicts_with = "save")]
        dry_run: bool,
        /// Persist new metadata before fetching; kept if the fetch later fails
        #[arg(long)]
        save: bool,
        /// Citation key or paper locator
        selector: String,
    },
    /// Fetch and open a paper's arXiv PDF
    Open {
        /// Download even if a cached PDF already exists
        #[arg(long, conflicts_with_all = ["browser", "no_download"])]
        force: bool,
        /// Open the arXiv PDF URL in a browser without downloading it
        #[arg(long, conflicts_with = "no_download")]
        browser: bool,
        /// Refuse to download the PDF if it is not already cached
        #[arg(long)]
        no_download: bool,
        /// Persist new metadata before opening; kept if the open later fails
        #[arg(long)]
        save: bool,
        /// Citation key or paper locator
        selector: String,
    },
    /// Export the committed manifest as a deterministic format
    Export {
        /// Write BibTeX to standard output
        #[arg(long, required = true)]
        bibtex: bool,
    },
    /// Commit only cita.toml to Git
    Commit,
}

/// Field to sort `cita list` by.
#[derive(Clone, Copy, Debug, Default, clap::ValueEnum)]
enum SortBy {
    #[default]
    Key,
    Title,
    Author,
    Year,
}

/// Sort direction for `cita list`.
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
        Some(Command::Add {
            key,
            force,
            locators,
        }) => add(&cwd, key.as_deref(), force, &locators).await?,
        Some(Command::Remove { selectors }) => remove(&cwd, &selectors)?,
        Some(Command::List {
            sort_by,
            order,
            no_wrap_title,
        }) => list(&cwd, sort_by, order, !no_wrap_title)?,
        Some(Command::Fetch {
            force,
            dry_run,
            save,
            selector,
        }) => {
            fetch(&cwd, &selector, force, dry_run, save).await?;
        }
        Some(Command::Open {
            force,
            browser,
            no_download,
            save,
            selector,
        }) => {
            open(&cwd, &selector, force, browser, no_download, save).await?;
        }
        Some(Command::Export { bibtex: true }) => export(&cwd)?,
        Some(Command::Export { bibtex: false }) => unreachable!("clap requires --bibtex"),
        Some(Command::Commit) => git::commit(&find_manifest(&cwd)?)?,
    }
    Ok(())
}

fn init(cwd: &Path) -> Result<()> {
    let directory = git::repository_root(cwd).unwrap_or_else(|| cwd.to_path_buf());
    let path = directory.join("cita.toml");
    let existed = path.exists();
    if existed {
        Manifest::load(&path)?;
    } else {
        Manifest::create(&path)?;
    }
    ensure_cache_layout(&directory)?;
    if existed {
        println!("Already initialized {}", path.display());
    } else {
        println!("Initialized {}", path.display());
    }
    Ok(())
}

fn ensure_cache_layout(directory: &Path) -> Result<()> {
    let cache = directory.join(".cita/files");
    fs::create_dir_all(&cache)
        .with_context(|| format!("could not create document cache {}", cache.display()))?;

    let ignore_path = directory.join(".gitignore");
    let existing = match fs::read_to_string(&ignore_path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => {
            return Err(error).with_context(|| format!("could not read {}", ignore_path.display()));
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

async fn add(cwd: &Path, key: Option<&str>, force: bool, values: &[String]) -> Result<()> {
    add_with(
        cwd,
        key,
        force,
        values,
        || Ok(InspireProvider::new()?),
        &mut std::io::stdout(),
    )
    .await
}

async fn add_with<P, F>(
    cwd: &Path,
    key: Option<&str>,
    force: bool,
    values: &[String],
    provider_factory: F,
    output: &mut impl Write,
) -> Result<()>
where
    P: MetadataProvider,
    F: FnOnce() -> Result<P>,
{
    if key.is_some() && values.len() != 1 {
        bail!("--key can only be used with one locator");
    }
    let path = find_manifest(cwd)?;
    let mut manifest = Manifest::load(&path)?;
    let provider = provider_factory()?;
    let outcomes = if let Some(key) = key {
        let locator = values[0].parse::<Locator>()?;
        let paper = provider.resolve(&locator).await?;
        vec![manifest.add(paper, Some(key), force)?]
    } else {
        let mut resolved = Vec::with_capacity(values.len());
        for value in values {
            let locator = value.parse::<Locator>()?;
            resolved.push(provider.resolve(&locator).await?);
        }
        manifest.add_batch(resolved, force)?
    };
    for outcome in outcomes {
        match outcome {
            AddOutcome::Added(key) => writeln!(output, "Added {key}")?,
            AddOutcome::Existing(key) => writeln!(output, "Already present: {key}")?,
            AddOutcome::Updated(key) => writeln!(output, "Updated {key}")?,
        }
    }
    Ok(())
}

fn remove(cwd: &Path, selectors: &[String]) -> Result<()> {
    let path = find_manifest(cwd)?;
    let mut manifest = Manifest::load(&path)?;
    for (key, _) in manifest.remove_batch(selectors)? {
        println!("Removed {key}");
    }
    Ok(())
}

struct Row {
    key: String,
    title: String,
    author: String,
    year_num: Option<i32>,
    year: String,
}

/// First author (`et al.` when there is more than one), falling back to the
/// first collaboration, then `—` when neither is present.
fn author_display(record: &PaperRecord) -> String {
    match record.authors.first() {
        Some(first) if record.authors.len() > 1 => format!("{first} et al."),
        Some(first) => first.clone(),
        None => match record.collaborations.first() {
            Some(collaboration) => collaboration.clone(),
            None => "—".to_string(),
        },
    }
}

/// Truncates `title` to at most `width` chars, replacing the tail with `…`
/// when it doesn't fit. Operates on chars (not bytes) so multibyte and TeX
/// titles (e.g. `$Z\to ee$`) are never split mid-codepoint.
fn truncate(title: &str, width: usize) -> String {
    let chars: Vec<char> = title.chars().collect();
    if chars.len() <= width {
        return title.to_string();
    }
    if width == 0 {
        return String::new();
    }
    let mut truncated: String = chars[..width - 1].iter().collect();
    truncated.push('…');
    truncated
}

/// Greedy word-wraps `title` into lines of at most `width` chars. A single
/// word longer than `width` is hard-broken across lines.
fn wrap(title: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![title.to_string()];
    }
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in title.split_whitespace() {
        let word_len = word.chars().count();
        if word_len > width {
            if !current.is_empty() {
                lines.push(std::mem::take(&mut current));
            }
            let chars: Vec<char> = word.chars().collect();
            for chunk in chars.chunks(width) {
                lines.push(chunk.iter().collect());
            }
            continue;
        }
        let candidate_len = if current.is_empty() {
            word_len
        } else {
            current.chars().count() + 1 + word_len
        };
        if candidate_len > width {
            lines.push(std::mem::take(&mut current));
            current.push_str(word);
        } else {
            if !current.is_empty() {
                current.push(' ');
            }
            current.push_str(word);
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

/// Applies `order` to a field comparison, leaving `Equal` untouched so ties
/// still fall back to the prior (stable) order.
fn ordered(comparison: std::cmp::Ordering, order: Order) -> std::cmp::Ordering {
    match order {
        Order::Asc => comparison,
        Order::Desc => comparison.reverse(),
    }
}

fn list(cwd: &Path, sort_by: SortBy, order: Order, wrap_title: bool) -> Result<()> {
    let manifest = Manifest::load(find_manifest(cwd)?)?;
    if manifest.papers().is_empty() {
        return Ok(());
    }
    let mut rows: Vec<Row> = manifest
        .papers()
        .iter()
        .map(|(key, record)| Row {
            key: key.clone(),
            title: record.title.clone(),
            author: author_display(record),
            year_num: record.year,
            year: record.year.map_or_else(|| "—".into(), |y| y.to_string()),
        })
        .collect();

    match sort_by {
        SortBy::Key => {
            if let Order::Desc = order {
                rows.reverse();
            }
        }
        SortBy::Title => rows.sort_by(|a, b| ordered(a.title.cmp(&b.title), order)),
        SortBy::Author => rows.sort_by(|a, b| ordered(a.author.cmp(&b.author), order)),
        // Missing years always sort last, independent of direction (like SQL's
        // NULLS LAST); only the comparison between two present years flips.
        SortBy::Year => rows.sort_by(|a, b| match (a.year_num, b.year_num) {
            (None, None) => std::cmp::Ordering::Equal,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (Some(_), None) => std::cmp::Ordering::Less,
            (Some(a_year), Some(b_year)) => ordered(a_year.cmp(&b_year), order),
        }),
    }

    let headers = ["Key", "Title", "Author", "Year"];
    let key_width = column_width(headers[0], rows.iter().map(|row| row.key.as_str()));
    let author_width = column_width(headers[2], rows.iter().map(|row| row.author.as_str()));
    let year_width = column_width(headers[3], rows.iter().map(|row| row.year.as_str()));

    // `terminal_size()` falls back to stderr/stdin, which would return a
    // width from the shell's tty even when stdout is piped (e.g. `cita list
    // | cat`). Query stdout specifically so piped output stays plain.
    let terminal_width =
        terminal_size::terminal_size_of(std::io::stdout()).map(|(width, _)| width.0 as usize);
    let title_width = match terminal_width {
        Some(width) => {
            let reserved = key_width + author_width + year_width + 6;
            Some(width.saturating_sub(reserved).max(10))
        }
        None => None,
    };
    let title_column_width = title_width
        .unwrap_or_else(|| column_width(headers[1], rows.iter().map(|row| row.title.as_str())));

    let color =
        std::io::stdout().is_terminal() && env::var_os("NO_COLOR").is_none_or(|v| v.is_empty());
    let header_line = format!(
        "{:<kw$}  {:<tw$}  {:<aw$}  {:<yw$}",
        headers[0],
        headers[1],
        headers[2],
        headers[3],
        kw = key_width,
        tw = title_column_width,
        aw = author_width,
        yw = year_width
    );
    if color {
        let style = anstyle::Style::new()
            .bold()
            .fg_color(Some(anstyle::AnsiColor::Cyan.into()));
        println!("{style}{header_line}{style:#}");
    } else {
        println!("{header_line}");
    }

    for row in rows {
        let title_lines = match title_width {
            Some(width) if wrap_title => wrap(&row.title, width),
            Some(width) => vec![truncate(&row.title, width)],
            None => vec![row.title],
        };
        let mut lines = title_lines.into_iter();
        if let Some(first_line) = lines.next() {
            println!(
                "{:<kw$}  {:<tw$}  {:<aw$}  {:<yw$}",
                row.key,
                first_line,
                row.author,
                row.year,
                kw = key_width,
                tw = title_column_width,
                aw = author_width,
                yw = year_width
            );
        }
        for continuation in lines {
            println!(
                "{:<kw$}  {:<tw$}  {:<aw$}  {:<yw$}",
                "",
                continuation,
                "",
                "",
                kw = key_width,
                tw = title_column_width,
                aw = author_width,
                yw = year_width
            );
        }
    }
    Ok(())
}

fn column_width<'a>(header: &str, values: impl Iterator<Item = &'a str>) -> usize {
    values.fold(header.chars().count(), |width, value| {
        width.max(value.chars().count())
    })
}

#[derive(Debug)]
enum PaperSelection {
    Stored { key: String, record: PaperRecord },
    Transient(ResolvedPaper),
}

#[derive(Debug)]
struct SelectedPaper {
    key: String,
    record: PaperRecord,
}

async fn fetch(cwd: &Path, selector: &str, force: bool, dry_run: bool, save: bool) -> Result<()> {
    let policy = if force {
        FetchPolicy::Force
    } else {
        FetchPolicy::UseCache
    };
    fetch_with(
        cwd,
        selector,
        policy,
        dry_run,
        save,
        || Ok(InspireProvider::new()?),
        (&mut std::io::stdout(), &mut std::io::stderr()),
    )
    .await
}

async fn open(
    cwd: &Path,
    selector: &str,
    force: bool,
    browser: bool,
    no_download: bool,
    save: bool,
) -> Result<()> {
    if browser {
        return open_in_browser_with(
            cwd,
            selector,
            save,
            || Ok(InspireProvider::new()?),
            |url| opener::open(url).map_err(anyhow::Error::from),
            (&mut std::io::stdout(), &mut std::io::stderr()),
        )
        .await;
    }
    let policy = if force {
        FetchPolicy::Force
    } else if no_download {
        FetchPolicy::CacheOnly
    } else {
        FetchPolicy::UseCache
    };
    open_with(
        cwd,
        selector,
        policy,
        save,
        || Ok(InspireProvider::new()?),
        |path| opener::open(path).map_err(anyhow::Error::from),
        (&mut std::io::stdout(), &mut std::io::stderr()),
    )
    .await
}

async fn select_paper_with<P, F>(
    manifest: &Manifest,
    selector: &str,
    provider_factory: F,
) -> Result<PaperSelection>
where
    P: MetadataProvider,
    F: FnOnce() -> Result<P>,
{
    if let Some((key, record)) = manifest.find_paper(selector) {
        return Ok(PaperSelection::Stored {
            key: key.to_owned(),
            record: record.clone(),
        });
    }

    let locator = selector.parse::<Locator>().map_err(|error| {
        anyhow::Error::from(error).context(format!("paper `{selector}` was not found"))
    })?;
    let provider = provider_factory()?;
    Ok(PaperSelection::Transient(provider.resolve(&locator).await?))
}

fn prepare_selection(
    manifest: &mut Manifest,
    selection: PaperSelection,
    selector: &str,
    save: bool,
    output: &mut impl Write,
    diagnostics: &mut impl Write,
) -> Result<SelectedPaper> {
    match selection {
        PaperSelection::Stored { key, record } => {
            if save {
                writeln!(output, "Already present: {key}")?;
            }
            Ok(SelectedPaper { key, record })
        }
        PaperSelection::Transient(resolved) => {
            if !save {
                let record = resolved.record.clone();
                let key = resolved
                    .suggested_key
                    .unwrap_or_else(|| fallback_key(&record));
                return Ok(SelectedPaper { key, record });
            }

            let resolved_record = resolved.record.clone();
            match manifest.add(resolved, None, false)? {
                AddOutcome::Added(key) => {
                    writeln!(output, "Added {key}")?;
                    Ok(SelectedPaper {
                        key,
                        record: resolved_record,
                    })
                }
                AddOutcome::Existing(key) => {
                    writeln!(output, "Already present: {key}")?;
                    let record = manifest
                        .papers()
                        .get(&key)
                        .expect("Existing key must be present")
                        .clone();
                    if record != resolved_record {
                        writeln!(
                            diagnostics,
                            "warning: stored metadata for `{key}` differs from INSPIRE; \
                             using fresh metadata for this action without changing cita.toml; \
                             run `cita add --force {selector}` to update it"
                        )?;
                    }
                    Ok(SelectedPaper {
                        key,
                        record: resolved_record,
                    })
                }
                AddOutcome::Updated(key) => {
                    writeln!(output, "Updated {key}")?;
                    Ok(SelectedPaper {
                        key,
                        record: resolved_record,
                    })
                }
            }
        }
    }
}

async fn select_for_action_with<P, F>(
    cwd: &Path,
    selector: &str,
    save: bool,
    provider_factory: F,
    output: &mut impl Write,
    diagnostics: &mut impl Write,
) -> Result<(PathBuf, SelectedPaper)>
where
    P: MetadataProvider,
    F: FnOnce() -> Result<P>,
{
    let manifest_path = find_manifest(cwd)?;
    let mut manifest = Manifest::load(&manifest_path)?;
    let selection = select_paper_with(&manifest, selector, provider_factory).await?;
    let selected = prepare_selection(
        &mut manifest,
        selection,
        selector,
        save,
        output,
        diagnostics,
    )?;
    Ok((manifest_path, selected))
}

async fn fetch_with<P, F>(
    cwd: &Path,
    selector: &str,
    policy: FetchPolicy,
    dry_run: bool,
    save: bool,
    provider_factory: F,
    writers: (&mut impl Write, &mut impl Write),
) -> Result<()>
where
    P: MetadataProvider,
    F: FnOnce() -> Result<P>,
{
    let (output, diagnostics) = writers;
    if dry_run && save {
        bail!("--dry-run cannot be used with --save");
    }
    let (manifest_path, selected) =
        select_for_action_with(cwd, selector, save, provider_factory, output, diagnostics).await?;
    let url = arxiv_pdf_url(&selected.record)?.to_string();
    if dry_run {
        writeln!(output, "{url}")?;
        writeln!(output, "[dry run] skipped download")?;
        return Ok(());
    }
    let outcome = fetch_selected(&manifest_path, &selected.record, policy).await?;
    writeln!(
        output,
        "{}",
        fetch_url_outcome_message(&selected.key, &url, &outcome)
    )?;
    Ok(())
}

async fn open_with<P, F>(
    cwd: &Path,
    selector: &str,
    policy: FetchPolicy,
    save: bool,
    provider_factory: F,
    launch: impl FnOnce(&Path) -> Result<()>,
    writers: (&mut impl Write, &mut impl Write),
) -> Result<()>
where
    P: MetadataProvider,
    F: FnOnce() -> Result<P>,
{
    let (output, diagnostics) = writers;
    let (manifest_path, selected) =
        select_for_action_with(cwd, selector, save, provider_factory, output, diagnostics).await?;
    let url = arxiv_pdf_url(&selected.record)?.to_string();
    let outcome = fetch_selected(&manifest_path, &selected.record, policy).await?;
    writeln!(
        output,
        "{}",
        fetch_url_outcome_message(&selected.key, &url, &outcome)
    )?;
    let path = outcome.path();
    launch(path).with_context(|| format!("could not open {}", path.display()))?;
    writeln!(output, "Opened {url}")?;
    Ok(())
}

async fn open_in_browser_with<P, F>(
    cwd: &Path,
    selector: &str,
    save: bool,
    provider_factory: F,
    launch: impl FnOnce(&str) -> Result<()>,
    writers: (&mut impl Write, &mut impl Write),
) -> Result<()>
where
    P: MetadataProvider,
    F: FnOnce() -> Result<P>,
{
    let (output, diagnostics) = writers;
    let (_, selected) =
        select_for_action_with(cwd, selector, save, provider_factory, output, diagnostics).await?;
    let url = arxiv_pdf_url(&selected.record)?;
    launch(url.as_str()).with_context(|| format!("could not open {url}"))?;
    writeln!(output, "Opened {url}")?;
    Ok(())
}

async fn fetch_selected(
    manifest_path: &Path,
    paper: &PaperRecord,
    policy: FetchPolicy,
) -> Result<FetchOutcome> {
    let project_root = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    if policy != FetchPolicy::CacheOnly {
        ensure_cache_layout(project_root)?;
    }
    let store = DocumentStore::new(project_root.join(".cita/files"))?;
    let outcome = store
        .fetch(paper, policy)
        .await
        .map_err(|error| document_error_with_hint(error, policy))?;
    Ok(outcome)
}

fn document_error_with_hint(error: DocumentError, policy: FetchPolicy) -> anyhow::Error {
    match error {
        error @ DocumentError::InvalidCachedPdf(_) if policy == FetchPolicy::CacheOnly => {
            anyhow::Error::from(error).context("drop --no-download and retry with --force")
        }
        error @ DocumentError::InvalidCachedPdf(_) => {
            anyhow::Error::from(error).context("retry with --force")
        }
        error @ DocumentError::NotCached(_) => {
            anyhow::Error::from(error).context("rerun without --no-download")
        }
        error => error.into(),
    }
}

fn fetch_url_outcome_message(key: &str, url: &str, outcome: &FetchOutcome) -> String {
    match outcome {
        FetchOutcome::Downloaded(_) => format!("Fetched {key}: {url}"),
        FetchOutcome::Cached(_) => format!("Already fetched {key}: {url}"),
    }
}

fn export(cwd: &Path) -> Result<()> {
    let manifest = Manifest::load(find_manifest(cwd)?)?;
    let output = export_bibtex(manifest.papers());
    std::io::stdout().write_all(output.as_bytes())?;
    Ok(())
}

fn find_manifest(start: &Path) -> Result<PathBuf> {
    for directory in start.ancestors() {
        let candidate = directory.join("cita.toml");
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    bail!(
        "no cita.toml found in {} or its parents; run `cita init`",
        start.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use cita_core::{INSPIRE_SOURCE, ProviderError};
    use std::{
        cell::RefCell,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
    };

    struct FakeProvider {
        paper: ResolvedPaper,
        calls: Arc<AtomicUsize>,
    }

    #[async_trait]
    impl MetadataProvider for FakeProvider {
        async fn resolve(&self, _: &Locator) -> Result<ResolvedPaper, ProviderError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.paper.clone())
        }
    }

    fn resolved(key: &str, source_id: &str, arxiv_id: Option<&str>) -> ResolvedPaper {
        ResolvedPaper {
            suggested_key: Some(key.into()),
            record: PaperRecord {
                title: "Resolved paper".into(),
                source: INSPIRE_SOURCE.into(),
                source_id: Some(source_id.into()),
                arxiv_ids: arxiv_id.into_iter().map(str::to_owned).collect(),
                ..PaperRecord::default()
            },
        }
    }

    fn fake_factory(
        paper: ResolvedPaper,
        factory_calls: Arc<AtomicUsize>,
        resolve_calls: Arc<AtomicUsize>,
    ) -> impl FnOnce() -> Result<FakeProvider> {
        move || {
            factory_calls.fetch_add(1, Ordering::SeqCst);
            Ok(FakeProvider {
                paper,
                calls: resolve_calls,
            })
        }
    }

    fn unused_provider() -> Result<FakeProvider> {
        panic!("stored selectors must not construct a provider")
    }

    #[tokio::test]
    async fn open_no_download_passes_the_cached_pdf_to_the_launcher() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join("cita.toml"),
            "schema = 1\n\n[papers.Example]\ntitle = 'Example'\nsource = 'inspire'\narxiv_ids = ['1207.7214']\n",
        )
        .unwrap();
        let pdf = directory.path().join(".cita/files/arxiv/1207.7214.pdf");
        fs::create_dir_all(pdf.parent().unwrap()).unwrap();
        fs::write(&pdf, b"%PDF-cached").unwrap();
        let launched = RefCell::new(None);
        let mut output = Vec::new();

        open_with(
            directory.path(),
            "Example",
            FetchPolicy::CacheOnly,
            false,
            unused_provider,
            |path| {
                launched.replace(Some(path.to_owned()));
                Ok(())
            },
            (&mut output, &mut std::io::sink()),
        )
        .await
        .unwrap();

        assert_eq!(launched.into_inner(), Some(pdf));
        assert_eq!(
            String::from_utf8(output).unwrap(),
            "Already fetched Example: https://arxiv.org/pdf/1207.7214\nOpened https://arxiv.org/pdf/1207.7214\n"
        );
    }

    #[tokio::test]
    async fn browser_open_passes_the_arxiv_pdf_url_to_the_launcher_without_creating_a_cache() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join("cita.toml"),
            "schema = 1\n\n[papers.Example]\ntitle = 'Example'\nsource = 'inspire'\narxiv_ids = ['hep-th/9901001']\n",
        )
        .unwrap();
        let launched = RefCell::new(None);

        let mut output = Vec::new();
        open_in_browser_with(
            directory.path(),
            "Example",
            false,
            unused_provider,
            |url| {
                launched.replace(Some(url.to_owned()));
                Ok(())
            },
            (&mut output, &mut std::io::sink()),
        )
        .await
        .unwrap();

        assert_eq!(
            launched.into_inner().as_deref(),
            Some("https://arxiv.org/pdf/hep-th/9901001")
        );
        assert!(!directory.path().join(".cita").exists());
        assert!(!directory.path().join(".gitignore").exists());
        assert_eq!(
            String::from_utf8(output).unwrap(),
            "Opened https://arxiv.org/pdf/hep-th/9901001\n"
        );
    }

    #[test]
    fn open_modes_are_mutually_exclusive() {
        for arguments in [
            vec!["cita", "open", "--force", "--browser", "Example"],
            vec!["cita", "open", "--force", "--no-download", "Example"],
            vec!["cita", "open", "--browser", "--no-download", "Example"],
        ] {
            assert!(Cli::try_parse_from(arguments).is_err());
        }
    }

    #[test]
    fn save_argument_rules_match_document_modes() {
        assert!(
            Cli::try_parse_from(["cita", "fetch", "--dry-run", "--save", "1207.7214"]).is_err()
        );
        for arguments in [
            vec!["cita", "open", "--save", "1207.7214"],
            vec!["cita", "open", "--save", "--browser", "1207.7214"],
            vec!["cita", "open", "--save", "--no-download", "1207.7214"],
            vec!["cita", "open", "--save", "--force", "1207.7214"],
        ] {
            assert!(Cli::try_parse_from(arguments).is_ok());
        }
    }

    #[test]
    fn save_help_explains_when_metadata_is_persisted() {
        let command = Cli::command();
        for subcommand in ["fetch", "open"] {
            let help = command
                .find_subcommand(subcommand)
                .unwrap()
                .get_arguments()
                .find(|argument| argument.get_id() == "save")
                .unwrap()
                .get_help()
                .unwrap()
                .to_string();
            assert!(help.contains("before"), "{subcommand}: {help}");
            assert!(help.contains("later fails"), "{subcommand}: {help}");
        }
    }

    #[test]
    fn add_accepts_force_flag() {
        for arguments in [
            vec!["cita", "add", "--force", "1207.7214"],
            vec!["cita", "add", "-f", "1207.7214"],
            vec!["cita", "add", "--force", "--key", "Custom", "1207.7214"],
        ] {
            assert!(Cli::try_parse_from(arguments).is_ok());
        }
    }

    #[tokio::test]
    async fn add_force_refreshes_stored_metadata_through_the_cli_wiring() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("cita.toml");
        let mut stored = resolved("Existing", "1", Some("1207.7214"));
        stored.record.title = "Stored metadata".into();
        let mut manifest = Manifest::create(&path).unwrap();
        manifest.add(stored, Some("Existing"), false).unwrap();

        let mut refreshed = resolved("Suggested", "1", Some("1207.7214"));
        refreshed.record.title = "Fresh metadata".into();
        let factory_calls = Arc::new(AtomicUsize::new(0));
        let resolve_calls = Arc::new(AtomicUsize::new(0));
        let mut output = Vec::new();
        add_with(
            directory.path(),
            None,
            true,
            &["1207.7214".into()],
            fake_factory(
                refreshed.clone(),
                factory_calls.clone(),
                resolve_calls.clone(),
            ),
            &mut output,
        )
        .await
        .unwrap();

        assert_eq!(factory_calls.load(Ordering::SeqCst), 1);
        assert_eq!(resolve_calls.load(Ordering::SeqCst), 1);
        let reloaded = Manifest::load(path).unwrap();
        assert_eq!(reloaded.papers()["Existing"], refreshed.record);
        assert!(!reloaded.papers().contains_key("Suggested"));
        assert_eq!(String::from_utf8(output).unwrap(), "Updated Existing\n");
    }

    #[tokio::test]
    async fn selection_constructs_the_provider_only_for_valid_manifest_misses() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("cita.toml");
        fs::write(
            &path,
            "schema = 1\n\n[papers.Example]\ntitle = 'Example'\nsource = 'inspire'\narxiv_ids = ['1207.7214']\n",
        )
        .unwrap();
        let manifest = Manifest::load(path).unwrap();

        let stored = select_paper_with(&manifest, "Example", unused_provider)
            .await
            .unwrap();
        assert!(matches!(
            stored,
            PaperSelection::Stored { key, .. } if key == "Example"
        ));

        let stored_by_locator = select_paper_with(&manifest, "1207.7214", unused_provider)
            .await
            .unwrap();
        assert!(matches!(
            stored_by_locator,
            PaperSelection::Stored { key, .. } if key == "Example"
        ));

        let factory_calls = Arc::new(AtomicUsize::new(0));
        let resolve_calls = Arc::new(AtomicUsize::new(0));
        let transient = select_paper_with(
            &manifest,
            "2401.00001",
            fake_factory(
                resolved("New", "2", Some("2401.00001")),
                factory_calls.clone(),
                resolve_calls.clone(),
            ),
        )
        .await
        .unwrap();
        assert!(matches!(transient, PaperSelection::Transient(_)));
        assert_eq!(factory_calls.load(Ordering::SeqCst), 1);
        assert_eq!(resolve_calls.load(Ordering::SeqCst), 1);

        let factory_calls = Arc::new(AtomicUsize::new(0));
        let error = select_paper_with(
            &manifest,
            "not-a-key-or-locator",
            fake_factory(
                resolved("Unused", "3", None),
                factory_calls.clone(),
                Arc::new(AtomicUsize::new(0)),
            ),
        )
        .await
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("paper `not-a-key-or-locator` was not found")
        );
        assert!(format!("{error:#}").contains("invalid locator"));
        assert_eq!(factory_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn transient_dry_run_resolves_once_and_does_not_change_the_manifest() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("cita.toml");
        Manifest::create(&path).unwrap();
        let before = fs::read(&path).unwrap();
        let factory_calls = Arc::new(AtomicUsize::new(0));
        let resolve_calls = Arc::new(AtomicUsize::new(0));
        let mut output = Vec::new();

        fetch_with(
            directory.path(),
            "1207.7214",
            FetchPolicy::UseCache,
            true,
            false,
            fake_factory(
                resolved("Resolved:2012", "1", Some("1207.7214")),
                factory_calls.clone(),
                resolve_calls.clone(),
            ),
            (&mut output, &mut std::io::sink()),
        )
        .await
        .unwrap();

        assert_eq!(factory_calls.load(Ordering::SeqCst), 1);
        assert_eq!(resolve_calls.load(Ordering::SeqCst), 1);
        assert_eq!(fs::read(path).unwrap(), before);
        assert_eq!(
            String::from_utf8(output).unwrap(),
            "https://arxiv.org/pdf/1207.7214\n[dry run] skipped download\n"
        );
        assert!(!directory.path().join(".cita").exists());
    }

    #[tokio::test]
    async fn transient_fetch_reuses_a_prepopulated_cache_without_saving() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("cita.toml");
        Manifest::create(&path).unwrap();
        let before = fs::read(&path).unwrap();
        let pdf = directory.path().join(".cita/files/arxiv/1207.7214.pdf");
        fs::create_dir_all(pdf.parent().unwrap()).unwrap();
        fs::write(&pdf, b"%PDF-cached").unwrap();
        let mut output = Vec::new();

        fetch_with(
            directory.path(),
            "1207.7214",
            FetchPolicy::UseCache,
            false,
            false,
            fake_factory(
                resolved("Resolved:2012", "1", Some("1207.7214")),
                Arc::new(AtomicUsize::new(0)),
                Arc::new(AtomicUsize::new(0)),
            ),
            (&mut output, &mut std::io::sink()),
        )
        .await
        .unwrap();

        assert_eq!(fs::read(path).unwrap(), before);
        assert_eq!(
            String::from_utf8(output).unwrap(),
            "Already fetched Resolved:2012: https://arxiv.org/pdf/1207.7214\n"
        );
    }

    #[tokio::test]
    async fn transient_local_and_browser_open_use_injected_launchers() {
        let directory = tempfile::tempdir().unwrap();
        Manifest::create(directory.path().join("cita.toml")).unwrap();
        let pdf = directory.path().join(".cita/files/arxiv/1207.7214.pdf");
        fs::create_dir_all(pdf.parent().unwrap()).unwrap();
        fs::write(&pdf, b"%PDF-cached").unwrap();
        let local_launch = RefCell::new(None);
        let mut local_output = Vec::new();

        open_with(
            directory.path(),
            "1207.7214",
            FetchPolicy::CacheOnly,
            false,
            fake_factory(
                resolved("Resolved:2012", "1", Some("1207.7214")),
                Arc::new(AtomicUsize::new(0)),
                Arc::new(AtomicUsize::new(0)),
            ),
            |path| {
                local_launch.replace(Some(path.to_owned()));
                Ok(())
            },
            (&mut local_output, &mut std::io::sink()),
        )
        .await
        .unwrap();
        assert_eq!(local_launch.into_inner(), Some(pdf));

        let browser_launch = RefCell::new(None);
        let mut browser_output = Vec::new();
        open_in_browser_with(
            directory.path(),
            "2401.00001",
            false,
            fake_factory(
                resolved("Resolved:2024", "2", Some("2401.00001")),
                Arc::new(AtomicUsize::new(0)),
                Arc::new(AtomicUsize::new(0)),
            ),
            |url| {
                browser_launch.replace(Some(url.to_owned()));
                Ok(())
            },
            (&mut browser_output, &mut std::io::sink()),
        )
        .await
        .unwrap();
        assert_eq!(
            browser_launch.into_inner().as_deref(),
            Some("https://arxiv.org/pdf/2401.00001")
        );
        assert_eq!(
            String::from_utf8(browser_output).unwrap(),
            "Opened https://arxiv.org/pdf/2401.00001\n"
        );
    }

    #[tokio::test]
    async fn save_adds_before_the_action_and_uses_the_saved_key() {
        let directory = tempfile::tempdir().unwrap();
        Manifest::create(directory.path().join("cita.toml")).unwrap();
        let pdf = directory.path().join(".cita/files/arxiv/1207.7214.pdf");
        fs::create_dir_all(pdf.parent().unwrap()).unwrap();
        fs::write(pdf, b"%PDF-cached").unwrap();
        let mut output = Vec::new();

        fetch_with(
            directory.path(),
            "1207.7214",
            FetchPolicy::UseCache,
            false,
            true,
            fake_factory(
                resolved("Saved:2012", "1", Some("1207.7214")),
                Arc::new(AtomicUsize::new(0)),
                Arc::new(AtomicUsize::new(0)),
            ),
            (&mut output, &mut std::io::sink()),
        )
        .await
        .unwrap();

        assert!(
            Manifest::load(directory.path().join("cita.toml"))
                .unwrap()
                .find_paper("Saved:2012")
                .is_some()
        );
        assert_eq!(
            String::from_utf8(output).unwrap(),
            "Added Saved:2012\nAlready fetched Saved:2012: https://arxiv.org/pdf/1207.7214\n"
        );
    }

    #[tokio::test]
    async fn save_reports_identity_overlap_with_the_existing_key() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("cita.toml");
        fs::write(
            &path,
            "schema = 1\n\n[papers.Existing]\ntitle = 'Existing'\nsource = 'inspire'\ndois = ['10.1000/existing']\n",
        )
        .unwrap();
        let before = fs::read(&path).unwrap();
        let mut overlap = resolved("Suggested", "unused", Some("2401.00001"));
        overlap.record.source_id = None;
        overlap.record.dois = vec!["10.1000/EXISTING".into()];
        let mut output = Vec::new();
        let mut diagnostics = Vec::new();

        let (_, selected) = select_for_action_with(
            directory.path(),
            "2401.00001",
            true,
            fake_factory(
                overlap,
                Arc::new(AtomicUsize::new(0)),
                Arc::new(AtomicUsize::new(0)),
            ),
            &mut output,
            &mut diagnostics,
        )
        .await
        .unwrap();

        assert_eq!(selected.key, "Existing");
        assert_eq!(selected.record.arxiv_ids, vec!["2401.00001"]);
        assert_eq!(selected.record.title, "Resolved paper");
        assert_eq!(
            String::from_utf8(output).unwrap(),
            "Already present: Existing\n"
        );
        assert_eq!(
            String::from_utf8(diagnostics).unwrap(),
            "warning: stored metadata for `Existing` differs from INSPIRE; \
             using fresh metadata for this action without changing cita.toml; \
             run `cita add --force 2401.00001` to update it\n"
        );
        assert_eq!(fs::read(path).unwrap(), before);
    }

    #[tokio::test]
    async fn save_identity_overlap_with_equal_metadata_does_not_warn() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("cita.toml");
        let mut existing = resolved("Existing", "1", Some("2401.00001"));
        existing.record.title = "Same".into();
        let mut manifest = Manifest::create(&path).unwrap();
        manifest
            .add(existing.clone(), Some("Existing"), false)
            .unwrap();
        let before = fs::read(&path).unwrap();

        let mut overlap = existing;
        overlap.suggested_key = Some("Suggested".into());
        let mut output = Vec::new();
        let mut diagnostics = Vec::new();

        let (_, selected) = select_for_action_with(
            directory.path(),
            "2401.00001",
            true,
            fake_factory(
                overlap,
                Arc::new(AtomicUsize::new(0)),
                Arc::new(AtomicUsize::new(0)),
            ),
            &mut output,
            &mut diagnostics,
        )
        .await
        .unwrap();

        assert_eq!(selected.key, "Existing");
        assert_eq!(selected.record.title, "Same");
        assert_eq!(
            String::from_utf8(output).unwrap(),
            "Already present: Existing\n"
        );
        assert!(diagnostics.is_empty());
        assert_eq!(fs::read(path).unwrap(), before);
    }

    #[tokio::test]
    async fn saved_metadata_remains_after_cache_missing_arxiv_and_launcher_failures() {
        let directory = tempfile::tempdir().unwrap();
        Manifest::create(directory.path().join("cita.toml")).unwrap();
        let mut output = Vec::new();
        let cache_error = open_with(
            directory.path(),
            "1207.7214",
            FetchPolicy::CacheOnly,
            true,
            fake_factory(
                resolved("CacheMiss", "1", Some("1207.7214")),
                Arc::new(AtomicUsize::new(0)),
                Arc::new(AtomicUsize::new(0)),
            ),
            |_| panic!("cache miss must not launch"),
            (&mut output, &mut std::io::sink()),
        )
        .await
        .unwrap_err();
        assert!(format!("{cache_error:#}").contains("PDF is not cached"));

        let mut output = Vec::new();
        let arxiv_error = open_in_browser_with(
            directory.path(),
            "2401.00001",
            true,
            fake_factory(
                resolved("NoArxiv", "2", None),
                Arc::new(AtomicUsize::new(0)),
                Arc::new(AtomicUsize::new(0)),
            ),
            |_| panic!("missing arXiv id must not launch"),
            (&mut output, &mut std::io::sink()),
        )
        .await
        .unwrap_err();
        assert!(format!("{arxiv_error:#}").contains("paper has no arXiv identifier"));

        let mut output = Vec::new();
        let launcher_error = open_in_browser_with(
            directory.path(),
            "2501.00001",
            true,
            fake_factory(
                resolved("LaunchFail", "3", Some("2501.00001")),
                Arc::new(AtomicUsize::new(0)),
                Arc::new(AtomicUsize::new(0)),
            ),
            |_| bail!("launcher failed"),
            (&mut output, &mut std::io::sink()),
        )
        .await
        .unwrap_err();
        assert!(format!("{launcher_error:#}").contains("launcher failed"));

        let manifest = Manifest::load(directory.path().join("cita.toml")).unwrap();
        for key in ["CacheMiss", "NoArxiv", "LaunchFail"] {
            assert!(manifest.find_paper(key).is_some(), "missing {key}");
        }
    }

    #[tokio::test]
    async fn insertion_failure_prevents_the_document_action() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join("cita.toml"),
            "schema = 1\n\n[papers.Conflict]\ntitle = 'Existing'\nsource = 'inspire'\nsource_id = '1'\n",
        )
        .unwrap();
        let launches = Arc::new(AtomicUsize::new(0));
        let launched = launches.clone();
        let mut output = Vec::new();

        let error = open_in_browser_with(
            directory.path(),
            "2401.00001",
            true,
            fake_factory(
                resolved("Conflict", "2", Some("2401.00001")),
                Arc::new(AtomicUsize::new(0)),
                Arc::new(AtomicUsize::new(0)),
            ),
            move |_| {
                launched.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
            (&mut output, &mut std::io::sink()),
        )
        .await
        .unwrap_err();

        assert!(format!("{error:#}").contains("citation key conflict"));
        assert_eq!(launches.load(Ordering::SeqCst), 0);
        assert!(output.is_empty());
    }

    #[test]
    fn list_accepts_no_wrap_title_flag() {
        let Command::List {
            sort_by: _,
            order: _,
            no_wrap_title,
        } = Cli::try_parse_from(["cita", "list", "--no-wrap-title"])
            .unwrap()
            .command
            .unwrap()
        else {
            panic!("expected List command");
        };
        assert!(no_wrap_title);
    }

    #[test]
    fn list_order_defaults_to_ascending() {
        let Command::List { order, .. } = Cli::try_parse_from(["cita", "list"])
            .unwrap()
            .command
            .unwrap()
        else {
            panic!("expected List command");
        };
        assert!(matches!(order, Order::Asc));
    }

    #[test]
    fn list_accepts_order_desc() {
        let Command::List { order, .. } = Cli::try_parse_from(["cita", "list", "--order", "desc"])
            .unwrap()
            .command
            .unwrap()
        else {
            panic!("expected List command");
        };
        assert!(matches!(order, Order::Desc));
    }

    #[test]
    fn truncate_passes_short_titles_through_unchanged() {
        assert_eq!(truncate("A short title", 20), "A short title");
        assert_eq!(truncate("Exactly ten", 11), "Exactly ten");
    }

    #[test]
    fn truncate_ellipsizes_long_titles() {
        assert_eq!(truncate("A very long title indeed", 10), "A very lo…");
    }

    #[test]
    fn truncate_is_multibyte_and_tex_safe() {
        // "$Z\to ee$" contains only ASCII, but exercise a genuine multibyte
        // title (é) alongside TeX markup to ensure char, not byte, slicing.
        assert_eq!(truncate("Café $Z\\to ee$", 6), "Café …");
    }

    #[test]
    fn wrap_breaks_on_word_boundaries() {
        assert_eq!(
            wrap("the quick brown fox", 10),
            vec!["the quick".to_string(), "brown fox".to_string()]
        );
    }

    #[test]
    fn wrap_hard_breaks_an_overlong_word() {
        assert_eq!(
            wrap("supercalifragilisticexpialidocious", 10),
            vec![
                "supercalif".to_string(),
                "ragilistic".to_string(),
                "expialidoc".to_string(),
                "ious".to_string()
            ]
        );
    }

    #[test]
    fn author_display_shows_single_author() {
        let record = PaperRecord {
            authors: vec!["Higgs, Peter".to_string()],
            ..Default::default()
        };
        assert_eq!(author_display(&record), "Higgs, Peter");
    }

    #[test]
    fn author_display_shows_et_al_for_multiple_authors() {
        let record = PaperRecord {
            authors: vec!["Higgs, Peter".to_string(), "Englert, Francois".to_string()],
            ..Default::default()
        };
        assert_eq!(author_display(&record), "Higgs, Peter et al.");
    }

    #[test]
    fn author_display_falls_back_to_collaboration() {
        let record = PaperRecord {
            collaborations: vec!["ATLAS Collaboration".to_string()],
            ..Default::default()
        };
        assert_eq!(author_display(&record), "ATLAS Collaboration");
    }

    #[test]
    fn author_display_falls_back_to_placeholder() {
        assert_eq!(author_display(&PaperRecord::default()), "—");
    }
}
