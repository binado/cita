# AGENTS.md

## What this is

bibi is a Git-friendly bibliography CLI. The `references.bib` file *is* the
source of truth: entries are authoritative BibTeX, and bibi's own bookkeeping
lives in `x-bibi-*` fields on those entries. There is no manifest and no
generated artifact, so there is nothing that can drift. Rust edition 2024,
MSRV 1.88.

## Commands

```bash
cargo build --workspace
cargo test --workspace
cargo test -p bibi-bibfile
cargo test --test cli
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo run -p bibi -- add 1207.7214
cargo run -p bibi -- import local.bib
cargo run -p bibi -- sync
cargo run -p bibi -- export
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
        bibi-bibfile ──────────────────────┤
              └──────── bibi ──────────────┘
```

- `bibi-core`: `Reference`, provider traits, locators, and normalization.
- `bibi-bibliography`: standalone BibTeX snapshots and projections, whole-file
  span scanning, and raw-entry re-keying plus field insertion and removal.
- `bibi-inspire-client`: lean `InspireRecord`s (authoritative BibTeX plus
  record id, timestamp, and canonical arXiv/DOI), stable-record-ID refresh
  batches, bounded queries, and 429 retries; cross-checks its BibTeX against the
  selected JSON through `bibi-bibliography`.
- `bibi-bibfile`: the `.bib` file as the store — loading, byte-preserving
  mutation, identity uniqueness, validation, and path resolution.
- `bibi-documents`: accepts a validated arXiv ID and atomically caches PDFs and
  safely extracts gzip-compressed TeX source packages beneath `.bibi/files/arxiv`.
- `bibi`: CLI, sync reconciliation, and export policy.

## Key decisions

### The file is the store

Entries are authoritative BibTeX; `Reference` (title, authors, year,
publication, arXiv/DOI) is projected from them. Tool-owned bookkeeping lives in
one namespace on the entries themselves:

| Field | Meaning |
|---|---|
| `x-bibi-inspire-id` | stable INSPIRE record id; its **presence** marks an entry as managed |
| `x-bibi-inspire-updated` | provider timestamp, used to skip refreshes that change nothing |
| `x-bibi-arxiv` | curated normalized arXiv id, overriding the projected one |
| `x-bibi-doi` | curated normalized DOI, overriding the projected one |
| `x-bibi-frozen` | never refreshed, never resolved |

There is no `source = "inspire" | "import"` tag: managed-ness is derived from
field presence, so an entry cannot claim to be managed without carrying a
refresh key. LaTeX toolchains ignore unknown fields, so an annotated file
compiles as-is; `bibi export` is for handing the file to a human or a reference
manager, not for building a document.

### Byte preservation

The file is hand-edited, so a mutation must rewrite only the entries it touches.
`Bibfile` holds the bytes *between* entries alongside the entries and renders by
concatenation, which makes preservation structural rather than careful: no code
path inspects comments, `@string` directives, or the author's spacing, so
nothing can lose them. The governing test is that load-then-render is
byte-identical across a corpus covering CRLF, unicode, one-line entries, missing
trailing newlines, and directives — keep it passing.

New entries append at the end; the user owns the ordering, so nothing re-sorts.
`bibi-bibliography::scan_entries` is the lenient whole-file scanner; `parse` and
every single-entry helper stay strict, because a standalone snapshot is rendered
from scratch and would lose anything unmodelled.

Reject malformed or duplicate entries, missing titles, keys outside
`[A-Za-z0-9._:+-]+`, and duplicate normalized DOI/eprint identities.

### Atomic mutations

Add, import, remove, rekey, and sync validate a complete candidate before
writing, then replace the file in one `atomic_write`. There is one file and no
ordering contract, so an interrupted run leaves the old contents intact.

`add` resolves before it writes, so a failed lookup never leaves a stub behind.

### Path resolution

`--path`, then `$BIBI_BIB`, then `./references.bib`. Never a walk up the tree: a
`.bib` is not a project marker, and silently adopting a parent directory's
bibliography is worse than asking. A directory argument resolves to the default
file name inside it. A missing file is always an error, never created — a
mistyped directory must not become a new, empty bibliography. Every mutating
command echoes the file it wrote, because there is no longer a single legal
target to infer.

### Exports

`bibi export` writes a separate artifact and never touches the bibliography.
It **removes** the `x-bibi-*` namespace by default (`--keep-metadata` opts out)
and adds `url = {https://arxiv.org/pdf/<id>}` to entries with an arXiv id.
Layout is normalized, unlike a write back to the source of truth: an export is
derived and nobody edits it.

Fields are added through `bibi-bibliography::insert_field`, which splices after
an entry's last field value using scanner-owned spans, before any trailing
whitespace or inline comment, so comma placement is exact and the field cannot
be swallowed by a comment. `strip_fields_with_prefix` is its exact inverse; a
test asserts the two round-trip byte-for-byte. An entry that already defines a
field keeps its authored value, which makes repeated exports byte-stable.

Exports are pure functions of the bibliography: no network, no cache probing, no
machine-specific paths. `--output` refuses exactly one target, the source of
truth itself; `-` writes to stdout.

### INSPIRE sync

`bibi sync` is one command doing one thing by two lookups. Managed entries
refresh by stable record id, batched at 100 records or a 6 KiB encoded `q`.
Unmanaged entries with a DOI or arXiv id are resolved through the direct
`/api/arxiv` and `/api/doi` endpoints, which return exactly one record or a
not-found, so there is no ambiguity to arbitrate. Resolution runs first, so an
entry adopted in a run is counted by the same pass.

Adoption attaches bookkeeping and keeps the user's own BibTeX: it answers "what
is this thing I have", not "replace it". Recording the provider's current
timestamp makes that stick, since the refresh then finds them equal and writes
nothing. A record INSPIRE does not know is reported, not fatal — a bibliography
legitimately holds textbooks and theses.

Every managed record and returned result must be explained. Two entries claiming
one record id is refused; `import` cannot catch that case, because it dedupes on
DOI and arXiv id. Local citation keys are independent from provider texkeys and
never change during refresh. Output summarizes rather than enumerates, since a
mixed bibliography always has entries INSPIRE cannot place.

Retry 429 three times using `Retry-After` capped at sixty seconds, otherwise
five seconds, and report each retry on stderr.

### Selectors and documents

Exact local key wins, then provider ID, normalized DOI, and normalized arXiv ID.
Transient `fetch` resolves INSPIRE JSON only unless `--save` also fetches the
authoritative BibTeX and stores the record. It returns either an absolute cached
PDF path, an absolute extracted source directory with `--source`, or, with
`--url`, the arXiv PDF URL; `--source` and `--url` are mutually exclusive;
`--open` launches that target. Documents use the projected arXiv ID; stored IDs
are versionless and cache paths retain legacy arXiv archive directories. The
cache lives beside the bibliography.

### Git

Git integration is the user's own. bibi writes one tracked text file in the
format the user reads, so `git diff` already shows what changed.

## Error handling

Library crates use typed `thiserror` enums. The binary uses `anyhow` for context.
Add typed library variants for new failure modes.
