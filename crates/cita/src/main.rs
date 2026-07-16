mod git;

use anyhow::{Context, Result, bail};
use cita_core::{Locator, MetadataProvider, PaperRecord};
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
        /// Sort order for the listing
        #[arg(long, value_enum, default_value_t = SortBy::Key)]
        sort_by: SortBy,
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
        #[arg(long)]
        dry_run: bool,
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

/// Sort order for `cita list`.
#[derive(Clone, Copy, Debug, Default, clap::ValueEnum)]
enum SortBy {
    #[default]
    Key,
    Title,
    Author,
    Year,
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
        Some(Command::Add { key, locators }) => add(&cwd, key.as_deref(), &locators).await?,
        Some(Command::Remove { selectors }) => remove(&cwd, &selectors)?,
        Some(Command::List {
            sort_by,
            no_wrap_title,
        }) => list(&cwd, sort_by, !no_wrap_title)?,
        Some(Command::Fetch {
            force,
            dry_run,
            selector,
        }) => {
            fetch(&cwd, &selector, force, dry_run).await?;
        }
        Some(Command::Open {
            force,
            browser,
            no_download,
            selector,
        }) => {
            open(&cwd, &selector, force, browser, no_download).await?;
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

async fn add(cwd: &Path, key: Option<&str>, values: &[String]) -> Result<()> {
    if key.is_some() && values.len() != 1 {
        bail!("--key can only be used with one locator");
    }
    let path = find_manifest(cwd)?;
    let mut manifest = Manifest::load(&path)?;
    let provider = InspireProvider::new()?;
    let outcomes = if let Some(key) = key {
        let locator = values[0].parse::<Locator>()?;
        let paper = provider.resolve(&locator).await?;
        vec![manifest.add(paper, Some(key))?]
    } else {
        let mut resolved = Vec::with_capacity(values.len());
        for value in values {
            let locator = value.parse::<Locator>()?;
            resolved.push(provider.resolve(&locator).await?);
        }
        manifest.add_batch(resolved)?
    };
    for outcome in outcomes {
        match outcome {
            AddOutcome::Added(key) => println!("Added {key}"),
            AddOutcome::Existing(key) => println!("Already present: {key}"),
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

fn list(cwd: &Path, sort_by: SortBy, wrap_title: bool) -> Result<()> {
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
        SortBy::Key => {}
        SortBy::Title => rows.sort_by(|a, b| a.title.cmp(&b.title)),
        SortBy::Author => rows.sort_by(|a, b| a.author.cmp(&b.author)),
        SortBy::Year => rows.sort_by(|a, b| {
            (a.year_num.is_none(), a.year_num).cmp(&(b.year_num.is_none(), b.year_num))
        }),
    }

    let headers = ["Key", "Title", "Author", "Year"];
    let key_width = column_width(headers[0], rows.iter().map(|row| row.key.as_str()));
    let author_width = column_width(headers[2], rows.iter().map(|row| row.author.as_str()));
    let year_width = column_width(headers[3], rows.iter().map(|row| row.year.as_str()));

    let terminal_width = terminal_size::terminal_size().map(|(width, _)| width.0 as usize);
    let title_width = match terminal_width {
        Some(width) => {
            let reserved = key_width + author_width + year_width + 6;
            Some(width.saturating_sub(reserved).max(10))
        }
        None => None,
    };
    let title_column_width = title_width
        .unwrap_or_else(|| column_width(headers[1], rows.iter().map(|row| row.title.as_str())));

    let color = std::io::stdout().is_terminal();
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

async fn fetch(cwd: &Path, selector: &str, force: bool, dry_run: bool) -> Result<()> {
    if dry_run {
        let url = arxiv_url_for_selector(cwd, selector)?;
        println!("{url}");
        println!("[dry run] skipped download");
        return Ok(());
    }
    let policy = if force {
        FetchPolicy::Force
    } else {
        FetchPolicy::UseCache
    };
    let (key, url, outcome) = fetch_paper(cwd, selector, policy).await?;
    print_fetch_url_outcome(&key, &url, &outcome);
    Ok(())
}

async fn open(
    cwd: &Path,
    selector: &str,
    force: bool,
    browser: bool,
    no_download: bool,
) -> Result<()> {
    if browser {
        return open_in_browser_with(cwd, selector, |url| {
            opener::open(url).map_err(anyhow::Error::from)
        });
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
        |path| opener::open(path).map_err(anyhow::Error::from),
        std::io::stdout(),
    )
    .await
}

async fn open_with(
    cwd: &Path,
    selector: &str,
    policy: FetchPolicy,
    launch: impl FnOnce(&Path) -> Result<()>,
    mut output: impl Write,
) -> Result<()> {
    let (key, url, outcome) = fetch_paper(cwd, selector, policy).await?;
    writeln!(
        output,
        "{}",
        fetch_url_outcome_message(&key, &url, &outcome)
    )?;
    let path = outcome.path();
    launch(path).with_context(|| format!("could not open {}", path.display()))?;
    writeln!(output, "Opened {url}")?;
    Ok(())
}

fn open_in_browser_with(
    cwd: &Path,
    selector: &str,
    launch: impl FnOnce(&str) -> Result<()>,
) -> Result<()> {
    let manifest = Manifest::load(find_manifest(cwd)?)?;
    let (_, paper) = manifest.paper(selector)?;
    let url = arxiv_pdf_url(paper)?;
    launch(url.as_str()).with_context(|| format!("could not open {url}"))?;
    println!("Opened {url}");
    Ok(())
}

async fn fetch_paper(
    cwd: &Path,
    selector: &str,
    policy: FetchPolicy,
) -> Result<(String, String, FetchOutcome)> {
    let manifest_path = find_manifest(cwd)?;
    let manifest = Manifest::load(&manifest_path)?;
    let (key, paper) = manifest.paper(selector)?;
    let url = arxiv_pdf_url(paper)?.to_string();
    let project_root = manifest_path.parent().unwrap_or_else(|| Path::new("."));
    if policy != FetchPolicy::CacheOnly {
        ensure_cache_layout(project_root)?;
    }
    let store = DocumentStore::new(project_root.join(".cita/files"))?;
    let outcome = store
        .fetch(paper, policy)
        .await
        .map_err(|error| document_error_with_hint(error, policy))?;
    Ok((key.to_owned(), url, outcome))
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

fn arxiv_url_for_selector(cwd: &Path, selector: &str) -> Result<String> {
    let manifest = Manifest::load(find_manifest(cwd)?)?;
    let (_, paper) = manifest.paper(selector)?;
    Ok(arxiv_pdf_url(paper)?.to_string())
}

fn print_fetch_url_outcome(key: &str, url: &str, outcome: &FetchOutcome) {
    println!("{}", fetch_url_outcome_message(key, url, outcome));
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
    use std::cell::RefCell;

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
            |path| {
                launched.replace(Some(path.to_owned()));
                Ok(())
            },
            &mut output,
        )
        .await
        .unwrap();

        assert_eq!(launched.into_inner(), Some(pdf));
        assert_eq!(
            String::from_utf8(output).unwrap(),
            "Already fetched Example: https://arxiv.org/pdf/1207.7214\nOpened https://arxiv.org/pdf/1207.7214\n"
        );
    }

    #[test]
    fn browser_open_passes_the_arxiv_pdf_url_to_the_launcher_without_creating_a_cache() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join("cita.toml"),
            "schema = 1\n\n[papers.Example]\ntitle = 'Example'\nsource = 'inspire'\narxiv_ids = ['hep-th/9901001']\n",
        )
        .unwrap();
        let launched = RefCell::new(None);

        open_in_browser_with(directory.path(), "Example", |url| {
            launched.replace(Some(url.to_owned()));
            Ok(())
        })
        .unwrap();

        assert_eq!(
            launched.into_inner().as_deref(),
            Some("https://arxiv.org/pdf/hep-th/9901001")
        );
        assert!(!directory.path().join(".cita").exists());
        assert!(!directory.path().join(".gitignore").exists());
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
    fn list_accepts_no_wrap_title_flag() {
        let Command::List {
            sort_by: _,
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
