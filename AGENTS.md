# AGENTS.md

## What this is

cita is a Git-friendly reference manager. Structured, authoritative reference
fields plus user-owned tags and notes live in schema-2 `cita.toml`, alongside the
opaque BibTeX a source supplied; `references.bib` is a deterministic, tracked
generated artifact. A schema-1 `cita-library.toml` can register multiple
independent shelf projects by stable name and safe relative path. Rust edition
2024, MSRV 1.88.

## Commands

```bash
cargo build --workspace
cargo test --workspace
cargo test -p cita-bibliography
cargo test --test cli
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p cita -- add 1207.7214
cargo run -p cita -- import local.bib
cargo run -p cita -- edit
cargo run -p cita -- list --tag reading-list
cargo run -p cita -- generate
cargo run -p cita -- export
cargo run -p cita -- library list
cargo run -p cita -- sync --shelf paper   # one registered shelf
cargo run -p cita -- sync --all-shelves   # every registered shelf
cargo test --test e2e -- --ignored  # live INSPIRE, network required
```

CI runs format, clippy-as-errors, test, and build on 1.88 and stable. Tests are
hermetic; INSPIRE and arXiv suites use local `TcpListener`s. The exception is
`crates/cita/tests/e2e.rs`, a single `#[ignore]`d end-to-end test that hits the
real INSPIRE API; CI runs it on every push and pull request, plus a weekly
schedule as a standalone liveness check.

## Commit messages

Use Conventional Commits: `<type>: <description>`.

## Workspace architecture

```text
cita-core
   ↑                ↑                      ↑
cita-bibliography ← cita-inspire-client   cita-documents
   └──────────┬────────┘                   │
        cita-manifest ─────────────────────┤
              └──────── cita ──────────────┘
```

- `cita-core`: `Reference`, provider traits, locators, and normalization.
- `cita-bibliography`: strict standalone BibTeX snapshots, ingest-time
  projections, and raw-entry re-keying and field insertion.
- `cita-inspire-client`: lean `InspireSnapshot`s (authoritative BibTeX plus
  record id, timestamp, and canonical arXiv/DOI), stable-record-ID refresh
  batches, bounded queries, and 429 retries; attaches BibTeX only after
  cross-checking it against the slim API JSON model through `cita-bibliography`.
- `cita-manifest`: schema-2 shelf authority (the `Entry` type and its two ingest
  constructors), library registry/path validation, identity indexes,
  deterministic TOML, generated bibliography verification, and coordinated
  writes.
- `cita-documents`: accepts a validated arXiv ID and atomically caches PDFs and
  safely extracts gzip-compressed TeX source packages beneath `.cita/files/arxiv`.
- `cita`: CLI, parent discovery, sync reconciliation, derived-export policy,
  and whole-manifest editing policy.

## Key decisions

### Structured entries and opaque BibTeX

A schema-2 `cita.toml` entry stores structured fields (`type`, `title`,
`authors`, `collaborations`, `year`, `doi`, `arxiv`), user-owned `tags` and
`notes`, and an optional `bibtex` blob. The structured fields are authoritative.
They are seeded exactly once, at ingest, by the only two projection sites in the
codebase: `Entry::from_inspire` and `Entry::from_bibtex`. Nothing re-derives them
from `bibtex` at read or validation time, and nothing ever writes `bibtex` from
them, so the two lanes never need reconciling. `bibtex` is immutable opaque
cargo, supplied verbatim by a provider or an import; a reference no provider
knows about simply has none. Do not add a handwritten BibTeX writer.

Provenance is the presence of a provider sub-table, not a tag field. An entry
carrying `[references.<key>.inspire]` (`record_id`, `updated`) is managed and
refreshed by `cita sync`; an entry without one is unmanaged and never touched by
a provider. `Entry` declares `inspire` last, because TOML requires every value
before any sub-table and a field declared after it fails to serialize.

Ownership is enforced in code, in two places. `Manifest::replace_inspire`
overwrites provider-owned fields through `Entry::refresh_from_inspire`, which
destructures a freshly seeded entry so that adding a field to `Entry` stops
compiling until someone decides whether a refresh owns it; tags, notes, and the
local key carry forward untouched. `cita edit` rejects any change to a managed
entry's provider-owned fields, and to `bibtex` on any entry, naming the offending
`key.field`.

Manifest validation reads stored fields only: safe keys, non-blank titles,
non-zero record ids, and unique normalized identities. The cross-check between an
INSPIRE record's curated identifiers and its BibTeX belongs at ingest, in
`cita-inspire-client::wire::attach_bibtex`.

BibTeX parsing still uses raw spans plus semantic `biblatex` parsing. Rendering
sorts by local key, **skips entries with no `bibtex`**, changes only the raw key
token, joins entries with one blank line, and appends one newline. Only entries
and whitespace are allowed. Reject directives, comments/non-entry content,
malformed or duplicate entries, missing titles, texkeys outside
`[A-Za-z0-9._:+-]+`, and duplicate normalized DOI/eprint identities.

Schema 1 is a hard break with no migration: it stored opaque blobs with no
structured fields to read. `rm cita.toml && cita init` rebuilds from
`references.bib`, but that carries no `record_id`, so every entry returns
unmanaged; re-run `cita add` for those locators to restore management.

### Whole-manifest editing

`cita edit` renders the manifest into a temporary buffer, launches `$VISUAL`,
`$EDITOR`, or `vi`, and re-parses the result. A rejected buffer is re-opened with
the reasons injected as `# cita:` comments above the user's own bytes, which are
preserved verbatim; saving it back unchanged aborts and repeats the reasons on
stderr. Every line of a diagnostic is commented, because a TOML parse error spans
several lines and an uncommented continuation would corrupt the buffer it
describes. The saved manifest is re-rendered from parsed data, so no comment can
reach `cita.toml`. Adding and deleting keys is allowed: that is how a
provider-less reference is created and removed.

### Derived exports

`cita export` writes a separate artifact and never touches `references.bib`.
Layout stays in `cita-manifest::Manifest::render_derived`, so every rendered
bibliography shares one set of rules; only field-level policy lives in the CLI.
Fields are added through `cita-bibliography::insert_field`, which splices after
an entry's last field value using scanner-owned spans, before any trailing
whitespace or inline comment, so comma placement is exact and the field cannot
be swallowed by a comment. An entry that already defines the field is returned
unchanged, which keeps authored values and makes repeated exports byte-stable.

Exports are pure functions of `cita.toml`: no network, no cache probing, no
machine-specific paths. They are untracked, unverified, never read back, and
refuse to run against bibliography drift. `--output` is the only CLI path that
can leave the discovered project, so it refuses both this project's managed
files and any managed file a *different* project or library owns; a managed name
only counts inside the directory that owns it. Shelf exports are named for the
registered shelf name, not the shelf directory.

### Atomic mutations

Add, import, remove, edit, and sync validate a complete candidate before writing.
Persist and sync `references.bib` first, then persist and sync `cita.toml` as the
commit point. The old manifest remains authoritative after an interrupted
second write, and `cita generate` repairs detectable drift.

Library registration is a separate atomic write. Shelf initialization completes
before registration, so a failed registry write leaves a valid standalone shelf.
Library-wide operations run shelves in name order and continue after failures;
there is no crash-atomic transaction across shelves.

### Libraries and shelves

Each registered shelf is an independent cita project with its own manifest,
bibliography, identities, and cache. `cita-library.toml` only maps
stable names to library-relative paths. Paths cannot escape the root, overlap,
nest, or alias through symlinks. The library root cannot itself contain
`cita.toml` or `references.bib`. There is no aggregate bibliography, shared
cache, or cross-shelf uniqueness.

Scope is an argument, not a command level. `cita library` covers shelf lifecycle
only (`init`, `list`, `new`); every operation *inside* a shelf is the ordinary
command with `-s/--shelf`, and `--all-shelves` on `generate`, `export`, and
`sync` is the batch form. So there is no second command list to keep in step
with the first, and new commands are shelf-aware by construction. `--all-shelves`
is confined to those three because they are idempotent and derive their result
from each shelf's own manifest. Scope resolves to a `commands::library::Target`
before dispatch, so every command stays a function of a directory; a `Target`
carries the registered shelf name too, because a shelf export is named for that
name rather than its directory.

### INSPIRE sync

Refresh by stable INSPIRE record ID. A refresh merges: it replaces every
provider-owned field, including the BibTeX blob, and never touches `tags`,
`notes`, or the local key. Batch at 100 records or a 6 KiB encoded `q`
value. Fetch JSON and BibTeX searches sequentially and match each raw entry to
exactly one JSON record through returned texkeys. Retry 429 three times using
`Retry-After` capped at sixty seconds, otherwise five seconds, and report each
retry on stderr.

Every managed record and returned result must be explained. Local citation keys
are independent from provider texkeys and never change during refresh. Unmanaged
entries cause no network request.

### Selectors and documents

Exact local key wins, then provider ID, normalized DOI, and normalized arXiv ID.
Every comparison reads stored fields, so a lookup costs no BibTeX parse.
Transient `fetch` resolves INSPIRE JSON only unless `--save` also fetches the
authoritative BibTeX and stores the record. It returns either an absolute cached
PDF path, an absolute extracted source directory with `--source`, or, with
`--url`, the arXiv PDF URL; `--source` and `--url` are mutually exclusive;
`--open` launches that target. Documents use the stored arXiv ID; stored IDs
are versionless and cache paths retain legacy arXiv archive directories.

`cita list` filters with repeatable `--tag`, which requires every named tag, and
shows a `Tags` column.

### Out of scope: Git orchestration

cita guarantees diff-friendly, deterministically rendered files — sorted TOML
and a byte-stable generated `references.bib` — written atomically with a clear
commit point. Staging and committing those files is the caller's
responsibility, not cita's; there is no `git` subprocess dependency anywhere in
the binary. Do not reintroduce a `commit` command or any other Git
orchestration.

## Error handling

Library crates use typed `thiserror` enums. The binary uses `anyhow` for context.
Add typed library variants for new failure modes.
