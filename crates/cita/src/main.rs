mod git;

use anyhow::{Context, Result, bail};
use cita_bibliography::{
    AddOutcome, Bibliography, Entry, Error as BibliographyError, parse, rename_entry,
};
use cita_core::Locator;
use cita_documents::{
    DocumentStore, Error as DocumentError, FetchOutcome, FetchPolicy, arxiv_pdf_url,
};
use cita_inspire_client::Client;
use clap::{CommandFactory, Parser, Subcommand};
use std::{
    collections::{HashMap, HashSet},
    env, fs,
    fs::OpenOptions,
    io::{IsTerminal, Write},
    path::{Path, PathBuf},
};

const BIBLIOGRAPHY_FILE: &str = "references.bib";
const LEGACY_FILE: &str = "cita.toml";
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
    /// Initialize references.bib and the local document cache
    Init,
    /// Resolve and add one or more papers through INSPIRE
    Add {
        /// arXiv id, or an arxiv:, doi:, or inspire: locator
        #[arg(required = true)]
        locators: Vec<String>,
    },
    /// Refresh every stored entry from INSPIRE
    Sync,
    /// Remove papers by texkey or locator
    Remove {
        #[arg(required = true)]
        selectors: Vec<String>,
    },
    /// List stored papers
    List {
        #[arg(long, value_enum, default_value_t = SortBy::Key)]
        sort_by: SortBy,
        #[arg(long, value_enum, default_value_t = Order::Asc)]
        order: Order,
        #[arg(long)]
        no_wrap_title: bool,
    },
    /// Fetch a paper's arXiv PDF into the local cache
    Fetch {
        #[arg(long, conflicts_with = "dry_run")]
        force: bool,
        #[arg(long, conflicts_with = "save")]
        dry_run: bool,
        #[arg(long)]
        save: bool,
        selector: String,
    },
    /// Fetch and open a paper's arXiv PDF
    Open {
        #[arg(long, conflicts_with_all = ["browser", "no_download"])]
        force: bool,
        #[arg(long, conflicts_with = "no_download")]
        browser: bool,
        #[arg(long)]
        no_download: bool,
        #[arg(long)]
        save: bool,
        selector: String,
    },
    /// Commit only references.bib to Git
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
        Some(Command::Add { locators }) => add(&cwd, &locators).await?,
        Some(Command::Sync) => sync(&cwd).await?,
        Some(Command::Remove { selectors }) => remove(&cwd, &selectors).await?,
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
            let policy = if force {
                FetchPolicy::Force
            } else {
                FetchPolicy::UseCache
            };
            fetch(&cwd, &selector, policy, dry_run, save).await?;
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
        Some(Command::Commit) => git::commit(&find_bibliography(&cwd)?)?,
    }
    Ok(())
}

fn init(cwd: &Path) -> Result<()> {
    let directory = git::repository_root(cwd).unwrap_or_else(|| cwd.to_path_buf());
    let path = directory.join(BIBLIOGRAPHY_FILE);
    let existed = path.exists();
    if existed {
        Bibliography::load(&path)?;
    } else {
        reject_legacy_only(&directory)?;
        Bibliography::create(&path)?;
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

fn inspire_client() -> Result<Client> {
    let builder = Client::builder();
    match env::var("CITA_INSPIRE_BASE_URL") {
        Ok(base) => Ok(builder.base_url(base).build()?),
        Err(_) => Ok(builder.build()?),
    }
}

async fn add(cwd: &Path, values: &[String]) -> Result<()> {
    let locators = values
        .iter()
        .map(|value| value.parse::<Locator>())
        .collect::<Result<Vec<_>, _>>()?;
    let client = inspire_client()?;
    let mut bodies = Vec::with_capacity(locators.len());
    for locator in &locators {
        bodies.push(client.lookup(locator).await?);
    }
    let mut bibliography = Bibliography::load(find_bibliography(cwd)?)?;
    for outcome in bibliography.add_batch(&bodies)? {
        match outcome {
            AddOutcome::Added(key) => println!("Added {key}"),
            AddOutcome::Existing(key) => println!("Already present: {key}"),
        }
    }
    Ok(())
}

async fn sync(cwd: &Path) -> Result<()> {
    let mut bibliography = Bibliography::load(find_bibliography(cwd)?)?;
    let keys = bibliography.entries().keys().cloned().collect::<Vec<_>>();
    let client = inspire_client()?;
    let responses = client.lookup_keys(&keys).await?;
    let mut refreshed = Vec::with_capacity(keys.len());
    let mut primary_owner: HashMap<String, String> = HashMap::new();
    let mut warnings = Vec::new();

    for response in responses {
        let mut returned = parse(&response.bibtex)?;
        let requested: HashSet<&str> = response.requested_keys.iter().map(String::as_str).collect();
        let exact = response
            .requested_keys
            .iter()
            .filter(|key| returned.contains_key(key.as_str()))
            .cloned()
            .collect::<HashSet<_>>();
        for key in &response.requested_keys {
            if exact.contains(key) {
                let entry = returned.remove(key).expect("exact key exists");
                record_primary(&mut primary_owner, key, key)?;
                refreshed.push(entry.raw().to_owned());
                continue;
            }
            let single_body = client
                .lookup_key(key)
                .await
                .with_context(|| format!("could not reconcile stored texkey `{key}`"))?;
            let single = parse(&single_body)?;
            if single.len() != 1 {
                bail!(
                    "INSPIRE texkey `{key}` resolved to {} entries; sync is ambiguous",
                    single.len()
                );
            }
            let (new_key, _) = single.into_iter().next().expect("length checked");
            if requested.contains(new_key.as_str()) && exact.contains(&new_key) {
                bail!(
                    "stored texkeys `{key}` and `{new_key}` resolve to the same current INSPIRE record"
                );
            }
            record_primary(&mut primary_owner, &new_key, key)?;
            let entry = returned.remove(&new_key).ok_or_else(|| {
                anyhow::anyhow!(
                    "INSPIRE batch did not return `{new_key}` while reconciling `{key}`"
                )
            })?;
            refreshed.push(rename_entry(entry.raw(), key)?);
            warnings.push((new_key, key.clone()));
        }
        if !returned.is_empty() {
            bail!(
                "INSPIRE returned unexplained entries: {}",
                returned.keys().cloned().collect::<Vec<_>>().join(", ")
            );
        }
    }
    if refreshed.len() != keys.len() {
        bail!(
            "INSPIRE returned {} of {} requested references",
            refreshed.len(),
            keys.len()
        );
    }
    let changed = bibliography.replace_all(&refreshed)?;
    for (new_key, old_key) in warnings {
        eprintln!("warning: INSPIRE now prefers {new_key} for {old_key}; preserving {old_key}");
    }
    if changed {
        println!("Synced {} references", keys.len());
    } else {
        println!("Already in sync");
    }
    Ok(())
}

fn record_primary(owners: &mut HashMap<String, String>, primary: &str, local: &str) -> Result<()> {
    if let Some(other) = owners.insert(primary.to_owned(), local.to_owned())
        && other != local
    {
        bail!(
            "stored texkeys `{other}` and `{local}` resolve to the same current INSPIRE record `{primary}`"
        );
    }
    Ok(())
}

async fn remove(cwd: &Path, selectors: &[String]) -> Result<()> {
    let path = find_bibliography(cwd)?;
    let mut bibliography = Bibliography::load(&path)?;
    let client = inspire_client()?;
    let mut keys = Vec::with_capacity(selectors.len());
    for selector in selectors {
        if let Some(entry) = bibliography.find(selector) {
            keys.push(entry.key().to_owned());
            continue;
        }
        let locator = selector.parse::<Locator>().map_err(|error| {
            anyhow::Error::from(error).context(format!("paper `{selector}` was not found"))
        })?;
        let body = client.lookup(&locator).await?;
        let resolved = exactly_one(&body)?;
        if bibliography.entries().contains_key(resolved.key()) {
            keys.push(resolved.key().to_owned());
        } else {
            return Err(BibliographyError::PaperNotFound(selector.clone()).into());
        }
    }
    for entry in bibliography.remove_batch(&keys)? {
        println!("Removed {}", entry.key());
    }
    Ok(())
}

fn exactly_one(body: &str) -> Result<Entry> {
    let parsed = parse(body)?;
    if parsed.len() != 1 {
        bail!("INSPIRE returned {} entries instead of one", parsed.len());
    }
    Ok(parsed.into_values().next().expect("length checked"))
}

#[derive(Clone)]
struct Selected {
    entry: Entry,
}

async fn select(cwd: &Path, selector: &str, save: bool) -> Result<(PathBuf, Selected)> {
    let path = find_bibliography(cwd)?;
    let mut bibliography = Bibliography::load(&path)?;
    if let Some(entry) = bibliography.find(selector).cloned() {
        if save {
            println!("Already present: {}", entry.key());
        }
        return Ok((path, Selected { entry }));
    }
    let locator = selector.parse::<Locator>().map_err(|error| {
        anyhow::Error::from(error).context(format!("paper `{selector}` was not found"))
    })?;
    let body = inspire_client()?.lookup(&locator).await?;
    let fresh = exactly_one(&body)?;
    if save {
        let old = bibliography.entries().get(fresh.key()).cloned();
        match bibliography
            .add_batch(std::slice::from_ref(&body))?
            .pop()
            .expect("one result")
        {
            AddOutcome::Added(key) => println!("Added {key}"),
            AddOutcome::Existing(key) => {
                println!("Already present: {key}");
                if old.as_ref().is_some_and(|entry| entry.raw() != fresh.raw()) {
                    eprintln!(
                        "warning: stored data for `{key}` differs from INSPIRE; using fresh data for this action without changing references.bib; run `cita sync`"
                    );
                }
            }
        }
    }
    Ok((path, Selected { entry: fresh }))
}

async fn fetch(
    cwd: &Path,
    selector: &str,
    policy: FetchPolicy,
    dry_run: bool,
    save: bool,
) -> Result<()> {
    if dry_run && save {
        bail!("--dry-run cannot be used with --save");
    }
    let (path, selected) = select(cwd, selector, save).await?;
    let arxiv = selected.entry.first_arxiv().ok_or_else(|| {
        anyhow::anyhow!(
            "paper `{}` has no arXiv eprint, so no PDF can be fetched",
            selected.entry.key()
        )
    })?;
    let url = arxiv_pdf_url(arxiv)?.to_string();
    if dry_run {
        println!("{url}");
        println!("[dry run] skipped download");
        return Ok(());
    }
    let outcome = fetch_selected(&path, arxiv, policy).await?;
    println!("{}", fetch_message(selected.entry.key(), &url, &outcome));
    Ok(())
}

async fn open(
    cwd: &Path,
    selector: &str,
    force: bool,
    browser: bool,
    no_download: bool,
    save: bool,
) -> Result<()> {
    let (path, selected) = select(cwd, selector, save).await?;
    let arxiv = selected.entry.first_arxiv().ok_or_else(|| {
        anyhow::anyhow!(
            "paper `{}` has no arXiv eprint, so no PDF can be opened",
            selected.entry.key()
        )
    })?;
    let url = arxiv_pdf_url(arxiv)?;
    if browser {
        opener::open(url.as_str()).with_context(|| format!("could not open {url}"))?;
        println!("Opened {url}");
        return Ok(());
    }
    let policy = if force {
        FetchPolicy::Force
    } else if no_download {
        FetchPolicy::CacheOnly
    } else {
        FetchPolicy::UseCache
    };
    let outcome = fetch_selected(&path, arxiv, policy).await?;
    println!(
        "{}",
        fetch_message(selected.entry.key(), url.as_str(), &outcome)
    );
    opener::open(outcome.path())
        .with_context(|| format!("could not open {}", outcome.path().display()))?;
    println!("Opened {url}");
    Ok(())
}

async fn fetch_selected(path: &Path, arxiv: &str, policy: FetchPolicy) -> Result<FetchOutcome> {
    let root = path.parent().unwrap_or_else(|| Path::new("."));
    if policy != FetchPolicy::CacheOnly {
        ensure_cache_layout(root)?;
    }
    let store = DocumentStore::new(root.join(".cita/files"))?;
    store
        .fetch(arxiv, policy)
        .await
        .map_err(|error| document_error_with_hint(error, policy))
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
    let bibliography = Bibliography::load(find_bibliography(cwd)?)?;
    let mut rows = bibliography
        .entries()
        .values()
        .map(|entry| {
            let author = match entry.authors().first() {
                Some(first) if entry.authors().len() > 1 => format!("{first} et al."),
                Some(first) => first.clone(),
                None => "—".into(),
            };
            Row {
                key: entry.key().into(),
                title: entry.title().into(),
                author,
                year_num: entry.year(),
                year: entry
                    .year()
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
    match order {
        Order::Asc => value,
        Order::Desc => value.reverse(),
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
    let terminal_width =
        terminal_size::terminal_size_of(std::io::stdout()).map(|(width, _)| width.0 as usize);
    let title_width = terminal_width.map(|width| {
        width
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
    if std::io::stdout().is_terminal()
        && env::var_os("NO_COLOR").is_none_or(|value| value.is_empty())
    {
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
    }
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in value.split_whitespace() {
        let word_len = word.chars().count();
        if word_len > width {
            if !current.is_empty() {
                lines.push(std::mem::take(&mut current));
            }
            lines.extend(
                word.chars()
                    .collect::<Vec<_>>()
                    .chunks(width)
                    .map(|chunk| chunk.iter().collect()),
            );
        } else if !current.is_empty() && current.chars().count() + 1 + word_len > width {
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

fn find_bibliography(start: &Path) -> Result<PathBuf> {
    for directory in start.ancestors() {
        let candidate = directory.join(BIBLIOGRAPHY_FILE);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    for directory in start.ancestors() {
        if directory.join(LEGACY_FILE).is_file() {
            bail!(
                "found legacy cita.toml at {}; Cita no longer migrates TOML automatically—convert it to references.bib before continuing",
                directory.join(LEGACY_FILE).display()
            );
        }
    }
    bail!(
        "no references.bib found in {} or its parents; run `cita init`",
        start.display()
    )
}

fn reject_legacy_only(directory: &Path) -> Result<()> {
    let legacy = directory.join(LEGACY_FILE);
    if legacy.is_file() {
        bail!(
            "found legacy cita.toml at {}; Cita uses references.bib and provides no automatic migration",
            legacy.display()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removed_export_and_add_flags_are_rejected() {
        assert!(Cli::try_parse_from(["cita", "export", "--bibtex"]).is_err());
        assert!(Cli::try_parse_from(["cita", "add", "--key", "X", "1207.7214"]).is_err());
        assert!(Cli::try_parse_from(["cita", "add", "--force", "1207.7214"]).is_err());
    }

    #[test]
    fn document_argument_conflicts_are_enforced() {
        assert!(
            Cli::try_parse_from(["cita", "fetch", "--dry-run", "--save", "1207.7214"]).is_err()
        );
        assert!(
            Cli::try_parse_from(["cita", "open", "--browser", "--no-download", "1207.7214"])
                .is_err()
        );
    }

    #[test]
    fn title_helpers_are_unicode_safe() {
        assert_eq!(truncate("αβγ", 2), "α…");
        assert_eq!(wrap("one two three", 7), ["one two", "three"]);
    }
}
