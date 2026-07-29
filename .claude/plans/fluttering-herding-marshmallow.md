# Small bibi: phase one

## Context

bibi has been pulled between two products: a project-local bibliography *file*
manager, and a full personal library manager. `BIBI_TWO_BINARIES.md` resolves
that by splitting them into two binaries, and defers the large one.

This plan is phase one of that document: finish the small binary against its
file-centered promise. **Every step is a subtraction from the existing tree.**
No workspace reorganization happens here — the eight current crates are already
the right shape for small bibi, and the shared-crate layout in
`BIBI_TWO_BINARIES.md` ("Where sync lives", the retrieval/document split) is
design intent that waits for a second caller. Extracting a seam now would mean
guessing what large bibi wants, and a wrong guess costs more than no seam.

Outcome: `bibi.toml` beside the project is the only thing bibi targets, there is
no ambient global state left, and `fetch` acquires a file instead of managing a
collection.

The one place this adds rather than removes is `fetch`, which gains `-o/--output`
and `--source --url`, and downloads into the working directory.

## Commit 1 — remove global-manifest targeting

Deletes `-g/--global` and everything that existed only to serve it.

- `crates/bibi/src/cli.rs` — drop `TargetArgs::global` and the `conflicts_with`
  on `--path`; update the module doc, which currently explains scope in terms of
  two flags.
- `crates/bibi/src/main.rs:46` — `TargetSelection` construction.
- `crates/bibi-application/src/target.rs` — drop `TargetSelection` entirely and
  let `TargetResolver::resolve` take `Option<&Path>`; a one-field struct wrapping
  an `Option<PathBuf>` earns nothing. Remove `PlatformPaths::global_manifest`,
  `GLOBAL_MANIFEST_ENV`, and the two-arm ambiguity error. `PlatformPaths` keeps
  `cache_root` for now and dies in commit 3.
- `crates/bibi-manifest/src/store.rs:82` — `ManifestStore::creating_parents` has
  exactly one caller, the global arm. Delete it.
- `crates/bibi-manifest/src/atomic.rs` — with `creating_parents` gone,
  `ParentPolicy::Create` is unreachable and the enum's doc comment ("the fixed
  platform location of the global manifest") no longer describes anything.
  Delete the enum and drop `atomic_replace`'s third parameter. Update the
  `pub use` in `lib.rs:26`.
- Tests: `crates/bibi/tests/cli.rs:216`
  (`the_global_manifest_is_an_ordinary_project_at_a_fixed_path`) goes; the
  `BIBI_GLOBAL_MANIFEST` env in the two `bibi*` helpers goes.
  `an_explicit_path_targets_another_project_without_searching_upwards` stays and
  becomes the primary targeting test. `target.rs`'s unit tests lose their global
  cases.

## Commit 2 — remove `export`, unify the filters

`list --format bibtex` becomes the only multi-record BibTeX output. Shell
redirection chooses the destination.

- `crates/bibi/src/cli.rs` — remove `Command::Export`, `ExportArgs`, and
  `RenderFilterArgs`. Point `CheckArgs` at `FilterArgs`, which carries
  `--provider`. The exclusion existed only because `export --provider` named the
  provider to *sync*; with `export` gone the ambiguity is gone, so `check
  --provider` is now a plain filter.
- `crates/bibi/src/commands/export.rs` — keep `run_check` only; rename the
  module to `check.rs`. Update `mod.rs` and the `main.rs` dispatch.
- `crates/bibi-application/src/export.rs` — delete `ExportRequest`,
  `ExportReport`, `export()`, `output_path()`, and `DEFAULT_OUTPUT`. Keep
  `check`, `CheckOutcome`, `describe_drift`. Rename to `check.rs` and rewrite the
  module doc, which currently explains why bibi keeps no `.bib` in step — still
  true, but it now argues for the absence of the command rather than its shape.
- Cascading dead code, each with export as its only user: `Error::SyncFailed`
  (`error.rs:54`) and its display helper, `target::output()` (`target.rs:142`),
  and the `atomic_replace`/`ParentPolicy` re-export from `bibi-manifest`
  (`ManifestStore` still uses them internally).
- `crates/bibi-application/src/lib.rs:49` — trim the re-export list.
- Tests: `crates/bibi-application/tests/export.rs` becomes check-only. In
  `crates/bibi/tests/cli.rs`, the four `export_*` tests
  (`:326`, `:370`, `:386`, `:406`) are rewritten against
  `list --format bibtex` plus shell-free file writes, keeping their assertions
  about filters and `--local`. `export_refuses_to_write_over_the_manifest`
  (`:370`) is deleted outright — with no destination argument there is nothing to
  refuse, which is the point of the change. Add one test that `check --provider`
  now filters.

## Commit 3 — rewrite `fetch`, delete the document cache

The largest step, and the only one that adds behavior. `fetch` acquires one
file into the working directory; it manages nothing.

**`bibi-documents` — cache out, retrieval in.**

- Delete `clean.rs`, and `source.rs`'s `extract` and archive-walking (source is
  downloaded as an archive, not unpacked). Keep a byte ceiling. Drop the `tar`
  and `flate2` dependencies.
- `path.rs` — replace the cached layout (`artifact_path`, `validate_root`) with
  filename derivation: `<arxiv-id>.pdf` / `<arxiv-id>.tar.gz`, `/` → `-` for
  legacy ids (`hep-th-9901001.pdf`). Adopting arXiv's own naming is what deletes
  the slugging rule, the author lookup, and the year segment.
- `store.rs` — `DocumentStore` loses `cache_root`, `path_for`, `clean`,
  `usable`, `FetchPolicy`, and `FetchOutcome`. `publish_pdf`'s
  validate-temp-sibling-rename sequence is kept and generalized over a caller
  supplied destination; `publish_source` collapses to the same shape with a gzip
  magic-byte check (`1f 8b`) instead of extraction, which is what catches arXiv's
  HTML holding page for a withdrawn work.
- Add `source_url` beside the existing free `pdf_url`, for `--source --url`.
  Both stay free functions on the public base address so `--url` answers without
  a client, which is the invariant `fetch_url_answers_even_when_the_cache_root_is_unusable`
  currently protects.
- `bibi-core`: `ArxivId::legacy_parts` (`identifiers.rs:94`) documents itself in
  terms of the cache layout and has no other caller. Delete it with its test.

**Application and CLI.**

- `crates/bibi-application/src/documents.rs` — delete `clean_cache`. `fetch`
  gains an output path and a source-URL branch. The existing no-arXiv-id error
  (`:61`) already fails cleanly and stays; reword it to drop "in this version".
  Collision rule: an explicit `--output` is never overwritten; an existing
  *default* destination is an error (exit 1) unless `--open`, which opens it.
- `crates/bibi-application/src/services.rs` — `PlatformPaths` is now empty, so
  delete the type, the `paths` field, `Error::NoPlatformDirectory`, and the
  `directories` dependency. `Services` keeps providers and a lazily built
  retrieval client. `with_documents` (currently zero callers) becomes the seam
  the tests use.
- `crates/bibi/src/cli.rs` — `FetchArgs` gains `-o/--output`, loses `--force`;
  `--source` no longer conflicts with `--url`. Remove `Command::Cache`,
  `CacheCommand`, `CacheCleanArgs`.
- `crates/bibi/src/commands/documents.rs` — delete `run_cache`; adjust
  `run_fetch`'s stderr notes (there is no "from the cache" outcome).
- `crates/bibi/src/bootstrap.rs` — `services()` no longer discovers directories.
  Add `ARXIV_BASE_URL_ENV = "BIBI_ARXIV_BASE_URL"`, wired exactly like
  `INSPIRE_BASE_URL_ENV` (`:24`), documented the same way: a test seam, not a
  configuration surface.

**Tests.**

- `crates/bibi-documents/tests/cache.rs` → `download.rs`: destination naming,
  PDF signature rejection, gzip rejection, temp-sibling cleanup on failure,
  refusal to overwrite an explicit output.
- `crates/bibi/tests/cli.rs`: drop `BIBI_CACHE_ROOT` from the helpers and the
  two cache tests (`:498` onward). Because `fetch` now writes to the working
  directory, each test runs the binary with `current_dir` set to its own temp
  project. Add an end-to-end download against a local `TcpListener` through
  `BIBI_ARXIV_BASE_URL`, and a collision test asserting exit 1.

Two of the three redirection variables in `CLAUDE.md` are deleted by commits 1
and 3 — a real reduction in ambient state. `BIBI_ARXIV_BASE_URL` replaces
`BIBI_CACHE_ROOT` as the seam that keeps `fetch` hermetic.

## Commit 4 — documentation

`CLAUDE.md` and `AGENTS.md` are byte-identical and must stay so.

- `CLAUDE.md` / `AGENTS.md`: the command list, the "Scope" section (drop
  `-g/--global` and the upward-search note's second clause), "Rendering and
  output" (`render_manifest` now has two callers, not three; the
  `export --provider` paragraph goes), and the three-env-var paragraph under
  "Testing".
- `README.md`: `export` and cache examples.
- `IMPLEMENTATION.md` and `REDESIGN.md` carry the most references (30+ each).
  Rather than editing them line by line, mark the superseded sections and point
  at `BIBI_TWO_BINARIES.md`, which `CLAUDE.md` already names as reopening them.
  Full reconciliation is phase two, step 8 of that document.

## Verification

Per commit:

```bash
cargo fmt --all -- --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace
```

Clippy at `-D warnings` is the main safety net for the dead-code cascades —
`creating_parents`, `SyncFailed`, `target::output`, `legacy_parts` — since each
becomes unreachable rather than a compile error.

End to end, in a scratch directory:

```bash
cargo run -p bibi -- init && cargo run -p bibi -- add -f local.bib && cargo run -p bibi -- list --format bibtex > refs.bib && cargo run -p bibi -- check refs.bib
```

Then confirm the deletions are actually gone: `bibi -g list`, `bibi export`, and
`bibi cache clean` must each be a clap usage error (exit 2), and
`bibi fetch <selector>` must leave a `<arxiv-id>.pdf` in the working directory
rather than in any cache root.

The ignored liveness test still passes and is unaffected:

```bash
cargo test -p bibi-inspire --test e2e -- --ignored
```
