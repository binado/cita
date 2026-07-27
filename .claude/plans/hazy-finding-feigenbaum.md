# `cita export` — Zotero-friendly BibTeX export

## Context

`references.bib` imports into Zotero today, but the entries INSPIRE returns carry no
`url` field (verified against the real records in `references.bib` — INSPIRE relies on
`eprint`/`archivePrefix` and `doi` as locators). Zotero's BibTeX importer does not
synthesize one: it maps only fields literally present, so every imported item lands with
a blank URL and no clickable link. For arXiv-only preprints with no DOI, Zotero's
"Find Available PDF" then has nothing concrete to resolve against.

The fix cannot go into `references.bib`. That file is a byte-exact rendering of
`cita.toml` — `verify_bibliography` compares bytes and `cita generate` overwrites any
drift, and `rename_entry` is deliberately constrained to change only the key token
("Do not add a handwritten writer", AGENTS.md). Injecting a derived field there would be
destroyed on the next `generate` or `sync`.

So: a **separate, derived artifact**. `cita export` renders the same entries with a
derived `url = {https://arxiv.org/pdf/<ID>}` added to every entry that has an arXiv ID.
It stays a pure function of `cita.toml` — deterministic, same bytes on any machine, no
network, no filesystem probing.

**Explicitly out of scope** (deferred, documented as known limitations): attaching cached
PDFs via a `file =` field, re-import deduplication, and rewriting preprint entry types
(`@article` with no `journal` → `@misc`). All three are silent-quality issues rather than
failures, and `file =` was dropped specifically because Zotero copies files into its own
storage on import regardless — so it costs disk and sync quota without saving anything,
while reintroducing machine-dependent output.

## Decisions

| Decision | Choice |
| --- | --- |
| URL value | Direct arXiv PDF URL, via existing `arxiv_pdf_url` |
| Default output | `<project-dir-name>.bib`; shelves use `<shelf-name>.bib` |
| Override | `-o/--output`, resolved against the caller's directory |
| Surface | `cita export`, `cita library shelf <name> export`, `cita library export` |

## Design

### 1. `insert_field` in `cita-bibliography`

`scan_raw_entries` and `RawEntry` are private ([lib.rs:152](crates/cita-bibliography/src/lib.rs:152)),
so the byte spans aren't reachable from other crates. Add a public `insert_field` mirroring
[`rename_entry`](crates/cita-bibliography/src/lib.rs:261)'s shape: validate → scan →
assert exactly one entry → splice → re-validate via `BibtexSnapshot::new`.

**Insert after the last field, not before the closing brace.** `biblatex`'s `field()`
parser calls `comment()` after each value, so `year = {2025} % note\n}` is legal — appending
before `}` would land the new field after a `%` and corrupt the entry. Extend `RawEntry`
with `field_names: Vec<String>` and `last_field: Option<Range<usize>>`, computed inside
`scan_raw_entries` (which the code documents as "the sole authority for all raw source and
span invariants").

Note `abbr_field` eats trailing whitespace before returning, so the value span's end runs
past the field; the insertion offset is `source[..pair.value.span.end].trim_end().len()`.

Behaviour:
- Comma always emitted *before* the new field. When the entry already had a trailing comma,
  the insertion point sits before it, so no duplicate comma is possible.
- Indentation copied from the line where the last field's **name** starts (not its value —
  that misindents multi-line values). Single-line entries stay single-line.
- **Entry already defines the field → return source unchanged**, case-insensitively.
  Authored values beat derived ones, no duplicate `url` is ever emitted, and repeat runs
  are byte-stable.
- Reject unsafe names/values (`{`, `}`, `\`, `%`, control chars, empty). Reuse
  `Error::InvalidBibtex` with an explicit message rather than widening the error enum.

### 2. Render loop — extend `cita-manifest`, keep URL policy in the CLI

`render_bibliography` ([lib.rs:824](crates/cita-manifest/src/lib.rs:824)) is private and
hardcodes `rename_entry`. Refactor it into `render_entries(references, transform)` and add
a public `Manifest::render_derived(transform)` next to
[`Manifest::render_bibliography`](crates/cita-manifest/src/lib.rs:665). `render_bibliography`
becomes the identity transform, so `references.bib` stays byte-identical.

**Do not add `cita-documents` to `cita-manifest`.** That would put `reqwest`/`flate2` behind
the crate owning the authoritative manifest and contradict the AGENTS.md dependency graph,
where the two are siblings joined only at `cita`. URL policy therefore lives in the CLI crate;
layout policy stays in `cita-manifest` so the two artifacts can never diverge in formatting.

Generic over `E: From<Error>` so the CLI can use `anyhow::Error` and apply `?` to both
document and bibliography errors inside the closure.

### 3. Atomic write

Make [`atomic_write`](crates/cita-manifest/src/lib.rs:846) public — one durability policy,
no duplication, no new dependency. While making it public, fix the latent bug: `Path::parent()`
on a bare relative filename returns `Some("")`, not `None`, so the existing
`unwrap_or_else(|| Path::new("."))` doesn't protect `NamedTempFile::new_in`. Add
`.filter(|p| !p.as_os_str().is_empty())` (the correct check already exists at
[library.rs:337](crates/cita-manifest/src/library.rs:337)).

### 4. arXiv ID source — use the projected value

Use `reference.identifiers.arxiv.first()`, not `HepIdentifiers::arxiv`. There's no conflict
to reconcile: [`project_inspire`](crates/cita-inspire-client/src/snapshot.rs:114) already
overlays the curated arXiv onto the projected vector, so the projected value *is* the
curated value for INSPIRE entries and the `eprint` value for imports. One uniform rule,
covers both source kinds, and matches what [fetch.rs:107](crates/cita/src/commands/fetch.rs:107)
already does. Reading `HepIdentifiers` directly would silently skip every imported entry.

Note `Reference.identifiers.arxiv` is `Vec<String>`, not `Option<String>`.

### 5. Command wiring

New `crates/cita/src/commands/export.rs` following the `generate`/`generate_outcome` split
([generate.rs](crates/cita/src/commands/generate.rs)) so the batch path can reuse it:

```rust
pub(crate) fn export(project: &Path, caller: &Path, output: Option<&Path>) -> Result<()>
pub(crate) fn export_outcome(project: &Path, caller: &Path, output: Option<&Path>) -> Result<PathBuf>
fn derive_entry(key: &str, source: &SourceSnapshot, entry: String) -> Result<String>
```

Use `Manifest::load_verified`, not `load`. Every read command uses it; only `generate`
(which repairs drift) uses `load`. Export claims to hold the same entries as `references.bib`,
so it must refuse to run against a drifted bibliography rather than silently disagreeing.

- **main.rs**: `ExportArgs { output: Option<PathBuf> }` with `#[arg(short = 'o', long)]`;
  `Command::Export`, `ShelfCommand::Export(ExportArgs)`, and `LibraryCommand::Export`
  (**no args** — one `-o` can't name N shelf files). Dispatch arms alongside the existing ones.
- **commands/mod.rs**: `mod export;` + `pub(crate) use export::{export, export_outcome};`,
  and `batch_export as library_export` in the library re-export block.
- **library.rs**: `ShelfAction::Export(ExportArgs)` arm in `run_shelf_command`, plus
  `batch_export` cloned from [`batch_generate`](crates/cita/src/commands/library.rs:114) —
  same `Result<bool>` "at least one failed" contract, same stdout `Shelf {name}: ...` lines,
  same `print_batch_failure` continue-after-failure.
- Shelf files are named for the **registered shelf name**, not the directory:
  `cita library shelf paper init --path papers/one` registers `paper` at `one`, and these
  legitimately differ.

### 6. Default path and the collision guard

```rust
directory.file_name()  // OsString::push(".bib"), not format! — non-UTF-8 safe
```

`file_name()` is `None` only at filesystem root; error with a "pass `--output`" hint rather
than inventing a fallback. Shelf names are already validated as `[A-Za-z0-9][A-Za-z0-9._-]*`
([library.rs:344](crates/cita-manifest/src/library.rs:344)), so shelf-derived filenames are
always safe.

**Guard against writing over managed files.** A project or shelf named `references` would
derive `references.bib` — the tracked artifact. Check both the default and `-o` against
`manifest.bibliography_path()` and `manifest.path()`, comparing with the parent directory
canonicalized so `..` and symlinks can't alias. Resolve `-o` to absolute before writing.

### 7. Gitignore — do nothing in v1

This repo's `.gitignore` has `*.bib` / `!references.bib`, but that is **not** written by
`cita init`: [`ensure_cache_layout`](crates/cita/src/commands/mod.rs:50) only appends
`/.cita/files/`. In a user project the export file will be untracked and visible. Silently
editing a user's `.gitignore` is more surprising than an untracked file — document it
instead, and consider a `--gitignore` flag later reusing the append-if-absent pattern.
`cita commit` force-adds only the two managed files, so it won't sweep the export in.

## Files

| File | Change |
| --- | --- |
| [cita-bibliography/src/lib.rs](crates/cita-bibliography/src/lib.rs) | `RawEntry` fields, `insert_field`, validators |
| [cita-manifest/src/lib.rs](crates/cita-manifest/src/lib.rs) | `render_entries`, `Manifest::render_derived`, `atomic_write` public + parent fix |
| `crates/cita/src/commands/export.rs` | **new** |
| [cita/src/commands/mod.rs](crates/cita/src/commands/mod.rs) | module + re-exports |
| [cita/src/commands/library.rs](crates/cita/src/commands/library.rs) | `ShelfAction::Export`, `batch_export`, `shelf_export_path` |
| [cita/src/main.rs](crates/cita/src/main.rs) | `ExportArgs`, three enum variants, dispatch |
| [cita/tests/cli.rs](crates/cita/tests/cli.rs) | integration tests |
| README, AGENTS.md `## Commands` | document the command and its limitations |

## Verification

Existing conventions only — hand-rolled `assert!`/`assert_eq!`, `tempfile::tempdir()`,
`std::process::Command` on `env!("CARGO_BIN_EXE_cita")`. **No snapshot crates** (none exist
in the workspace). Reuse the `entry()` and `arxiv_library()` fixtures in `cli.rs`.

**Unit — `cita-bibliography`**: layout follows the last field (exact string assertion,
comma + indent + tail); single-line entries stay single-line; existing field preserved
case-insensitively and insertion is a fixed point; survives a trailing `%` comment; rejects
unsafe names/values and multi-entry sources. The existing rename test must still pass
unchanged, proving the scanner wasn't disturbed.

**Unit — `cita-manifest`**: `render_derived` with the identity transform equals
`render_bibliography` — the regression lock on byte-identical `references.bib`; a failing
transform propagates; empty manifest renders empty.

**Integration — `cli.rs`**: arXiv entries get the URL and non-arXiv entries don't, with
`references.bib` bytes unchanged across the run; default filename derives from the project
directory and `-o` overrides it, resolving against the caller when run from a subdirectory;
export refuses to overwrite `references.bib` or `cita.toml` (including via `./sub/../`);
two runs produce identical bytes and an authored `url` survives; drift fails with
"run `cita generate`" and writes nothing; shelf export uses the shelf name not the directory
name; `cita library export` is ordered, continues after failure, prints one line per shelf
to stdout with empty stderr, and exits 1.

**Clap parse tests** in `main.rs`: extend `library_command_tree_routes_supported_shelf_operations`
with the three new paths; assert `cita export x.bib` (positional) and
`cita library export -o x.bib` both fail.

**Manual, once**: import the generated export into Zotero and confirm the URL lands on the
item and that "Find Available PDF" resolves an arXiv-only preprint. This is the one piece
that can't be tested in-repo, and it's what would justify revisiting the deferred `file =`
field.

```bash
cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --all -- --check
```
