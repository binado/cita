# AGENTS.md

## What this is

bibi is a Git-friendly bibliography CLI. Authoritative BibTeX (plus curated
INSPIRE identifiers) lives in schema-1 `cita.toml`; `references.bib` is a
deterministic, tracked generated artifact. That inversion is being replaced:
the `.bib` file becomes the single source of truth and `bibi-manifest` goes
away. Rust edition 2024, MSRV 1.88.

## Commands

```bash
cargo build --workspace
cargo test --workspace
cargo test -p bibi-bibliography
cargo test --test cli
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p bibi -- add 1207.7214
cargo run -p bibi -- import local.bib
cargo run -p bibi -- export
cargo run -p bibi -- sync
cargo test --test e2e -- --ignored  # live INSPIRE, network required
```

CI runs format, clippy-as-errors, test, and build on 1.88 and stable. Tests are
hermetic; INSPIRE and arXiv suites use local `TcpListener`s. The exception is
`crates/bibi/tests/e2e.rs`, a single `#[ignore]`d end-to-end test that hits the
real INSPIRE API; CI runs it on every push and pull request, plus a weekly
schedule as a standalone liveness check.

## Commit messages

Use Conventional Commits: `<type>: <description>`.

## Workspace architecture

```text
bibi-core
   ↑                ↑                      ↑
bibi-bibliography ← bibi-inspire-client   bibi-documents
   └──────────┬────────┘                   │
        bibi-manifest ─────────────────────┤
              └──────── bibi ──────────────┘
```

- `bibi-core`: `Reference`, provider traits, locators, and normalization.
- `bibi-bibliography`: strict standalone BibTeX snapshots, projections,
  and raw-entry re-keying and field insertion.
- `bibi-inspire-client`: lean `InspireRecord`s (authoritative BibTeX plus
  record id, timestamp, and canonical arXiv/DOI), stable-record-ID refresh
  batches, bounded queries, and 429 retries; cross-checks its BibTeX against the
  selected JSON through `bibi-bibliography`.
- `bibi-manifest`: schema-1 project authority, identity indexes, deterministic
  TOML, generated bibliography verification, and coordinated writes. Slated for
  replacement by a `.bib`-backed store.
- `bibi-documents`: accepts a validated arXiv ID and atomically caches PDFs and
  safely extracts gzip-compressed TeX source packages beneath `.bibi/files/arxiv`.
- `bibi`: CLI, parent discovery, sync reconciliation, and derived-export policy.

## Key decisions

### Source snapshots and raw entries

`cita.toml` snapshots are authoritative; projections are derived. Every source
stores authoritative standalone BibTeX and its `Reference` (title, authors, year,
publication, arXiv/DOI) is projected from that BibTeX. INSPIRE entries are tagged
`source = "inspire"` and additionally carry the refresh key (`record_id`), an
`updated` timestamp, and a curated `identifiers` block (canonical normalized
arXiv/DOI) that overrides the projected identity; imports are tagged
`source = "import"` and derive identity from their entry. BibTeX parsing uses raw
spans plus semantic `biblatex` parsing. Rendering sorts by local key, changes
only the raw key token, joins entries with one blank line, and appends one
newline. Do not add a handwritten writer.

Only entries and whitespace are allowed. Reject directives, comments/non-entry
content, malformed or duplicate entries, missing titles, texkeys outside
`[A-Za-z0-9._:+-]+`, and duplicate normalized DOI/eprint identities.

### Derived exports

`bibi export` writes a separate artifact and never touches `references.bib`.
Layout stays in `bibi-manifest::Manifest::render_derived`, so every rendered
bibliography shares one set of rules; only field-level policy lives in the CLI.
Fields are added through `bibi-bibliography::insert_field`, which splices after
an entry's last field value using scanner-owned spans, before any trailing
whitespace or inline comment, so comma placement is exact and the field cannot
be swallowed by a comment. An entry that already defines the field is returned
unchanged, which keeps authored values and makes repeated exports byte-stable.

Exports are pure functions of `cita.toml`: no network, no cache probing, no
machine-specific paths. They are untracked, unverified, never read back, and
refuse to run against bibliography drift. `--output` is the only CLI path that
can leave the discovered project, so it refuses both this project's managed
files and any managed file a *different* project owns; a managed name only
counts inside the directory that owns it.

### Atomic mutations

Add, import, remove, and sync validate a complete candidate before writing.
Persist and sync `references.bib` first, then persist and sync `cita.toml` as the
commit point. The old manifest remains authoritative after an interrupted
second write.

### INSPIRE sync

Refresh by stable INSPIRE record ID. Batch at 100 records or a 6 KiB encoded `q`
value. Fetch JSON and BibTeX searches sequentially and match each raw entry to
exactly one JSON record through returned texkeys. Retry 429 three times using
`Retry-After` capped at sixty seconds, otherwise five seconds, and report each
retry on stderr.

Every managed record and returned result must be explained. Local citation keys
are independent from provider texkeys and never change during refresh. Imported
BibTeX snapshots cause no network request.

### Selectors and documents

Exact local key wins, then provider ID, normalized DOI, and normalized arXiv ID.
Transient `fetch` resolves INSPIRE JSON only unless `--save` also fetches the
authoritative BibTeX and stores the record. It returns either an absolute cached
PDF path, an absolute extracted source directory with `--source`, or, with
`--url`, the arXiv PDF URL; `--source` and `--url` are mutually exclusive;
`--open` launches that target. Documents use the projected arXiv ID; stored IDs
are versionless and cache paths retain legacy arXiv archive directories.

## Error handling

Library crates use typed `thiserror` enums. The binary uses `anyhow` for context.
Add typed library variants for new failure modes.
