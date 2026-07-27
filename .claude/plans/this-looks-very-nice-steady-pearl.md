# bibi: replace cita's TOML manifest with the .bib file as SSOT

## Context

cita today keeps authoritative BibTeX inside a schema-1 `cita.toml` and treats
`references.bib` as a deterministic generated artifact. That inversion is the
source of most of the codebase's complexity: two-phase atomic writes, drift
verification, a `generate` repair command, a strict "entries and whitespace
only" rendering contract, and a manifest-shaped project/shelf/library hierarchy.

Making the `.bib` file itself the single source of truth removes all of it. The
tool-owned metadata that justified the manifest (INSPIRE record id, refresh
timestamp, curated identifiers) moves into namespaced BibTeX fields on the
entries themselves, where LaTeX toolchains already ignore unknown fields.

This branch **replaces** cita rather than shipping alongside it, and renames the
project to **bibi** throughout. Clean break: no migration from `cita.toml`.

Expected outcome: ~10,200 → ~7,200 lines, thirteen commands → nine, one global
`--path` flag in place of two scope arg structs, and no derived artifact.

## Target design

### Crates

| Crate | Fate |
|---|---|
| `cita-core` → `bibi-core` | rename only |
| `cita-inspire-client` → `bibi-inspire-client` | rename only |
| `cita-documents` → `bibi-documents` | rename; cache moves to `.bibi/files` |
| `cita-bibliography` → `bibi-bibliography` | rename **+ grows** (see P2) |
| `cita-manifest` | **deleted** (2263 lines) |
| `bibi-bibfile` | **new** — the store |
| `cita` → `bibi` | rename + CLI rewrite |

### Tool-owned fields

Prefix `x-bibi-`, defined as one constant in `bibi-bibfile`.

| Field | Meaning |
|---|---|
| `x-bibi-inspire-id` | stable INSPIRE record id; its **presence** marks an entry as managed |
| `x-bibi-inspire-updated` | provider timestamp; used to skip no-op refreshes |
| `x-bibi-arxiv` | curated normalized arXiv id, overrides projection |
| `x-bibi-doi` | curated normalized DOI, overrides projection |
| `x-bibi-frozen` | never refresh, never resolve — covers both hand-edited entries and non-INSPIRE works |

`SourceSnapshot`'s `source = "inspire" \| "import"` tag does **not** survive:
managed-ness is derived from field presence, so the inconsistent state is
unrepresentable.

### Splice model

The `.bib` is user-owned and hand-edited, so writes must never reformat entries
the command did not touch. Untouched bytes come out identical.

- `Bibfile` holds the verbatim file text plus per-entry spans.
- A mutation produces `Vec<Edit>` (`Replace`/`Remove`/`Append`), applied
  right-to-left so earlier spans stay valid.
- New entries append at the end — the user owns ordering, so no re-sorting.
- Governing invariant: **load → write with zero mutations is byte-identical**,
  for any valid input including comments, `@string`, and odd whitespace.

### Command surface

```
bibi [-p|--path <file>] <command>

  add <locator>...     [--key] [--overwrite]
  import <path|->...   [--overwrite]
  remove <selector>...
  rekey <selector> <new-key>
  list                 [--sort-by] [--order] [--no-wrap-title]
  check
  sync                 [--dry-run] [--verbose]
  fetch <selector>     [--source|--url] [--force|--cache-only] [--open] [--save]
  export               [--output|-] [--keep-metadata]
  completions <shell>
```

Gone: `init`, `generate`, `commit`, `library {init,list,new}`, `--shelf`,
`--all-shelves`. New: `rekey`.

Path resolution is explicit, never a parent walk:
`--path` > `$BIBI_BIB` > `./references.bib`. A directory argument appends
`references.bib`. Missing file is always an error — never auto-created — with
the fix inline in the message. Every mutating command echoes the resolved path.

## Phases

Each phase leaves the workspace green (`cargo test --workspace`, clippy clean).

### P0 — Rename cita → bibi

Mechanical, zero behavior change, its own commit so later diffs are readable.

- Six crate directories and `[package] name`s; `Cargo.toml` workspace deps.
- `CITA_INSPIRE_BASE_URL` → `BIBI_INSPIRE_BASE_URL` (3 sites).
- `.cita/files` → `.bibi/files` (16 sites, incl. `ensure_cache_layout`'s
  gitignore rule in `crates/cita/src/commands/mod.rs:36-37`).
- `release-plz.toml` package names; `AGENTS.md`, `README.md`, `docs/CONTEXT.md`.
- Pre-flight: confirm `bibi` is free on crates.io.

Repo rename on GitHub and deprecating the published `cita*` crates are
follow-ups, not blockers for the branch.

### P1 — Delete what is leaving

Pure deletions against the still-manifest-backed CLI. Shrinks what P4 must port.

- `crates/bibi-manifest/src/library.rs` (608), `commands/library.rs` (231),
  `commands/init.rs` (41), `commands/generate.rs` (16), `src/git.rs` (391).
- `ShelfArg`/`ScopeArgs`/`Target`/`resolve_target` and their ~14 flattened uses
  in `main.rs`.
- Corresponding tests in `tests/cli.rs` (13 shelf/library test fns, ~62
  git-touching lines) and the `Libraries and shelves` / `Git` sections of
  `AGENTS.md`.

### P2 — Extend `bibi-bibliography`

`scan_raw_entries` (`crates/bibi-bibliography/src/lib.rs:166`) is already the
whole-file span map — it returns `entry_range`, `key_range`, `field_names`, and
`last_field` per entry. It needs exporting and relaxing, not writing.

- Make it `pub fn scan_entries` and export `RawEntry` with its span fields.
- **Relax strictness**: drop the directive rejection and the inter-entry gap
  rejection. Comments and `@string`/`@preamble` become pass-through bytes,
  preserved automatically by span-based splicing. Keep key validation, duplicate
  -key rejection, and every span invariant.
- Add `pub fn field(source, name) -> Option<String>` (case-insensitive, mirrors
  `insert_field`'s comparison).
- Add `pub fn strip_fields_with_prefix(source, prefix)` — the exact mirror of
  `insert_field` (`lib.rs:319`), same scanner, same comma-placement care.
- Add `pub fn remove_field(source, name)` for sync's re-splice.

`insert_field` and `rename_entry` stay single-entry: `Bibfile` extracts an entry
by span, calls them, splices the result back. No duplicated span logic.

### P3 — `bibi-bibfile` store

New crate, ~500 lines plus tests. Typed `thiserror` enum.

```rust
pub struct Bibfile { path: PathBuf, source: String, entries: Vec<Entry>, by_key: BTreeMap<String, usize> }

impl Bibfile {
    pub fn load(path) -> Result<Self, Error>;
    pub fn resolve(explicit: Option<&Path>) -> Result<PathBuf, Error>;  // --path > $BIBI_BIB > ./references.bib
    pub fn keys(&self) -> impl Iterator<Item = &str>;
    pub fn raw(&self, key: &str) -> Option<&str>;
    pub fn projected(&self) -> Result<Vec<ProjectedReference>, Error>;
    pub fn find(&self, selector: &str) -> Result<Option<ProjectedReference>, Error>;
    pub fn managed(&self) -> Vec<(String, u64)>;      // key -> x-bibi-inspire-id
    pub fn unmanaged(&self) -> Vec<(String, Locator)>; // key -> DOI/arXiv for resolve
    pub fn add_batch(&mut self, Vec<PendingReference>, ConflictPolicy) -> Result<Vec<AddOutcome>, Error>;
    pub fn remove_batch(&mut self, &[String]) -> Result<Vec<ProjectedReference>, Error>;
    pub fn rekey(&mut self, selector: &str, new_key: &str) -> Result<(), Error>;
    pub fn apply_records(&mut self, Vec<(String, InspireRecord)>) -> Result<Vec<SyncOutcome>, Error>;
    pub fn check(&self) -> Result<(), Vec<Diagnostic>>;
    pub fn write(&self) -> Result<(), Error>;
    pub fn render_bare(&self) -> Result<String, Error>;
}
```

Port from `bibi-manifest` largely unchanged: `ConflictPolicy`, `AddOutcome`,
`PendingReference`, `KeyRequest`, `ProjectedReference`, `atomic_write`, and
`find`'s selector precedence (`lib.rs:451` — exact key, then `Locator::Inspire`,
`Doi`, `Arxiv`), with identifiers now read from `x-bibi-arxiv`/`x-bibi-doi`
falling back to `project_bibtex`.

Duplicate-identity detection becomes a scan over projected entries instead of a
maintained index.

**Test first**: the byte-identical round-trip property over a fixture corpus
(comments, `@string`, single-line entries, no trailing newline, trailing-comma
and not, CRLF). Hand-written fixtures — no new proptest dependency.

### P4 — CLI rewrite, offline commands

`main.rs` 459 → ~250. Global `--path`:

```rust
#[arg(short = 'p', long, global = true, value_name = "FILE", env = "BIBI_BIB")]
path: Option<PathBuf>,
```

`global = true` makes both `bibi -p x.bib add …` and `bibi add -p x.bib …` parse;
clap's `env` documents the fallback in `--help` for free.

Port onto `Bibfile`: `check` (replaces `generate`), `list`
(`commands/list.rs`, unchanged apart from the store type; add a `file` sort
option now that file order carries meaning), `remove`, `rekey` (new, wraps
`rename_entry`), `import`, `export`.

- `import` takes `Vec<String>` sources like `add` takes locators; validates all
  as one candidate, writes once. Refuses source == target by canonical path.
  **Always strips `x-bibi-*`** from incoming entries — safe because `sync`
  re-establishes ids by identity.
- `export` inverts from cita's: strips `x-bibi-*` and adds the derived `url`
  field by default (`--keep-metadata` opts out). Reuse `derive_entry`'s arXiv
  URL policy from `commands/export.rs:52` and `arxiv_pdf_url`. The `--output`
  guard collapses to one rule: refuse the SSOT itself.

Reuse as-is from `commands/mod.rs`: `ensure_cache_layout`, `inspire_client`,
`highlight_style`, `add_message`, `print_add_outcomes`.

### P5 — Network commands

`fetch` ports unchanged (`commands/fetch.rs`, 233 lines) — `bibi-documents` is
untouched and `--save` still adds before fetching.

`sync` absorbs resolve. One command, one reconciliation pass:

```rust
enum RefreshKey { Record(u64), Identity(Locator) }
```

1. Partition entries: `x-bibi-frozen` → skipped; has `x-bibi-inspire-id` →
   `Record`; has a DOI/arXiv → `Identity`; otherwise → reported unmanaged.
2. `Record` batch via existing `Client::refresh_records(&[u64])`
   (`client.rs:91`) — keeps the 100-record / 6 KiB batching and 429 retries.
3. `Identity` lookups via existing `Client::resolve_snapshot(&Locator)`
   (`client.rs:75`), sequentially. **Require exactly one match** or report the
   entry unresolved — this preserves the "every result must be explained"
   invariant against a search that can return several records.
4. Skip the splice when the returned `updated` equals `x-bibi-inspire-updated`.
5. Error when two entries resolve to the same record id — a new failure mode,
   since `import` dedupes on DOI/arXiv and cannot catch it.
6. Local keys never change during refresh (unchanged rule from cita).

Output summarizes rather than enumerates; `--verbose` lists, `--dry-run` plans.

`add` shares the resolve lookup path but stays atomic: resolve first, write only
on success, so a failed lookup never leaves a stub in the SSOT.

### P6 — Delete `bibi-manifest`, docs, release

Drop the crate and its `release-plz.toml` block. Rewrite `AGENTS.md` (~169 →
~110): delete `Atomic mutations`, rewrite `Source snapshots and raw entries`
around the field namespace and the new leniency, halve `Derived exports`, update
the architecture diagram. Update `README.md` and `docs/CONTEXT.md`. Ship as
**1.0.0** — the format is the product and a clean break deserves the signal.

## Verification

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo test -p bibi-bibfile roundtrip     # the load→write identity property
cargo test --test e2e -- --ignored       # live INSPIRE
```

End-to-end by hand, in a scratch directory:

```bash
touch references.bib
bibi add 1207.7214 --key Higgs2012 && bibi list
bibi sync --dry-run                      # no-op: timestamp unchanged
printf '@book{Peskin1995,\n  title = {An Introduction to Quantum Field Theory},\n  year = {1995},\n}\n' > other.bib
bibi import other.bib && bibi sync       # Peskin reported unresolved, not failed
bibi rekey Higgs2012 Atlas2012 && bibi check
bibi export -o share.bib                 # no x-bibi-* fields, url present
bibi fetch Atlas2012 --url
```

Round-trip check that must hold at every step: `git diff references.bib` shows
only the entries the command touched, with no reformatting elsewhere.

## Out of scope

- Migration from `cita.toml` (clean break, per decision).
- Batched identity lookups in `sync` — sequential `resolve_snapshot` is fine at
  personal-bibliography scale; revisit if it bites.
- `--refresh-only` on sync — the managed/unmanaged split is bookkeeping users
  should not have to think about.
- GitHub repo rename and crates.io deprecation of the `cita*` packages.
